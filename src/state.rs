//! Compositor state: Smithay protocol state plus window-management policy.

use std::{
    ffi::OsString,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Context;
use smithay::{
    backend::renderer::gles::GlesRenderer,
    desktop::{
        LayerSurface, PopupManager, Space, Window, WindowSurfaceType, layer_map_for_output,
        utils::{send_frames_surface_tree, under_from_surface_tree},
    },
    input::{
        Seat, SeatState,
        keyboard::XkbConfig,
        pointer::{CursorImageStatus, MotionEvent},
    },
    output::Output,
    reexports::{
        calloop::{
            EventLoop, Interest, LoopHandle, LoopSignal, Mode, PostAction, RegistrationToken,
            generic::Generic,
        },
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::{
            Display, DisplayHandle,
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::wl_surface::WlSurface,
        },
    },
    utils::{Logical, Point, SERIAL_COUNTER},
    wayland::{
        compositor::{CompositorClientState, CompositorState},
        cursor_shape::CursorShapeManagerState,
        dmabuf::DmabufState,
        fractional_scale::FractionalScaleManagerState,
        idle_inhibit::IdleInhibitManagerState,
        output::OutputManagerState,
        selection::{data_device::DataDeviceState, primary_selection::PrimarySelectionState},
        session_lock::SessionLockManagerState,
        shell::{
            wlr_layer::{Layer, WlrLayerShellState},
            xdg::{XdgShellState, decoration::XdgDecorationState},
        },
        shm::ShmState,
        socket::ListeningSocketSource,
        viewporter::ViewporterState,
        xdg_activation::XdgActivationState,
    },
};
use tracing::{error, info, warn};

use crate::{
    capture::{image_copy::ImageCopyState, screencopy::ScreencopyState},
    config::{Config, KeyboardConfig},
    fade::Fade,
    focus::{FocusHistory, WindowId},
    group::Groups,
    handlers::{IdleNotifyState, OutputPowerState, ScreenLock},
    keyboard::Bindings,
    keymap,
    layout::{Layout, Rect},
    osd::Osd,
    overview::Overview,
    panel::Panel,
    session::ChildEnv,
    switcher::Switcher,
    udev::UdevData,
    window::ManagedWindow,
    winit::WinitData,
    xwayland::Satellite,
};

/// Where the compositor draws and gets input from.
pub enum Backend {
    /// A window inside an existing desktop session, for development.
    Winit(WinitData),
    /// Directly on the hardware, as a session of its own.
    Udev(UdevData),
}

impl Backend {
    pub fn renderer(&mut self) -> &mut GlesRenderer {
        match self {
            Backend::Winit(winit) => winit.backend.renderer(),
            Backend::Udev(udev) => &mut udev.renderer,
        }
    }
}

pub struct State {
    pub start_time: Instant,
    pub socket_name: OsString,
    pub display_handle: DisplayHandle,
    pub loop_handle: LoopHandle<'static, State>,
    pub loop_signal: LoopSignal,
    pub backend: Backend,

    // Smithay protocol state.
    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    // Some of these are never read, but dropping them would remove their
    // Wayland globals.
    #[allow(dead_code)]
    pub xdg_decoration_state: XdgDecorationState,
    pub layer_shell_state: WlrLayerShellState,
    pub shm_state: ShmState,
    pub dmabuf_state: DmabufState,
    #[allow(dead_code)]
    pub viewporter_state: ViewporterState,
    #[allow(dead_code)]
    pub fractional_scale_state: FractionalScaleManagerState,
    #[allow(dead_code)]
    pub cursor_shape_state: CursorShapeManagerState,
    #[allow(dead_code)]
    pub output_manager_state: OutputManagerState,
    pub xdg_activation_state: XdgActivationState,
    pub screencopy_state: ScreencopyState,
    pub image_copy_state: ImageCopyState,
    pub session_lock_state: SessionLockManagerState,
    pub idle_notify: IdleNotifyState,
    #[allow(dead_code)]
    pub idle_inhibit_state: IdleInhibitManagerState,
    pub output_power: OutputPowerState,
    pub seat_state: SeatState<State>,
    pub data_device_state: DataDeviceState,
    pub primary_selection_state: PrimarySelectionState,
    pub seat: Seat<State>,
    /// The pointer image clients asked for. Only the DRM backend draws it;
    /// nested, the host shows its own cursor.
    pub cursor_status: CursorImageStatus,

    /// Positions and stacking order of mapped windows, plus the outputs they
    /// are shown on. Smithay renders from this.
    pub space: Space<Window>,
    pub popups: PopupManager,

    // Window-management policy.
    /// Every toplevel the compositor knows about, mapped or not.
    pub windows: Vec<ManagedWindow>,
    next_window_id: u64,
    pub focus: FocusHistory,
    pub groups: Groups,
    /// The focused window and its layout when the current Super hold started, so a group member zoomed to full screen can go back to that tile.
    pub snap_hold: Option<(WindowId, Layout)>,
    pub bindings: Bindings,
    /// The held key whose action repeats, and the timer repeating it.
    pub key_repeat: Option<(u32, RegistrationToken)>,
    /// When a click on a layer surface, such as a notification, last got
    /// an activation token, and the client that got it. The next token
    /// request from another client within a moment is granted too.
    pub shell_click: Option<(Instant, ClientId)>,
    /// Runs X11 apps; `None` when xwayland-satellite is not installed.
    pub xwayland: Option<Satellite>,
    pub switcher: Switcher,
    /// The Super tap overview.
    pub overview: Overview,
    /// The brightness and volume indicator.
    pub osd: Osd,
    pub panel: Panel,
    pub config: Config,
    /// A layer surface that has taken keyboard focus. While set, it keeps
    /// focus over windows.
    pub layer_focus: Option<WlSurface>,
    /// The screen lock (ext-session-lock). While locked, only the lock
    /// screen is drawn and gets input.
    pub screen_lock: ScreenLock,
    /// Surfaces whose clients asked to keep the session from going idle,
    /// such as a video player. Only visible ones count.
    pub idle_inhibitors: Vec<WlSurface>,
    /// The screen fading to black before the lock and back on input.
    pub fade: Fade,
}

impl State {
    pub fn new(
        event_loop: &mut EventLoop<'static, State>,
        display: Display<State>,
        backend: Backend,
        dmabuf_state: DmabufState,
        config: Config,
    ) -> anyhow::Result<Self> {
        let dh = display.handle();

        let compositor_state = CompositorState::new::<Self>(&dh);
        let xdg_shell_state = XdgShellState::new::<Self>(&dh);
        let xdg_decoration_state = XdgDecorationState::new::<Self>(&dh);
        let layer_shell_state = WlrLayerShellState::new::<Self>(&dh);
        let shm_state = ShmState::new::<Self>(&dh, vec![]);
        let viewporter_state = ViewporterState::new::<Self>(&dh);
        let fractional_scale_state = FractionalScaleManagerState::new::<Self>(&dh);
        let cursor_shape_state = CursorShapeManagerState::new::<Self>(&dh);
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&dh);
        let xdg_activation_state = XdgActivationState::new::<Self>(&dh);
        let screencopy_state = ScreencopyState::new::<Self>(&dh);
        let image_copy_state = ImageCopyState::new::<Self>(&dh);
        let session_lock_state = SessionLockManagerState::new::<Self, _>(&dh, |_| true);
        let idle_notify = IdleNotifyState::new(&dh);
        let idle_inhibit_state = IdleInhibitManagerState::new::<Self>(&dh);
        let output_power = OutputPowerState::new(&dh);
        let data_device_state = DataDeviceState::new::<Self>(&dh);
        let primary_selection_state = PrimarySelectionState::new::<Self>(&dh);

        let mut seat_state = SeatState::new();
        let mut seat = seat_state.new_wl_seat(&dh, "seat0");
        add_keyboard(&mut seat, &config.keyboard)?;
        seat.add_pointer();

        let socket_name = init_wayland_listener(display, event_loop)?;
        let osd = Osd::new(&event_loop.handle())?;
        let overview = Overview::new(&event_loop.handle())?;

        Ok(Self {
            start_time: Instant::now(),
            socket_name,
            display_handle: dh,
            loop_handle: event_loop.handle(),
            loop_signal: event_loop.get_signal(),
            backend,
            compositor_state,
            xdg_shell_state,
            xdg_decoration_state,
            layer_shell_state,
            shm_state,
            dmabuf_state,
            viewporter_state,
            fractional_scale_state,
            cursor_shape_state,
            output_manager_state,
            xdg_activation_state,
            screencopy_state,
            image_copy_state,
            session_lock_state,
            idle_notify,
            idle_inhibit_state,
            output_power,
            seat_state,
            data_device_state,
            primary_selection_state,
            seat,
            cursor_status: CursorImageStatus::default_named(),
            space: Space::default(),
            popups: PopupManager::default(),
            windows: Vec::new(),
            next_window_id: 0,
            focus: FocusHistory::default(),
            groups: Groups::default(),
            snap_hold: None,
            bindings: Bindings::default(),
            key_repeat: None,
            shell_click: None,
            xwayland: None,
            switcher: Switcher::default(),
            overview,
            osd,
            panel: Panel::default(),
            config,
            layer_focus: None,
            screen_lock: ScreenLock::default(),
            idle_inhibitors: Vec::new(),
            fade: Fade::default(),
        })
    }

    /// Runs after every batch of events. Replies to clients are sent here
    /// rather than only after rendering: the host stops asking the nested
    /// window to redraw while it is hidden, and clients waiting on a reply
    /// would hang.
    pub fn after_dispatch(&mut self) {
        self.space.refresh();
        self.popups.cleanup();
        self.update_idle_inhibit();
        if let Err(err) = self.display_handle.flush_clients() {
            warn!(%err, "failed to flush clients");
        }
    }

    /// Tells clients on `output` that a frame was shown, so they draw the
    /// next one.
    pub fn send_frame_callbacks(&self, output: &Output) {
        let time = self.start_time.elapsed();
        let throttle = Some(Duration::ZERO);
        // Hidden windows wait for the lock to end.
        if self.is_locked() {
            if let Some(surface) = self.lock_surface(output) {
                send_frames_surface_tree(surface.wl_surface(), output, time, throttle, |_, _| {
                    Some(output.clone())
                });
            }
            return;
        }
        for window in self.space.elements() {
            window.send_frame(output, time, throttle, |_, _| Some(output.clone()));
        }
        let mut layers = layer_map_for_output(output);
        for layer in layers.layers() {
            layer.send_frame(output, time, throttle, |_, _| Some(output.clone()));
        }
        layers.cleanup();
    }

    /// Moves windows whose output went away onto `output` and lays them out
    /// there. With a single output, this is how windows survive a monitor
    /// being replaced.
    pub fn adopt_windows(&mut self, output: &Output) {
        let known: Vec<Output> = self.space.outputs().cloned().collect();
        let mut adopted = Vec::new();
        for window in &mut self.windows {
            if !known.contains(&window.output) {
                window.output = output.clone();
                adopted.push(window.id);
            }
        }
        for id in adopted {
            self.apply_layout(id);
        }
    }

    pub fn next_window_id(&mut self) -> WindowId {
        self.next_window_id += 1;
        WindowId(self.next_window_id)
    }

    pub fn window(&self, id: WindowId) -> Option<&ManagedWindow> {
        self.windows.iter().find(|w| w.id == id)
    }

    /// Finds the managed window whose toplevel surface is `surface`.
    pub fn window_for_surface(&self, surface: &WlSurface) -> Option<&ManagedWindow> {
        self.windows
            .iter()
            .find(|w| w.wl_surface() == Some(surface))
    }

    /// The output new windows and layer surfaces go to.
    ///
    /// With a single output this is simply that output. Multi-monitor support
    /// would pick the output under the pointer here instead.
    pub fn primary_output(&self) -> Option<Output> {
        self.space.outputs().next().cloned()
    }

    /// The part of `output` available to windows: its full area minus
    /// exclusive zones claimed by layer surfaces such as panels.
    pub fn usable_area(&self, output: &Output) -> Option<Rect> {
        let geo = self.space.output_geometry(output)?;
        let zone = layer_map_for_output(output).non_exclusive_zone();
        Some(Rect::new(
            geo.loc.x + zone.loc.x,
            geo.loc.y + zone.loc.y,
            zone.size.w,
            zone.size.h,
        ))
    }

    /// Changes a window's layout, telling the client its new size and moving
    /// it to its new position.
    pub fn set_layout(&mut self, id: WindowId, layout: Layout) {
        let Some(window) = self.windows.iter_mut().find(|w| w.id == id) else {
            return;
        };
        window.layout = Some(layout);
        window.fullscreen = false;
        info!(window = id.0, ?layout, "layout changed");
        self.apply_layout(id);
    }

    /// Sets whether the window with toplevel `surface` covers its whole
    /// output, as the client asked. A dialog leaving full screen picks its own
    /// size again.
    pub fn set_fullscreen(&mut self, surface: &WlSurface, fullscreen: bool) {
        let Some(window) = self
            .windows
            .iter_mut()
            .find(|w| w.wl_surface() == Some(surface))
        else {
            return;
        };
        window.fullscreen = fullscreen;
        let (id, floating) = (window.id, window.layout.is_none());
        info!(window = id.0, fullscreen, "full screen changed");

        if !fullscreen && floating {
            let window = window.clone();
            if let Some(toplevel) = window.window.toplevel() {
                toplevel.with_pending_state(|state| {
                    state.size = None;
                    state.states.unset(xdg_toplevel::State::Fullscreen);
                });
                if toplevel.is_initial_configure_sent() {
                    toplevel.send_pending_configure();
                }
            }
        } else {
            self.apply_layout(id);
        }
        if fullscreen {
            self.focus_window(id);
        }
    }

    /// Sends the configure for a window's current layout, or for covering
    /// its output while it is full screen, and, if mapped, moves it in the
    /// space without changing the stacking order.
    pub fn apply_layout(&mut self, id: WindowId) {
        let Some(window) = self.window(id) else {
            return;
        };
        let geometry = if window.fullscreen {
            let Some(geo) = self.space.output_geometry(&window.output) else {
                return;
            };
            let geometry = Rect::new(geo.loc.x, geo.loc.y, geo.size.w, geo.size.h);
            window.configure_fullscreen(geometry);
            geometry
        } else {
            let Some(layout) = window.layout else {
                return;
            };
            let Some(area) = self.usable_area(&window.output) else {
                return;
            };
            let geometry = layout.geometry(area);
            window.configure_for_layout(layout, geometry, area);
            geometry
        };

        if window.mapped {
            let element = window.window.clone();
            // Mapping moves the window to the top, so the windows that were
            // above it are raised again in their order. Otherwise re-laying
            // out an output, as when a notification closes, would bring the
            // newest window to the front.
            let above: Vec<Window> = self
                .space
                .elements()
                .skip_while(|e| *e != &element)
                .skip(1)
                .cloned()
                .collect();
            self.space
                .map_element(element, (geometry.x, geometry.y), false);
            for window in above {
                self.space.raise_element(&window, false);
            }
        }
    }

    /// Re-applies every window's layout on `output`, after its size or
    /// exclusive zones changed. Lock surfaces are resized too.
    pub fn relayout_output(&mut self, output: &Output) {
        self.configure_lock_surfaces();
        let ids: Vec<WindowId> = self
            .windows
            .iter()
            .filter(|w| &w.output == output)
            .map(|w| w.id)
            .collect();
        for id in ids {
            self.apply_layout(id);
        }
    }

    pub fn child_env(&self) -> ChildEnv {
        ChildEnv {
            wayland_display: self.socket_name.clone(),
            x_display: self.xwayland.as_ref().map(|s| s.display.clone()),
        }
    }

    /// Focuses a window because the user or a new window asked for it,
    /// recording it as most recently used.
    pub fn focus_window(&mut self, id: WindowId) {
        self.focus.promote(id);
        self.sync_focus();
    }

    /// Makes the compositor's visible and keyboard focus match
    /// `self.focus.current()`: raises and activates that window and gives it
    /// keyboard focus unless a layer surface holds it or the overview is
    /// open.
    pub fn sync_focus(&mut self) {
        let current = self.focus.current();
        let current_window = current.and_then(|id| self.window(id)).cloned();

        match &current_window {
            Some(window) if window.mapped => {
                self.raise_group_of(window.id);
                self.space.raise_element(&window.window, true);
            }
            _ => {
                for element in self.space.elements() {
                    element.set_activated(false);
                }
            }
        }
        for element in self.space.elements() {
            if let Some(toplevel) = element.toplevel()
                && toplevel.is_initial_configure_sent()
            {
                toplevel.send_pending_configure();
            }
        }

        // While locked, only the lock screen gets the keyboard.
        let target = if self.is_locked() {
            self.lock_focus().map(|s| s.wl_surface().clone())
        } else if self.overview.open {
            None
        } else {
            match &self.layer_focus {
                Some(layer) => Some(layer.clone()),
                None => current_window.and_then(|w| w.wl_surface().cloned()),
            }
        };
        let changed = self
            .seat
            .get_keyboard()
            .is_some_and(|k| k.current_focus() != target);
        if changed {
            if self.is_locked() {
                info!("focus changed to lock screen");
            } else if self.layer_focus.is_some() {
                info!("focus changed to layer surface");
            } else {
                info!(window = current.map(|id| id.0), "focus changed");
            }
        }
        self.set_keyboard_focus(target);
    }

    pub fn set_keyboard_focus(&mut self, surface: Option<WlSurface>) {
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        if keyboard.current_focus() == surface {
            return;
        }
        keyboard.set_focus(self, surface, SERIAL_COUNTER.next_serial());
    }

    /// Finds the surface under `pos`, in stacking order: overlay and top
    /// layers, then windows, then bottom and background layers. While
    /// locked, only the lock screen is there; while the overview is open,
    /// only the overlay and top layers, such as the panel.
    pub fn surface_under(
        &self,
        pos: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        let output = self.space.output_under(pos).next()?;
        let output_geo = self.space.output_geometry(output)?;
        if self.is_locked() {
            let surface = self.lock_surface(output)?.wl_surface();
            return under_from_surface_tree(surface, pos, output_geo.loc, WindowSurfaceType::ALL)
                .map(|(surface, loc)| (surface, loc.to_f64()));
        }
        let layers = layer_map_for_output(output);
        let pos_in_output = pos - output_geo.loc.to_f64();

        let layer_hit = |layer_kind: Layer| {
            let layer = layers.layer_under(layer_kind, pos_in_output)?;
            let layer_loc = layers.layer_geometry(layer)?.loc + output_geo.loc;
            layer
                .surface_under(pos - layer_loc.to_f64(), WindowSurfaceType::ALL)
                .map(|(surface, loc)| (surface, (loc + layer_loc).to_f64()))
        };

        layer_hit(Layer::Overlay)
            .or_else(|| layer_hit(Layer::Top))
            .or_else(|| {
                if self.overview.open {
                    return None;
                }
                self.space
                    .element_under(pos)
                    .and_then(|(window, location)| {
                        window
                            .surface_under(pos - location.to_f64(), WindowSurfaceType::ALL)
                            .map(|(surface, loc)| (surface, (loc + location).to_f64()))
                    })
            })
            .or_else(|| layer_hit(Layer::Bottom).filter(|_| !self.overview.open))
            .or_else(|| layer_hit(Layer::Background).filter(|_| !self.overview.open))
    }

    /// Sends the pointer's current position again, so that the surface
    /// under it gets pointer focus after what is shown changed, such as
    /// when the screen locks.
    pub fn refresh_pointer_focus(&mut self) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        let pos = pointer.current_location();
        let under = self.surface_under(pos);
        let time = self.start_time.elapsed().as_millis() as u32;
        pointer.motion(
            self,
            under,
            &MotionEvent {
                location: pos,
                serial: SERIAL_COUNTER.next_serial(),
                time,
            },
        );
        pointer.frame(self);
    }

    /// Whether the backend shows a picture at all: not while another VT has
    /// the screen, or while the monitor is off or missing.
    pub fn is_showing(&self) -> bool {
        match &self.backend {
            Backend::Winit(_) => true,
            Backend::Udev(udev) => udev
                .output()
                .is_some_and(|output| udev.is_active() && !self.is_powered_off(output)),
        }
    }

    /// Finds the layer surface with `surface` as its main surface.
    pub fn layer_for_surface(&self, surface: &WlSurface) -> Option<(Output, LayerSurface)> {
        self.space.outputs().find_map(|output| {
            let map = layer_map_for_output(output);
            map.layer_for_surface(surface, WindowSurfaceType::TOPLEVEL)
                .map(|layer| (output.clone(), layer.clone()))
        })
    }
}

/// Adds the keyboard with the keymap from [`keymap::resolve`]. A keymap
/// that does not compile, such as a misspelt layout, is logged and replaced
/// by the US layout rather than stopping the compositor.
fn add_keyboard(seat: &mut Seat<State>, config: &KeyboardConfig) -> anyhow::Result<()> {
    let names = keymap::resolve(config);
    let (delay, rate) = (config.repeat_delay, config.repeat_rate);
    match seat.add_keyboard(xkb_config(&names), delay, rate) {
        Ok(_) => info!(?names, "keyboard layout set"),
        Err(err) => {
            warn!(?names, ?err, "invalid keyboard layout; using us");
            let us = XkbConfig {
                layout: "us",
                ..XkbConfig::default()
            };
            seat.add_keyboard(us, delay, rate)
                .context("failed to set up keyboard")?;
        }
    }
    Ok(())
}

pub fn xkb_config(names: &keymap::Names) -> XkbConfig<'_> {
    XkbConfig {
        rules: "",
        model: &names.model,
        layout: &names.layout,
        variant: &names.variant,
        options: (!names.options.is_empty()).then(|| names.options.clone()),
    }
}

fn init_wayland_listener(
    display: Display<State>,
    event_loop: &mut EventLoop<'static, State>,
) -> anyhow::Result<OsString> {
    let socket = ListeningSocketSource::new_auto()
        .context("failed to create Wayland socket (is XDG_RUNTIME_DIR set?)")?;
    let socket_name = socket.socket_name().to_os_string();

    let handle = event_loop.handle();
    handle
        .insert_source(socket, |stream, _, state| {
            if let Err(err) = state
                .display_handle
                .insert_client(stream, Arc::new(ClientState::default()))
            {
                warn!(?err, "failed to add client");
            }
        })
        .map_err(|err| anyhow::anyhow!("failed to add socket to event loop: {err}"))?;

    handle
        .insert_source(
            Generic::new(display, Interest::READ, Mode::Level),
            |_, display, state| {
                // Safety: the display is owned by this event source and is
                // never dropped while the source is alive.
                let result = unsafe { display.get_mut().dispatch_clients(state) };
                if let Err(err) = result {
                    error!(?err, "failed to dispatch clients");
                }
                Ok(PostAction::Continue)
            },
        )
        .map_err(|err| anyhow::anyhow!("failed to add display to event loop: {err}"))?;

    Ok(socket_name)
}

#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, client_id: ClientId, reason: DisconnectReason) {
        if let DisconnectReason::ProtocolError(err) = reason {
            warn!(?client_id, ?err, "client disconnected with protocol error");
        }
    }
}
