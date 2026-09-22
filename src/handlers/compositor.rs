use smithay::{
    backend::renderer::utils::on_commit_buffer_handler,
    delegate_compositor, delegate_shm,
    reexports::wayland_server::{
        Client,
        protocol::{wl_buffer, wl_surface::WlSurface},
    },
    wayland::{
        buffer::BufferHandler,
        compositor::{
            CompositorClientState, CompositorHandler, CompositorState, get_parent,
            is_sync_subsurface,
        },
        shm::{ShmHandler, ShmState},
    },
};

use super::{handle_layer_commit, handle_xdg_commit};
use crate::state::{ClientState, State};

impl CompositorHandler for State {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        // Every client is inserted with ClientState in the socket listener,
        // so the data is always there.
        &client
            .get_data::<ClientState>()
            .expect("client inserted without ClientState")
            .compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        // Imports the new buffer so the renderer can draw it.
        on_commit_buffer_handler::<Self>(surface);

        // Subsurface commits only take effect with their parent, so let the
        // owning window refresh its cached state from the root surface.
        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            if let Some(window) = self.window_for_surface(&root) {
                window.window.on_commit();
            }
        }

        self.popups.commit(surface);
        handle_xdg_commit(self, surface);
        handle_layer_commit(self, surface);
    }
}

impl BufferHandler for State {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

impl ShmHandler for State {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

delegate_compositor!(State);
delegate_shm!(State);
