//! # icon_finder
//!
//! Find the path to a Linux application's icon by name and resolution,
//! following the [XDG Icon Theme Specification](https://specifications.freedesktop.org/icon-theme-spec/icon-theme-spec-latest.html).
//!
//! Supports both regular icon names (`firefox`) and Flatpak reverse-DNS names
//! (`com.obsproject.Studio`). When an exact match isn't found, a fuzzy search
//! is performed against reverse-DNS icon names on the system.

use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const EXTENSIONS: &[&str] = &["svg", "png", "xpm"];

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

pub fn current_icon_theme() -> String {
    if let Ok(theme) = env::var("ICON_THEME") {
        let theme = theme.trim().to_string();
        if !theme.is_empty() {
            return theme;
        }
    }

    let home = env::var("HOME").unwrap_or_default();
    let gtk_configs = [
        format!("{}/.config/gtk-4.0/settings.ini", home),
        format!("{}/.config/gtk-3.0/settings.ini", home),
        "/etc/gtk-3.0/settings.ini".to_string(),
    ];

    for config_path in &gtk_configs {
        if let Some(theme) = read_gtk_icon_theme(Path::new(config_path)) {
            return theme;
        }
    }

    if let Ok(output) = Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "icon-theme"])
        .output()
    {
        if output.status.success() {
            let theme = String::from_utf8_lossy(&output.stdout)
                .trim()
                .trim_matches('\'')
                .to_string();
            if !theme.is_empty() {
                return theme;
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

fn search_in_size_dir(size_dir: &Path, app: &str) -> Option<PathBuf> {
    for ext in EXTENSIONS {
        let path = size_dir.join(format!("{}.{}", app, ext));
        if path.exists() {
            return Some(path);
        }
    }

    let mut category_dirs: Vec<PathBuf> = fs::read_dir(size_dir)
        .ok()?
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.path())
        .collect();

    category_dirs.sort_by_key(
        |p| match p.file_name().and_then(|n| n.to_str()).unwrap_or("") {
            "apps" => 0u8,
            _ => 1u8,
        },
    );

    for category_dir in &category_dirs {
        for ext in EXTENSIONS {
            let path = category_dir.join(format!("{}.{}", app, ext));
            if path.exists() {
                return Some(path);
            }
        }
    }

    None
}

fn find_in_theme_all_bases(
    base_dirs: &[PathBuf],
    theme: &str,
    app: &str,
    size: u32,
) -> Option<PathBuf> {
    let size_str = size.to_string();

    struct SizeDir {
        path: PathBuf,
        bucket: u8,
        distance: u32,
    }

    let mut size_dirs: Vec<SizeDir> = Vec::new();

    for base in base_dirs {
        if base.ends_with("pixmaps") {
            continue;
        }
        let theme_dir = base.join(theme);
        if !theme_dir.exists() {
            continue;
        }

        let Ok(subdirs) = fs::read_dir(&theme_dir) else {
            continue;
        };

        for entry in subdirs.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(name) = path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|s| s.to_string())
            else {
                continue;
            };

            let (bucket, distance) = if name.starts_with(&size_str) {
                let hidpi_penalty = if name.contains('@') { 1u32 } else { 0u32 };
                (0u8, hidpi_penalty)
            } else if name.to_lowercase().contains("scalable") {
                (1u8, 0u32)
            } else {
                let num: u32 = name.split('x').next().unwrap_or("0").parse().unwrap_or(0);
                let dist = if num > size { num - size } else { size - num };
                (2u8, dist)
            };

            size_dirs.push(SizeDir {
                path,
                bucket,
                distance,
            });
        }
    }

    size_dirs.sort_by_key(|d| (d.bucket, d.distance));

    for sd in &size_dirs {
        if let Some(p) = search_in_size_dir(&sd.path, app) {
            return Some(p);
        }
    }

    None
}

pub fn find_flat(dir: &Path, app: &str) -> Option<PathBuf> {
    for ext in EXTENSIONS {
        let path = dir.join(format!("{}.{}", app, ext));
        if path.exists() {
            return Some(path);
        }
    }
    None
}

/// Check whether a reverse-DNS icon name (e.g. `com.obsproject.Studio`) matches
/// a short query (e.g. `obs`).
///
/// Matching rules (case-insensitive):
/// - The full name equals the query (`firefox` == `firefox`)
/// - Any `.`-separated component equals the query (`Studio` for `studio`)
/// - Any component starts with the query (`obsproject` for `obs`)
fn matches_query(icon_stem: &str, query: &str) -> bool {
    let query_lower = query.to_lowercase();

    if !icon_stem.contains('.') {
        return false;
    }

    for component in icon_stem.split('.') {
        let comp_lower = component.to_lowercase();
        if comp_lower == query_lower || comp_lower.starts_with(&query_lower) {
            return true;
        }
    }

    false
}

pub fn find_fuzzy_candidates(query: &str, base_dirs: &[PathBuf]) -> Vec<String> {
    let mut candidates: BTreeSet<String> = BTreeSet::new();

    for base in base_dirs {
        let Ok(themes) = fs::read_dir(base) else {
            continue;
        };

        for theme_entry in themes.flatten() {
            let theme_path = theme_entry.path();
            if !theme_path.is_dir() {
                continue;
            }
            let Ok(sizes) = fs::read_dir(&theme_path) else {
                continue;
            };

            for size_entry in sizes.flatten() {
                let size_path = size_entry.path();
                if !size_path.is_dir() {
                    continue;
                }

                let dirs_to_scan: Vec<PathBuf> = {
                    let mut v = vec![size_path.clone()];
                    if let Ok(cats) = fs::read_dir(&size_path) {
                        v.extend(
                            cats.flatten()
                                .filter(|e| e.path().is_dir())
                                .map(|e| e.path()),
                        );
                    }
                    v
                };

                for dir in &dirs_to_scan {
                    let Ok(files) = fs::read_dir(dir) else {
                        continue;
                    };
                    for file in files.flatten() {
                        let file_path = file.path();
                        if !file_path.is_file() {
                            continue;
                        }
                        // Strip the extension to get the icon stem
                        let Some(stem) = file_path.file_stem().and_then(|s| s.to_str()) else {
                            continue;
                        };
                        if matches_query(stem, query) {
                            candidates.insert(stem.to_string());
                        }
                    }
                }
            }
        }
    }

    candidates.into_iter().collect()
}

/// * `app` - Application name: `"firefox"`, `"obs"`, or `"com.obsproject.Studio"`
/// * `size` - Desired icon size in pixels, e.g. `256`
pub fn find_icon(app: &str, size: u32) -> Option<PathBuf> {
    let base_dirs = icon_base_dirs();
    let active_theme = current_icon_theme();

    let mut priority_themes: Vec<String> = vec![active_theme.clone()];
    if active_theme != "hicolor" {
        priority_themes.push("hicolor".to_string());
    }

    let mut other_themes: BTreeSet<String> = BTreeSet::new();
    for base in &base_dirs {
        if base.ends_with("pixmaps") {
            continue;
        }
        if let Ok(entries) = fs::read_dir(base) {
            for name in entries
                .flatten()
                .filter(|e| e.path().is_dir())
                .filter_map(|e| e.file_name().into_string().ok())
            {
                if !priority_themes.contains(&name) {
                    other_themes.insert(name);
                }
            }
        }
    }

    let try_name = |name: &str| -> Option<PathBuf> {
        for theme in &priority_themes {
            if let Some(p) = find_in_theme_all_bases(&base_dirs, theme, name, size) {
                return Some(p);
            }
        }
        for theme in &other_themes {
            if let Some(p) = find_in_theme_all_bases(&base_dirs, theme, name, size) {
                return Some(p);
            }
        }
        for base in &base_dirs {
            if base.ends_with("pixmaps") {
                if let Some(p) = find_flat(base, name) {
                    return Some(p);
                }
            }
        }
        None
    };

    // --- Pass 1: exact name ---
    if let Some(p) = try_name(app) {
        return Some(p);
    }

    // --- Pass 2: fuzzy reverse-DNS search (only if no dots in query) ---
    // If the user already typed a full reverse-DNS name that wasn't found, give up.
    if app.contains('.') {
        return None;
    }

    let candidates = find_fuzzy_candidates(app, &base_dirs);
    for candidate in &candidates {
        if let Some(p) = try_name(candidate) {
            return Some(p);
        }
    }

    None
}

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
    fn find_flat_returns_none_for_missing_dir() {
        assert!(find_flat(Path::new("/nonexistent/path"), "someapp").is_none());
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
