//! The default pointer image, drawn by the DRM backend when no client has
//! set its own cursor surface.

use std::{env, fs};

use smithay::{
    backend::{allocator::Fourcc, renderer::element::memory::MemoryRenderBuffer},
    utils::{Logical, Point, Transform},
};
use tracing::warn;
use xcursor::{CursorTheme, parser::parse_xcursor};

pub struct Cursor {
    pub buffer: MemoryRenderBuffer,
    pub hotspot: Point<i32, Logical>,
}

impl Cursor {
    /// Loads the "default" cursor from the XCURSOR_THEME theme, falling back
    /// to a plain arrow drawn in code when no theme is installed.
    ///
    /// `buffer_scale` is how many image pixels make up one logical pixel; a
    /// scale of 2 loads an image twice the size, which then appears at the
    /// normal size.
    pub fn load(buffer_scale: i32) -> Self {
        let theme = env::var("XCURSOR_THEME").unwrap_or_else(|_| "default".into());
        let size = env::var("XCURSOR_SIZE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(24u32);

        match load_theme_image(&theme, size, buffer_scale.max(1)) {
            Some(cursor) => cursor,
            None => {
                warn!(theme, "cursor theme not found; using built-in cursor");
                fallback()
            }
        }
    }
}

fn load_theme_image(theme: &str, size: u32, buffer_scale: i32) -> Option<Cursor> {
    let size = size * buffer_scale as u32;
    let path = CursorTheme::load(theme).load_icon("default")?;
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
