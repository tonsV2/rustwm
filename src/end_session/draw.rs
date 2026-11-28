//! What the end-session dialog shows and where its parts go. Pure, with no
//! Smithay rendering: [`layout`] is the only source of geometry, for both
//! drawing and the pointer's hit tests.

use fontdue::Font;
use resvg::tiny_skia::Pixmap;
use smithay::utils::{Logical, Point, Rectangle};

use crate::{
    overview::draw_icon,
    switcher::{
        ACCENT, BACKGROUND, Canvas, HIGHLIGHT, Image, NAME_COLOR, RADIUS, TEXT_COLOR, TextStyle,
    },
};

// Sizes in logical pixels.
const WIDTH: i32 = 440;
const PADDING: i32 = 24;
const TITLE_HEIGHT: i32 = 28;
pub const TITLE_SIZE: f32 = 20.0;
const LINE_HEIGHT: i32 = 20;
pub const TEXT_SIZE: f32 = 14.0;
const DETAIL_SIZE: f32 = 12.0;
/// Between the title, the text, the rows and the buttons.
const GAP: i32 = 12;
const ROW_HEIGHT: i32 = 40;
pub const ROW_ICON: i32 = 24;
/// Between a row's icon and its text.
const ROW_TEXT_GAP: i32 = 12;
const BUTTON_HEIGHT: i32 = 40;
const BUTTON_GAP: i32 = 12;
const BUTTON_RADIUS: f32 = 10.0;
/// How far the selected button's ring reaches around it.
const RING: f32 = 2.0;
/// From the bottom of the area to the bottom of a panel placed there.
const BOTTOM_MARGIN: i32 = 48;

/// The panel behind the buttons, opaque, so the ring drawn behind a
/// selected button does not show through it.
const BUTTON_BASE: [f32; 4] = [BACKGROUND[0], BACKGROUND[1], BACKGROUND[2], 1.0];
const RING_COLOR: [f32; 4] = [0.95, 0.95, 0.95, 0.85];

/// The widest the text may be.
pub const TEXT_WIDTH: i32 = WIDTH - 2 * PADDING;

/// Where the panel goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// In the middle of the screen, over the dimmed desktop.
    Centre,
    /// At the bottom, out of the way of the windows' save prompts.
    Bottom,
}

/// An application listed in the dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub name: String,
    /// Dimmer, beside the name; may be empty.
    pub detail: String,
    /// An icon name; without one, the name's first letter is shown.
    pub icon: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ButtonView {
    pub label: String,
    /// Drawn in the accent colour.
    pub suggested: bool,
}

/// Everything the panel shows, compared to decide whether to draw it again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    pub title: String,
    /// Already wrapped to [`TEXT_WIDTH`].
    pub lines: Vec<String>,
    pub rows: Vec<Row>,
    /// How many applications there are beyond the rows.
    pub more: usize,
    pub buttons: Vec<ButtonView>,
    pub selected: usize,
    pub placement: Placement,
}

/// Where the panel's parts are, in logical pixels relative to the output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub panel: Rectangle<i32, Logical>,
    title: Rectangle<i32, Logical>,
    lines: Vec<Rectangle<i32, Logical>>,
    rows: Vec<Rectangle<i32, Logical>>,
    more: Option<Rectangle<i32, Logical>>,
    pub buttons: Vec<Rectangle<i32, Logical>>,
}

/// What is under the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Button(usize),
    Panel,
    Outside,
}

impl Layout {
    /// What is at `pos`, relative to the output.
    pub fn hit(&self, pos: Point<f64, Logical>) -> Hit {
        if let Some(index) = self
            .buttons
            .iter()
            .position(|button| button.to_f64().contains(pos))
        {
            Hit::Button(index)
        } else if self.panel.to_f64().contains(pos) {
            Hit::Panel
        } else {
            Hit::Outside
        }
    }
}

/// Lays `view` out in `area`, relative to the output: in its middle, or
/// at its bottom.
pub fn layout(view: &View, area: Rectangle<i32, Logical>) -> Layout {
    let width = WIDTH.min(area.size.w).max(1);
    let inner = width - 2 * PADDING;
    // Each part's top relative to the panel, top to bottom.
    let mut y = PADDING;
    let mut next = |height: i32| {
        let top = y;
        y += height;
        top
    };
    let title = (next(TITLE_HEIGHT), TITLE_HEIGHT);
    if !view.lines.is_empty() {
        next(GAP / 2);
    }
    let lines: Vec<(i32, i32)> = view
        .lines
        .iter()
        .map(|_| (next(LINE_HEIGHT), LINE_HEIGHT))
        .collect();
    if !view.rows.is_empty() {
        next(GAP / 2);
    }
    let rows: Vec<(i32, i32)> = view
        .rows
        .iter()
        .map(|_| (next(ROW_HEIGHT), ROW_HEIGHT))
        .collect();
    let more = (view.more > 0).then(|| (next(LINE_HEIGHT), LINE_HEIGHT));
    next(2 * GAP);
    let buttons_top = next(BUTTON_HEIGHT);
    next(PADDING);
    let height = y;

    let left = area.loc.x + (area.size.w - width) / 2;
    let top = match view.placement {
        Placement::Centre => area.loc.y + (area.size.h - height) / 2,
        Placement::Bottom => area.loc.y + area.size.h - BOTTOM_MARGIN - height,
    }
    .max(area.loc.y);
    let line = |(y, h): (i32, i32)| -> Rectangle<i32, Logical> {
        Rectangle::new((left + PADDING, top + y).into(), (inner, h).into())
    };

    let count = view.buttons.len() as i32;
    let button_width = if count > 0 {
        (inner - (count - 1) * BUTTON_GAP) / count
    } else {
        0
    };
    let buttons = (0..count)
        .map(|i| {
            Rectangle::new(
                (
                    left + PADDING + i * (button_width + BUTTON_GAP),
                    top + buttons_top,
                )
                    .into(),
                (button_width, BUTTON_HEIGHT).into(),
            )
        })
        .collect();
    Layout {
        panel: Rectangle::new((left, top).into(), (width, height).into()),
        title: line(title),
        lines: lines.into_iter().map(line).collect(),
        rows: rows.into_iter().map(line).collect(),
        more: more.map(line),
        buttons,
    }
}

/// Draws the panel at `scale`. `icons` has one entry per row.
pub fn draw(
    font: Option<&Font>,
    view: &View,
    layout: &Layout,
    icons: &[Option<&Pixmap>],
    scale: f64,
) -> Image {
    let panel = layout.panel;
    let width = (panel.size.w as f64 * scale).round() as i32;
    let height = (panel.size.h as f64 * scale).round() as i32;
    let mut canvas = Canvas::new(width, height);
    // Scale from logical to image pixels, exact for this image's size.
    let s = width as f32 / panel.size.w.max(1) as f32;
    let rect = |r: Rectangle<i32, Logical>| {
        (
            (r.loc.x - panel.loc.x) as f32 * s,
            (r.loc.y - panel.loc.y) as f32 * s,
            r.size.w as f32 * s,
            r.size.h as f32 * s,
        )
    };
    let style = |size: f32, color| TextStyle {
        size: size * s,
        color,
    };

    canvas.rounded_rect(
        (0.0, 0.0, width as f32, height as f32),
        RADIUS * s,
        BACKGROUND,
    );

    for (index, (button, place)) in view.buttons.iter().zip(&layout.buttons).enumerate() {
        let (x, y, w, h) = rect(*place);
        let radius = BUTTON_RADIUS * s;
        if index == view.selected {
            let ring = RING * s;
            canvas.rounded_rect(
                (x - ring, y - ring, w + 2.0 * ring, h + 2.0 * ring),
                radius + ring,
                RING_COLOR,
            );
            canvas.rounded_rect((x, y, w, h), radius, BUTTON_BASE);
        }
        let color = if button.suggested { ACCENT } else { HIGHLIGHT };
        canvas.rounded_rect((x, y, w, h), radius, color);
        if let Some(font) = font {
            canvas.centred_text(
                font,
                &button.label,
                style(TEXT_SIZE, TEXT_COLOR),
                x + w / 2.0,
                w - 8.0 * s,
                y + h / 2.0,
            );
        }
    }

    for (index, (row, place)) in view.rows.iter().zip(&layout.rows).enumerate() {
        let (x, y, w, h) = rect(*place);
        let icon_size = ROW_ICON as f32 * s;
        let icon_box = (x, y + (h - icon_size) / 2.0, icon_size, icon_size);
        let icon = icons.get(index).copied().flatten();
        draw_icon(&mut canvas, font, icon, icon_box, &row.name);
        let Some(font) = font else {
            continue;
        };
        let left = x + icon_size + ROW_TEXT_GAP as f32 * s;
        let room = x + w - left;
        let middle = y + h / 2.0;
        let used = canvas.left_text(
            font,
            &row.name,
            style(TEXT_SIZE, TEXT_COLOR),
            left,
            room,
            middle,
        );
        if !row.detail.is_empty() {
            let gap = 8.0 * s;
            canvas.left_text(
                font,
                &row.detail,
                style(DETAIL_SIZE, NAME_COLOR),
                left + used + gap,
                (room - used - gap).max(0.0),
                middle,
            );
        }
    }

    let Some(font) = font else {
        return canvas.into_image();
    };
    let (x, y, w, h) = rect(layout.title);
    canvas.centred_text(
        font,
        &view.title,
        style(TITLE_SIZE, TEXT_COLOR),
        x + w / 2.0,
        w,
        y + h / 2.0,
    );
    for (text, place) in view.lines.iter().zip(&layout.lines) {
        let (x, y, w, h) = rect(*place);
        canvas.centred_text(
            font,
            text,
            style(TEXT_SIZE, NAME_COLOR),
            x + w / 2.0,
            w,
            y + h / 2.0,
        );
    }
    if let Some(place) = layout.more {
        let (x, y, w, h) = rect(place);
        canvas.left_text(
            font,
            &format!("{} more", view.more),
            style(TEXT_SIZE, NAME_COLOR),
            x,
            w,
            y + h / 2.0,
        );
    }
    canvas.into_image()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(rows: usize, buttons: usize, placement: Placement) -> View {
        View {
            title: "Power Off".into(),
            lines: vec!["The system will power off automatically in 60 seconds.".into()],
            rows: (0..rows)
                .map(|i| Row {
                    name: format!("App {i}"),
                    detail: String::new(),
                    icon: None,
                })
                .collect(),
            more: 0,
            buttons: (0..buttons)
                .map(|i| ButtonView {
                    label: format!("Button {i}"),
                    suggested: i + 1 == buttons,
                })
                .collect(),
            selected: buttons - 1,
            placement,
        }
    }

    fn screen() -> Rectangle<i32, Logical> {
        Rectangle::from_size((1280, 800).into())
    }

    fn pixel(image: &Image, x: i32, y: i32) -> [u8; 4] {
        let i = ((y * image.width + x) * 4) as usize;
        image.pixels[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn buttons_are_inside_the_panel_equal_and_in_order() {
        let layout = layout(&view(0, 3, Placement::Centre), screen());
        assert_eq!(layout.buttons.len(), 3);
        for button in &layout.buttons {
            assert!(layout.panel.contains_rect(*button));
            assert_eq!(button.size, layout.buttons[0].size);
        }
        assert!(
            layout
                .buttons
                .windows(2)
                .all(|pair| pair[0].loc.x + pair[0].size.w < pair[1].loc.x)
        );
    }

    #[test]
    fn panel_is_centred_or_at_the_bottom_and_grows_with_rows() {
        let centre = layout(&view(0, 2, Placement::Centre), screen());
        let middle = centre.panel.loc + centre.panel.size.downscale(2).to_point();
        assert!((middle.x - 640).abs() <= 1 && (middle.y - 400).abs() <= 1);

        let area = Rectangle::new((0, 30).into(), (1280, 770).into());
        let bottom = layout(&view(2, 2, Placement::Bottom), area);
        assert_eq!(
            bottom.panel.loc.y + bottom.panel.size.h,
            800 - BOTTOM_MARGIN
        );
        assert_eq!(
            bottom.panel.size.h,
            centre.panel.size.h + GAP / 2 + 2 * ROW_HEIGHT
        );
    }

    #[test]
    fn hits_tell_buttons_panel_and_outside_apart() {
        let layout = layout(&view(0, 2, Placement::Centre), screen());
        let centre = |r: Rectangle<i32, Logical>| (r.loc + r.size.downscale(2).to_point()).to_f64();
        assert_eq!(layout.hit(centre(layout.buttons[1])), Hit::Button(1));
        assert_eq!(layout.hit(centre(layout.title)), Hit::Panel);
        assert_eq!(layout.hit((5.0, 5.0).into()), Hit::Outside);
    }

    #[test]
    fn panel_follows_scale_and_suggests_the_confirming_button() {
        let view = view(1, 2, Placement::Centre);
        let layout = layout(&view, screen());
        let image = draw(None, &view, &layout, &[None], 1.5);
        assert_eq!(
            image.width,
            (layout.panel.size.w as f64 * 1.5).round() as i32
        );
        assert_eq!(
            image.height,
            (layout.panel.size.h as f64 * 1.5).round() as i32
        );
        // The corners are rounded off.
        assert_eq!(pixel(&image, 0, 0)[3], 0);
        let at = |r: Rectangle<i32, Logical>| {
            let x = (r.loc.x - layout.panel.loc.x) as f64 + r.size.w as f64 / 2.0;
            let y = (r.loc.y - layout.panel.loc.y) as f64 + r.size.h as f64 / 2.0;
            pixel(&image, (x * 1.5) as i32, (y * 1.5) as i32)
        };
        // Pixels are B, G, R, A: the confirming button is bluer.
        let cancel = at(layout.buttons[0]);
        let confirm = at(layout.buttons[1]);
        assert!(confirm[0] > cancel[0] + 60);
        assert!(confirm[0] > confirm[2]);
    }
}
