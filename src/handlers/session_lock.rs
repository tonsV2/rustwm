//! ext-session-lock: a locker such as swaylock covers every output and
//! takes all input until the user unlocks.
//!
//! From the moment a lock is requested until it is released, no window,
//! layer surface or overlay is drawn: each output shows the locker's
//! surface for it, or a solid colour while there is none. If the locker
//! crashes, the session stays locked; a new locker can take over and
//! unlock it.

use smithay::{
    output::Output,
    reexports::{
        wayland_protocols::ext::session_lock::v1::server::{
            ext_session_lock_manager_v1::ExtSessionLockManagerV1,
            ext_session_lock_surface_v1::ExtSessionLockSurfaceV1,
            ext_session_lock_v1::{self, ExtSessionLockV1},
        },
        wayland_server::{
            Client, DataInit, Dispatch, DisplayHandle, Resource, delegate_dispatch,
            delegate_global_dispatch, protocol::wl_output::WlOutput,
        },
    },
    utils::SERIAL_COUNTER,
    wayland::session_lock::{
        ExtLockSurfaceUserData, LockSurface, SessionLockHandler, SessionLockManagerGlobalData,
        SessionLockManagerState, SessionLockState, SessionLocker,
    },
};
use tracing::{info, warn};

use crate::state::State;

/// Behind the lock screen, and all there is until the locker draws one.
pub const LOCKED_BACKGROUND: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// Whether the session is locked.
#[derive(Debug, Default)]
pub enum Lock {
    #[default]
    Unlocked,
    /// A locker asked to lock. Windows are hidden already, but the locker
    /// is only told the session is locked once a frame without them has
    /// been shown.
    Locking(SessionLocker),
    /// Locked until the locker unlocks.
    Locked,
}

/// The screen lock and the locker holding it.
#[derive(Debug, Default)]
pub struct ScreenLock {
    pub lock: Lock,
    /// The lock object of the client that holds the lock. Only it can
    /// unlock.
    locker: Option<ExtSessionLockV1>,
    /// The locker's surface for each output.
    surfaces: Vec<(Output, LockSurface)>,
}

impl State {
    pub fn is_locked(&self) -> bool {
        !matches!(self.screen_lock.lock, Lock::Unlocked)
    }

    /// The lock surface shown on `output`, if the locker has made one.
    pub fn lock_surface(&self, output: &Output) -> Option<&LockSurface> {
        self.screen_lock
            .surfaces
            .iter()
            .find(|(o, surface)| o == output && surface.alive())
            .map(|(_, surface)| surface)
    }

    /// The lock surface that gets keyboard focus while locked.
    pub fn lock_focus(&self) -> Option<&LockSurface> {
        let output = self.primary_output()?;
        self.lock_surface(&output)
    }

    /// Called by the backend after it presented a frame drawn while locked.
    /// Only now can the locker be told that the session is locked, since
    /// nothing from before the lock is on screen any more.
    pub fn lock_frame_shown(&mut self) {
        if let Lock::Locking(_) = self.screen_lock.lock
            && let Lock::Locking(locker) =
                std::mem::replace(&mut self.screen_lock.lock, Lock::Locked)
        {
            info!("session locked");
            locker.lock();
        }
    }

    /// Resizes the lock surfaces to their outputs, after an output changed
    /// size.
    pub fn configure_lock_surfaces(&self) {
        for (output, surface) in &self.screen_lock.surfaces {
            if let Some(geo) = self.space.output_geometry(output) {
                surface.with_pending_state(|state| {
                    state.size = Some((geo.size.w as u32, geo.size.h as u32).into());
                });
                surface.send_configure();
            }
        }
    }

    /// Takes input away from windows when the screen locks.
    fn start_lock(&mut self) {
        // An Alt+Tab switch in progress ends with nothing changed, and the
        // overview closes.
        self.focus.cancel_switch();
        self.switcher.hide();
        self.close_overview();
        // A popup menu or a held mouse button must not keep input flowing
        // to a window.
        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.unset_grab(self);
        }
        if let Some(pointer) = self.seat.get_pointer() {
            let time = self.start_time.elapsed().as_millis() as u32;
            pointer.unset_grab(self, SERIAL_COUNTER.next_serial(), time);
        }
        // A cursor surface is drawn by its client, which could be a window.
        self.cursor_status = smithay::input::pointer::CursorImageStatus::default_named();
        self.sync_focus();
        self.refresh_pointer_focus();
    }
}

impl SessionLockHandler for State {
    fn lock_state(&mut self) -> &mut SessionLockManagerState {
        &mut self.session_lock_state
    }

    fn lock(&mut self, confirmation: SessionLocker) {
        // Dropping the confirmation tells the client it did not get the lock.
        if self
            .screen_lock
            .locker
            .as_ref()
            .is_some_and(|locker| locker.is_alive())
        {
            warn!("lock requested while another locker holds it; refusing");
            return;
        }
        let was_locked = self.is_locked();
        self.screen_lock.locker = Some(confirmation.ext_session_lock().clone());
        // Surfaces of a locker that died are replaced by the new one's.
        self.screen_lock.surfaces.clear();

        if was_locked {
            // The screen shows no windows already; the new locker takes over.
            info!("new locker took over the locked session");
            self.screen_lock.lock = Lock::Locked;
            confirmation.lock();
            return;
        }

        info!("locking session");
        self.screen_lock.lock = Lock::Locking(confirmation);
        self.start_lock();
        // Nothing is on screen that could give away the session, such as
        // while another VT is active or the monitor is off, so the next
        // frame shown will be locked.
        if !self.is_showing() {
            self.lock_frame_shown();
        }
    }

    fn unlock(&mut self) {
        info!("session unlocked");
        self.screen_lock = ScreenLock::default();
        // Focus returns to the window that had it.
        self.sync_focus();
        self.refresh_pointer_focus();
    }

    fn new_surface(&mut self, surface: LockSurface, wl_output: WlOutput) {
        // Only the client holding the lock may cover the screen.
        let locker = self.screen_lock.locker.as_ref().and_then(|l| l.client());
        if locker.is_none() || surface.wl_surface().client() != locker {
            warn!("lock surface from a client not holding the lock; ignoring");
            return;
        }
        let Some(output) = Output::from_resource(&wl_output) else {
            return;
        };
        let Some(geo) = self.space.output_geometry(&output) else {
            return;
        };
        // Smithay sends the configure with this size once this returns.
        surface.with_pending_state(|state| {
            state.size = Some((geo.size.w as u32, geo.size.h as u32).into());
        });
        output.enter(surface.wl_surface());
        info!(output = output.name(), "lock surface created");

        self.screen_lock.surfaces.retain(|(o, _)| o != &output);
        self.screen_lock.surfaces.push((output, surface));
        self.sync_focus();
        self.refresh_pointer_focus();
    }
}

// The lock object is dispatched here rather than by the delegate macro, so
// that only the locker holding a confirmed lock can unlock. Smithay unlocks
// on any `unlock_and_destroy`, even from a client whose lock was refused.
delegate_global_dispatch!(State: [ExtSessionLockManagerV1: SessionLockManagerGlobalData] => SessionLockManagerState);
delegate_dispatch!(State: [ExtSessionLockManagerV1: ()] => SessionLockManagerState);
delegate_dispatch!(State: [ExtSessionLockSurfaceV1: ExtLockSurfaceUserData] => SessionLockManagerState);

impl Dispatch<ExtSessionLockV1, SessionLockState> for State {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &ExtSessionLockV1,
        request: ext_session_lock_v1::Request,
        data: &SessionLockState,
        dh: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if matches!(request, ext_session_lock_v1::Request::UnlockAndDestroy) {
            let holds_lock = matches!(state.screen_lock.lock, Lock::Locked)
                && state.screen_lock.locker.as_ref() == Some(resource);
            if !holds_lock {
                warn!("unlock from a client not holding the lock; refusing");
                resource.post_error(
                    ext_session_lock_v1::Error::InvalidUnlock,
                    "this lock was never confirmed",
                );
                return;
            }
        }
        <SessionLockManagerState as Dispatch<ExtSessionLockV1, SessionLockState, Self>>::request(
            state, client, resource, request, data, dh, data_init,
        );
    }
}
