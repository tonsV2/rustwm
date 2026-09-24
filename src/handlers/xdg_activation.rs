//! xdg-activation: lets one application bring another's window forward,
//! such as a terminal opening a link in the running browser.
//!
//! The requesting application first gets a token, then hands it to the
//! other application, which presents it with its window. Tokens are only
//! handed out for an input event the requesting client has just received:
//! a key or click in the client with keyboard focus, or a click on the
//! client under the pointer, such as a notification from mako asking for
//! the app that sent it. They are only honoured for a short while, so
//! applications cannot steal focus on their own.

use std::time::Duration;

use smithay::{
    delegate_xdg_activation,
    input::Seat,
    reexports::wayland_server::{Resource, protocol::wl_surface::WlSurface},
    wayland::xdg_activation::{
        XdgActivationHandler, XdgActivationState, XdgActivationToken, XdgActivationTokenData,
    },
};
use tracing::info;

use crate::state::State;

/// How long a token stays valid after it was handed out.
const TOKEN_LIFETIME: Duration = Duration::from_secs(10);

impl XdgActivationHandler for State {
    fn activation_state(&mut self) -> &mut XdgActivationState {
        &mut self.xdg_activation_state
    }

    fn token_created(&mut self, _token: XdgActivationToken, data: XdgActivationTokenData) -> bool {
        let Some((serial, seat)) = &data.serial else {
            info!(app_id = ?data.app_id, client = self.client_name(&data), "activation token refused: no input event");
            return false;
        };
        if Seat::<State>::from_resource(seat).as_ref() != Some(&self.seat) {
            return false;
        }
        let client = |surface: Option<WlSurface>| {
            surface
                .and_then(|surface| surface.client())
                .map(|client| client.id())
        };

        // A key or click in the focused client: the event must have come
        // after the keyboard entered its current surface, and that surface
        // must belong to the requesting client.
        let keyboard = self.seat.get_keyboard();
        let keyboard_ok = keyboard.is_some_and(|keyboard| {
            keyboard
                .last_enter()
                .is_some_and(|enter| serial.is_no_older_than(&enter))
                && client(keyboard.current_focus()) == data.client_id
        });

        // A click on the client under the pointer, which may not take the
        // keyboard: the event must be newer than the pointer entering its
        // surface, so hovering alone is not enough.
        let pointer = self.seat.get_pointer();
        let pointer_ok = pointer.is_some_and(|pointer| {
            pointer
                .last_enter()
                .is_some_and(|enter| *serial != enter && serial.is_no_older_than(&enter))
                && client(pointer.current_focus()) == data.client_id
        });

        if !(keyboard_ok || pointer_ok) {
            info!(
                app_id = ?data.app_id,
                client = self.client_name(&data),
                "activation token refused: not from a recent input event"
            );
        }
        keyboard_ok || pointer_ok
    }

    fn request_activation(
        &mut self,
        _token: XdgActivationToken,
        token_data: XdgActivationTokenData,
        surface: WlSurface,
    ) {
        if token_data.timestamp.elapsed() > TOKEN_LIFETIME {
            info!("activation refused: token expired");
            return;
        }
        let Some(window) = self.window_for_surface(&surface) else {
            info!("activation refused: not a window");
            return;
        };
        let id = window.id;
        if !window.mapped {
            return;
        }
        info!(window = id.0, "window activated by request");
        self.focus_window(id);
    }
}

impl State {
    /// The requesting client's process as `name (pid)`, for the log: the
    /// app id is often missing.
    fn client_name(&self, data: &XdgActivationTokenData) -> String {
        let Some(credentials) = data.client_id.clone().and_then(|id| {
            self.display_handle
                .backend_handle()
                .get_client_credentials(id)
                .ok()
        }) else {
            return "unknown".into();
        };
        let pid = credentials.pid;
        let name = std::fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default();
        format!("{} ({pid})", name.trim())
    }
}

delegate_xdg_activation!(State);
