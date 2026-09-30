//! wlr-screencopy (`zwlr_screencopy_manager_v1`, version 3). grim takes
//! screenshots with it; older xdg-desktop-portal-wlr versions share the
//! screen with it.
//!
//! `copy` is answered right away. `copy_with_damage` waits until the output
//! changed since the manager's previous frame; each manager keeps a damage
//! tracker for that.

use std::{
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use smithay::{
    backend::renderer::damage::OutputDamageTracker,
    output::Output,
    reexports::{
        wayland_protocols_wlr::screencopy::v1::server::{
            zwlr_screencopy_frame_v1::{self, Flags, ZwlrScreencopyFrameV1},
            zwlr_screencopy_manager_v1::{self, ZwlrScreencopyManagerV1},
        },
        wayland_server::{
            Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
            backend::ClientId,
        },
    },
    utils::{Logical, Physical, Rectangle, Transform},
};
use tracing::{debug, warn};

use super::{
    CaptureHandler, CaptureRequest, DMABUF_FORMAT, Drawn, SHM_FORMAT, Subject, Target, buffer_size,
    output_rect, split_time, when_drawn,
};

const VERSION: u32 = 3;

/// The global, plus `copy_with_damage` requests waiting for the output to
/// change.
pub struct ScreencopyState {
    queue: Vec<Screencopy>,
}

impl ScreencopyState {
    pub fn new<D>(dh: &DisplayHandle) -> Self
    where
        D: GlobalDispatch<ZwlrScreencopyManagerV1, ()> + 'static,
    {
        dh.create_global::<D, ZwlrScreencopyManagerV1, ()>(VERSION, ());
        Self { queue: Vec::new() }
    }
}

/// Per client manager: the damage tracker `copy_with_damage` compares
/// against. Screen sharing uses one manager per stream.
pub struct ManagerData {
    tracker: Mutex<OutputDamageTracker>,
}

/// What a frame captures, fixed when it is created.
#[derive(Clone)]
struct Capture {
    manager: ZwlrScreencopyManagerV1,
    output: Output,
    region: Rectangle<i32, Physical>,
    overlay_cursor: bool,
}

pub struct FrameData {
    /// `None` when the request was invalid; the frame has already failed.
    capture: Option<Capture>,
    copied: AtomicBool,
}

/// A frame with its buffer attached, being drawn or waiting for damage.
/// Dropping it before it is ready tells the client it failed.
struct Screencopy {
    frame: ZwlrScreencopyFrameV1,
    capture: Capture,
    target: Target,
    with_damage: bool,
    ready: bool,
}

impl Screencopy {
    fn send_ready(&mut self, time: Duration) {
        self.frame.flags(Flags::empty());
        let (hi, lo, nsec) = split_time(time);
        self.frame.ready(hi, lo, nsec);
        self.ready = true;
    }
}

impl Drop for Screencopy {
    fn drop(&mut self) {
        if !self.ready {
            self.frame.failed();
        }
    }
}

impl<D> GlobalDispatch<ZwlrScreencopyManagerV1, (), D> for ScreencopyState
where
    D: Dispatch<ZwlrScreencopyManagerV1, ManagerData> + 'static,
{
    fn bind(
        _state: &mut D,
        _dh: &DisplayHandle,
        _client: &Client,
        manager: New<ZwlrScreencopyManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, D>,
    ) {
        let tracker = OutputDamageTracker::new((0, 0), 1.0, Transform::Normal);
        data_init.init(
            manager,
            ManagerData {
                tracker: Mutex::new(tracker),
            },
        );
    }
}

impl<D> Dispatch<ZwlrScreencopyManagerV1, ManagerData, D> for ScreencopyState
where
    D: Dispatch<ZwlrScreencopyFrameV1, FrameData> + 'static,
{
    fn request(
        _state: &mut D,
        _client: &Client,
        manager: &ZwlrScreencopyManagerV1,
        request: zwlr_screencopy_manager_v1::Request,
        _data: &ManagerData,
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        use zwlr_screencopy_manager_v1::Request;
        let (frame, overlay_cursor, output, region) = match request {
            Request::CaptureOutput {
                frame,
                overlay_cursor,
                output,
            } => (frame, overlay_cursor, output, None),
            Request::CaptureOutputRegion {
                frame,
                overlay_cursor,
                output,
                x,
                y,
                width,
                height,
            } => {
                let region = Rectangle::<i32, Logical>::new((x, y).into(), (width, height).into());
                (frame, overlay_cursor, output, Some(region))
            }
            // Destroying the manager leaves its frames working.
            _ => return,
        };

        let capture = Output::from_resource(&output).and_then(|output| {
            let full = output_rect(&output)?;
            let region = match region {
                None => full,
                // The region is in logical coordinates within the output.
                Some(region) if !region.is_empty() => {
                    let scale = output.current_scale().fractional_scale();
                    region
                        .to_f64()
                        .to_physical_precise_round(scale)
                        .intersection(full)?
                }
                Some(_) => return None,
            };
            Some(Capture {
                manager: manager.clone(),
                output,
                region,
                overlay_cursor: overlay_cursor != 0,
            })
        });

        let frame = data_init.init(
            frame,
            FrameData {
                capture: capture.clone(),
                copied: AtomicBool::new(false),
            },
        );
        let Some(capture) = capture else {
            debug!("screencopy of an unknown output or an empty region");
            frame.failed();
            return;
        };

        let size = buffer_size(&capture.output, capture.region);
        let (w, h) = (size.w as u32, size.h as u32);
        frame.buffer(SHM_FORMAT, w, h, w * 4);
        if frame.version() >= 3 {
            frame.linux_dmabuf(DMABUF_FORMAT as u32, w, h);
            frame.buffer_done();
        }
    }
}

impl<D> Dispatch<ZwlrScreencopyFrameV1, FrameData, D> for ScreencopyState
where
    D: CaptureHandler,
{
    fn request(
        state: &mut D,
        _client: &Client,
        frame: &ZwlrScreencopyFrameV1,
        request: zwlr_screencopy_frame_v1::Request,
        data: &FrameData,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        use zwlr_screencopy_frame_v1::{Error, Request};
        let (buffer, with_damage) = match request {
            Request::Copy { buffer } => (buffer, false),
            Request::CopyWithDamage { buffer } => (buffer, true),
            _ => return,
        };
        let Some(capture) = &data.capture else {
            return;
        };
        if data.copied.swap(true, Ordering::SeqCst) {
            frame.post_error(Error::AlreadyUsed, "frame was already copied");
            return;
        }
        let size = buffer_size(&capture.output, capture.region);
        let Some(target) = Target::new(&buffer, size) else {
            frame.post_error(
                Error::InvalidBuffer,
                "buffer does not match the offered format and size",
            );
            return;
        };
        let screencopy = Screencopy {
            frame: frame.clone(),
            capture: capture.clone(),
            target,
            with_damage,
            ready: false,
        };
        draw(state, screencopy);
    }

    fn destroyed(
        state: &mut D,
        _client: ClientId,
        frame: &ZwlrScreencopyFrameV1,
        _data: &FrameData,
    ) {
        state.screencopy_state().queue.retain(|s| &s.frame != frame);
    }
}

/// Answers the `copy_with_damage` requests for `output` if it changed, and
/// fails those for outputs that are gone.
pub fn render_queued<D: CaptureHandler>(state: &mut D, output: &Output) {
    let mut queue = std::mem::take(&mut state.screencopy_state().queue);
    queue.retain(|s| state.is_output_mapped(&s.capture.output));
    let (now, later): (Vec<_>, Vec<_>) =
        queue.into_iter().partition(|s| &s.capture.output == output);
    state.screencopy_state().queue = later;
    for screencopy in now {
        draw(state, screencopy);
    }
}

/// Draws the frame and tells the client when it is ready, or queues it
/// when it waits for damage and nothing changed.
fn draw<D: CaptureHandler>(state: &mut D, screencopy: Screencopy) {
    let manager = screencopy.capture.manager.clone();
    let data = manager
        .data::<ManagerData>()
        .expect("screencopy manager without data");
    let mut tracker = data.tracker.lock().unwrap_or_else(|e| e.into_inner());
    let result = state.draw_capture(CaptureRequest {
        subject: Subject::Region {
            output: &screencopy.capture.output,
            region: screencopy.capture.region,
        },
        cursor: screencopy.capture.overlay_cursor,
        target: &screencopy.target,
        tracker: &mut tracker,
        wait_for_damage: screencopy.with_damage,
    });
    match result {
        Ok(Some(drawn)) => {
            drop(tracker);
            ready(state, screencopy, drawn);
        }
        Ok(None) => state.screencopy_state().queue.push(screencopy),
        Err(err) => {
            warn!("screen capture failed: {err:#}");
            // Start over, so the next frame is drawn in full.
            *tracker = OutputDamageTracker::new((0, 0), 1.0, Transform::Normal);
            // Dropping the frame tells the client it failed.
        }
    }
}

fn ready<D: CaptureHandler>(state: &mut D, mut screencopy: Screencopy, drawn: Drawn) {
    if screencopy.with_damage {
        for rect in &drawn.damage {
            screencopy.frame.damage(
                rect.loc.x as u32,
                rect.loc.y as u32,
                rect.size.w as u32,
                rect.size.h as u32,
            );
        }
    }
    when_drawn(drawn.sync, &state.loop_handle(), move |time| {
        screencopy.send_ready(time);
    });
}

/// Wires the protocol to `State`, like Smithay's `delegate_*!` macros.
#[macro_export]
macro_rules! delegate_screencopy {
    ($ty:ty) => {
        smithay::reexports::wayland_server::delegate_global_dispatch!($ty: [
            smithay::reexports::wayland_protocols_wlr::screencopy::v1::server::zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1: ()
        ] => $crate::capture::screencopy::ScreencopyState);
        smithay::reexports::wayland_server::delegate_dispatch!($ty: [
            smithay::reexports::wayland_protocols_wlr::screencopy::v1::server::zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1: $crate::capture::screencopy::ManagerData
        ] => $crate::capture::screencopy::ScreencopyState);
        smithay::reexports::wayland_server::delegate_dispatch!($ty: [
            smithay::reexports::wayland_protocols_wlr::screencopy::v1::server::zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1: $crate::capture::screencopy::FrameData
        ] => $crate::capture::screencopy::ScreencopyState);
    };
}
