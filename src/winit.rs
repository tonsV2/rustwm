//! Nested backend: runs the compositor inside a window of an existing
//! Wayland or X11 session, for safe development.

use anyhow::Context;
use smithay::{
    backend::{
        egl::EGLDevice,
        renderer::{ImportDma, damage::OutputDamageTracker, gles::GlesRenderer},
        winit::{self, WinitEvent, WinitEventLoop, WinitGraphicsBackend},
    },
    desktop::{layer_map_for_output, space::render_output},
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::{calloop::EventLoop, wayland_server::DisplayHandle},
    utils::{Rectangle, Transform},
    wayland::dmabuf::{DmabufFeedbackBuilder, DmabufState},
};
use tracing::{info, warn};

use crate::{
    state::{Backend, State},
    switcher::SwitcherElement,
};

const REFRESH_MHZ: i32 = 60_000;
const BACKGROUND: [f32; 4] = [0.1, 0.1, 0.1, 1.0];

pub struct WinitData {
    pub backend: WinitGraphicsBackend<GlesRenderer>,
    pub output: Output,
    damage_tracker: OutputDamageTracker,
}

/// Opens the host window and creates the output and dmabuf global.
pub fn init_backend(
    dh: &DisplayHandle,
) -> anyhow::Result<(WinitData, WinitEventLoop, DmabufState)> {
    let (mut backend, winit_loop) = winit::init::<GlesRenderer>()
        .map_err(|err| anyhow::anyhow!("failed to open nested window: {err}"))?;

    let mode = Mode {
        size: backend.window_size(),
        refresh: REFRESH_MHZ,
    };
    let output = Output::new(
        "winit".to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "rustwm".into(),
            model: "nested".into(),
        },
    );
    output.create_global::<State>(dh);
    // GL renders upside down relative to the window, hence the flip.
    output.change_current_state(
        Some(mode),
        Some(Transform::Flipped180),
        None,
        Some((0, 0).into()),
    );
    output.set_preferred(mode);

    let dmabuf_state = create_dmabuf_global(dh, &mut backend);
    let damage_tracker = OutputDamageTracker::from_output(&output);

    Ok((
        WinitData {
            backend,
            output,
            damage_tracker,
        },
        winit_loop,
        dmabuf_state,
    ))
}

/// Lets GPU clients share buffers with the compositor. Mesa's EGL needs
/// dmabuf v4 feedback, which requires knowing the render node.
fn create_dmabuf_global(
    dh: &DisplayHandle,
    backend: &mut WinitGraphicsBackend<GlesRenderer>,
) -> DmabufState {
    let mut dmabuf_state = DmabufState::new();
    let formats = backend.renderer().dmabuf_formats();

    let render_node = EGLDevice::device_for_display(backend.renderer().egl_context().display())
        .ok()
        .and_then(|device| device.try_get_render_node().ok().flatten());
    let feedback = render_node.and_then(|node| {
        DmabufFeedbackBuilder::new(node.dev_id(), formats.clone())
            .build()
            .ok()
    });

    match feedback {
        Some(feedback) => {
            dmabuf_state.create_global_with_default_feedback::<State>(dh, &feedback);
        }
        None => {
            warn!("no render node found; GPU clients may fall back to software rendering");
            dmabuf_state.create_global::<State>(dh, formats);
        }
    }
    dmabuf_state
}

pub fn run(
    event_loop: &mut EventLoop<'static, State>,
    state: &mut State,
    winit_loop: WinitEventLoop,
) -> anyhow::Result<()> {
    if let Backend::Winit(winit) = &state.backend {
        state.space.map_output(&winit.output, (0, 0));
    }

    event_loop
        .handle()
        .insert_source(winit_loop, |event, _, state| match event {
            WinitEvent::Resized { size, .. } => {
                let Backend::Winit(winit) = &state.backend else {
                    return;
                };
                let output = winit.output.clone();
                info!(width = size.w, height = size.h, "output resized");
                output.change_current_state(
                    Some(Mode {
                        size,
                        refresh: REFRESH_MHZ,
                    }),
                    None,
                    None,
                    None,
                );
                layer_map_for_output(&output).arrange();
                state.relayout_output(&output);
            }
            WinitEvent::Input(event) => state.process_input_event(event),
            WinitEvent::Redraw => render(state),
            WinitEvent::CloseRequested => state.loop_signal.stop(),
            _ => {}
        })
        .map_err(|err| anyhow::anyhow!("failed to add winit source: {err}"))
        .context("failed to start nested backend")?;
    Ok(())
}

fn render(state: &mut State) {
    let Backend::Winit(winit) = &state.backend else {
        return;
    };
    let output = winit.output.clone();
    let overlay = state.overlay_elements(&output);
    let Backend::Winit(winit) = &mut state.backend else {
        return;
    };
    let size = winit.backend.window_size();

    let rendered = match winit.backend.bind() {
        Ok((renderer, mut framebuffer)) => render_output::<_, SwitcherElement, _, _>(
            &output,
            renderer,
            &mut framebuffer,
            1.0,
            0,
            [&state.space],
            &overlay,
            &mut winit.damage_tracker,
            BACKGROUND,
        )
        .is_ok(),
        Err(err) => {
            warn!(%err, "failed to bind nested window");
            false
        }
    };

    if rendered {
        let damage = Rectangle::from_size(size);
        if let Err(err) = winit.backend.submit(Some(&[damage])) {
            warn!(%err, "failed to present frame");
        }
    }
    // Winit only redraws on request, so keep the loop going at vsync.
    winit.backend.window().request_redraw();

    state.send_frame_callbacks(&output);
}
