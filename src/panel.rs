//! The panel is an outside layer-shell bar (Waybar through `rustwm-panel`). rustwm only starts it and, in auto-hide mode, keeps it off screen until the pointer touches the screen edge it sits on.

use std::time::Duration;

use smithay::{
    desktop::{LayerSurface, layer_map_for_output},
    output::Output,
    reexports::{
        calloop::{
            RegistrationToken,
            timer::{TimeoutAction, Timer},
        },
        wayland_server::protocol::wl_surface::WlSurface,
    },
    utils::{Logical, Point, Rectangle},
};
use tracing::{info, warn};

use crate::{
    commands,
    config::{PanelMode, PanelPosition},
    state::State,
};

#[derive(Default)]
pub struct Panel {
    surface: Option<(Output, LayerSurface)>,
    shown: bool,
    hide_timer: Option<RegistrationToken>,
}

impl State {
    pub fn start_panel(&mut self) {
        let config = &self.config.panel;
        if config.command.is_empty() {
            return;
        }
        info!(command = config.command, ?config.position, ?config.mode, "starting panel");
        let envs = [
            ("RUSTWM_PANEL_POSITION", config.position.name()),
            ("RUSTWM_PANEL_MODE", config.mode.name()),
        ];
        commands::run_with(&config.command, &self.child_env(), &envs, || {});
    }

    fn auto_hide(&self) -> bool {
        self.config.panel.mode == PanelMode::AutoHide
    }

    pub fn is_panel(&self, namespace: &str) -> bool {
        self.auto_hide() && namespace == self.config.panel.namespace
    }

    pub fn panel_created(&mut self, output: Output, layer: LayerSurface) {
        self.panel.surface = Some((output, layer));
        self.panel.shown = true;
    }

    /// Takes the panel off screen once it has drawn its first frame, so it starts hidden.
    pub fn panel_committed(&mut self, surface: &WlSurface, has_buffer: bool) {
        let first_frame = has_buffer
            && self
                .panel
                .surface
                .as_ref()
                .is_some_and(|(_, layer)| layer.wl_surface() == surface);
        if first_frame
            && self.panel.shown
            && self.panel.hide_timer.is_none()
            && !self.pointer_over_panel()
        {
            self.hide_panel();
        }
    }

    pub fn panel_destroyed(&mut self, surface: &WlSurface) {
        if self
            .panel
            .surface
            .as_ref()
            .is_some_and(|(_, layer)| layer.wl_surface() == surface)
        {
            self.cancel_panel_hide();
            self.panel = Panel::default();
        }
    }

    /// Shows the panel when the pointer reaches its screen edge, and hides it a moment after the pointer leaves it.
    pub fn panel_pointer_moved(&mut self, pos: Point<f64, Logical>) {
        let Some((output, _)) = self.panel.surface.clone() else {
            return;
        };
        if self.is_locked() {
            return;
        }
        if !self.panel.shown {
            if let Some(geo) = self.space.output_geometry(&output) {
                let at_edge = match self.config.panel.position {
                    PanelPosition::Top => pos.y < (geo.loc.y + 1) as f64,
                    PanelPosition::Bottom => pos.y >= (geo.loc.y + geo.size.h - 1) as f64,
                };
                if at_edge {
                    self.show_panel();
                }
            }
        } else if self.pointer_over_panel_at(pos) {
            self.cancel_panel_hide();
        } else if self.panel.hide_timer.is_none() {
            let delay = Duration::from_millis(self.config.panel.hide_delay_ms);
            let timer =
                self.loop_handle
                    .insert_source(Timer::from_duration(delay), |_, _, state| {
                        state.panel.hide_timer = None;
                        if !state.pointer_over_panel() {
                            state.hide_panel();
                        }
                        TimeoutAction::Drop
                    });
            match timer {
                Ok(token) => self.panel.hide_timer = Some(token),
                Err(err) => warn!(%err, "failed to schedule hiding the panel"),
            }
        }
    }

    fn panel_geometry(&self) -> Option<Rectangle<i32, Logical>> {
        let (output, layer) = self.panel.surface.as_ref()?;
        let output_loc = self.space.output_geometry(output)?.loc;
        let geometry = layer_map_for_output(output).layer_geometry(layer)?;
        Some(Rectangle::new(geometry.loc + output_loc, geometry.size))
    }

    fn pointer_over_panel(&self) -> bool {
        let pos = self
            .seat
            .get_pointer()
            .map(|p| p.current_location())
            .unwrap_or_default();
        self.pointer_over_panel_at(pos)
    }

    fn pointer_over_panel_at(&self, pos: Point<f64, Logical>) -> bool {
        self.panel_geometry()
            .is_some_and(|geo| geo.to_f64().contains(pos))
    }

    fn show_panel(&mut self) {
        let Some((output, layer)) = self.panel.surface.clone() else {
            return;
        };
        let mut map = layer_map_for_output(&output);
        if let Err(err) = map.map_layer(&layer) {
            warn!(?err, "failed to show the panel");
            return;
        }
        map.arrange();
        self.panel.shown = true;
    }

    fn hide_panel(&mut self) {
        let Some((output, layer)) = self.panel.surface.clone() else {
            return;
        };
        layer_map_for_output(&output).unmap_layer(&layer);
        self.panel.shown = false;
    }

    fn cancel_panel_hide(&mut self) {
        if let Some(token) = self.panel.hide_timer.take() {
            self.loop_handle.remove(token);
        }
    }
}
