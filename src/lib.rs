//! # icon_finder
//!
//! Find the path to a Linux application's icon by name and resolution,
//! following the [XDG Icon Theme Specification](https://specifications.freedesktop.org/icon-theme-spec/icon-theme-spec-latest.html).
//!
//! Supports both regular icon names (`firefox`) and Flatpak reverse-DNS names
//! (`com.obsproject.Studio`). When an exact match isn't found, a fuzzy search
//! is performed against reverse-DNS icon names on the system.
//!
//! Results are cached after the first call — repeated calls for different apps
//! reuse the pre-computed theme and directory lists.
//!
//! ## Example
//!
//! ```no_run
//! use icon_finder::find_icon;
//!
//! if let Some(path) = find_icon("firefox", 128) {
//!     println!("Icon found: {}", path.display());
//! }
//!
//! if let Some(path) = find_icon("obs", 256) {
//!     println!("OBS icon: {}", path.display());
//! }
//! ```

use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// Supported icon extensions in preference order.
const EXTENSIONS: &[&str] = &["svg", "png", "xpm"];

// ---------------------------------------------------------------------------
// Cached globals — computed once per process, never again
// ---------------------------------------------------------------------------

static BASE_DIRS: OnceLock<Vec<PathBuf>> = OnceLock::new();
static ACTIVE_THEME: OnceLock<String> = OnceLock::new();
/// Cached index: maps icon stem → first resolved path, built lazily on first fuzzy search.
static FUZZY_INDEX: OnceLock<Vec<String>> = OnceLock::new();

fn base_dirs_cached() -> &'static [PathBuf] {
    BASE_DIRS.get_or_init(icon_base_dirs)
}

fn active_theme_cached() -> &'static str {
    ACTIVE_THEME.get_or_init(current_icon_theme)
}

/// Returns a cached, deduplicated list of all reverse-DNS icon stems found on
/// the system. Built once by scanning only ONE size directory per theme (since
/// icon names are identical across all sizes), then reused for every fuzzy query.
fn fuzzy_index_cached(base_dirs: &[PathBuf]) -> &'static [String] {
    FUZZY_INDEX.get_or_init(|| build_fuzzy_index(base_dirs))
}

/// Scan icon dirs to collect all reverse-DNS icon stems (names containing `.`).
///
/// For each theme, scans every `apps/` subdirectory across all size dirs.
/// This is necessary because Flatpak apps may only ship icons at specific sizes
/// (e.g. only `512x512/apps/`) so sampling a single size dir misses them.
/// The number of size dirs per theme is small (10-20), so this is still fast.
fn build_fuzzy_index(base_dirs: &[PathBuf]) -> Vec<String> {
    let mut stems: BTreeSet<String> = BTreeSet::new();

    for base in base_dirs {
        if base.ends_with("pixmaps") {
            continue;
        }
        let Ok(themes_rd) = fs::read_dir(base) else {
            continue;
        };

        for theme_entry in themes_rd.flatten() {
            let theme_path = theme_entry.path();
            let Ok(meta) = fs::metadata(&theme_path) else {
                continue;
            };
            if !meta.is_dir() {
                continue;
            }

            // Walk every size dir, but only look inside `apps/` within each.
            // Flatpak icons are always in apps/ — skipping other categories
            // (mimetypes, devices, etc.) avoids indexing irrelevant names.
            let Ok(sizes_rd) = fs::read_dir(&theme_path) else {
                continue;
            };
            for size_entry in sizes_rd.flatten() {
                let size_path = size_entry.path();
                let Ok(meta) = fs::metadata(&size_path) else {
                    continue;
                };
                if !meta.is_dir() {
                    continue;
                }

                // Only look in apps/ — that's where Flatpak icons always live
                let apps_dir = size_path.join("apps");
                let dir_to_scan = if apps_dir.exists() {
                    apps_dir
                } else {
                    // Some themes place icons directly in the size dir
                    size_path
                };

                let Ok(files) = fs::read_dir(&dir_to_scan) else {
                    continue;
                };
                for file in files.flatten() {
                    let Ok(meta) = fs::metadata(file.path()) else {
                        continue;
                    };
                    if !meta.is_file() {
                        continue;
                    }

                    let fname = file.file_name();
                    let Some(fname_str) = fname.to_str() else {
                        continue;
                    };

                    let stem = match fname_str.rfind('.') {
                        Some(i) => &fname_str[..i],
                        None => fname_str,
                    };

                    // Only index reverse-DNS names (contain a dot)
                    if stem.contains('.') {
                        stems.insert(stem.to_string());
                    }
                }
            }
        }
    }

    stems.into_iter().collect()
}

// ---------------------------------------------------------------------------
// Public: directory / theme discovery
// ---------------------------------------------------------------------------

/// Returns the ordered list of base directories to search for icons.
pub fn icon_base_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    if let Ok(home) = env::var("HOME") {
        dirs.push(PathBuf::from(&home).join(".icons"));
        dirs.push(PathBuf::from(&home).join(".local/share/icons"));
    }

    let xdg_data_dirs =
        env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".to_string());

    for data_dir in xdg_data_dirs.split(':') {
        dirs.push(PathBuf::from(data_dir).join("icons"));
    }

    dirs.push(PathBuf::from("/usr/share/pixmaps"));
    dirs
}

/// Returns the user's active GTK icon theme name.
pub fn current_icon_theme() -> String {
    if let Ok(theme) = env::var("ICON_THEME") {
        let t = theme.trim().to_string();
        if !t.is_empty() {
            return t;
        }
    }

    let home = env::var("HOME").unwrap_or_default();
    for config in [
        format!("{}/.config/gtk-4.0/settings.ini", home),
        format!("{}/.config/gtk-3.0/settings.ini", home),
        "/etc/gtk-3.0/settings.ini".to_string(),
    ] {
        if let Some(theme) = read_gtk_icon_theme(Path::new(&config)) {
            return theme;
        }
    }

    if let Ok(out) = Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "icon-theme"])
        .output()
    {
        if out.status.success() {
            let t = String::from_utf8_lossy(&out.stdout)
                .trim()
                .trim_matches('\'')
                .to_string();
            if !t.is_empty() {
                return t;
            }
        }
    }

    "hicolor".to_string()
}

fn read_gtk_icon_theme(path: &Path) -> Option<String> {
    let content = fs::read_to_string(path).ok()?;
    for line in content.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("gtk-icon-theme-name") {
            let value = rest
                .trim_start()
                .strip_prefix('=')?
                .trim()
                .trim_matches('\'')
                .trim_matches('"');
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Core search
// ---------------------------------------------------------------------------

/// Search for `app` inside a size directory, trying direct children first,
/// then `apps/` subdirectory, then any other category subdirectory.
///
/// Reuses a pre-allocated filename buffer to avoid per-check heap allocations.
fn search_in_size_dir(size_dir: &Path, app: &str, buf: &mut PathBuf) -> Option<PathBuf> {
    // Check directly in size_dir (some themes skip the category level)
    for ext in EXTENSIONS {
        buf.clear();
        buf.push(size_dir);
        buf.push(format_icon_name(app, ext));
        if buf.exists() {
            return Some(buf.clone());
        }
    }

    // Read category subdirs; visit `apps/` first without a full sort
    let Ok(rd) = fs::read_dir(size_dir) else {
        return None;
    };

    let mut others: Vec<PathBuf> = Vec::new();
    let mut apps_dir: Option<PathBuf> = None;

    for entry in rd.flatten() {
        // Use fs::metadata() to follow symlinks — Nix/Flatpak dirs use them heavily.
        let Ok(meta) = fs::metadata(entry.path()) else {
            continue;
        };
        if !meta.is_dir() {
            continue;
        }
        let path = entry.path();
        if entry.file_name() == "apps" {
            apps_dir = Some(path);
        } else {
            others.push(path);
        }
    }

    // Try apps/ first, then the rest
    let dirs_to_check = apps_dir.iter().chain(others.iter());
    for cat_dir in dirs_to_check {
        for ext in EXTENSIONS {
            buf.clear();
            buf.push(cat_dir);
            buf.push(format_icon_name(app, ext));
            if buf.exists() {
                return Some(buf.clone());
            }
        }
    }

    None
}

/// Format `"app.ext"` into a stack-local array to avoid a heap `String`.
/// Returns a `&str` valid for the duration of the call — implemented as a
/// tiny wrapper that builds the name once and hands back a `String` we reuse
/// via the buffer strategy in `search_in_size_dir`.
#[inline(always)]
fn format_icon_name(app: &str, ext: &str) -> String {
    // This String is pushed onto a PathBuf that we .clone() only on a hit,
    // so it is allocated at most once per found icon.
    let mut s = String::with_capacity(app.len() + 1 + ext.len());
    s.push_str(app);
    s.push('.');
    s.push_str(ext);
    s
}

struct SizeDir {
    path: PathBuf,
    /// 0 = exact, 1 = scalable, 2 = other
    bucket: u8,
    /// numeric distance from target (used within bucket 2)
    distance: u32,
}

/// Collect and sort all size-directories for `theme` across every base dir.
fn collect_size_dirs(base_dirs: &[PathBuf], theme: &str, size: u32) -> Vec<SizeDir> {
    let size_str = size.to_string();
    let mut size_dirs: Vec<SizeDir> = Vec::new();

    for base in base_dirs {
        if base.ends_with("pixmaps") {
            continue;
        }
        let theme_dir = base.join(theme);
        // Use read_dir directly — avoid the extra exists() stat
        let Ok(rd) = fs::read_dir(&theme_dir) else {
            continue;
        };

        for entry in rd.flatten() {
            let Ok(meta) = fs::metadata(entry.path()) else {
                continue;
            };
            if !meta.is_dir() {
                continue;
            }

            let path = entry.path();
            let name = match entry.file_name().into_string() {
                Ok(n) => n,
                Err(_) => continue,
            };

            let (bucket, distance) = if name.starts_with(&size_str) {
                (0u8, if name.contains('@') { 1u32 } else { 0u32 })
            } else if name.eq_ignore_ascii_case("scalable") {
                (1u8, 0u32)
            } else {
                let num: u32 = name.split('x').next().unwrap_or("0").parse().unwrap_or(0);
                let dist = size.abs_diff(num);
                (2u8, dist)
            };

            size_dirs.push(SizeDir {
                path,
                bucket,
                distance,
            });
        }
    }

    size_dirs.sort_unstable_by_key(|d| (d.bucket, d.distance));
    size_dirs
}

fn find_in_theme_all_bases(
    base_dirs: &[PathBuf],
    theme: &str,
    app: &str,
    size: u32,
) -> Option<PathBuf> {
    let size_dirs = collect_size_dirs(base_dirs, theme, size);
    // Reusable path buffer — allocated once, reused for every candidate check
    let mut buf = PathBuf::with_capacity(128);

    for sd in &size_dirs {
        if let Some(p) = search_in_size_dir(&sd.path, app, &mut buf) {
            return Some(p);
        }
    }

    None
}

pub fn find_flat(dir: &Path, app: &str) -> Option<PathBuf> {
    for ext in EXTENSIONS {
        let path = dir.join(format_icon_name(app, ext));
        if path.exists() {
            return Some(path);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Fuzzy reverse-DNS matching
// ---------------------------------------------------------------------------

/// Returns true if the reverse-DNS `icon_stem` matches the short `query`.
///
/// Matching is case-insensitive. Any `.`-separated component that equals or
/// starts with `query` is considered a match.
fn matches_query(icon_stem: &str, query: &str) -> bool {
    if !icon_stem.contains('.') {
        return false;
    }
    for component in icon_stem.split('.') {
        if component.eq_ignore_ascii_case(query)
            || component
                .get(..query.len())
                .map_or(false, |prefix| prefix.eq_ignore_ascii_case(query))
        {
            return true;
        }
    }
    false
}

/// Search for a fuzzy reverse-DNS match for `query`.
///
/// Uses a pre-built index of all reverse-DNS icon stems on the system,
/// which is computed once and cached. Subsequent calls just filter the
/// in-memory list and do a targeted search — no directory walking.
fn find_fuzzy(
    query: &str,
    base_dirs: &[PathBuf],
    priority_themes: &[&str],
    other_themes: &BTreeSet<String>,
    size: u32,
) -> Option<PathBuf> {
    let index = fuzzy_index_cached(base_dirs);

    // Filter the cached stem list — pure memory operation, no I/O
    let matches: Vec<&str> = index
        .iter()
        .filter(|stem| matches_query(stem, query))
        .map(|s| s.as_str())
        .collect();

    for candidate in matches {
        for theme in priority_themes {
            if let Some(p) = find_in_theme_all_bases(base_dirs, theme, candidate, size) {
                return Some(p);
            }
        }
        for theme in other_themes {
            if let Some(p) = find_in_theme_all_bases(base_dirs, theme, candidate, size) {
                return Some(p);
            }
        }
        for b in base_dirs {
            if b.ends_with("pixmaps") {
                if let Some(p) = find_flat(b, candidate) {
                    return Some(p);
                }
            }
        }
    }

    None
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Find the path to a Linux application icon by name and desired resolution.
///
/// Uses cached theme and directory data after the first call, making repeated
/// lookups significantly faster.
///
/// # Arguments
///
/// * `app`  - e.g. `"firefox"`, `"obs"`, or `"com.obsproject.Studio"`
/// * `size` - desired size in pixels, e.g. `256`
pub fn find_icon(app: &str, size: u32) -> Option<PathBuf> {
    let base_dirs = base_dirs_cached();
    let active_theme = active_theme_cached();

    // Build priority theme list (active → hicolor), deduplicated
    let mut priority_themes: Vec<&str> = vec![active_theme];
    if active_theme != "hicolor" {
        priority_themes.push("hicolor");
    }

    // Collect other themes once
    let mut other_themes: BTreeSet<String> = BTreeSet::new();
    for base in base_dirs {
        if base.ends_with("pixmaps") {
            continue;
        }
        let Ok(rd) = fs::read_dir(base) else { continue };
        for name in rd.flatten().filter_map(|e| {
            fs::metadata(e.path()).ok().filter(|m| m.is_dir())?;
            e.file_name().into_string().ok()
        }) {
            if !priority_themes.contains(&name.as_str()) {
                other_themes.insert(name);
            }
        }
    }

    // Helper closure: try one name through all themes + pixmaps
    let try_name = |name: &str| -> Option<PathBuf> {
        for theme in &priority_themes {
            if let Some(p) = find_in_theme_all_bases(base_dirs, theme, name, size) {
                return Some(p);
            }
        }
        for theme in &other_themes {
            if let Some(p) = find_in_theme_all_bases(base_dirs, theme, name, size) {
                return Some(p);
            }
        }
        for base in base_dirs {
            if base.ends_with("pixmaps") {
                if let Some(p) = find_flat(base, name) {
                    return Some(p);
                }
            }
        }
        None
    };

    // Pass 1: exact name
    if let Some(p) = try_name(app) {
        return Some(p);
    }

    // Pass 2: fuzzy reverse-DNS (only for plain names with no dots)
    if app.contains('.') {
        return None;
    }

    find_fuzzy(app, base_dirs, &priority_themes, &other_themes, size)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_dirs_are_non_empty() {
        assert!(!icon_base_dirs().is_empty());
    }

    #[test]
    fn base_dirs_contain_pixmaps() {
        assert!(icon_base_dirs().iter().any(|p| p.ends_with("pixmaps")));
    }

    #[test]
    fn find_flat_returns_none_for_missing() {
        assert!(find_flat(Path::new("/nonexistent"), "someapp").is_none());
    }

    #[test]
    fn find_icon_returns_none_for_fake_app() {
        assert!(find_icon("__fake_app_that_does_not_exist__", 128).is_none());
    }

    #[test]
    fn current_theme_is_non_empty() {
        assert!(!current_icon_theme().is_empty());
    }

    #[test]
    fn matches_query_exact_component() {
        assert!(matches_query("com.obsproject.Studio", "studio"));
        assert!(matches_query("com.obsproject.Studio", "Studio"));
    }

    #[test]
    fn matches_query_prefix_component() {
        assert!(matches_query("com.obsproject.Studio", "obs"));
        assert!(matches_query("org.mozilla.firefox", "mozilla"));
    }

    #[test]
    fn matches_query_no_false_positives() {
        assert!(!matches_query("com.obsproject.Studio", "ox"));
        assert!(!matches_query("com.obsproject.Studio", "xyz"));
        assert!(!matches_query("firefox", "fire"));
    }
}
