//! A small control socket, so the panel can show and toggle the night light
//! with `rustwm night-light status` and `rustwm night-light toggle`.
//!
//! The socket is `$XDG_RUNTIME_DIR/rustwm/<wayland socket name>.sock`, so
//! each running rustwm has its own and a client finds it from
//! `WAYLAND_DISPLAY`. A client sends one line and gets one line back, then
//! the connection closes. The compositor reads and writes without blocking,
//! so a slow or stuck client cannot stall it.

use std::{
    env,
    fs::{self, DirBuilder},
    io::{ErrorKind, Read, Write},
    os::unix::{
        fs::DirBuilderExt,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, anyhow, bail};
use smithay::reexports::calloop::{Interest, Mode, PostAction, generic::Generic};
use tracing::{debug, info, warn};

use crate::{
    night_light::{Status, until_text},
    state::State,
};

/// Longer requests are not ours; the connection is closed.
const MAX_REQUEST: usize = 256;
const USAGE: &str = "usage: rustwm night-light toggle|status";
/// Replies that start with this are errors.
const ERROR_PREFIX: &str = "error: ";

/// The listening socket's file, removed when this is dropped.
pub struct ControlSocket {
    path: PathBuf,
}

impl Drop for ControlSocket {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

impl State {
    /// Listens on the control socket. Keep the result until exit; dropping
    /// it removes the socket file. `None` when the socket cannot be made,
    /// which only costs the panel its night light button.
    pub fn start_control_socket(&mut self) -> Option<ControlSocket> {
        match self.listen_for_control() {
            Ok(socket) => Some(socket),
            Err(err) => {
                warn!(%err, "no control socket; the panel cannot toggle the night light");
                None
            }
        }
    }

    fn listen_for_control(&mut self) -> anyhow::Result<ControlSocket> {
        let name = self.socket_name.to_string_lossy().into_owned();
        let path = socket_path(&name)?;
        if let Some(dir) = path.parent() {
            // Only we may connect: the socket toggles our night light.
            DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)
                .with_context(|| format!("failed to create {}", dir.display()))?;
        }
        // The Wayland socket's lock file makes the name ours, so a file
        // here is left over from a rustwm that did not exit cleanly.
        let _ = fs::remove_file(&path);
        let listener = UnixListener::bind(&path)
            .with_context(|| format!("failed to listen on {}", path.display()))?;
        let socket = ControlSocket { path };
        listener.set_nonblocking(true)?;
        self.loop_handle
            .insert_source(
                Generic::new(listener, Interest::READ, Mode::Level),
                |_, listener, state| {
                    accept_all(listener, state);
                    Ok(PostAction::Continue)
                },
            )
            .map_err(|err| anyhow!("failed to add the control socket: {}", err.error))?;
        info!(path = %socket.path.display(), "control socket ready");
        Ok(socket)
    }

    /// The reply to one request line.
    fn control_reply(&mut self, request: &str) -> String {
        match request {
            "night-light toggle" => {
                self.toggle_night_light();
                waybar_json(self.night_light_status())
            }
            "night-light status" => waybar_json(self.night_light_status()),
            _ => format!("{ERROR_PREFIX}unknown command {request:?}; {USAGE}"),
        }
    }
}

/// Takes every waiting connection and watches each for its request.
fn accept_all(listener: &UnixListener, state: &mut State) {
    loop {
        match listener.accept() {
            Ok((stream, _)) => watch_connection(stream, state),
            Err(err) if err.kind() == ErrorKind::WouldBlock => return,
            Err(err) => {
                warn!(%err, "control socket accept failed");
                return;
            }
        }
    }
}

/// Reads the connection's request as it arrives, without blocking, then
/// replies and closes it.
fn watch_connection(stream: UnixStream, state: &mut State) {
    if let Err(err) = stream.set_nonblocking(true) {
        debug!(%err, "control connection dropped");
        return;
    }
    let mut request = Vec::new();
    let result = state.loop_handle.insert_source(
        Generic::new(stream, Interest::READ, Mode::Level),
        move |_, stream, state| {
            // Reading through `&UnixStream` needs no mutable access to the
            // source's stream.
            let stream: &UnixStream = stream;
            Ok(read_request(stream, &mut request, state))
        },
    );
    if let Err(err) = result {
        warn!(err = %err.error, "failed to watch a control connection");
    }
}

/// Reads what has arrived. Once the line is complete, replies and asks for
/// the connection to be removed, which closes it.
fn read_request(mut stream: &UnixStream, request: &mut Vec<u8>, state: &mut State) -> PostAction {
    let mut chunk = [0; MAX_REQUEST];
    let ended = match stream.read(&mut chunk) {
        Ok(0) => true,
        Ok(n) => {
            request.extend_from_slice(&chunk[..n]);
            false
        }
        Err(err) if err.kind() == ErrorKind::WouldBlock => return PostAction::Continue,
        Err(err) => {
            debug!(%err, "control connection failed");
            return PostAction::Remove;
        }
    };
    let line_end = request.iter().position(|&b| b == b'\n');
    if line_end.is_none() && !ended {
        if request.len() > MAX_REQUEST {
            debug!("control request too long");
            return PostAction::Remove;
        }
        return PostAction::Continue;
    }
    let line = &request[..line_end.unwrap_or(request.len())];
    let reply = match std::str::from_utf8(line) {
        Ok(line) => state.control_reply(line.trim()),
        Err(_) => format!("{ERROR_PREFIX}{USAGE}"),
    };
    // A reply this short fits in the socket's buffer, so it is written
    // whole without waiting.
    if let Err(err) = stream.write_all(format!("{reply}\n").as_bytes()) {
        debug!(%err, "failed to answer a control request");
    }
    PostAction::Remove
}

/// What Waybar's custom module shows, as its JSON. Empty text hides the
/// module, so it shows only while the night light is on or paused.
fn waybar_json(status: Status) -> String {
    match status {
        Status::Off => r#"{"text":""}"#.to_string(),
        Status::On { until } => format!(
            r#"{{"text":"Night Light","class":"on","tooltip":"On{}"}}"#,
            until_text(until)
        ),
        Status::Paused { until } => format!(
            r#"{{"text":"Night Light Off","class":"paused","tooltip":"Off{}"}}"#,
            until_text(until)
        ),
    }
}

/// The control socket of the rustwm whose Wayland socket is `display`.
fn socket_path(display: &str) -> anyhow::Result<PathBuf> {
    // WAYLAND_DISPLAY may be a full path to the socket.
    let name = Path::new(display)
        .file_name()
        .ok_or_else(|| anyhow!("WAYLAND_DISPLAY {display:?} names no socket"))?;
    let runtime_dir = env::var_os("XDG_RUNTIME_DIR").context("XDG_RUNTIME_DIR is not set")?;
    let mut file = name.to_os_string();
    file.push(".sock");
    Ok(PathBuf::from(runtime_dir).join("rustwm").join(file))
}

/// `rustwm night-light toggle|status`: asks the running rustwm and prints
/// its reply.
pub fn run_client(args: &[String]) -> anyhow::Result<()> {
    let request = args.join(" ");
    if !matches!(
        request.as_str(),
        "night-light toggle" | "night-light status"
    ) {
        bail!(USAGE);
    }
    let display = env::var("WAYLAND_DISPLAY")
        .context("WAYLAND_DISPLAY is not set; run this inside rustwm")?;
    let path = socket_path(&display)?;
    let mut stream = UnixStream::connect(&path).with_context(|| {
        format!(
            "failed to connect to {}; is rustwm running?",
            path.display()
        )
    })?;
    let timeout = Some(Duration::from_secs(2));
    stream.set_read_timeout(timeout)?;
    stream.set_write_timeout(timeout)?;
    stream.write_all(format!("{request}\n").as_bytes())?;
    let mut reply = String::new();
    stream
        .read_to_string(&mut reply)
        .context("no reply from rustwm")?;
    let reply = reply.trim_end();
    if let Some(error) = reply.strip_prefix(ERROR_PREFIX) {
        bail!("{error}");
    }
    println!("{reply}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Local};

    use super::*;

    #[test]
    fn status_as_waybar_json() {
        assert_eq!(waybar_json(Status::Off), r#"{"text":""}"#);
        assert_eq!(
            waybar_json(Status::On { until: None }),
            r#"{"text":"Night Light","class":"on","tooltip":"On"}"#
        );
        let until = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let time = until.with_timezone(&Local).format("%H:%M").to_string();
        assert_eq!(
            waybar_json(Status::Paused { until: Some(until) }),
            format!(
                r#"{{"text":"Night Light Off","class":"paused","tooltip":"Off until {time}"}}"#
            )
        );
    }

    #[test]
    fn socket_path_from_the_wayland_display() {
        // Tests share the process environment; this is the only one that
        // touches XDG_RUNTIME_DIR, and only reads it.
        let Some(runtime_dir) = env::var_os("XDG_RUNTIME_DIR") else {
            return;
        };
        let expected = PathBuf::from(runtime_dir).join("rustwm/wayland-1.sock");
        assert_eq!(socket_path("wayland-1").unwrap(), expected);
        assert_eq!(socket_path("/run/user/1000/wayland-1").unwrap(), expected);
        assert!(socket_path("/").is_err());
    }
}
