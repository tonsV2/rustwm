//! wlr-layer-shell: surfaces anchored to an output rather than managed as
//! windows. Fuzzel draws itself as one of these.

use smithay::{
    backend::renderer::utils::with_renderer_surface_state,
    delegate_layer_shell,
    desktop::{LayerSurface, layer_map_for_output},
    output::Output,
    reexports::wayland_server::protocol::{wl_output::WlOutput, wl_surface::WlSurface},
    wayland::{
        compositor::with_states,
        shell::wlr_layer::{
            Layer, LayerSurface as WlrLayerSurface, LayerSurfaceData, WlrLayerShellHandler,
            WlrLayerShellState,
        },
    },
};
use tracing::{info, warn};

use crate::state::State;

impl WlrLayerShellHandler for State {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.layer_shell_state
    }

    fn new_layer_surface(
        &mut self,
        surface: WlrLayerSurface,
        wl_output: Option<WlOutput>,
        layer: Layer,
        namespace: String,
    ) {
        let Some(output) = wl_output
            .as_ref()
            .and_then(Output::from_resource)
            .or_else(|| self.primary_output())
        else {
            warn!(namespace, "layer surface but no output; closing it");
            surface.send_close();
            return;
        };
        info!(
            namespace,
            ?layer,
            output = output.name(),
            "layer surface created"
        );

        let is_panel = self.is_panel(&namespace);
        let layer = LayerSurface::new(surface, namespace);
        if let Err(err) = layer_map_for_output(&output).map_layer(&layer) {
            warn!(?err, "failed to map layer surface");
            return;
        }
        if is_panel {
            self.panel_created(output, layer);
        }
    }

    fn layer_destroyed(&mut self, surface: WlrLayerSurface) {
        let wl_surface = surface.wl_surface().clone();
        self.panel_destroyed(&wl_surface);
        if let Some((output, layer)) = self.layer_for_surface(&wl_surface) {
            info!(namespace = layer.namespace(), "layer surface destroyed");
            layer_map_for_output(&output).unmap_layer(&layer);
            // Its exclusive zone, if any, is gone, so windows may grow.
            self.relayout_output(&output);
        }

        if self.layer_focus.as_ref() == Some(&wl_surface) {
            self.layer_focus = None;
            self.sync_focus();
        }
    }
}
delegate_layer_shell!(State);

/// Arranges layer surfaces after a commit and hands keyboard focus to ones
/// that ask for it.
pub fn handle_commit(state: &mut State, surface: &WlSurface) {
    let Some((output, layer)) = state.layer_for_surface(surface) else {
        return;
    };

    let initial_configure_sent = with_states(surface, |states| {
        states
            .data_map
            .get::<LayerSurfaceData>()
            .and_then(|data| data.lock().ok())
            .map(|data| data.initial_configure_sent)
            .unwrap_or(false)
    });

    // Arranging before the first configure lets it carry the right size.
    let zones_changed = layer_map_for_output(&output).arrange();
    if !initial_configure_sent {
        layer.layer_surface().send_configure();
    }
    if zones_changed {
        state.relayout_output(&output);
    }

    // A launcher on the top or overlay layer that wants the keyboard gets it
    // as soon as it is visible, and keeps it until it goes away.
    let mapped = with_renderer_surface_state(surface, |s| s.buffer().is_some()).unwrap_or(false);
    state.panel_committed(surface, mapped);
    let wants_focus = mapped
        && layer.can_receive_keyboard_focus()
        && matches!(layer.layer(), Layer::Top | Layer::Overlay);
    let has_focus = state.layer_focus.as_ref() == Some(surface);

    if wants_focus && !has_focus {
        state.layer_focus = Some(surface.clone());
        state.sync_focus();
    } else if !wants_focus && has_focus {
        state.layer_focus = None;
        state.sync_focus();
    }
}
