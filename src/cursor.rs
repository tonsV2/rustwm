//! The pointer images the DRM backend draws: the arrow when no client has set its own cursor surface, and every shape a client asks for by name through the cursor-shape protocol.

use std::{collections::HashMap, env, fs};

use smithay::{
    backend::{allocator::Fourcc, renderer::element::memory::MemoryRenderBuffer},
    input::pointer::CursorIcon,
    utils::{Logical, Point, Transform},
};
use tracing::warn;
use xcursor::{CursorTheme, parser::parse_xcursor};

pub struct Cursor {
    pub buffer: MemoryRenderBuffer,
    pub hotspot: Point<i32, Logical>,
}

/// The XCURSOR_THEME theme's images, loaded the first time each shape is shown.
pub struct Cursors {
    theme: CursorTheme,
    size: u32,
    buffer_scale: i32,
    loaded: HashMap<CursorIcon, Cursor>,
}

impl Cursors {
    /// `buffer_scale` is how many image pixels make up one logical pixel; a scale of 2 loads an image twice the size, which then appears at the normal size.
    pub fn load(buffer_scale: i32) -> Self {
        let theme = env::var("XCURSOR_THEME").unwrap_or_else(|_| "default".into());
        let size = env::var("XCURSOR_SIZE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(24u32);
        Self {
            theme: CursorTheme::load(&theme),
            size,
            buffer_scale: buffer_scale.max(1),
            loaded: HashMap::new(),
        }
    }

    /// The image for `icon`, trying its alternative names and then the arrow, and a plain arrow drawn in code when the theme has neither.
    pub fn get(&mut self, icon: CursorIcon) -> &Cursor {
        if !self.loaded.contains_key(&icon) {
            let names = std::iter::once(icon.name()).chain(icon.alt_names().iter().copied());
            let cursor = names
                .chain(["default", "left_ptr"])
                .find_map(|name| self.load_image(name))
                .unwrap_or_else(|| {
                    warn!(
                        name = icon.name(),
                        "cursor not found in theme; using built-in cursor"
                    );
                    fallback()
                });
            self.loaded.insert(icon, cursor);
        }
        &self.loaded[&icon]
    }

    fn load_image(&self, name: &str) -> Option<Cursor> {
        load_theme_image(&self.theme, name, self.size, self.buffer_scale)
    }
}

fn load_theme_image(
    theme: &CursorTheme,
    name: &str,
    size: u32,
    buffer_scale: i32,
) -> Option<Cursor> {
    let size = size * buffer_scale as u32;
    let path = theme.load_icon(name)?;
    let data = fs::read(path).ok()?;
    let images = parse_xcursor(&data)?;
    let image = images
        .iter()
        .min_by_key(|image| (image.size as i32 - size as i32).abs())?;

    Some(Cursor {
        // xcursor's "rgba" pixels are laid out as ARGB8888 in memory.
        buffer: MemoryRenderBuffer::from_slice(
            &image.pixels_rgba,
            Fourcc::Argb8888,
            (image.width as i32, image.height as i32),
            buffer_scale,
            Transform::Normal,
            None,
        ),
        hotspot: (
            image.xhot as i32 / buffer_scale,
            image.yhot as i32 / buffer_scale,
        )
            .into(),
    })
}

/// A 12x18 white arrow with a black outline.
fn fallback() -> Cursor {
    const W: usize = 12;
    const H: usize = 18;
    let mut pixels = vec![0u8; W * H * 4];
    for y in 0..H {
        // The arrow widens by one pixel per row, up to the full width.
        let row_width = (y + 1).min(W);
        for x in 0..row_width {
            let edge = x == 0 || x + 1 == row_width || y + 1 == H;
            let [b, g, r, a] = if edge {
                [0, 0, 0, 255]
            } else {
                [255, 255, 255, 255]
            };
            let i = (y * W + x) * 4;
            pixels[i..i + 4].copy_from_slice(&[b, g, r, a]);
        }
    }
    Cursor {
        buffer: MemoryRenderBuffer::from_slice(
            &pixels,
            Fourcc::Argb8888,
            (W as i32, H as i32),
            1,
            Transform::Normal,
            None,
        ),
        hotspot: (0, 0).into(),
    }
}
