//! Application names and icons, looked up from desktop files and icon
//! themes by a window's app id.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use resvg::{tiny_skia, usvg};
use tracing::debug;

/// What the desktop knows about an application.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppInfo {
    /// The display name, such as "Ghostty".
    pub name: String,
    /// An icon name to look up in the icon theme, or an absolute path.
    pub icon: Option<String>,
}

/// Finds the application with `app_id`: first the desktop file named after
/// it, then any desktop file whose `StartupWMClass` matches. Falls back to a
/// name made from the app id itself.
pub fn app_info(app_id: &str) -> AppInfo {
    let entry = find_desktop_entry(app_id);
    AppInfo {
        name: entry
            .as_ref()
            .and_then(|e| e.name.clone())
            .unwrap_or_else(|| fallback_name(app_id)),
        // Without a desktop file, many apps install an icon named after
        // their app id.
        icon: entry
            .and_then(|e| e.icon)
            .or_else(|| (!app_id.is_empty()).then(|| app_id.to_string())),
    }
}

/// Loads icon `icon` from the icon theme, or from its path if it is
/// absolute, scaled to fit a `size` by `size` pixel square.
pub fn load_icon(icon: &str, size: u32) -> Option<tiny_skia::Pixmap> {
    let path = if Path::new(icon).is_absolute() {
        PathBuf::from(icon)
    } else {
        find_icon(icon, size, "apps")?
    };
    render_found_icon(&path, size)
}

/// Loads status icon `name`, such as `audio-volume-high-symbolic`, from the
/// icon theme, scaled to fit a `size` by `size` pixel square.
pub fn load_status_icon(name: &str, size: u32) -> Option<tiny_skia::Pixmap> {
    render_found_icon(&find_icon(name, size, "status")?, size)
}

fn render_found_icon(path: &Path, size: u32) -> Option<tiny_skia::Pixmap> {
    let pixmap = render_icon(path, size);
    if pixmap.is_none() {
        debug!(path = %path.display(), "cannot load icon");
    }
    pixmap
}

#[derive(Debug, Default, PartialEq, Eq)]
struct DesktopEntry {
    name: Option<String>,
    icon: Option<String>,
    wm_class: Option<String>,
}

fn find_desktop_entry(app_id: &str) -> Option<DesktopEntry> {
    if app_id.is_empty() || app_id.contains('/') {
        return None;
    }
    let dirs: Vec<PathBuf> = data_dirs().map(|dir| dir.join("applications")).collect();

    let file = format!("{app_id}.desktop");
    let exact = dirs
        .iter()
        .find_map(|dir| fs::read_to_string(dir.join(&file)).ok())
        .map(|text| parse_desktop_entry(&text));
    if exact.is_some() {
        return exact;
    }

    // Apps whose desktop file is named differently, such as JetBrains IDEs
    // installed by the Toolbox, say which app id they belong to.
    dirs.iter()
        .filter_map(|dir| fs::read_dir(dir).ok())
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "desktop"))
        .filter_map(|path| fs::read_to_string(path).ok())
        .map(|text| parse_desktop_entry(&text))
        .find(|entry| {
            entry
                .wm_class
                .as_ref()
                .is_some_and(|class| class.eq_ignore_ascii_case(app_id))
        })
}

fn parse_desktop_entry(text: &str) -> DesktopEntry {
    let mut entry = DesktopEntry::default();
    let mut in_entry = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let field = match key.trim() {
            "Name" => &mut entry.name,
            "Icon" => &mut entry.icon,
            "StartupWMClass" => &mut entry.wm_class,
            _ => continue,
        };
        if field.is_none() && !value.trim().is_empty() {
            *field = Some(value.trim().to_string());
        }
    }
    entry
}

/// A readable name from the app id alone: "com.mitchellh.ghostty" becomes
/// "Ghostty".
fn fallback_name(app_id: &str) -> String {
    let last = app_id.rsplit('.').next().unwrap_or(app_id);
    let mut chars = last.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// `$XDG_DATA_HOME` followed by `$XDG_DATA_DIRS`, where desktop files and
/// icons live.
fn data_dirs() -> impl Iterator<Item = PathBuf> {
    let home = env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")));
    let dirs = env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|dirs| !dirs.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    let dirs: Vec<PathBuf> = dirs
        .split(':')
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .collect();
    home.into_iter().chain(dirs)
}

/// Searches the hicolor theme, where applications install their icons, and
/// Adwaita, then the old pixmaps directory. `context` is the theme's
/// directory for the kind of icon, such as `apps` or `status`.
fn find_icon(name: &str, size: u32, context: &str) -> Option<PathBuf> {
    if name.contains('/') {
        return None;
    }
    for theme in ["hicolor", "Adwaita"] {
        let candidates: Vec<(Option<u32>, PathBuf)> = data_dirs()
            .map(|dir| dir.join("icons").join(theme))
            .flat_map(|theme_dir| theme_candidates(&theme_dir, name, context))
            .collect();
        if let Some(path) = best_icon(candidates, size) {
            return Some(path);
        }
    }
    data_dirs()
        .map(|dir| dir.join("pixmaps"))
        .flat_map(|dir| ["png", "svg"].map(|ext| dir.join(format!("{name}.{ext}"))))
        .find(|path| path.is_file())
}

/// The files for icon `name` in one theme's `context` directories, with
/// their pixel size; `None` for scalable ones.
fn theme_candidates(theme_dir: &Path, name: &str, context: &str) -> Vec<(Option<u32>, PathBuf)> {
    let Ok(entries) = fs::read_dir(theme_dir) else {
        return Vec::new();
    };
    let mut candidates = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let dir_name = entry.file_name();
        let dir_name = dir_name.to_string_lossy();
        // "scalable", "symbolic" (single-colour and scalable), "48x48" or
        // "128x128@2" (twice the pixels).
        let size = if dir_name == "scalable" || dir_name == "symbolic" {
            None
        } else {
            let (base, factor) = match dir_name.split_once('@') {
                Some((base, factor)) => (base, factor.parse::<u32>().ok()),
                None => (dir_name.as_ref(), Some(1)),
            };
            let Some(side) = base
                .split_once('x')
                .and_then(|(w, _)| w.parse::<u32>().ok())
            else {
                continue;
            };
            let Some(factor) = factor else { continue };
            Some(side * factor)
        };
        for ext in ["png", "svg"] {
            let path = entry.path().join(context).join(format!("{name}.{ext}"));
            if path.is_file() {
                candidates.push((if ext == "svg" { None } else { size }, path));
            }
        }
    }
    candidates
}

/// Picks the icon that looks best at `size`: the smallest bitmap at least
/// that big, else a scalable one, else the biggest bitmap.
fn best_icon(candidates: Vec<(Option<u32>, PathBuf)>, size: u32) -> Option<PathBuf> {
    let big_enough = candidates
        .iter()
        .filter_map(|(s, path)| s.filter(|&s| s >= size).map(|s| (s, path)))
        .min_by_key(|(s, _)| *s);
    let scalable = candidates.iter().find(|(s, _)| s.is_none());
    let biggest = candidates
        .iter()
        .filter_map(|(s, path)| s.map(|s| (s, path)))
        .max_by_key(|(s, _)| *s);
    big_enough
        .map(|(_, path)| path)
        .or(scalable.map(|(_, path)| path))
        .or(biggest.map(|(_, path)| path))
        .cloned()
}

fn render_icon(path: &Path, size: u32) -> Option<tiny_skia::Pixmap> {
    let data = fs::read(path).ok()?;
    let mut pixmap = tiny_skia::Pixmap::new(size, size)?;
    let is_svg = path.extension().is_some_and(|ext| ext == "svg");
    if is_svg {
        let tree = usvg::Tree::from_data(&data, &usvg::Options::default()).ok()?;
        let (w, h) = (tree.size().width(), tree.size().height());
        let scale = size as f32 / w.max(h);
        let transform = tiny_skia::Transform::from_scale(scale, scale).post_translate(
            (size as f32 - w * scale) / 2.0,
            (size as f32 - h * scale) / 2.0,
        );
        resvg::render(&tree, transform, &mut pixmap.as_mut());
    } else {
        let image = tiny_skia::Pixmap::decode_png(&data).ok()?;
        let (w, h) = (image.width() as f32, image.height() as f32);
        let scale = size as f32 / w.max(h);
        let transform = tiny_skia::Transform::from_scale(scale, scale).post_translate(
            (size as f32 - w * scale) / 2.0,
            (size as f32 - h * scale) / 2.0,
        );
        let paint = tiny_skia::PixmapPaint {
            quality: tiny_skia::FilterQuality::Bicubic,
            ..Default::default()
        };
        pixmap.draw_pixmap(0, 0, image.as_ref(), &paint, transform, None);
    }
    Some(pixmap)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_entry_fields_come_from_the_entry_section() {
        let text = "[Desktop Entry]\nType=Application\nName=Ghostty\nIcon=com.mitchellh.ghostty\nStartupWMClass=com.mitchellh.ghostty\n\n[Desktop Action new]\nName=New Window\nIcon=other\n";
        assert_eq!(
            parse_desktop_entry(text),
            DesktopEntry {
                name: Some("Ghostty".into()),
                icon: Some("com.mitchellh.ghostty".into()),
                wm_class: Some("com.mitchellh.ghostty".into()),
            }
        );
        assert_eq!(
            parse_desktop_entry("[Other]\nName=X\n"),
            DesktopEntry::default()
        );
    }

    #[test]
    fn fallback_name_uses_last_app_id_segment() {
        assert_eq!(fallback_name("com.mitchellh.ghostty"), "Ghostty");
        assert_eq!(fallback_name("jetbrains-idea"), "Jetbrains-idea");
        assert_eq!(fallback_name(""), "");
    }

    #[test]
    fn best_icon_prefers_smallest_big_enough_bitmap() {
        let c = |s: Option<u32>, p: &str| (s, PathBuf::from(p));
        let all = vec![
            c(Some(48), "48"),
            c(Some(128), "128"),
            c(Some(256), "256"),
            c(None, "svg"),
        ];
        assert_eq!(best_icon(all.clone(), 80), Some("128".into()));
        assert_eq!(best_icon(all, 512), Some("svg".into()));
        assert_eq!(
            best_icon(vec![c(Some(48), "48"), c(Some(32), "32")], 80),
            Some("48".into())
        );
        assert_eq!(best_icon(Vec::new(), 80), None);
    }

    #[test]
    fn svg_icons_are_scaled_to_fit() {
        let dir = env::temp_dir().join(format!("rustwm-icon-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("icon.svg");
        fs::write(
            &path,
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="8"><rect width="16" height="8" fill="red"/></svg>"#,
        )
        .unwrap();

        let pixmap = load_icon(path.to_str().unwrap(), 64).unwrap();
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!((pixmap.width(), pixmap.height()), (64, 64));
        // Wide icons are centred vertically: filled in the middle, empty
        // at the top.
        assert_eq!(pixmap.pixel(32, 32).unwrap().alpha(), 255);
        assert_eq!(pixmap.pixel(32, 2).unwrap().alpha(), 0);
    }
}
