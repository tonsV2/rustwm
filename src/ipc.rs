//! What the compositor's control socket and `rustwmctl` agree on: where the
//! socket is and what goes over it.
//!
//! The socket is `$XDG_RUNTIME_DIR/rustwm/<wayland socket name>.sock`, so
//! each running rustwm has its own and a client finds it from
//! `WAYLAND_DISPLAY`. A client sends one request line and gets one line
//! back, then the connection closes.

use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
};

/// Turn the night light off or on until morning, as Super+n does.
pub const NIGHT_LIGHT_TOGGLE: &str = "night-light toggle";
/// What the night light is doing, as JSON for Waybar.
pub const NIGHT_LIGHT_STATUS: &str = "night-light status";
pub const USAGE: &str = "usage: rustwmctl night-light toggle|status";
/// Replies that start with this are errors.
pub const ERROR_PREFIX: &str = "error: ";

/// The requests the compositor understands.
pub fn is_request(text: &str) -> bool {
    text == NIGHT_LIGHT_TOGGLE || text == NIGHT_LIGHT_STATUS
}

/// The control socket of the rustwm whose Wayland socket is `display`,
/// under `runtime_dir` (`XDG_RUNTIME_DIR`). `None` when `display` names no
/// socket.
pub fn socket_path(runtime_dir: &OsStr, display: &str) -> Option<PathBuf> {
    // WAYLAND_DISPLAY may be a full path to the socket.
    let name = Path::new(display).file_name()?;
    let mut file = name.to_os_string();
    file.push(".sock");
    Some(Path::new(runtime_dir).join("rustwm").join(file))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_path_from_the_wayland_display() {
        let runtime_dir = OsStr::new("/run/user/1000");
        let expected = PathBuf::from("/run/user/1000/rustwm/wayland-1.sock");
        assert_eq!(
            socket_path(runtime_dir, "wayland-1"),
            Some(expected.clone())
        );
        assert_eq!(
            socket_path(runtime_dir, "/run/user/1000/wayland-1"),
            Some(expected)
        );
        assert_eq!(socket_path(runtime_dir, "/"), None);
    }

    #[test]
    fn requests() {
        assert!(is_request("night-light toggle"));
        assert!(is_request("night-light status"));
        assert!(!is_request("night-light"));
        assert!(!is_request(""));
    }
}
