//! Runs X11 applications through xwayland-satellite.
//!
//! The compositor contains no X11 code. xwayland-satellite is an ordinary
//! Wayland client that runs a rootless Xwayland and shows each X11 window as
//! an xdg-shell toplevel, so rustwm manages it like any other window.

use std::{
    ffi::OsStr,
    path::Path,
    process::{Child, Command, Stdio},
};

use tracing::{info, warn};

use crate::launcher::is_on_path;

const COMMAND: &str = "xwayland-satellite";

/// Highest X display number tried when looking for a free one.
const MAX_DISPLAY: u32 = 32;

/// A running xwayland-satellite. Dropping it stops the process.
pub struct Satellite {
    /// The X display it serves, such as `:1`.
    pub display: String,
    child: Child,
}

impl Satellite {
    /// Starts xwayland-satellite on our Wayland socket, if it is installed.
    ///
    /// The display is chosen here and passed in, rather than left to the
    /// satellite, so the compositor knows which `DISPLAY` to give its
    /// children without parsing the satellite's output.
    pub fn start(wayland_display: &OsStr) -> Option<Self> {
        if !is_on_path(COMMAND) {
            info!("xwayland-satellite not found on PATH; X11 apps will not run");
            return None;
        }
        let Some(number) = first_free_display(display_taken) else {
            warn!("no free X display number; not starting xwayland-satellite");
            return None;
        };
        let x_display = format!(":{number}");

        info!(display = x_display, "starting xwayland-satellite");
        let result = Command::new(COMMAND)
            .arg(&x_display)
            .env("WAYLAND_DISPLAY", wayland_display)
            // Nested, this is the host's X server, which the satellite has no
            // use for.
            .env_remove("DISPLAY")
            .stdin(Stdio::null())
            .spawn();
        match result {
            Ok(child) => Some(Self {
                display: x_display,
                child,
            }),
            Err(err) => {
                warn!(%err, "failed to start xwayland-satellite");
                None
            }
        }
    }
}

impl Drop for Satellite {
    fn drop(&mut self) {
        // It would also exit once our socket goes away, but only after it
        // notices; stopping it here leaves nothing behind.
        info!("stopping xwayland-satellite");
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Whether an X server already uses display `number`: it holds a lock file
/// or a socket for it.
fn display_taken(number: u32) -> bool {
    Path::new(&format!("/tmp/.X{number}-lock")).exists()
        || Path::new(&format!("/tmp/.X11-unix/X{number}")).exists()
}

fn first_free_display(taken: impl Fn(u32) -> bool) -> Option<u32> {
    (0..=MAX_DISPLAY).find(|&number| !taken(number))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_lowest_free_display() {
        assert_eq!(first_free_display(|n| n < 2), Some(2));
        assert_eq!(first_free_display(|_| false), Some(0));
        assert_eq!(first_free_display(|n| n != 5), Some(5));
    }

    #[test]
    fn gives_up_when_all_displays_are_taken() {
        assert_eq!(first_free_display(|_| true), None);
    }
}
