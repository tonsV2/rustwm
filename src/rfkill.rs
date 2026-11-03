//! Runs GNOME's rfkill daemon so GNOME Settings shows its Bluetooth panel.
//!
//! GNOME Settings only lists Bluetooth when `gsd-rfkill` answers on the
//! session bus as `org.gnome.SettingsDaemon.Rfkill` and reports an adapter;
//! the airplane mode switches in its Wi-Fi and Bluetooth panels go through
//! it too. Its systemd unit requires a running gnome-session, so outside
//! GNOME it never starts, and it is not D-Bus activatable either. rustwm
//! therefore starts it itself, on whichever bus rustwm runs on.

use std::{
    path::Path,
    process::{Child, Command, Stdio},
};

use tracing::{info, warn};

/// Where distributions install it: Fedora and current Debian use libexec,
/// Arch puts it straight in /usr/lib, older Debian and Ubuntu releases in a
/// directory of its own.
const CANDIDATES: [&str; 3] = [
    "/usr/libexec/gsd-rfkill",
    "/usr/lib/gsd-rfkill",
    "/usr/lib/gnome-settings-daemon/gsd-rfkill",
];

/// A running `gsd-rfkill`. Dropping it stops the process.
pub struct Daemon {
    child: Child,
}

impl Daemon {
    /// Starts `gsd-rfkill` if gnome-settings-daemon is installed.
    ///
    /// If another copy already owns the name, such as one left behind by a
    /// rustwm killed by a signal, this one only idles until rustwm stops it.
    pub fn start() -> Option<Self> {
        let Some(path) = find(|path| Path::new(path).is_file()) else {
            info!("gsd-rfkill not found; GNOME Settings will not show Bluetooth");
            return None;
        };
        // Without gnome-session it logs "Unable to register client" at start;
        // that registration only matters for GNOME's logout, so it is harmless.
        info!(path, "starting gsd-rfkill");
        // It has no window, so it gets none of the Wayland or X variables.
        let result = Command::new(path)
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("DISPLAY")
            .stdin(Stdio::null())
            .spawn();
        match result {
            Ok(child) => Some(Self { child }),
            Err(err) => {
                warn!(%err, "failed to start gsd-rfkill");
                None
            }
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        // On rustwm's private bus it would exit once the bus goes away, but on
        // the shared user bus it would outlive the session.
        info!("stopping gsd-rfkill");
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn find(exists: impl Fn(&str) -> bool) -> Option<&'static str> {
    CANDIDATES.into_iter().find(|path| exists(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_first_installed_location() {
        assert_eq!(
            find(|path| path.starts_with("/usr/lib/")),
            Some("/usr/lib/gsd-rfkill")
        );
        assert_eq!(find(|_| true), Some("/usr/libexec/gsd-rfkill"));
    }

    #[test]
    fn finds_nothing_when_not_installed() {
        assert_eq!(find(|_| false), None);
    }
}
