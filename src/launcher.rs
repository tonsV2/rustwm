//! Starts the external application launcher (Fuzzel).
//!
//! The compositor only spawns the process. Fuzzel finds, searches and
//! launches applications on its own.

use std::{
    env,
    ffi::OsStr,
    path::Path,
    process::{Child, Command},
};

use tracing::{info, warn};

/// Launcher executable, overridable with `RUSTWM_LAUNCHER` for testing.
const DEFAULT_COMMAND: &str = "fuzzel";

pub struct Launcher {
    command: String,
    child: Option<Child>,
    /// The X display of xwayland-satellite, given to launched apps as
    /// `DISPLAY` so X11-only apps can run.
    x_display: Option<String>,
}

impl Launcher {
    pub fn new() -> Self {
        let command = env::var("RUSTWM_LAUNCHER").unwrap_or_else(|_| DEFAULT_COMMAND.into());
        if !is_on_path(&command) {
            warn!(
                command,
                "launcher not found on PATH; Super will do nothing until it is installed"
            );
        }
        Self {
            command,
            child: None,
            x_display: None,
        }
    }

    pub fn set_x_display(&mut self, display: Option<String>) {
        self.x_display = display;
    }

    /// Starts the launcher, or closes it if it is already open.
    ///
    /// Toggling keeps a second Super tap from stacking launcher instances.
    pub fn toggle(&mut self, wayland_display: &OsStr) {
        if let Some(child) = &mut self.child {
            match child.try_wait() {
                Ok(None) => {
                    info!("closing launcher");
                    let _ = child.kill();
                    let _ = child.wait();
                    self.child = None;
                    return;
                }
                // Already exited; reaping it here avoids a zombie process.
                Ok(Some(_)) | Err(_) => self.child = None,
            }
        }

        info!(command = self.command, "starting launcher");
        let mut command = Command::new(&self.command);
        command.env("WAYLAND_DISPLAY", wayland_display);
        // Keep apps started from the launcher on our Wayland socket instead
        // of falling back to the host's X server when nested. X11-only apps
        // get xwayland-satellite's display, whose windows are ours too.
        match &self.x_display {
            Some(display) => command.env("DISPLAY", display),
            None => command.env_remove("DISPLAY"),
        };
        let result = command.spawn();
        match result {
            Ok(child) => self.child = Some(child),
            Err(err) => warn!(command = self.command, %err, "failed to start launcher"),
        }
    }
}

pub fn is_on_path(command: &str) -> bool {
    if command.contains('/') {
        return Path::new(command).is_file();
    }
    env::var_os("PATH")
        .map(|paths| env::split_paths(&paths).any(|dir| dir.join(command).is_file()))
        .unwrap_or(false)
}

/// Makes apps that D-Bus starts on our behalf connect to this compositor.
///
/// Single-instance apps (GTK apps, Ghostty, ...) first look for a running
/// copy on the D-Bus session bus and hand the request to it. On the bus
/// shared with a GNOME session, that copy lives in GNOME, so the window
/// opens there. `rustwm-session` therefore gives rustwm a bus of its own
/// when another graphical session runs; this then points that bus's
/// activation environment at our Wayland socket.
///
/// As the only graphical session, `rustwm-session` keeps the shared user bus
/// (where PAM unlocked the keyring) and sets `RUSTWM_SHARED_BUS=1`. Then the
/// shared bus and the systemd user manager get our environment too. Without
/// that marker the shared bus is left alone: changing its activation
/// environment would send GNOME's apps here instead. A nested compositor
/// never touches it.
///
/// `x_display` is xwayland-satellite's display, exported as `DISPLAY` when
/// it runs.
pub fn export_to_dbus(wayland_display: &OsStr, x_display: Option<&str>, nested: bool) {
    let bus = env::var("DBUS_SESSION_BUS_ADDRESS").unwrap_or_default();
    let shared_bus = env::var("XDG_RUNTIME_DIR")
        .map(|dir| format!("unix:path={dir}/bus"))
        .unwrap_or_default();
    let on_shared_bus = bus.is_empty() || bus == shared_bus;
    let shared_bus_ok = !nested && env::var_os("RUSTWM_SHARED_BUS").is_some_and(|v| v == "1");

    if on_shared_bus && !shared_bus_ok {
        warn!(
            "running on the shared D-Bus session bus; single-instance apps that are \
             already running in another session will open their windows there. \
             Start rustwm with `rustwm-session` to avoid this"
        );
        return;
    }

    let mut arg = std::ffi::OsString::from("WAYLAND_DISPLAY=");
    arg.push(wayland_display);
    let mut command = Command::new("dbus-update-activation-environment");
    if on_shared_bus {
        command.arg("--systemd");
    }
    command.arg(arg).arg("XDG_CURRENT_DESKTOP=rustwm");
    if let Some(display) = x_display {
        command.arg(format!("DISPLAY={display}"));
    }
    match command.status() {
        Ok(status) if status.success() => info!("D-Bus activation environment updated"),
        Ok(status) => warn!(%status, "failed to update D-Bus activation environment"),
        Err(err) => warn!(%err, "failed to run dbus-update-activation-environment"),
    }
}
