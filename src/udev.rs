//! DRM/KMS backend: runs directly on the hardware from a text console.
//!
//! Deliberately simple: one GPU and one output. The first connected monitor
//! is used; others are ignored until it is unplugged.

use std::{path::PathBuf, time::Duration};

use anyhow::{Context, anyhow};
use smithay::{
    backend::{
        allocator::{
            Fourcc,
            gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
        },
        drm::{
            DrmDevice, DrmDeviceFd, DrmEvent, DrmNode,
            compositor::FrameFlags,
            exporter::gbm::GbmFramebufferExporter,
            output::{DrmOutput, DrmOutputManager, DrmOutputRenderElements},
        },
        egl::{EGLContext, EGLDevice, EGLDisplay},
        input::InputEvent,
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::{
            ImportDma, ImportMemWl,
            element::{
                Kind,
                memory::MemoryRenderBufferRenderElement,
                render_elements,
                surface::{WaylandSurfaceRenderElement, render_elements_from_surface_tree},
            },
            gles::GlesRenderer,
        },
        session::{Event as SessionEvent, Session, libseat::LibSeatSession},
        udev::{UdevBackend, UdevEvent, all_gpus, primary_gpu},
    },
    desktop::space::{SpaceRenderElements, space_render_elements},
    input::pointer::{CursorImageStatus, CursorImageSurfaceData},
    output::{Mode, Output, PhysicalProperties, Scale as OutputScale, Subpixel},
    reexports::{
        calloop::{
            EventLoop, LoopHandle,
            timer::{TimeoutAction, Timer},
        },
        drm::control::{Device as ControlDevice, ModeTypeFlags, connector, crtc},
        input::{self, Libinput},
        rustix::fs::OFlags,
        wayland_server::{DisplayHandle, backend::GlobalId},
    },
    utils::{DeviceFd, IsAlive, Logical, Point, Scale, Transform},
    wayland::{
        compositor::with_states,
        dmabuf::{DmabufFeedbackBuilder, DmabufState},
    },
};
use tracing::{error, info, warn};

use crate::{
    config::{ClickMethod, InputConfig, OutputConfig, auto_scale},
    cursor::Cursors,
    handlers::LOCKED_BACKGROUND,
    state::{Backend, State},
    switcher::SwitcherElement,
};

type GbmOutputManager = DrmOutputManager<
    GbmAllocator<DrmDeviceFd>,
    GbmFramebufferExporter<DrmDeviceFd>,
    (),
    DrmDeviceFd,
>;
type GbmOutput =
    DrmOutput<GbmAllocator<DrmDeviceFd>, GbmFramebufferExporter<DrmDeviceFd>, (), DrmDeviceFd>;

/// Formats tried for the screen's framebuffer, best first.
const COLOR_FORMATS: [Fourcc; 2] = [Fourcc::Argb8888, Fourcc::Xrgb8888];
pub const BACKGROUND: [f32; 4] = [0.1, 0.1, 0.1, 1.0];

render_elements! {
    pub OutputElements<=GlesRenderer>;
    Space=SpaceRenderElements<GlesRenderer, WaylandSurfaceRenderElement<GlesRenderer>>,
    CursorSurface=WaylandSurfaceRenderElement<GlesRenderer>,
    CursorImage=MemoryRenderBufferRenderElement<GlesRenderer>,
    Switcher=SwitcherElement,
}

pub struct UdevData {
    pub session: LibSeatSession,
    libinput: Libinput,
    /// Kept so input settings can be applied again when the config changes.
    input_devices: Vec<smithay::reexports::input::Device>,
    pub renderer: GlesRenderer,
    drm: GbmOutputManager,
    screen: Option<Screen>,
    pub cursors: Cursors,
    /// False while another VT has the session; the GPU must not be touched.
    pub active: bool,
    /// A render is already queued, either waiting for vblank or on a timer.
    render_pending: bool,
    /// The queued frame was drawn while locked; once it is on screen the
    /// locker can be told that the session is locked.
    lock_frame_queued: bool,
}

/// The monitor currently in use.
struct Screen {
    output: Output,
    global: GlobalId,
    connector: connector::Handle,
    drm_output: GbmOutput,
    refresh: Duration,
}

/// Opens the session and GPU and wires their events into the event loop.
pub fn init_backend(
    event_loop: &mut EventLoop<'static, State>,
    dh: &DisplayHandle,
) -> anyhow::Result<(UdevData, DmabufState)> {
    let (session, session_notifier) = LibSeatSession::new().context(
        "failed to start a session; run rustwm from a text console (Ctrl+Alt+F3), \
         not from inside a desktop, and make sure logind or seatd is running",
    )?;
    let seat = session.seat();

    let gpu_path = find_gpu(&seat)?;
    info!(gpu = %gpu_path.display(), "using GPU");
    let node = DrmNode::from_path(&gpu_path).context("GPU is not a DRM device")?;

    let fd = session
        .clone()
        .open(
            &gpu_path,
            OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK,
        )
        .with_context(|| {
            format!(
                "failed to open {} (permission problem?)",
                gpu_path.display()
            )
        })?;
    let fd = DrmDeviceFd::new(DeviceFd::from(fd));

    let (drm, drm_notifier) =
        DrmDevice::new(fd.clone(), true).context("failed to initialise DRM device")?;
    let gbm = GbmDevice::new(fd.clone()).context("failed to initialise GBM")?;

    // Safety: the GBM device outlives the display, which holds a clone of it.
    let egl_display =
        unsafe { EGLDisplay::new(gbm.clone()) }.context("failed to initialise EGL")?;
    let render_node = EGLDevice::device_for_display(&egl_display)
        .ok()
        .and_then(|device| device.try_get_render_node().ok().flatten())
        .unwrap_or(node);
    let context = EGLContext::new(&egl_display).context("failed to create EGL context")?;
    // Safety: the context is current only on this thread.
    let renderer =
        unsafe { GlesRenderer::new(context) }.context("failed to initialise OpenGL renderer")?;

    let render_formats = renderer.egl_context().dmabuf_render_formats().clone();
    let drm = DrmOutputManager::new(
        drm,
        GbmAllocator::new(
            gbm.clone(),
            GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT,
        ),
        GbmFramebufferExporter::new(gbm.clone(), Some(render_node)),
        Some(gbm),
        COLOR_FORMATS,
        render_formats,
    );

    let mut dmabuf_state = DmabufState::new();
    let formats = renderer.dmabuf_formats();
    match DmabufFeedbackBuilder::new(render_node.dev_id(), formats.clone()).build() {
        Ok(feedback) => {
            dmabuf_state.create_global_with_default_feedback::<State>(dh, &feedback);
        }
        Err(err) => {
            warn!(
                ?err,
                "no dmabuf feedback; GPU clients may fall back to software"
            );
            dmabuf_state.create_global::<State>(dh, formats);
        }
    }

    let mut libinput = Libinput::new_with_udev(LibinputSessionInterface::from(session.clone()));
    libinput
        .udev_assign_seat(&seat)
        .map_err(|()| anyhow!("failed to assign libinput to seat {seat}"))?;

    let handle = event_loop.handle();
    insert_sources(
        &handle,
        drm_notifier,
        session_notifier,
        LibinputInputBackend::new(libinput.clone()),
        UdevBackend::new(&seat).context("failed to watch udev for monitor changes")?,
        node,
    )?;

    Ok((
        UdevData {
            session,
            libinput,
            input_devices: Vec::new(),
            renderer,
            drm,
            screen: None,
            cursors: Cursors::load(1),
            active: true,
            render_pending: false,
            lock_frame_queued: false,
        },
        dmabuf_state,
    ))
}

fn find_gpu(seat: &str) -> anyhow::Result<PathBuf> {
    if let Some(path) = primary_gpu(seat).context("failed to query udev for GPUs")? {
        return Ok(path);
    }
    all_gpus(seat)
        .context("failed to query udev for GPUs")?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("no GPU found (is DRM available?)"))
}

fn insert_sources(
    handle: &LoopHandle<'static, State>,
    drm_notifier: smithay::backend::drm::DrmDeviceNotifier,
    session_notifier: smithay::backend::session::libseat::LibSeatSessionNotifier,
    libinput: LibinputInputBackend,
    udev: UdevBackend,
    node: DrmNode,
) -> anyhow::Result<()> {
    let insert_error = |err| anyhow!("failed to add event source: {err}");

    handle
        .insert_source(drm_notifier, |event, _, state| match event {
            DrmEvent::VBlank(crtc) => on_vblank(state, crtc),
            DrmEvent::Error(err) => error!(?err, "DRM error"),
        })
        .map_err(|e| insert_error(e.error))?;

    handle
        .insert_source(libinput, |mut event, _, state| {
            if let Backend::Udev(udev) = &mut state.backend {
                match &mut event {
                    InputEvent::DeviceAdded { device } => {
                        configure_device(device, &state.config.input);
                        udev.input_devices.push(device.clone());
                    }
                    InputEvent::DeviceRemoved { device } => {
                        udev.input_devices.retain(|d| d != device);
                    }
                    _ => {}
                }
            }
            state.process_input_event(event);
        })
        .map_err(|e| insert_error(e.error))?;

    handle
        .insert_source(session_notifier, |event, _, state| {
            let Backend::Udev(udev) = &mut state.backend else {
                return;
            };
            match event {
                SessionEvent::PauseSession => {
                    info!("session paused (switched away)");
                    udev.active = false;
                    udev.libinput.suspend();
                    udev.drm.pause();
                    // The held key's release will not arrive.
                    state.stop_key_repeat();
                    // Nothing of the session is on screen any more.
                    state.lock_frame_shown();
                }
                SessionEvent::ActivateSession => {
                    info!("session resumed");
                    udev.active = true;
                    if udev.libinput.resume().is_err() {
                        error!("failed to resume input");
                    }
                    if let Err(err) = udev.drm.activate(false) {
                        error!(%err, "failed to resume display");
                    }
                    udev.render_pending = false;
                    // Resuming lit the monitor again; the user switched back
                    // and wants to see it.
                    state.power_on_outputs();
                    render(state);
                }
            }
        })
        .map_err(|e| insert_error(e.error))?;

    handle
        .insert_source(udev, move |event, _, state| {
            // Only connector changes on our GPU matter; other GPUs are ignored.
            if let UdevEvent::Changed { device_id } = event
                && device_id == node.dev_id()
            {
                scan_connectors(state);
            }
        })
        .map_err(|e| insert_error(e.error))?;

    Ok(())
}

/// Sets up the first connected monitor if none is in use, and tears down
/// the current one if it was unplugged.
pub fn scan_connectors(state: &mut State) {
    let Backend::Udev(udev) = &mut state.backend else {
        return;
    };
    let device = udev.drm.device();
    let resources = match device.resource_handles() {
        Ok(resources) => resources,
        Err(err) => {
            error!(%err, "failed to read display resources");
            return;
        }
    };
    let connected: Vec<connector::Info> = resources
        .connectors()
        .iter()
        .filter_map(|&handle| device.get_connector(handle, true).ok())
        .filter(|info| info.state() == connector::State::Connected)
        .collect();

    if let Some(screen) = &udev.screen
        && !connected
            .iter()
            .any(|info| info.handle() == screen.connector)
    {
        remove_screen(state);
    }

    let Backend::Udev(udev) = &state.backend else {
        return;
    };
    if udev.screen.is_some() {
        return;
    }
    let Some(info) = connected.into_iter().next() else {
        warn!("no monitor connected");
        return;
    };
    let Some(crtc) = pick_crtc(udev.drm.device(), &info, &resources) else {
        warn!(connector = ?info.handle(), "no free display controller for monitor");
        return;
    };
    add_screen(state, info, crtc);
}

fn pick_crtc(
    device: &DrmDevice,
    info: &connector::Info,
    resources: &smithay::reexports::drm::control::ResourceHandles,
) -> Option<crtc::Handle> {
    info.encoders()
        .iter()
        .filter_map(|&encoder| device.get_encoder(encoder).ok())
        .flat_map(|encoder| resources.filter_crtcs(encoder.possible_crtcs()))
        .next()
}

fn add_screen(state: &mut State, info: connector::Info, crtc: crtc::Handle) {
    let Backend::Udev(udev) = &mut state.backend else {
        return;
    };
    let Some(&drm_mode) = info
        .modes()
        .iter()
        .find(|mode| mode.mode_type().contains(ModeTypeFlags::PREFERRED))
        .or_else(|| info.modes().first())
    else {
        warn!("monitor reports no modes");
        return;
    };
    let mode = Mode::from(drm_mode);
    let name = format!("{}-{}", info.interface().as_str(), info.interface_id());
    let (phys_w, phys_h) = info.size().unwrap_or((0, 0));
    let scale = output_scale(&state.config.output, mode, phys_w);

    let output = Output::new(
        name.clone(),
        PhysicalProperties {
            size: (phys_w as i32, phys_h as i32).into(),
            subpixel: Subpixel::Unknown,
            make: "Unknown".into(),
            model: "Unknown".into(),
        },
    );
    let global = output.create_global::<State>(&state.display_handle);
    output.change_current_state(
        Some(mode),
        Some(Transform::Normal),
        Some(OutputScale::Fractional(scale)),
        Some((0, 0).into()),
    );
    // Cursor images come in whole-number sizes; drawing a larger one scaled
    // down keeps it sharp.
    udev.cursors = Cursors::load(scale.ceil() as i32);
    output.set_preferred(mode);

    let drm_output = match udev.drm.initialize_output::<_, OutputElements>(
        crtc,
        drm_mode,
        &[info.handle()],
        &output,
        None,
        &mut udev.renderer,
        &DrmOutputRenderElements::default(),
    ) {
        Ok(drm_output) => drm_output,
        Err(err) => {
            error!(%err, output = name, "failed to set up monitor");
            state.display_handle.remove_global::<State>(global);
            return;
        }
    };

    let refresh = Duration::from_secs_f64(1000.0 / mode.refresh.max(1) as f64);
    info!(
        output = name,
        width = mode.size.w,
        height = mode.size.h,
        refresh_mhz = mode.refresh,
        scale,
        "output added"
    );
    udev.screen = Some(Screen {
        output: output.clone(),
        global,
        connector: info.handle(),
        drm_output,
        refresh,
    });

    state.space.map_output(&output, (0, 0));
    state.adopt_windows(&output);
    if let Some(pointer) = state.seat.get_pointer() {
        let center = (
            mode.size.w as f64 / scale / 2.0,
            mode.size.h as f64 / scale / 2.0,
        );
        pointer.set_location(center.into());
    }
    render(state);
}

fn output_scale(config: &OutputConfig, mode: Mode, width_mm: u32) -> f64 {
    config
        .scale
        .unwrap_or_else(|| auto_scale(mode.size.w, width_mm))
        .clamp(1.0, 4.0)
}

/// Applies the input settings to every connected device.
pub fn reconfigure_input(state: &mut State) {
    if let Backend::Udev(udev) = &mut state.backend {
        for device in &mut udev.input_devices {
            configure_device(device, &state.config.input);
        }
    }
}

/// Gives the monitor the configured scale, if it changed.
pub fn rescale_output(state: &mut State) {
    let Backend::Udev(udev) = &mut state.backend else {
        return;
    };
    let Some(output) = udev.output().cloned() else {
        return;
    };
    let Some(mode) = output.current_mode() else {
        return;
    };
    let width_mm = output.physical_properties().size.w.max(0) as u32;
    let scale = output_scale(&state.config.output, mode, width_mm);
    if scale == output.current_scale().fractional_scale() {
        return;
    }
    info!(output = output.name(), scale, "output scale changed");
    output.change_current_state(None, None, Some(OutputScale::Fractional(scale)), None);
    udev.cursors = Cursors::load(scale.ceil() as i32);
    state.output_rescaled(&output);
}

fn remove_screen(state: &mut State) {
    let Backend::Udev(udev) = &mut state.backend else {
        return;
    };
    let Some(screen) = udev.screen.take() else {
        return;
    };
    info!(output = screen.output.name(), "output removed");
    state.output_power_removed(&screen.output);
    state.space.unmap_output(&screen.output);
    state.display_handle.disable_global::<State>(screen.global);
}

fn on_vblank(state: &mut State, crtc: crtc::Handle) {
    let Backend::Udev(udev) = &mut state.backend else {
        return;
    };
    let Some(screen) = &mut udev.screen else {
        return;
    };
    if screen.drm_output.crtc() != crtc {
        return;
    }
    if let Err(err) = screen.drm_output.frame_submitted() {
        warn!(?err, "frame was not presented");
    }
    udev.render_pending = false;
    if std::mem::take(&mut udev.lock_frame_queued) {
        state.lock_frame_shown();
    }
    render(state);
}

/// Draws a frame if anything changed, then sends frame callbacks.
///
/// When nothing changed no page flip happens, so no vblank will arrive to
/// trigger the next frame; a timer at the refresh interval takes its place.
pub fn render(state: &mut State) {
    let locked = state.is_locked();
    let output = match &state.backend {
        Backend::Udev(UdevData {
            active: true,
            render_pending: false,
            screen: Some(screen),
            ..
        }) => Some(screen.output.clone()),
        _ => None,
    };
    // A monitor that is off stays dark until something turns it on.
    if output.as_ref().is_some_and(|o| state.is_powered_off(o)) {
        return;
    }
    let switcher = match &output {
        Some(output) if !locked => state.overlay_elements(output),
        _ => Vec::new(),
    };
    let lock_surface = output
        .as_ref()
        .and_then(|o| state.lock_surface(o))
        .map(|s| s.wl_surface().clone());
    let pointer_location = state
        .seat
        .get_pointer()
        .map(|p| p.current_location())
        .unwrap_or_default();
    let State {
        backend,
        space,
        cursor_status,
        loop_handle,
        ..
    } = state;
    let Backend::Udev(udev) = backend else {
        return;
    };
    if !udev.active || udev.render_pending {
        return;
    }
    let Some(screen) = &mut udev.screen else {
        return;
    };

    let mut elements: Vec<OutputElements> = Vec::new();

    // The cursor comes first: elements are ordered front to back.
    let scale = Scale::from(screen.output.current_scale().fractional_scale());
    elements.extend(cursor_elements(
        &mut udev.renderer,
        &mut udev.cursors,
        cursor_status,
        pointer_location,
        scale,
    ));

    // While locked, the lock screen replaces windows, layer surfaces and
    // overlays. It has the same element type as a cursor surface.
    if locked {
        if let Some(surface) = &lock_surface {
            elements.extend(render_elements_from_surface_tree(
                &mut udev.renderer,
                surface,
                (0, 0),
                scale,
                1.0,
                Kind::Unspecified,
            ));
        }
    } else {
        elements.extend(switcher.into_iter().map(OutputElements::Switcher));

        match space_render_elements(&mut udev.renderer, [&*space], &screen.output, 1.0) {
            Ok(space_elements) => elements.extend(space_elements.into_iter().map(Into::into)),
            Err(err) => warn!(?err, "failed to collect window elements"),
        }
    }

    let background = if locked {
        LOCKED_BACKGROUND
    } else {
        BACKGROUND
    };
    let (queued, unchanged) = match screen.drm_output.render_frame(
        &mut udev.renderer,
        &elements,
        background,
        FrameFlags::DEFAULT,
    ) {
        Ok(result) if !result.is_empty => match screen.drm_output.queue_frame(()) {
            Ok(()) => (true, false),
            Err(err) => {
                warn!(?err, "failed to queue frame");
                (false, false)
            }
        },
        Ok(_) => (false, true),
        Err(err) => {
            warn!(?err, "failed to render frame");
            (false, false)
        }
    };

    udev.render_pending = true;
    udev.lock_frame_queued = locked && queued;
    if !queued {
        let _ = loop_handle.insert_source(Timer::from_duration(screen.refresh), |_, _, state| {
            if let Backend::Udev(udev) = &mut state.backend {
                udev.render_pending = false;
            }
            render(state);
            TimeoutAction::Drop
        });
    }

    let output = screen.output.clone();
    // Nothing changed since a frame that was already drawn locked.
    if locked && unchanged {
        state.lock_frame_shown();
    }
    crate::capture::render_queued(state, &output);
    state.send_frame_callbacks(&output);
}

/// The pointer at `pointer_location`, for the screen and for screen
/// captures. A cursor surface whose client went away falls back to the
/// default arrow.
pub fn cursor_elements(
    renderer: &mut GlesRenderer,
    cursors: &mut Cursors,
    cursor_status: &mut CursorImageStatus,
    pointer_location: Point<f64, Logical>,
    scale: Scale<f64>,
) -> Vec<OutputElements> {
    if let CursorImageStatus::Surface(surface) = cursor_status
        && !surface.alive()
    {
        *cursor_status = CursorImageStatus::default_named();
    }
    let mut elements = Vec::new();
    match cursor_status {
        CursorImageStatus::Hidden => {}
        CursorImageStatus::Surface(surface) => {
            let hotspot = with_states(surface, |states| {
                states
                    .data_map
                    .get::<CursorImageSurfaceData>()
                    .and_then(|data| data.lock().ok().map(|attrs| attrs.hotspot))
                    .unwrap_or_default()
            });
            let location = (pointer_location - hotspot.to_f64())
                .to_physical(scale)
                .to_i32_round();
            elements.extend(render_elements_from_surface_tree(
                renderer,
                surface,
                location,
                scale,
                1.0,
                Kind::Cursor,
            ));
        }
        CursorImageStatus::Named(icon) => {
            let cursor = cursors.get(*icon);
            let location = (pointer_location - cursor.hotspot.to_f64()).to_physical(scale);
            match MemoryRenderBufferRenderElement::from_buffer(
                renderer,
                location,
                &cursor.buffer,
                None,
                None,
                None,
                Kind::Cursor,
            ) {
                Ok(element) => elements.push(element.into()),
                Err(err) => warn!(?err, "failed to draw cursor"),
            }
        }
    }
    elements
}

/// Turns the monitor off by disabling its display controller, or back on
/// by drawing a frame, which enables it again.
pub fn set_power(state: &mut State, output: &Output, on: bool) {
    let Backend::Udev(udev) = &mut state.backend else {
        return;
    };
    let Some(screen) = udev.screen.as_mut().filter(|s| &s.output == output) else {
        return;
    };
    if on {
        // Redraw everything rather than only what changed while off.
        screen.drm_output.reset_buffers();
        udev.render_pending = false;
        render(state);
    } else if udev.active {
        if let Err(err) = screen.drm_output.with_compositor(|c| c.clear()) {
            warn!(%err, "failed to turn the monitor off");
        }
        udev.lock_frame_queued = false;
    }
}

/// Applies the input settings to a newly connected mouse, touchpad or
/// keyboard. Settings a device does not support are skipped.
fn configure_device(device: &mut input::Device, config: &InputConfig) {
    if device.config_tap_finger_count() > 0 {
        let _ = device.config_tap_set_enabled(config.tap_to_click);
    }
    let method = match config.click_method {
        ClickMethod::Fingers => input::ClickMethod::Clickfinger,
        ClickMethod::Areas => input::ClickMethod::ButtonAreas,
    };
    if device.config_click_methods().contains(&method) {
        let _ = device.config_click_set_method(method);
    }
    if device.config_scroll_has_natural_scroll() {
        let _ = device.config_scroll_set_natural_scroll_enabled(config.natural_scroll);
    }
    if device.config_accel_is_available() {
        let _ = device.config_accel_set_speed(config.pointer_speed.clamp(-1.0, 1.0));
    }
}

impl UdevData {
    pub fn shm_formats(&self) -> Vec<smithay::reexports::wayland_server::protocol::wl_shm::Format> {
        self.renderer.shm_formats().collect()
    }

    /// The output being drawn to, if a monitor is connected.
    pub fn output(&self) -> Option<&Output> {
        self.screen.as_ref().map(|screen| &screen.output)
    }

    /// False while another VT has the session.
    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn change_vt(&mut self, vt: i32) {
        info!(vt, "switching virtual terminal");
        if let Err(err) = self.session.change_vt(vt) {
            warn!(?err, vt, "failed to switch virtual terminal");
        }
    }
}
