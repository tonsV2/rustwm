//! relative-pointer and pointer-constraints: games, Blender and virtual
//! machines read raw mouse motion and lock the pointer in place or confine
//! it to their window, for mouse-look and grabbed pointers.
//!
//! Smithay keeps each constraint on its surface and turns it off when the
//! pointer leaves that surface; when to turn it on is up to rustwm. A
//! constraint holds only while its surface's window has the keyboard and
//! the pointer is over it, inside the region the client gave, and never in
//! the overview or on the lock screen, so moving the focus elsewhere, by
//! Alt+Tab, Super or a lock, always frees the pointer.

use smithay::{
    delegate_pointer_constraints, delegate_relative_pointer,
    input::pointer::{PointerHandle, RelativeMotionEvent},
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point},
    wayland::{
        compositor::{RegionAttributes, get_parent},
        pointer_constraints::{
            PointerConstraint, PointerConstraintsHandler, with_pointer_constraint,
        },
    },
};

use tracing::info;

use crate::state::State;

/// The constraint holding the pointer right now.
pub enum Held {
    /// The pointer stays where it is; only relative motion is sent.
    Locked,
    /// The pointer stays on `surface`, whose top left corner is at
    /// `origin`, inside `region` when the client gave one.
    Confined {
        surface: WlSurface,
        origin: Point<f64, Logical>,
        region: Option<RegionAttributes>,
    },
}

impl State {
    /// The active constraint of the surface under the pointer, if any.
    pub fn held_pointer(&self) -> Option<Held> {
        let pointer = self.seat.get_pointer()?;
        let (surface, origin) = self.constrainable_surface(&pointer)?;
        with_pointer_constraint(&surface, &pointer, |constraint| {
            let constraint = constraint.filter(|c| c.is_active())?;
            Some(match &*constraint {
                PointerConstraint::Locked(_) => Held::Locked,
                PointerConstraint::Confined(confined) => Held::Confined {
                    surface: surface.clone(),
                    origin,
                    region: confined.region().cloned(),
                },
            })
        })
    }

    /// Whether a confined pointer may go to `pos`: still over the confining
    /// surface and inside its region.
    pub fn confined_allows(&self, held: &Held, pos: Point<f64, Logical>) -> bool {
        let Held::Confined {
            surface,
            origin,
            region,
        } = held
        else {
            return false;
        };
        let over = self
            .surface_under(pos)
            .is_some_and(|(under, _)| &under == surface);
        over && region
            .as_ref()
            .is_none_or(|region| region.contains((pos - *origin).to_i32_round()))
    }

    /// Turns on the constraint of the surface under the pointer once it may
    /// hold: its window has the keyboard and the pointer is in its region.
    /// Called after the pointer moved, the keyboard focus changed or a
    /// client asked for a constraint.
    pub fn update_pointer_constraint(&mut self) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        let Some((surface, origin)) = self.constrainable_surface(&pointer) else {
            return;
        };
        let pos = pointer.current_location();
        with_pointer_constraint(&surface, &pointer, |constraint| {
            let Some(constraint) = constraint else {
                return;
            };
            let inside = constraint
                .region()
                .is_none_or(|region| region.contains((pos - origin).to_i32_round()));
            if !constraint.is_active() && inside {
                constraint.activate();
                info!("pointer constraint activated");
            }
        });
    }

    /// Turns off the active constraint of the surface under the pointer when
    /// it may no longer hold, such as when its window lost the keyboard or
    /// Alt+Tab started.
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

    /// The surface under the pointer, with its top left corner, when it is
    /// the pointer's focus and may hold a constraint.
    fn constrainable_surface(
        &self,
        pointer: &PointerHandle<State>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        let focus = pointer.current_focus()?;
        let (surface, origin) = self.surface_under(pointer.current_location())?;
        (surface == focus && self.may_constrain(&surface)).then_some((surface, origin))
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
        let mut root = surface.clone();
        while let Some(parent) = get_parent(&root) {
            root = parent;
        }
        root == focus
    }

    /// Sends raw motion to the surface under the pointer, for clients
    /// using relative-pointer.
    pub fn send_relative_motion(&mut self, event: Option<RelativeMotionEvent>) {
        let (Some(pointer), Some(event)) = (self.seat.get_pointer(), event) else {
            return;
        };
        let under = self.surface_under(pointer.current_location());
        pointer.relative_motion(self, under, &event);
    }
}

impl PointerConstraintsHandler for State {
    fn new_constraint(&mut self, _surface: &WlSurface, _pointer: &PointerHandle<Self>) {
        self.update_pointer_constraint();
    }

    /// A game drawing its own cursor while the pointer is locked says where;
    /// the pointer goes there, without telling anyone, so that it shows up
    /// in that spot when the lock ends.
    fn cursor_position_hint(
        &mut self,
        surface: &WlSurface,
        pointer: &PointerHandle<Self>,
        location: Point<f64, Logical>,
    ) {
        let locked = with_pointer_constraint(surface, pointer, |constraint| {
            constraint.is_some_and(|c| c.is_active() && matches!(&*c, PointerConstraint::Locked(_)))
        });
        if !locked {
            return;
        }
        if let Some((under, origin)) = self.surface_under(pointer.current_location())
            && &under == surface
        {
            pointer.set_location(origin + location);
        }
    }
}
delegate_pointer_constraints!(State);
delegate_relative_pointer!(State);
