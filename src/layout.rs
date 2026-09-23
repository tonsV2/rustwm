//! Window layout geometry and snapping rules.
//!
//! Pure logic with no Smithay types, so it can be unit tested in isolation.

/// An integer rectangle in logical coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Rect {
    pub fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

/// Where a managed window sits on its output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    Fullscreen,
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// A snapping direction, as pressed with Super+Arrow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

/// Which part of one axis a layout covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Span {
    Full,
    /// Left or top half.
    First,
    /// Right or bottom half.
    Second,
}

impl Layout {
    /// Splits the layout into its horizontal and vertical spans.
    fn spans(self) -> (Span, Span) {
        match self {
            Layout::Fullscreen => (Span::Full, Span::Full),
            Layout::Left => (Span::First, Span::Full),
            Layout::Right => (Span::Second, Span::Full),
            Layout::Top => (Span::Full, Span::First),
            Layout::Bottom => (Span::Full, Span::Second),
            Layout::TopLeft => (Span::First, Span::First),
            Layout::TopRight => (Span::Second, Span::First),
            Layout::BottomLeft => (Span::First, Span::Second),
            Layout::BottomRight => (Span::Second, Span::Second),
        }
    }

    fn from_spans(horizontal: Span, vertical: Span) -> Self {
        match (horizontal, vertical) {
            (Span::Full, Span::Full) => Layout::Fullscreen,
            (Span::First, Span::Full) => Layout::Left,
            (Span::Second, Span::Full) => Layout::Right,
            (Span::Full, Span::First) => Layout::Top,
            (Span::Full, Span::Second) => Layout::Bottom,
            (Span::First, Span::First) => Layout::TopLeft,
            (Span::Second, Span::First) => Layout::TopRight,
            (Span::First, Span::Second) => Layout::BottomLeft,
            (Span::Second, Span::Second) => Layout::BottomRight,
        }
    }

    /// The half of the screen a single direction snaps to.
    pub fn half(direction: Direction) -> Self {
        match direction {
            Direction::Left => Layout::Left,
            Direction::Right => Layout::Right,
            Direction::Up => Layout::Top,
            Direction::Down => Layout::Bottom,
        }
    }

    /// Computes the absolute geometry of this layout inside `area`.
    ///
    /// The second half gets `size - size / 2`, so odd sizes are split
    /// without gaps or overlaps.
    pub fn geometry(self, area: Rect) -> Rect {
        let (horizontal, vertical) = self.spans();
        let (x, width) = split(area.x, area.width, horizontal);
        let (y, height) = split(area.y, area.height, vertical);
        Rect::new(x, y, width, height)
    }

    /// Computes the layout after pressing `direction`.
    ///
    /// `first_in_hold` is true for the first arrow pressed since Super went
    /// down. That press snaps to a plain half. Further presses in the same
    /// hold change only their own axis, so Left then Up gives TopLeft.
    /// Pressing towards where the window already is toggles back to
    /// fullscreen, which is the only way back from a snapped layout.
    pub fn after_direction(self, direction: Direction, first_in_hold: bool) -> Self {
        let target = if first_in_hold {
            Layout::half(direction)
        } else {
            let (horizontal, vertical) = self.spans();
            match direction {
                Direction::Left => Layout::from_spans(Span::First, vertical),
                Direction::Right => Layout::from_spans(Span::Second, vertical),
                Direction::Up => Layout::from_spans(horizontal, Span::First),
                Direction::Down => Layout::from_spans(horizontal, Span::Second),
            }
        };

        if target == self {
            Layout::Fullscreen
        } else {
            target
        }
    }

    /// The quarters of the screen this layout covers, one bit each: top left, top right, bottom left, bottom right.
    fn quarters(self) -> u8 {
        let (horizontal, vertical) = self.spans();
        let columns = match horizontal {
            Span::Full => 0b11,
            Span::First => 0b01,
            Span::Second => 0b10,
        };
        match vertical {
            Span::Full => columns | columns << 2,
            Span::First => columns,
            Span::Second => columns << 2,
        }
    }

    /// The xdg-toplevel edges this layout touches, used for "tiled" states
    /// so clients drop shadows and rounded corners on those sides.
    pub fn tiled_edges(self) -> TiledEdges {
        let (horizontal, vertical) = self.spans();
        TiledEdges {
            left: horizontal != Span::Second,
            right: horizontal != Span::First,
            top: vertical != Span::Second,
            bottom: vertical != Span::First,
        }
    }
}

/// Which output edges a layout touches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TiledEdges {
    pub left: bool,
    pub right: bool,
    pub top: bool,
    pub bottom: bool,
}

/// Picks the snapped windows that can be seen, given the layouts of a stack of windows from the top down (`None` for floating dialogs, which are passed over): those above the topmost full-screen window that are not hidden behind snapped windows above them. Returns their indices, top first.
pub fn visible_tiles(stack: &[Option<Layout>]) -> Vec<usize> {
    let mut covered = 0;
    let mut visible = Vec::new();
    for (index, layout) in stack.iter().enumerate() {
        match layout {
            None => {}
            Some(Layout::Fullscreen) => break,
            Some(layout) => {
                let quarters = layout.quarters();
                if quarters & !covered != 0 {
                    visible.push(index);
                }
                covered |= quarters;
            }
        }
    }
    visible
}

/// Returns `(start, length)` of `span` along an axis of `length` at `start`.
fn split(start: i32, length: i32, span: Span) -> (i32, i32) {
    let first = length / 2;
    match span {
        Span::Full => (start, length),
        Span::First => (start, first),
        Span::Second => (start + first, length - first),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Layout; 9] = [
        Layout::Fullscreen,
        Layout::Left,
        Layout::Right,
        Layout::Top,
        Layout::Bottom,
        Layout::TopLeft,
        Layout::TopRight,
        Layout::BottomLeft,
        Layout::BottomRight,
    ];

    #[test]
    fn geometry_even_output() {
        let area = Rect::new(0, 0, 1000, 800);
        let cases = [
            (Layout::Fullscreen, Rect::new(0, 0, 1000, 800)),
            (Layout::Left, Rect::new(0, 0, 500, 800)),
            (Layout::Right, Rect::new(500, 0, 500, 800)),
            (Layout::Top, Rect::new(0, 0, 1000, 400)),
            (Layout::Bottom, Rect::new(0, 400, 1000, 400)),
            (Layout::TopLeft, Rect::new(0, 0, 500, 400)),
            (Layout::TopRight, Rect::new(500, 0, 500, 400)),
            (Layout::BottomLeft, Rect::new(0, 400, 500, 400)),
            (Layout::BottomRight, Rect::new(500, 400, 500, 400)),
        ];
        for (layout, expected) in cases {
            assert_eq!(layout.geometry(area), expected, "{layout:?}");
        }
    }

    #[test]
    fn geometry_is_offset_by_area_origin() {
        let area = Rect::new(1920, 30, 1000, 800);
        assert_eq!(
            Layout::BottomRight.geometry(area),
            Rect::new(2420, 430, 500, 400)
        );
    }

    #[test]
    fn odd_output_halves_cover_exactly() {
        let area = Rect::new(0, 0, 1921, 1081);

        let left = Layout::Left.geometry(area);
        let right = Layout::Right.geometry(area);
        assert_eq!(left.x + left.width, right.x);
        assert_eq!(left.width + right.width, 1921);

        let top = Layout::Top.geometry(area);
        let bottom = Layout::Bottom.geometry(area);
        assert_eq!(top.y + top.height, bottom.y);
        assert_eq!(top.height + bottom.height, 1081);
    }

    #[test]
    fn odd_output_quarters_tile_exactly() {
        let area = Rect::new(0, 0, 1921, 1081);
        let quarters = [
            Layout::TopLeft,
            Layout::TopRight,
            Layout::BottomLeft,
            Layout::BottomRight,
        ]
        .map(|l| l.geometry(area));

        // Every pixel is covered by exactly one quarter.
        let mut coverage = vec![0u8; (area.width * area.height) as usize];
        for q in quarters {
            for y in q.y..q.y + q.height {
                for x in q.x..q.x + q.width {
                    coverage[(y * area.width + x) as usize] += 1;
                }
            }
        }
        assert!(coverage.iter().all(|&c| c == 1));
    }

    #[test]
    fn every_layout_stays_inside_area() {
        let area = Rect::new(0, 0, 1921, 1081);
        for layout in ALL {
            let g = layout.geometry(area);
            assert!(g.x >= 0 && g.y >= 0, "{layout:?}");
            assert!(g.x + g.width <= area.width, "{layout:?}");
            assert!(g.y + g.height <= area.height, "{layout:?}");
        }
    }

    #[test]
    fn first_press_snaps_to_half() {
        use Direction::*;
        assert_eq!(Layout::Fullscreen.after_direction(Left, true), Layout::Left);
        assert_eq!(Layout::Fullscreen.after_direction(Up, true), Layout::Top);
        // A new hold starts over rather than combining with the old layout.
        assert_eq!(Layout::TopLeft.after_direction(Right, true), Layout::Right);
        assert_eq!(Layout::TopLeft.after_direction(Left, true), Layout::Left);
    }

    #[test]
    fn combining_in_one_hold_gives_quarters() {
        use Direction::*;
        let hold = |first: Direction, second: Direction| {
            Layout::Fullscreen
                .after_direction(first, true)
                .after_direction(second, false)
        };
        assert_eq!(hold(Left, Up), Layout::TopLeft);
        assert_eq!(hold(Up, Left), Layout::TopLeft);
        assert_eq!(hold(Right, Up), Layout::TopRight);
        assert_eq!(hold(Left, Down), Layout::BottomLeft);
        assert_eq!(hold(Right, Down), Layout::BottomRight);
        assert_eq!(hold(Down, Right), Layout::BottomRight);
    }

    #[test]
    fn same_axis_in_one_hold_replaces() {
        use Direction::*;
        assert_eq!(Layout::Left.after_direction(Right, false), Layout::Right);
        assert_eq!(
            Layout::TopLeft.after_direction(Down, false),
            Layout::BottomLeft
        );
    }

    #[test]
    fn pressing_current_direction_returns_to_fullscreen() {
        use Direction::*;
        assert_eq!(Layout::Left.after_direction(Left, true), Layout::Fullscreen);
        assert_eq!(Layout::Top.after_direction(Up, true), Layout::Fullscreen);
        assert_eq!(
            Layout::Left.after_direction(Left, false),
            Layout::Fullscreen
        );
        assert_eq!(
            Layout::TopLeft.after_direction(Up, false),
            Layout::Fullscreen
        );
    }

    #[test]
    fn visible_tiles_stop_at_full_screen_and_skip_hidden_ones() {
        use Layout::*;
        // Four quarters over a full-screen window.
        let stack = [
            Some(TopLeft),
            Some(TopRight),
            Some(BottomLeft),
            Some(BottomRight),
            Some(Fullscreen),
            Some(Left),
        ];
        assert_eq!(visible_tiles(&stack), [0, 1, 2, 3]);

        // A left half behind two left quarters is hidden, a dialog is passed over.
        let stack = [
            Some(TopLeft),
            None,
            Some(BottomLeft),
            Some(Left),
            Some(Right),
        ];
        assert_eq!(visible_tiles(&stack), [0, 2, 4]);

        // A half partly covered by a quarter still shows.
        assert_eq!(visible_tiles(&[Some(TopLeft), Some(Left)]), [0, 1]);
        assert_eq!(
            visible_tiles(&[Some(Fullscreen), Some(Left)]),
            [] as [usize; 0]
        );
    }

    #[test]
    fn tiled_edges_match_layout() {
        let e = Layout::TopRight.tiled_edges();
        assert!(!e.left && e.right && e.top && !e.bottom);
        let f = Layout::Fullscreen.tiled_edges();
        assert!(f.left && f.right && f.top && f.bottom);
    }
}
