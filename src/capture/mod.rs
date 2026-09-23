//! Screen capture for screenshots and screen sharing.
//!
//! Two protocols hand the compositor a client buffer to draw the screen
//! into: wlr-screencopy ([`screencopy`]), which grim and most tools use,
//! and ext-image-copy-capture ([`image_copy`]), which
//! xdg-desktop-portal-wlr 0.8 prefers for sharing the screen. Smithay 0.7
//! implements neither. Both leave the drawing to [`CaptureHandler`].
//!
//! A capture is drawn again from the output's elements rather than read
//! back from the screen: on the DRM backend, windows and the cursor can sit
//! on planes of their own and are then missing from the composited frame.
//!
//! A screen sharing client asks for the next frame as soon as it has the
//! previous one. Such a request is answered once the output changed, after
//! a later render of the output ([`render_queued`]); each client keeps a
//! damage tracker to tell.

pub mod image_copy;
pub mod screencopy;

use std::time::Duration;

use smithay::{
    backend::{
        allocator::{Buffer as _, Fourcc, dmabuf::Dmabuf},
        renderer::{damage::OutputDamageTracker, sync::SyncPoint},
    },
    output::Output,
    reexports::{
        calloop::{Interest, LoopHandle, Mode, PostAction, generic::Generic},
        wayland_server::protocol::{wl_buffer::WlBuffer, wl_shm},
    },
    utils::{Buffer, Clock, Monotonic, Physical, Rectangle, Size},
    wayland::{dmabuf, shm},
};
use tracing::warn;

use self::{image_copy::ImageCopyState, screencopy::ScreencopyState};

/// The one pixel format offered, for both buffer kinds. Every client
/// supports it, and it matches the screen.
pub const SHM_FORMAT: wl_shm::Format = wl_shm::Format::Xrgb8888;
pub const DMABUF_FORMAT: Fourcc = Fourcc::Xrgb8888;

pub trait CaptureHandler: 'static + Sized {
    fn screencopy_state(&mut self) -> &mut ScreencopyState;
    fn image_copy_state(&mut self) -> &mut ImageCopyState;
    fn loop_handle(&self) -> LoopHandle<'static, Self>;
    fn is_output_mapped(&self, output: &Output) -> bool;

    /// Draws the request into its buffer. Returns `None` without drawing
    /// when [`CaptureRequest::wait_for_damage`] is set and nothing changed
    /// since the tracker's previous frame.
    fn draw_capture(&mut self, request: CaptureRequest<'_>) -> anyhow::Result<Option<Drawn>>;

    /// The render node and the modifiers a dmabuf in [`DMABUF_FORMAT`] can
    /// have, for clients that ask before allocating one.
    fn dmabuf_constraints(&mut self) -> Option<DmabufConstraints>;
}

pub struct CaptureRequest<'a> {
    pub output: &'a Output,
    /// The captured area in the output's physical pixels, after the output
    /// transform: the whole output, or a region of it.
    pub region: Rectangle<i32, Physical>,
    pub cursor: bool,
    pub target: &'a Target,
    pub tracker: &'a mut OutputDamageTracker,
    pub wait_for_damage: bool,
}

/// A drawn capture: what changed since the tracker's previous frame, in
/// buffer coordinates, and when the GPU is done.
pub struct Drawn {
    pub damage: Vec<Rectangle<i32, Buffer>>,
    pub sync: SyncPoint,
}

pub struct DmabufConstraints {
    pub device: u64,
    pub modifiers: Vec<u64>,
}

/// Where the client wants the pixels.
pub enum Target {
    Shm(WlBuffer),
    Dmabuf(Dmabuf),
}

impl Target {
    pub fn size(&self) -> Size<i32, Physical> {
        match self {
            Target::Shm(buffer) => shm::with_buffer_contents(buffer, |_, _, data| {
                Size::from((data.width, data.height))
            })
            .unwrap_or_default(),
            Target::Dmabuf(dmabuf) => Size::from((dmabuf.width() as i32, dmabuf.height() as i32)),
        }
    }

    /// The client's buffer, if it has the offered format and `size`.
    pub fn new(buffer: &WlBuffer, size: Size<i32, Physical>) -> Option<Self> {
        if let Ok(dmabuf) = dmabuf::get_dmabuf(buffer) {
            let ok = dmabuf.format().code == DMABUF_FORMAT
                && dmabuf.width() == size.w as u32
                && dmabuf.height() == size.h as u32;
            return ok.then(|| Target::Dmabuf(dmabuf.clone()));
        }
        let ok = shm::with_buffer_contents(buffer, |_, _, data| {
            data.format == SHM_FORMAT
                && data.width == size.w
                && data.height == size.h
                && data.stride >= size.w * 4
        })
        .unwrap_or(false);
        ok.then(|| Target::Shm(buffer.clone()))
    }
}

/// The size of a buffer holding `region` of `output`: the region before
/// the output transform.
pub fn buffer_size(output: &Output, region: Rectangle<i32, Physical>) -> Size<i32, Physical> {
    output
        .current_transform()
        .invert()
        .transform_size(region.size)
}

/// The whole of `output` in physical pixels, after its transform.
pub fn output_rect(output: &Output) -> Option<Rectangle<i32, Physical>> {
    let mode = output.current_mode()?;
    Some(Rectangle::from_size(
        output.current_transform().transform_size(mode.size),
    ))
}

/// Answers the waiting screen sharing requests for `output` if it changed.
/// The backends call this after each render of the output.
pub fn render_queued<D: CaptureHandler>(state: &mut D, output: &Output) {
    screencopy::render_queued(state, output);
    image_copy::render_queued(state, output);
}

/// Runs `done` with the time the capture was drawn once the GPU reached
/// `sync`, without blocking the compositor. Dropping `done` unrun, when
/// waiting fails, must tell the client the capture failed.
pub fn when_drawn<D: 'static>(
    sync: SyncPoint,
    loop_handle: &LoopHandle<'static, D>,
    done: impl FnOnce(Duration) + 'static,
) {
    // The client paces a stream by this time.
    let time: Duration = Clock::<Monotonic>::new().now().into();
    let Some(fence) = sync.export() else {
        if let Err(err) = sync.wait() {
            warn!(?err, "failed to wait for screen capture");
        }
        done(time);
        return;
    };
    let mut done = Some(done);
    let source = Generic::new(fence, Interest::READ, Mode::OneShot);
    let result = loop_handle.insert_source(source, move |_, _, _| {
        if let Some(done) = done.take() {
            done(time);
        }
        Ok(PostAction::Remove)
    });
    if let Err(err) = result {
        warn!(error = %err.error, "failed to wait for screen capture");
    }
}

/// Splits a timestamp into the protocols' seconds high, seconds low and
/// nanoseconds.
pub fn split_time(time: Duration) -> (u32, u32, u32) {
    let secs = time.as_secs();
    ((secs >> 32) as u32, secs as u32, time.subsec_nanos())
}
