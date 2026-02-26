# icon_finder

Find the path to a Linux application's icon by name and resolution,
following the [XDG Icon Theme Specification](https://specifications.freedesktop.org/icon-theme-spec/icon-theme-spec-latest.html).

## Library usage

```toml
[dependencies]
icon_finder = "1.0.0"
```

```rust
use icon_finder::find_icon;

if let Some(path) = find_icon("firefox", 128) {
    println!("{}", path.display());
}
```

## CLI usage

```bash
cargo install icon_finder
icon_finder firefox 128
# /usr/share/icons/hicolor/128x128/apps/firefox.png
```
