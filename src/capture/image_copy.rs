//! ext-image-copy-capture-v1 with ext-output-image-capture-source-v1 and
//! ext-foreign-toplevel-image-capture-source-v1: the capture protocol
//! xdg-desktop-portal-wlr 0.8 prefers for sharing the screen or a window.
//! (Its wlr-screencopy path stops after the first frame, and cannot share
//! a window.)
//!
//! A client makes a capture source from an output, or from a window it
//! found through ext-foreign-toplevel-list, opens a session on it and
//! captures frames one at a time. The first frame of a session is drawn
//! right away; later ones wait until the source changed. A window source
//! is captured at the window's own size; when that changes, the session
//! announces the new size and the client allocates new buffers. Cursor
//! sessions (the pointer as a separate image) never produce frames: clients
//! get the pointer painted into the frame instead.

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
                ext_foreign_toplevel_image_capture_source_manager_v1::{
                    self, ExtForeignToplevelImageCaptureSourceManagerV1,
                },
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
    wayland::foreign_toplevel_list::{ForeignToplevelHandle, ForeignToplevelWeakHandle},
};
use tracing::{debug, warn};

use super::{
    CaptureHandler, CaptureRequest, DMABUF_FORMAT, Drawn, SHM_FORMAT, Subject, Target,
    ToplevelPlace, buffer_size, output_rect, split_time, when_drawn,
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
            + GlobalDispatch<ExtForeignToplevelImageCaptureSourceManagerV1, ()>
            + GlobalDispatch<ExtImageCopyCaptureManagerV1, ()>
            + 'static,
    {
        dh.create_global::<D, ExtOutputImageCaptureSourceManagerV1, ()>(1, ());
        dh.create_global::<D, ExtForeignToplevelImageCaptureSourceManagerV1, ()>(1, ());
        dh.create_global::<D, ExtImageCopyCaptureManagerV1, ()>(1, ());
        Self {
            sessions: Vec::new(),
            queue: Vec::new(),
        }
    }
}

/// A capture source: what it was made from, or `None` when that was
/// already gone.
pub struct SourceData {
    source: Option<Source>,
}

/// What a capture source shows. Both are held weakly: the session stops
/// when the output or the window goes away.
#[derive(Clone)]
enum Source {
    Output(WeakOutput),
    Toplevel(ForeignToplevelWeakHandle),
}

impl Source {
    /// Whether the source is gone for good: its output left, or its window
    /// closed. A window that is only not shown for now, such as while its
    /// output is being replaced, pauses its session instead.
    fn is_gone<D: CaptureHandler>(&self, state: &D) -> bool {
        match self {
            Source::Output(output) => output
                .upgrade()
                .is_none_or(|output| !state.is_output_mapped(&output)),
            Source::Toplevel(toplevel) => toplevel.upgrade().is_none_or(|t| t.is_closed()),
        }
    }
}

/// A source that is still shown. It is looked up again for every frame,
/// since outputs and windows come and go and windows change size.
enum Shown {
    Output(Output),
    Toplevel {
        toplevel: ForeignToplevelHandle,
        place: ToplevelPlace,
    },
}

impl Shown {
    fn find<D: CaptureHandler>(state: &D, source: &Source) -> Option<Self> {
        match source {
            Source::Output(output) => {
                let output = output.upgrade()?;
                let shown = state.is_output_mapped(&output) && output_rect(&output).is_some();
                shown.then_some(Shown::Output(output))
            }
            Source::Toplevel(toplevel) => {
                // A closed window's handle lives on while clients hold it.
                let toplevel = toplevel.upgrade().filter(|t| !t.is_closed())?;
                let place = state.toplevel_place(&toplevel)?;
                Some(Shown::Toplevel { toplevel, place })
            }
        }
    }

    /// The output the source is on. Its renders answer the waiting frames.
    fn output(&self) -> &Output {
        match self {
            Shown::Output(output) => output,
            Shown::Toplevel { place, .. } => &place.output,
        }
    }

    fn subject(&self) -> Subject<'_> {
        match self {
            Shown::Output(output) => Subject::Region {
                output,
                region: output_rect(output).unwrap_or_default(),
            },
            Shown::Toplevel { toplevel, .. } => Subject::Toplevel(toplevel),
        }
    }

    /// The size of a buffer holding the whole source.
    fn buffer_size(&self) -> Size<i32, Physical> {
        match self {
            Shown::Output(output) => output_rect(output)
                .map(|full| buffer_size(output, full))
                .unwrap_or_default(),
            Shown::Toplevel { place, .. } => place.region.size,
        }
    }

    /// How the client turns the buffer to show it upright. An output's
    /// buffer is in the monitor's orientation; a window is drawn upright.
    fn transform(&self) -> Transform {
        match self {
            Shown::Output(output) => output.current_transform(),
            Shown::Toplevel { .. } => Transform::Normal,
        }
    }
}

pub struct SessionData {
    /// `None` for a cursor session's capture session, which never
    /// produces frames.
    source: Option<Source>,
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

    /// The captured source, unless the session stopped.
    fn source(&self) -> Option<Source> {
        if self.inner().stopped {
            return None;
        }
        self.source.clone()
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
            let output = Output::from_resource(&output).map(|o| Source::Output(o.downgrade()));
            data_init.init(source, SourceData { source: output });
        }
    }
}

impl<D> GlobalDispatch<ExtForeignToplevelImageCaptureSourceManagerV1, (), D> for ImageCopyState
where
    D: Dispatch<ExtForeignToplevelImageCaptureSourceManagerV1, ()> + 'static,
{
    fn bind(
        _state: &mut D,
        _dh: &DisplayHandle,
        _client: &Client,
        manager: New<ExtForeignToplevelImageCaptureSourceManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(manager, ());
    }
}

impl<D> Dispatch<ExtForeignToplevelImageCaptureSourceManagerV1, (), D> for ImageCopyState
where
    D: Dispatch<ExtImageCaptureSourceV1, SourceData> + 'static,
{
    fn request(
        _state: &mut D,
        _client: &Client,
        _manager: &ExtForeignToplevelImageCaptureSourceManagerV1,
        request: ext_foreign_toplevel_image_capture_source_manager_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        use ext_foreign_toplevel_image_capture_source_manager_v1::Request;
        if let Request::CreateSource {
            source,
            toplevel_handle,
        } = request
        {
            let toplevel = ForeignToplevelHandle::from_resource(&toplevel_handle)
                .map(|t| Source::Toplevel(t.downgrade()));
            data_init.init(source, SourceData { source: toplevel });
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
                let source = source.data::<SourceData>().and_then(|s| s.source.clone());
                let session = data_init.init(
                    session,
                    SessionData {
                        source,
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
/// source is gone.
fn start_session<D: CaptureHandler>(state: &mut D, session: ExtImageCopyCaptureSessionV1) {
    match session_shown(state, &session) {
        Some(shown) => {
            send_constraints(state, &session, shown.buffer_size());
            state.image_copy_state().sessions.push(session);
        }
        None => stop_session(&session),
    }
}

/// What the session captures, if it still runs and its source is shown.
fn session_shown<D: CaptureHandler>(
    state: &D,
    session: &ExtImageCopyCaptureSessionV1,
) -> Option<Shown> {
    let source = session.data::<SessionData>()?.source()?;
    Shown::find(state, &source)
}

/// Whether the session stopped or its source is gone for good.
fn session_gone<D: CaptureHandler>(state: &D, session: &ExtImageCopyCaptureSessionV1) -> bool {
    session
        .data::<SessionData>()
        .and_then(|data| data.source())
        .is_none_or(|source| source.is_gone(state))
}

fn send_constraints<D: CaptureHandler>(
    state: &mut D,
    session: &ExtImageCopyCaptureSessionV1,
    size: Size<i32, Physical>,
) {
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

/// Tells the client the session ended. It is sent once; the session
/// produces no frames after it.
fn stop_session(session: &ExtImageCopyCaptureSessionV1) {
    if let Some(data) = session.data::<SessionData>() {
        let mut inner = data.inner();
        if inner.stopped {
            return;
        }
        inner.stopped = true;
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
                    source: None,
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
    if data.source().is_none() {
        frame.failed(FailureReason::Stopped);
        return;
    }
    let size = data.inner().size;
    let Some(target) = Target::new(buffer, size) else {
        debug!("captured into a buffer that does not match the constraints");
        frame.failed(FailureReason::BufferConstraints);
        return;
    };
    let frame = Frame {
        frame: frame.clone(),
        session: session.clone(),
        target,
        size,
        failure: FailureReason::Unknown,
        ready: false,
    };
    draw(state, frame);
}

/// Stops sessions whose source is gone, announces new buffer constraints
/// when a source changed size, and answers the frames waiting for `output`
/// to change. Sessions whose source is only not shown for now wait.
pub fn render_queued<D: CaptureHandler>(state: &mut D, output: &Output) {
    let sessions = std::mem::take(&mut state.image_copy_state().sessions);
    let mut kept = Vec::new();
    for session in sessions {
        if !session.is_alive() {
            continue;
        }
        let data = session.data::<SessionData>().expect("session without data");
        match session_shown(state, &session) {
            Some(shown) => {
                let size = shown.buffer_size();
                if size != data.inner().size {
                    send_constraints(state, &session, size);
                }
                kept.push(session);
            }
            None if session_gone(state, &session) => stop_session(&session),
            None => kept.push(session),
        }
    }
    let image_copy = state.image_copy_state();
    image_copy.sessions = kept;

    // Frames whose source is gone go along and fail in draw; frames whose
    // source is only not shown for now keep waiting.
    let queue = std::mem::take(&mut image_copy.queue);
    let (now, later): (Vec<_>, Vec<_>) =
        queue
            .into_iter()
            .partition(|f| match session_shown(state, &f.session) {
                Some(shown) => shown.output() == output,
                None => session_gone(state, &f.session),
            });
    state.image_copy_state().queue = later;
    for frame in now {
        draw(state, frame);
    }
}

/// Draws the frame and tells the client when it is ready, or queues it
/// when nothing changed since the session's previous frame.
fn draw<D: CaptureHandler>(state: &mut D, mut frame: Frame) {
    let session = frame.session.clone();
    let data = session.data::<SessionData>().expect("session without data");
    let Some(shown) = session_shown(state, &session) else {
        if session_gone(state, &session) {
            // The client learns why from the session's `stopped` first.
            stop_session(&session);
            frame.failure = FailureReason::Stopped;
        } else {
            state.image_copy_state().queue.push(frame);
        }
        return;
    };
    let size = shown.buffer_size();
    if size != frame.size {
        // The source changed size. The client allocates new buffers after
        // the new constraints.
        if data.inner().size != size {
            send_constraints(state, &session, size);
        }
        frame.failure = FailureReason::BufferConstraints;
        return;
    }
    let mut inner = data.inner();
    let result = state.draw_capture(CaptureRequest {
        subject: shown.subject(),
        cursor: data.paint_cursors,
        target: &frame.target,
        tracker: &mut inner.tracker,
        wait_for_damage: true,
    });
    match result {
        Ok(Some(drawn)) => {
            drop(inner);
            ready(state, frame, shown.transform(), drawn);
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

fn ready<D: CaptureHandler>(state: &mut D, mut frame: Frame, transform: Transform, drawn: Drawn) {
    frame.frame.transform(transform.into());
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
        smithay::reexports::wayland_server::delegate_global_dispatch!($ty: [
            smithay::reexports::wayland_protocols::ext::image_capture_source::v1::server::ext_foreign_toplevel_image_capture_source_manager_v1::ExtForeignToplevelImageCaptureSourceManagerV1: ()
        ] => $crate::capture::image_copy::ImageCopyState);
        smithay::reexports::wayland_server::delegate_dispatch!($ty: [
            smithay::reexports::wayland_protocols::ext::image_capture_source::v1::server::ext_foreign_toplevel_image_capture_source_manager_v1::ExtForeignToplevelImageCaptureSourceManagerV1: ()
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
