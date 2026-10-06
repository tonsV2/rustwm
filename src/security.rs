//! security-context: Flatpak tells rustwm which clients run in a sandbox.
//! It hands over a listening socket of its own, and every client that
//! connects through it is marked with the sandbox's details. Globals that
//! read the screen, list the windows, lock the screen, turn monitors off,
//! tell when the user is away or put surfaces above windows (layer-shell)
//! are hidden from those clients, as [`is_privileged`] decides.

use std::{io, os::unix::net::UnixStream, sync::Arc};

use smithay::{
    delegate_security_context,
    reexports::{
        calloop::{self, EventSource, Poll, PostAction, Readiness, Token, TokenFactory},
        wayland_server::Client,
    },
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
        let listener = SandboxListener(source);
        let inserted = self
            .loop_handle
            .insert_source(listener, move |stream, _, state| {
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

/// A sandbox's socket that stops listening when accepting fails, rather
/// than handing the error to the event loop, which would end the session.
/// A client hands over the socket, so it could be broken on purpose, and a
/// sandboxed app can run rustwm out of file descriptors.
struct SandboxListener(SecurityContextListenerSource);

impl EventSource for SandboxListener {
    type Event = UnixStream;
    type Metadata = ();
    type Ret = ();
    type Error = io::Error;

    fn process_events<F>(
        &mut self,
        readiness: Readiness,
        token: Token,
        callback: F,
    ) -> io::Result<PostAction>
    where
        F: FnMut(UnixStream, &mut ()),
    {
        self.0
            .process_events(readiness, token, callback)
            .or_else(|err| {
                warn!(%err, "sandbox socket failed; no more apps can connect through it");
                Ok(PostAction::Remove)
            })
    }

    fn register(&mut self, poll: &mut Poll, tokens: &mut TokenFactory) -> calloop::Result<()> {
        self.0.register(poll, tokens)
    }

    fn reregister(&mut self, poll: &mut Poll, tokens: &mut TokenFactory) -> calloop::Result<()> {
        self.0.reregister(poll, tokens)
    }

    fn unregister(&mut self, poll: &mut Poll) -> calloop::Result<()> {
        self.0.unregister(poll)
    }
}
