//! relative-pointer and pointer-constraints: games, Blender and virtual
//! machines read raw mouse motion and lock the pointer in place or confine
//! it to their window, for mouse-look and grabbed pointers.
//!
//! Smithay keeps each constraint on its surface and turns it off when the
//! pointer leaves that surface; when to turn it on is up to rustwm. A
//! constraint starts only while its surface's window has the keyboard and
//! the pointer is over it, inside the region the client gave, and never in
//! the overview, on the lock screen or during Alt+Tab, so moving the focus
//! elsewhere, by Alt+Tab, Super or a lock, always frees the pointer. Once
//! it holds, the pointer stays on that surface even when a notification or
//! the panel shows up over it.

use smithay::{
    delegate_pointer_constraints, delegate_relative_pointer,
    desktop::{WindowSurfaceType, utils::under_from_surface_tree},
    input::pointer::{PointerHandle, RelativeMotionEvent},
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point},
    wayland::{
        compositor::{RegionAttributes, SubsurfaceCachedState, get_parent, with_states},
        pointer_constraints::{
            PointerConstraint, PointerConstraintsHandler, with_pointer_constraint,
        },
    },
};
use tracing::info;

use crate::state::State;

/// The constraint holding the pointer right now.
pub struct Held {
    /// The surface holding it, which keeps the pointer focus.
    surface: WlSurface,
    /// Where the surface's top left corner is.
    origin: Point<f64, Logical>,
    /// The pointer stays where it is and only raw motion is sent; otherwise
    /// it is confined to the surface.
    pub locked: bool,
    region: Option<RegionAttributes>,
}

impl Held {
    /// The surface and where it is, as pointer events take the focus.
    pub fn focus(&self) -> (WlSurface, Point<f64, Logical>) {
        (self.surface.clone(), self.origin)
    }
}

impl State {
    /// The active constraint of the surface with the pointer focus, if any.
    pub fn held_pointer(&self) -> Option<Held> {
        let pointer = self.seat.get_pointer()?;
        let surface = pointer.current_focus()?;
        if !self.may_constrain(&surface) {
            return None;
        }
        let origin = self.surface_origin(&surface)?;
        let held = with_pointer_constraint(&surface, &pointer, |constraint| {
            let constraint = constraint.filter(|c| c.is_active())?;
            Some(Held {
                surface: surface.clone(),
                origin,
                locked: matches!(&*constraint, PointerConstraint::Locked(_)),
                region: constraint.region().cloned(),
            })
        })?;
        // A window snapped or shrunk away from under a confined pointer
        // would leave it nowhere to go; it is let go until it comes back.
        let stranded = !held.locked && !self.confined_allows(&held, pointer.current_location());
        (!stranded).then_some(held)
    }

    /// Where a confined pointer moving from `old` towards `pos` ends up: as
    /// far as it can go, then sliding along the edge it met.
    pub fn confine(
        &self,
        held: &Held,
        old: Point<f64, Logical>,
        pos: Point<f64, Logical>,
    ) -> Point<f64, Logical> {
        let reached = self.reach(held, old, pos);
        let reached = self.reach(held, reached, (pos.x, reached.y).into());
        self.reach(held, reached, (reached.x, pos.y).into())
    }

    /// The furthest point from `from` towards `to` the confined pointer may
    /// go, found by halving the step; `from` itself if it may not move.
    fn reach(
        &self,
        held: &Held,
        from: Point<f64, Logical>,
        to: Point<f64, Logical>,
    ) -> Point<f64, Logical> {
        if self.confined_allows(held, to) {
            return to;
        }
        let along = |t: f64| from + (to - from).upscale(t);
        let (mut inside, mut outside) = (0.0, 1.0);
        for _ in 0..12 {
            let middle = (inside + outside) / 2.0;
            if self.confined_allows(held, along(middle)) {
                inside = middle;
            } else {
                outside = middle;
            }
        }
        along(inside)
    }

    /// Whether a confined pointer may be at `pos`: over the confining
    /// surface's input region and inside the region the client gave.
    fn confined_allows(&self, held: &Held, pos: Point<f64, Logical>) -> bool {
        is_over(&held.surface, held.origin, pos)
            && in_region(held.region.as_ref(), pos - held.origin)
    }

    /// Turns on the constraint of the surface with the pointer focus once
    /// it may hold: its window has the keyboard and the pointer is in its
    /// region. Called after the pointer moved, the keyboard focus changed or
    /// a client asked for a constraint.
    pub fn update_pointer_constraint(&mut self) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        let Some(surface) = pointer.current_focus() else {
            return;
        };
        if !self.may_constrain(&surface) {
            return;
        }
        let Some(origin) = self.surface_origin(&surface) else {
            return;
        };
        let pos = pointer.current_location();
        // The pointer focus can be stale, as after a window moved.
        if !is_over(&surface, origin, pos) {
            return;
        }
        with_pointer_constraint(&surface, &pointer, |constraint| {
            let Some(constraint) = constraint else {
                return;
            };
            if !constraint.is_active() && in_region(constraint.region(), pos - origin) {
                constraint.activate();
                info!("pointer constraint activated");
            }
        });
    }

    /// Turns off the active constraint of the surface with the pointer focus
    /// when it may no longer hold, such as when its window lost the keyboard
    /// or Alt+Tab started.
    pub fn release_pointer_constraint(&mut self) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        let Some(surface) = pointer.current_focus() else {
            return;
        };
        if self.may_constrain(&surface) {
            return;
        }
        with_pointer_constraint(&surface, &pointer, |constraint| {
            if let Some(constraint) = constraint.filter(|c| c.is_active()) {
                constraint.deactivate();
                info!("pointer constraint released");
            }
        });
    }

    /// Whether `surface` may hold a pointer constraint: it belongs to the
    /// window with the keyboard, outside the overview, the lock screen and
    /// Alt+Tab, whose thumbnails the pointer can click.
    fn may_constrain(&self, surface: &WlSurface) -> bool {
        if self.is_locked() || self.overview.open || self.focus.is_switching() {
            return false;
        }
        let Some(focus) = self.seat.get_keyboard().and_then(|k| k.current_focus()) else {
            return false;
        };
        root_of(surface) == focus
    }

    /// Where the top left corner of `surface`, part of a window, is: the
    /// window's place plus the offsets of the subsurfaces down to it.
    fn surface_origin(&self, surface: &WlSurface) -> Option<Point<f64, Logical>> {
        let mut offset = Point::<i32, Logical>::default();
        let mut current = surface.clone();
        while let Some(parent) = get_parent(&current) {
            offset += with_states(&current, |states| {
                states
                    .cached_state
                    .get::<SubsurfaceCachedState>()
                    .current()
                    .location
            });
            current = parent;
        }
        let window = self.window_for_surface(&current)?;
        let location = self.space.element_location(&window.window)? - window.window.geometry().loc;
        Some((location + offset).to_f64())
    }

    /// Sends raw motion to `under`, for clients using relative-pointer.
    pub fn send_relative_motion(
        &mut self,
        under: Option<(WlSurface, Point<f64, Logical>)>,
        event: Option<RelativeMotionEvent>,
    ) {
        let (Some(pointer), Some(event)) = (self.seat.get_pointer(), event) else {
            return;
        };
        pointer.relative_motion(self, under, &event);
    }
}

/// The surface a subsurface belongs to, or `surface` itself.
fn root_of(surface: &WlSurface) -> WlSurface {
    let mut root = surface.clone();
    while let Some(parent) = get_parent(&root) {
        root = parent;
    }
    root
}

/// Whether `pos` is over the input region of `surface`, whose top left
/// corner is at `origin`, or of one of its subsurfaces, such as a game's
/// video or HUD.
fn is_over(surface: &WlSurface, origin: Point<f64, Logical>, pos: Point<f64, Logical>) -> bool {
    under_from_surface_tree(surface, pos, origin.to_i32_round(), WindowSurfaceType::ALL).is_some()
}

/// Whether `pos`, relative to the surface, is in `region`; anywhere is
/// without one.
fn in_region(region: Option<&RegionAttributes>, pos: Point<f64, Logical>) -> bool {
    let pixel = (pos.x.floor() as i32, pos.y.floor() as i32);
    region.is_none_or(|region| region.contains(pixel))
}

impl PointerConstraintsHandler for State {
    fn new_constraint(&mut self, _surface: &WlSurface, _pointer: &PointerHandle<Self>) {
        self.update_pointer_constraint();
    }

    /// A game drawing its own cursor while the pointer is locked says where;
    /// the pointer goes there, without telling anyone, so that it shows up
    /// in that spot when the lock ends. Only a spot over the surface counts.
    fn cursor_position_hint(
        &mut self,
        surface: &WlSurface,
        pointer: &PointerHandle<Self>,
        location: Point<f64, Logical>,
    ) {
        let Some(held) = self.held_pointer() else {
            return;
        };
        let pos = held.origin + location;
        if held.locked && &held.surface == surface && is_over(surface, held.origin, pos) {
            pointer.set_location(pos);
        }
    }
}
delegate_pointer_constraints!(State);
delegate_relative_pointer!(State);
