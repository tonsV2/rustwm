//! ext-image-copy-capture-v1 with ext-output-image-capture-source-v1: the
//! capture protocol xdg-desktop-portal-wlr 0.8 prefers for sharing the
//! screen. (Its wlr-screencopy path stops after the first frame.)
//!
//! A client makes a capture source from an output, opens a session on it
//! and captures frames one at a time. The first frame of a session is drawn
//! right away; later ones wait until the output changed. Only outputs can
//! be captured, and cursor sessions (the pointer as a separate image) never
//! produce frames: clients get the pointer painted into the frame instead.

use std::{
    sync::{
        Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use smithay::{
    backend::renderer::damage::OutputDamageTracker,
    output::{Output, WeakOutput},
    reexports::{
        wayland_protocols::ext::{
            image_capture_source::v1::server::{
                ext_image_capture_source_v1::ExtImageCaptureSourceV1,
                ext_output_image_capture_source_manager_v1::{
                    self, ExtOutputImageCaptureSourceManagerV1,
                },
            },
            image_copy_capture::v1::server::{
                ext_image_copy_capture_cursor_session_v1::{
                    self, ExtImageCopyCaptureCursorSessionV1,
                },
                ext_image_copy_capture_frame_v1::{
                    self, ExtImageCopyCaptureFrameV1, FailureReason,
                },
                ext_image_copy_capture_manager_v1::{self, ExtImageCopyCaptureManagerV1, Options},
                ext_image_copy_capture_session_v1::{self, ExtImageCopyCaptureSessionV1},
            },
        },
        wayland_server::{
            Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
            backend::ClientId, protocol::wl_buffer::WlBuffer,
        },
    },
    utils::{Physical, Size, Transform},
};
use tracing::{debug, warn};

use super::{
    CaptureHandler, CaptureRequest, DMABUF_FORMAT, Drawn, SHM_FORMAT, Target, buffer_size,
    output_rect, split_time, when_drawn,
};

/// Open sessions, plus frames waiting for their output to change.
pub struct ImageCopyState {
    sessions: Vec<ExtImageCopyCaptureSessionV1>,
    queue: Vec<Frame>,
}

impl ImageCopyState {
    pub fn new<D>(dh: &DisplayHandle) -> Self
    where
        D: GlobalDispatch<ExtOutputImageCaptureSourceManagerV1, ()>
            + GlobalDispatch<ExtImageCopyCaptureManagerV1, ()>
            + 'static,
    {
        dh.create_global::<D, ExtOutputImageCaptureSourceManagerV1, ()>(1, ());
        dh.create_global::<D, ExtImageCopyCaptureManagerV1, ()>(1, ());
        Self {
            sessions: Vec::new(),
            queue: Vec::new(),
        }
    }
}

/// A capture source: the output it was made from, if that still exists.
pub struct SourceData {
    output: Option<WeakOutput>,
}

pub struct SessionData {
    /// `None` for a cursor session's capture session, which never
    /// produces frames.
    output: Option<WeakOutput>,
    paint_cursors: bool,
    inner: Mutex<SessionInner>,
}

struct SessionInner {
    tracker: OutputDamageTracker,
    /// The buffer size last announced to the client.
    size: Size<i32, Physical>,
    frame: Option<ExtImageCopyCaptureFrameV1>,
    stopped: bool,
}

impl SessionData {
    fn inner(&self) -> MutexGuard<'_, SessionInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The captured output, if the session still captures one.
    fn output(&self) -> Option<Output> {
        if self.inner().stopped {
            return None;
        }
        self.output.as_ref()?.upgrade()
    }
}

pub struct FrameData {
    session: ExtImageCopyCaptureSessionV1,
    inner: Mutex<FrameInner>,
}

#[derive(Default)]
struct FrameInner {
    buffer: Option<WlBuffer>,
    captured: bool,
}

pub struct CursorSessionData {
    has_session: AtomicBool,
}

/// A captured frame, being drawn or waiting for damage. Dropping it before
/// it is ready tells the client it failed.
struct Frame {
    frame: ExtImageCopyCaptureFrameV1,
    session: ExtImageCopyCaptureSessionV1,
    output: Output,
    target: Target,
    size: Size<i32, Physical>,
    failure: FailureReason,
    ready: bool,
}

impl Frame {
    fn send_ready(&mut self, time: Duration) {
        let (hi, lo, nsec) = split_time(time);
        self.frame.presentation_time(hi, lo, nsec);
        self.frame.ready();
        self.ready = true;
    }
}

impl Drop for Frame {
    fn drop(&mut self) {
        if !self.ready {
            self.frame.failed(self.failure);
        }
    }
}

impl<D> GlobalDispatch<ExtOutputImageCaptureSourceManagerV1, (), D> for ImageCopyState
where
    D: Dispatch<ExtOutputImageCaptureSourceManagerV1, ()> + 'static,
{
    fn bind(
        _state: &mut D,
        _dh: &DisplayHandle,
        _client: &Client,
        manager: New<ExtOutputImageCaptureSourceManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(manager, ());
    }
}

impl<D> Dispatch<ExtOutputImageCaptureSourceManagerV1, (), D> for ImageCopyState
where
    D: Dispatch<ExtImageCaptureSourceV1, SourceData> + 'static,
{
    fn request(
        _state: &mut D,
        _client: &Client,
        _manager: &ExtOutputImageCaptureSourceManagerV1,
        request: ext_output_image_capture_source_manager_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        use ext_output_image_capture_source_manager_v1::Request;
        if let Request::CreateSource { source, output } = request {
            let output = Output::from_resource(&output).map(|o| o.downgrade());
            data_init.init(source, SourceData { output });
        }
    }
}

impl<D> Dispatch<ExtImageCaptureSourceV1, SourceData, D> for ImageCopyState {
    fn request(
        _state: &mut D,
        _client: &Client,
        _source: &ExtImageCaptureSourceV1,
        _request: <ExtImageCaptureSourceV1 as Resource>::Request,
        _data: &SourceData,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
    }
}

impl<D> GlobalDispatch<ExtImageCopyCaptureManagerV1, (), D> for ImageCopyState
where
    D: Dispatch<ExtImageCopyCaptureManagerV1, ()> + 'static,
{
    fn bind(
        _state: &mut D,
        _dh: &DisplayHandle,
        _client: &Client,
        manager: New<ExtImageCopyCaptureManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(manager, ());
    }
}

impl<D> Dispatch<ExtImageCopyCaptureManagerV1, (), D> for ImageCopyState
where
    D: CaptureHandler
        + Dispatch<ExtImageCopyCaptureSessionV1, SessionData>
        + Dispatch<ExtImageCopyCaptureCursorSessionV1, CursorSessionData>,
{
    fn request(
        state: &mut D,
        _client: &Client,
        manager: &ExtImageCopyCaptureManagerV1,
        request: ext_image_copy_capture_manager_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        use ext_image_copy_capture_manager_v1::{Error, Request};
        match request {
            Request::CreateSession {
                session,
                source,
                options,
            } => {
                let Some(options) = options
                    .into_result()
                    .ok()
                    .filter(|o| Options::all().contains(*o))
                else {
                    manager.post_error(Error::InvalidOption, "unknown capture option");
                    return;
                };
                let output = source.data::<SourceData>().and_then(|s| s.output.clone());
                let session = data_init.init(
                    session,
                    SessionData {
                        output,
                        paint_cursors: options.contains(Options::PaintCursors),
                        inner: Mutex::new(SessionInner {
                            tracker: OutputDamageTracker::new((0, 0), 1.0, Transform::Normal),
                            size: Size::default(),
                            frame: None,
                            stopped: false,
                        }),
                    },
                );
                start_session(state, session);
            }
            Request::CreatePointerCursorSession { session, .. } => {
                data_init.init(
                    session,
                    CursorSessionData {
                        has_session: AtomicBool::new(false),
                    },
                );
            }
            _ => {}
        }
    }
}

/// Announces the buffer constraints, or stops the session at once if its
/// output is gone.
fn start_session<D: CaptureHandler>(state: &mut D, session: ExtImageCopyCaptureSessionV1) {
    let data = session.data::<SessionData>().expect("session without data");
    match data.output().filter(|o| state.is_output_mapped(o)) {
        Some(output) => {
            send_constraints(state, &session, &output);
            state.image_copy_state().sessions.push(session);
        }
        None => stop_session(&session),
    }
}

fn send_constraints<D: CaptureHandler>(
    state: &mut D,
    session: &ExtImageCopyCaptureSessionV1,
    output: &Output,
) {
    let Some(full) = output_rect(output) else {
        return;
    };
    let size = buffer_size(output, full);
    let data = session.data::<SessionData>().expect("session without data");
    data.inner().size = size;
    session.buffer_size(size.w as u32, size.h as u32);
    session.shm_format(SHM_FORMAT);
    if let Some(dmabuf) = state.dmabuf_constraints() {
        session.dmabuf_device(dmabuf.device.to_ne_bytes().to_vec());
        let modifiers = dmabuf
            .modifiers
            .iter()
            .flat_map(|m| m.to_ne_bytes())
            .collect();
        session.dmabuf_format(DMABUF_FORMAT as u32, modifiers);
    }
    session.done();
}

fn stop_session(session: &ExtImageCopyCaptureSessionV1) {
    if let Some(data) = session.data::<SessionData>() {
        data.inner().stopped = true;
    }
    session.stopped();
}

impl<D> Dispatch<ExtImageCopyCaptureCursorSessionV1, CursorSessionData, D> for ImageCopyState
where
    D: Dispatch<ExtImageCopyCaptureSessionV1, SessionData> + 'static,
{
    fn request(
        _state: &mut D,
        _client: &Client,
        cursor_session: &ExtImageCopyCaptureCursorSessionV1,
        request: ext_image_copy_capture_cursor_session_v1::Request,
        data: &CursorSessionData,
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        use ext_image_copy_capture_cursor_session_v1::{Error, Request};
        if let Request::GetCaptureSession { session } = request {
            if data.has_session.swap(true, Ordering::SeqCst) {
                cursor_session.post_error(Error::DuplicateSession, "capture session exists");
                return;
            }
            let session = data_init.init(
                session,
                SessionData {
                    output: None,
                    paint_cursors: false,
                    inner: Mutex::new(SessionInner {
                        tracker: OutputDamageTracker::new((0, 0), 1.0, Transform::Normal),
                        size: Size::default(),
                        frame: None,
                        stopped: false,
                    }),
                },
            );
            stop_session(&session);
        }
    }
}

impl<D> Dispatch<ExtImageCopyCaptureSessionV1, SessionData, D> for ImageCopyState
where
    D: CaptureHandler + Dispatch<ExtImageCopyCaptureFrameV1, FrameData>,
{
    fn request(
        _state: &mut D,
        _client: &Client,
        session: &ExtImageCopyCaptureSessionV1,
        request: ext_image_copy_capture_session_v1::Request,
        data: &SessionData,
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        use ext_image_copy_capture_session_v1::{Error, Request};
        if let Request::CreateFrame { frame } = request {
            let mut inner = data.inner();
            if inner.frame.as_ref().is_some_and(|f| f.is_alive()) {
                session.post_error(Error::DuplicateFrame, "the session has a frame");
                return;
            }
            let frame = data_init.init(
                frame,
                FrameData {
                    session: session.clone(),
                    inner: Mutex::new(FrameInner::default()),
                },
            );
            inner.frame = Some(frame);
        }
    }

    fn destroyed(
        state: &mut D,
        _client: ClientId,
        session: &ExtImageCopyCaptureSessionV1,
        _data: &SessionData,
    ) {
        let image_copy = state.image_copy_state();
        image_copy.sessions.retain(|s| s != session);
        image_copy.queue.retain(|f| &f.session != session);
    }
}

impl<D> Dispatch<ExtImageCopyCaptureFrameV1, FrameData, D> for ImageCopyState
where
    D: CaptureHandler,
{
    fn request(
        state: &mut D,
        _client: &Client,
        frame: &ExtImageCopyCaptureFrameV1,
        request: ext_image_copy_capture_frame_v1::Request,
        data: &FrameData,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        use ext_image_copy_capture_frame_v1::{Error, Request};
        let mut inner = data.inner.lock().unwrap_or_else(|e| e.into_inner());
        match request {
            Request::AttachBuffer { buffer } => inner.buffer = Some(buffer),
            Request::DamageBuffer {
                x,
                y,
                width,
                height,
            } => {
                // Every frame is drawn in full, so the client's damage is
                // only checked.
                if x < 0 || y < 0 || width <= 0 || height <= 0 {
                    frame.post_error(Error::InvalidBufferDamage, "invalid buffer damage");
                }
            }
            Request::Capture => {
                if inner.captured {
                    frame.post_error(Error::AlreadyCaptured, "frame was already captured");
                    return;
                }
                let Some(buffer) = inner.buffer.clone() else {
                    frame.post_error(Error::NoBuffer, "no buffer attached");
                    return;
                };
                inner.captured = true;
                drop(inner);
                capture(state, frame, &data.session, &buffer);
            }
            _ => {}
        }
    }

    fn destroyed(
        state: &mut D,
        _client: ClientId,
        frame: &ExtImageCopyCaptureFrameV1,
        _data: &FrameData,
    ) {
        state.image_copy_state().queue.retain(|f| &f.frame != frame);
    }
}

fn capture<D: CaptureHandler>(
    state: &mut D,
    frame: &ExtImageCopyCaptureFrameV1,
    session: &ExtImageCopyCaptureSessionV1,
    buffer: &WlBuffer,
) {
    let Some(data) = session.data::<SessionData>() else {
        return;
    };
    let Some(output) = data.output() else {
        frame.failed(FailureReason::Stopped);
        return;
    };
    let size = data.inner().size;
    let Some(target) = Target::new(buffer, size) else {
        debug!("captured into a buffer that does not match the constraints");
        frame.failed(FailureReason::BufferConstraints);
        return;
    };
    let frame = Frame {
        frame: frame.clone(),
        session: session.clone(),
        output,
        target,
        size,
        failure: FailureReason::Unknown,
        ready: false,
    };
    draw(state, frame);
}

/// Stops sessions whose output is gone, announces new buffer constraints
/// when `output` changed size, and answers the frames waiting for it to
/// change.
pub fn render_queued<D: CaptureHandler>(state: &mut D, output: &Output) {
    let sessions = std::mem::take(&mut state.image_copy_state().sessions);
    let mut kept = Vec::new();
    for session in sessions {
        if !session.is_alive() {
            continue;
        }
        let data = session.data::<SessionData>().expect("session without data");
        match data.output() {
            Some(o) if state.is_output_mapped(&o) => {
                let size = output_rect(&o).map(|full| buffer_size(&o, full));
                if &o == output && size.is_some_and(|s| s != data.inner().size) {
                    send_constraints(state, &session, &o);
                }
                kept.push(session);
            }
            _ => stop_session(&session),
        }
    }
    let image_copy = state.image_copy_state();
    image_copy.sessions = kept;

    let queue = std::mem::take(&mut image_copy.queue);
    let (now, later): (Vec<_>, Vec<_>) = queue.into_iter().partition(|f| &f.output == output);
    state.image_copy_state().queue = later;
    // Frames of stopped sessions fail in draw.
    for frame in now {
        draw(state, frame);
    }
    let queue = std::mem::take(&mut state.image_copy_state().queue);
    for mut frame in queue {
        if session_output(&frame.session).is_some() {
            state.image_copy_state().queue.push(frame);
        } else {
            frame.failure = FailureReason::Stopped;
        }
    }
}

fn session_output(session: &ExtImageCopyCaptureSessionV1) -> Option<Output> {
    session.data::<SessionData>()?.output()
}

/// Draws the frame and tells the client when it is ready, or queues it
/// when nothing changed since the session's previous frame.
fn draw<D: CaptureHandler>(state: &mut D, mut frame: Frame) {
    let session = frame.session.clone();
    let data = session.data::<SessionData>().expect("session without data");
    if data.output().is_none() {
        frame.failure = FailureReason::Stopped;
        return;
    }
    let Some(region) = output_rect(&frame.output) else {
        return;
    };
    if buffer_size(&frame.output, region) != frame.size {
        // The output changed size. The client allocates new buffers after
        // the new constraints.
        if data.inner().size != buffer_size(&frame.output, region) {
            send_constraints(state, &session, &frame.output);
        }
        frame.failure = FailureReason::BufferConstraints;
        return;
    }
    let mut inner = data.inner();
    let result = state.draw_capture(CaptureRequest {
        output: &frame.output,
        region,
        cursor: data.paint_cursors,
        target: &frame.target,
        tracker: &mut inner.tracker,
        wait_for_damage: true,
    });
    match result {
        Ok(Some(drawn)) => {
            drop(inner);
            ready(state, frame, drawn);
        }
        Ok(None) => {
            drop(inner);
            state.image_copy_state().queue.push(frame);
        }
        Err(err) => {
            warn!("screen capture failed: {err:#}");
            // Start over, so the next frame is drawn in full.
            inner.tracker = OutputDamageTracker::new((0, 0), 1.0, Transform::Normal);
        }
    }
}

fn ready<D: CaptureHandler>(state: &mut D, mut frame: Frame, drawn: Drawn) {
    frame
        .frame
        .transform(frame.output.current_transform().into());
    for rect in &drawn.damage {
        frame
            .frame
            .damage(rect.loc.x, rect.loc.y, rect.size.w, rect.size.h);
    }
    when_drawn(drawn.sync, &state.loop_handle(), move |time| {
        frame.send_ready(time);
    });
}

/// Wires the protocols to `State`, like Smithay's `delegate_*!` macros.
#[macro_export]
macro_rules! delegate_image_copy {
    ($ty:ty) => {
        smithay::reexports::wayland_server::delegate_global_dispatch!($ty: [
            smithay::reexports::wayland_protocols::ext::image_capture_source::v1::server::ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1: ()
        ] => $crate::capture::image_copy::ImageCopyState);
        smithay::reexports::wayland_server::delegate_global_dispatch!($ty: [
            smithay::reexports::wayland_protocols::ext::image_copy_capture::v1::server::ext_image_copy_capture_manager_v1::ExtImageCopyCaptureManagerV1: ()
        ] => $crate::capture::image_copy::ImageCopyState);
        smithay::reexports::wayland_server::delegate_dispatch!($ty: [
            smithay::reexports::wayland_protocols::ext::image_capture_source::v1::server::ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1: ()
        ] => $crate::capture::image_copy::ImageCopyState);
        smithay::reexports::wayland_server::delegate_dispatch!($ty: [
            smithay::reexports::wayland_protocols::ext::image_capture_source::v1::server::ext_image_capture_source_v1::ExtImageCaptureSourceV1: $crate::capture::image_copy::SourceData
        ] => $crate::capture::image_copy::ImageCopyState);
        smithay::reexports::wayland_server::delegate_dispatch!($ty: [
            smithay::reexports::wayland_protocols::ext::image_copy_capture::v1::server::ext_image_copy_capture_manager_v1::ExtImageCopyCaptureManagerV1: ()
        ] => $crate::capture::image_copy::ImageCopyState);
        smithay::reexports::wayland_server::delegate_dispatch!($ty: [
            smithay::reexports::wayland_protocols::ext::image_copy_capture::v1::server::ext_image_copy_capture_session_v1::ExtImageCopyCaptureSessionV1: $crate::capture::image_copy::SessionData
        ] => $crate::capture::image_copy::ImageCopyState);
        smithay::reexports::wayland_server::delegate_dispatch!($ty: [
            smithay::reexports::wayland_protocols::ext::image_copy_capture::v1::server::ext_image_copy_capture_cursor_session_v1::ExtImageCopyCaptureCursorSessionV1: $crate::capture::image_copy::CursorSessionData
        ] => $crate::capture::image_copy::ImageCopyState);
        smithay::reexports::wayland_server::delegate_dispatch!($ty: [
            smithay::reexports::wayland_protocols::ext::image_copy_capture::v1::server::ext_image_copy_capture_frame_v1::ExtImageCopyCaptureFrameV1: $crate::capture::image_copy::FrameData
        ] => $crate::capture::image_copy::ImageCopyState);
    };
}
