//! xdg-activation: lets one application bring another's window forward,
//! such as a terminal opening a link in the running browser.
//!
//! The requesting application first gets a token, then hands it to the
//! other application, which presents it with its window. Tokens are only
//! handed out to the client with keyboard focus for an input event it has
//! just received, and only honoured for a short while, so applications
//! cannot steal focus on their own.

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
            return false;
        };
        let Some(keyboard) = self.seat.get_keyboard() else {
            return false;
        };
        let same_seat = Seat::<State>::from_resource(seat).as_ref() == Some(&self.seat);
        // The event must have come after the keyboard entered its current
        // surface, and that surface must belong to the requesting client.
        let recent = keyboard
            .last_enter()
            .is_some_and(|enter| serial.is_no_older_than(&enter));
        let focused = keyboard
            .current_focus()
            .and_then(|surface| surface.client())
            .map(|client| client.id())
            == data.client_id;
        same_seat && recent && focused
    }

    fn request_activation(
        &mut self,
        _token: XdgActivationToken,
        token_data: XdgActivationTokenData,
        surface: WlSurface,
    ) {
        if token_data.timestamp.elapsed() > TOKEN_LIFETIME {
            return;
        }
        let Some(window) = self.window_for_surface(&surface) else {
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

delegate_xdg_activation!(State);
