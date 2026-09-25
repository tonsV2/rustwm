//! The panel is an outside layer-shell bar (Waybar through `rustwm-panel`). rustwm only starts it and, in auto-hide mode, keeps it off screen until the pointer touches the screen edge it sits on. It slides in and out over the configured times: while sliding in, the bar's surface is not in the layer map yet and the compositor draws it where it is along the way, then maps it when it arrives; while sliding out, it was unmapped when the slide began. While it is on screen the windows are laid out beside it, as they always are in the always shown mode, so it pushes them aside rather than covering them. It also stays on screen while the overview is open.

use std::time::{Duration, Instant};

use smithay::{
    backend::renderer::element::{Kind, surface::render_elements_from_surface_tree},
    desktop::{LayerSurface, PopupManager, layer_map_for_output},
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
    switcher::SwitcherElement,
};

#[derive(Default)]
pub struct Panel {
    surface: Option<(Output, LayerSurface)>,
    /// Whether the panel is meant to be on screen: mapped, or sliding in.
    shown: bool,
    /// Whether the surface is in the layer map, where it gets input and
    /// pushes windows aside.
    mapped: bool,
    /// Whether the bar has drawn its first frame, after which it starts
    /// hidden.
    drawn: bool,
    hide_timer: Option<RegistrationToken>,
    slide: Option<Slide>,
}

/// The panel sliding in or out.
struct Slide {
    start: Instant,
    /// How much of the panel was on screen when the slide began, 0 to 1.
    from: f64,
    showing: bool,
    /// How long a slide all the way in or out takes.
    duration: Duration,
}

impl Slide {
    /// How much of the panel is on screen at `now`, 0 to 1, and whether
    /// the slide is over.
    fn visible(&self, now: Instant) -> (f64, bool) {
        let target = if self.showing { 1.0 } else { 0.0 };
        let length = self.duration.as_secs_f64() * (target - self.from).abs();
        let elapsed = now.saturating_duration_since(self.start).as_secs_f64();
        let t = if length > 0.0 {
            (elapsed / length).clamp(0.0, 1.0)
        } else {
            1.0
        };
        // Quick at first, settling at the end.
        let eased = 1.0 - (1.0 - t).powi(3);
        (self.from + (target - self.from) * eased, t >= 1.0)
    }
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
        self.panel.mapped = true;
    }

    /// Takes the panel off screen once it has drawn its first frame, so it starts hidden.
    pub fn panel_committed(&mut self, surface: &WlSurface, has_buffer: bool) {
        let is_panel = self
            .panel
            .surface
            .as_ref()
            .is_some_and(|(_, layer)| layer.wl_surface() == surface);
        if !is_panel || !has_buffer || self.panel.drawn {
            return;
        }
        self.panel.drawn = true;
        if self.panel.shown && !self.panel_in_use() && !self.overview.open {
            self.hide_panel(false);
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
        if self.is_locked() || self.overview.open {
            return;
        }
        if !self.panel.shown {
            if let Some(geo) = self.space.output_geometry(&output) {
                let at_edge = match self.config.panel.position {
                    PanelPosition::Top => pos.y < (geo.loc.y + 1) as f64,
                    PanelPosition::Bottom => pos.y >= (geo.loc.y + geo.size.h - 1) as f64,
                };
                if at_edge {
                    self.show_panel(true);
                }
            }
        } else if self.panel_in_use_at(pos) {
            self.cancel_panel_hide();
        } else if self.panel.hide_timer.is_none() {
            let delay = Duration::from_millis(self.config.panel.hide_delay_ms);
            let timer =
                self.loop_handle
                    .insert_source(Timer::from_duration(delay), |_, _, state| {
                        state.panel.hide_timer = None;
                        if !state.panel_in_use() {
                            state.hide_panel(true);
                        }
                        TimeoutAction::Drop
                    });
            match timer {
                Ok(token) => self.panel.hide_timer = Some(token),
                Err(err) => warn!(%err, "failed to schedule hiding the panel"),
            }
        }
    }

    /// Where the panel is when fully on screen, relative to its output.
    /// While it is not in the layer map its size comes from its buffer and
    /// its place from the configured edge.
    fn panel_rect(&self) -> Option<Rectangle<i32, Logical>> {
        let (output, layer) = self.panel.surface.as_ref()?;
        if self.panel.mapped {
            return layer_map_for_output(output).layer_geometry(layer);
        }
        let geo = self.space.output_geometry(output)?;
        let size = layer.bbox().size;
        let y = match self.config.panel.position {
            PanelPosition::Top => 0,
            PanelPosition::Bottom => geo.size.h - size.h,
        };
        Some(Rectangle::new(((geo.size.w - size.w) / 2, y).into(), size))
    }

    fn panel_geometry(&self) -> Option<Rectangle<i32, Logical>> {
        let (output, _) = self.panel.surface.as_ref()?;
        let output_loc = self.space.output_geometry(output)?.loc;
        let rect = self.panel_rect()?;
        Some(Rectangle::new(rect.loc + output_loc, rect.size))
    }

    fn panel_in_use(&self) -> bool {
        let pos = self
            .seat
            .get_pointer()
            .map(|p| p.current_location())
            .unwrap_or_default();
        self.panel_in_use_at(pos)
    }

    /// The pointer is over the panel, or one of its menus (such as a tray icon's) is open.
    fn panel_in_use_at(&self, pos: Point<f64, Logical>) -> bool {
        let has_popup = self.panel.surface.as_ref().is_some_and(|(_, layer)| {
            PopupManager::popups_for_surface(layer.wl_surface())
                .next()
                .is_some()
        });
        has_popup
            || self
                .panel_geometry()
                .is_some_and(|geo| geo.to_f64().contains(pos))
    }

    /// Keeps the panel on screen while the overview is open, as part of the
    /// picture, and lets it hide again afterwards. Windows are not pushed
    /// aside for it meanwhile, since the overview covers them; if the panel
    /// stays because the pointer is on it, they are pushed once the overview
    /// closes.
    pub fn overview_panel(&mut self, open: bool) {
        if !self.auto_hide() {
            return;
        }
        if open {
            self.cancel_panel_hide();
            if !self.panel.mapped {
                self.show_panel(false);
            }
        } else if self.panel.shown {
            if self.panel_in_use() {
                if let Some((output, _)) = self.panel.surface.clone() {
                    self.relayout_output(&output);
                }
            } else {
                self.hide_panel(true);
            }
        }
    }

    /// Brings the panel on screen, sliding it in if `animate`, else at once.
    fn show_panel(&mut self, animate: bool) {
        if self.panel.surface.is_none() || self.panel.mapped {
            return;
        }
        self.panel.shown = true;
        if !animate {
            self.map_panel();
            return;
        }
        let from = self.panel_visible();
        self.panel.slide = Some(Slide {
            start: Instant::now(),
            from,
            showing: true,
            duration: Duration::from_millis(self.config.panel.slide_in_ms),
        });
    }

    /// Puts the panel in the layer map, where it gets input, and lays the
    /// windows out beside it unless the overview is open.
    fn map_panel(&mut self) {
        let Some((output, layer)) = self.panel.surface.clone() else {
            return;
        };
        let mut map = layer_map_for_output(&output);
        if let Err(err) = map.map_layer(&layer) {
            warn!(?err, "failed to show the panel");
            return;
        }
        map.arrange();
        drop(map);
        self.panel.mapped = true;
        self.panel.slide = None;
        if !self.overview.open {
            self.relayout_output(&output);
        }
    }

    /// Takes the panel out of the layer map, giving the windows its room
    /// back, and slides it out if `animate`.
    fn hide_panel(&mut self, animate: bool) {
        let Some((output, layer)) = self.panel.surface.clone() else {
            return;
        };
        let from = self.panel_visible();
        if self.panel.mapped {
            let mut map = layer_map_for_output(&output);
            map.unmap_layer(&layer);
            map.arrange();
            drop(map);
            self.panel.mapped = false;
            if !self.overview.open {
                self.relayout_output(&output);
            }
        }
        self.panel.shown = false;
        self.panel.slide = animate.then(|| Slide {
            start: Instant::now(),
            from,
            showing: false,
            duration: Duration::from_millis(self.config.panel.slide_out_ms),
        });
    }

    /// How much of the panel is on screen right now, 0 to 1.
    fn panel_visible(&self) -> f64 {
        match &self.panel.slide {
            Some(slide) => slide.visible(Instant::now()).0,
            None if self.panel.mapped => 1.0,
            None => 0.0,
        }
    }

    fn cancel_panel_hide(&mut self) {
        if let Some(token) = self.panel.hide_timer.take() {
            self.loop_handle.remove(token);
        }
    }

    /// The panel while it slides in or out, drawn where it is along the way.
    /// Empty otherwise. A slide that has arrived maps the panel.
    pub fn panel_slide_elements(&mut self, output: &Output) -> Vec<SwitcherElement> {
        let Some(slide) = &self.panel.slide else {
            return Vec::new();
        };
        let (visible, done) = slide.visible(Instant::now());
        if done {
            if slide.showing {
                self.map_panel();
            } else {
                self.panel.slide = None;
            }
            return Vec::new();
        }
        let Some((panel_output, layer)) = self.panel.surface.clone() else {
            return Vec::new();
        };
        if &panel_output != output {
            return Vec::new();
        }
        let Some(rect) = self.panel_rect() else {
            return Vec::new();
        };
        let offset = (1.0 - visible) * rect.size.h as f64;
        let y = match self.config.panel.position {
            PanelPosition::Top => rect.loc.y as f64 - offset,
            PanelPosition::Bottom => rect.loc.y as f64 + offset,
        };
        let scale = output.current_scale().fractional_scale();
        let location = Point::<f64, Logical>::from((rect.loc.x as f64, y))
            .to_physical(scale)
            .to_i32_round::<i32>();
        render_elements_from_surface_tree(
            self.backend.renderer(),
            layer.wl_surface(),
            location,
            scale,
            1.0,
            Kind::Unspecified,
        )
        .into_iter()
        .map(SwitcherElement::Surface)
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SLIDE: Duration = Duration::from_millis(200);

    #[test]
    fn slide_eases_from_where_it_started_and_ends_on_time() {
        let start = Instant::now();
        let slide = Slide {
            start,
            from: 0.0,
            showing: true,
            duration: SLIDE,
        };
        assert_eq!(slide.visible(start), (0.0, false));
        let (half, done) = slide.visible(start + SLIDE / 2);
        assert!(half > 0.5 && half < 1.0 && !done);
        assert_eq!(slide.visible(start + SLIDE), (1.0, true));

        // Sliding out from half way takes half the time.
        let slide = Slide {
            start,
            from: 0.5,
            showing: false,
            duration: SLIDE,
        };
        let (visible, done) = slide.visible(start + SLIDE / 4);
        assert!(visible < 0.5 && visible > 0.0 && !done);
        assert_eq!(slide.visible(start + SLIDE / 2), (0.0, true));
    }
}
