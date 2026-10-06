//! security-context: Flatpak tells rustwm which clients run in a sandbox.
//! It hands over a listening socket of its own, and every client that
//! connects through it is marked with the sandbox's details. Globals that
//! read the screen, list the windows, lock the screen or turn monitors off
//! are hidden from those clients, as [`is_privileged`] decides.

use std::sync::Arc;

use smithay::{
    delegate_security_context,
    reexports::wayland_server::Client,
    wayland::security_context::{
        SecurityContext, SecurityContextHandler, SecurityContextListenerSource,
    },
};
use tracing::{info, warn};

use crate::state::{ClientState, State};

/// Whether `client` may use the privileged globals: it connected through
/// rustwm's own socket rather than a sandbox's. A client without rustwm's
/// data is treated as sandboxed, to be safe.
pub fn is_privileged(client: &Client) -> bool {
    client
        .get_data::<ClientState>()
        .is_some_and(|data| data.security_context.is_none())
}

impl SecurityContextHandler for State {
    fn context_created(&mut self, source: SecurityContextListenerSource, context: SecurityContext) {
        info!(
            engine = context.sandbox_engine,
            app_id = context.app_id,
            "sandbox socket added"
        );
        // The source removes itself when the sandbox closes its end.
        let inserted = self
            .loop_handle
            .insert_source(source, move |stream, _, state| {
                let data = ClientState {
                    security_context: Some(context.clone()),
                    ..ClientState::default()
                };
                if let Err(err) = state.display_handle.insert_client(stream, Arc::new(data)) {
                    warn!(?err, "failed to add sandboxed client");
                }
            });
        if let Err(err) = inserted {
            warn!(%err, "failed to listen on a sandbox socket");
        }
    }
}
delegate_security_context!(State);
