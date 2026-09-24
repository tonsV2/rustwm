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
//!
//! A click on a shell surface, such as a mako notification or the panel,
//! is also lent to the next token request that follows within a moment.
//! Mako hands its own token to the app that sent the notification, but
//! apps such as Ghostty ignore it and ask for a new one with their last
//! key press, which is refused once another window has the keyboard.

use std::time::{Duration, Instant};

use smithay::{
    delegate_xdg_activation,
    input::Seat,
    reexports::wayland_server::{Resource, backend::ClientId, protocol::wl_surface::WlSurface},
    wayland::xdg_activation::{
        XdgActivationHandler, XdgActivationState, XdgActivationToken, XdgActivationTokenData,
    },
};
use tracing::info;

use crate::state::State;

/// How long a token stays valid after it was handed out.
const TOKEN_LIFETIME: Duration = Duration::from_secs(10);

/// How long a click on a shell surface is lent to the next token request.
const SHELL_CLICK_LENDING: Duration = Duration::from_secs(2);

impl XdgActivationHandler for State {
    fn activation_state(&mut self) -> &mut XdgActivationState {
        &mut self.xdg_activation_state
    }

    fn token_created(&mut self, _token: XdgActivationToken, data: XdgActivationTokenData) -> bool {
        if let Some((clicked, shell_client)) = self.shell_click.take()
            && clicked.elapsed() <= SHELL_CLICK_LENDING
            && data.client_id.as_ref() != Some(&shell_client)
        {
            info!(
                app_id = ?data.app_id,
                client = self.client_name(data.client_id.clone()),
                shell = self.client_name(Some(shell_client)),
                "activation token created for a click on the shell"
            );
            return true;
        }
        let Some((serial, seat)) = &data.serial else {
            info!(
                app_id = ?data.app_id,
                client = self.client_name(data.client_id.clone()),
                "activation token refused: no input event"
            );
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
        let keyboard = self.seat.get_keyboard();
        let pointer = self.seat.get_pointer();

        // A key or click in the focused client: the event must have come
        // after the keyboard entered its current surface, and that surface
        // must belong to the requesting client.
        let keyboard_ok = keyboard.as_ref().is_some_and(|keyboard| {
            keyboard
                .last_enter()
                .is_some_and(|enter| serial.is_no_older_than(&enter))
                && client(keyboard.current_focus()) == data.client_id
        });

        // A click on the client under the pointer, which may not take the
        // keyboard: the event must be newer than the pointer entering its
        // surface, so hovering alone is not enough.
        let pointer_ok = pointer.as_ref().is_some_and(|pointer| {
            pointer
                .last_enter()
                .is_some_and(|enter| *serial != enter && serial.is_no_older_than(&enter))
                && client(pointer.current_focus()) == data.client_id
        });

        let name = self.client_name(data.client_id.clone());
        if keyboard_ok || pointer_ok {
            info!(app_id = ?data.app_id, client = name, "activation token created");
            let on_shell = pointer_ok
                && pointer
                    .as_ref()
                    .and_then(|pointer| pointer.current_focus())
                    .is_some_and(|surface| self.layer_for_surface(&surface).is_some());
            if on_shell && let Some(client) = data.client_id.clone() {
                self.shell_click = Some((Instant::now(), client));
            }
            return true;
        }
        let keyboard_focus =
            self.client_name(client(keyboard.as_ref().and_then(|k| k.current_focus())));
        let pointer_focus =
            self.client_name(client(pointer.as_ref().and_then(|p| p.current_focus())));
        info!(
            app_id = ?data.app_id,
            client = name,
            ?serial,
            keyboard_focus,
            keyboard_enter = ?keyboard.as_ref().and_then(|k| k.last_enter()),
            pointer_focus,
            pointer_enter = ?pointer.as_ref().and_then(|p| p.last_enter()),
            "activation token refused: not from a recent input event"
        );
        false
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
    /// A client's process as `name (pid)`, for the log: the app id in a
    /// token request is often missing.
    fn client_name(&self, client: Option<ClientId>) -> String {
        let Some(credentials) = client.and_then(|id| {
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
