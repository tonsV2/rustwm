//! Runs the outside commands bound to the brightness, volume and Print
//! keys.
//!
//! The compositor has no audio, power or screenshot logic of its own. It
//! starts the configured command (`brightnessctl`, `wpctl`, `grim`, ...)
//! and lets it do the work.

use std::{
    ffi::OsStr,
    process::{Command, Stdio},
    thread,
};

use tracing::{debug, warn};

/// Exit status of `sh -c` when the command is not on `PATH`.
const NOT_FOUND: i32 = 127;

/// Runs `command` through `sh -c`, without waiting for it to finish.
/// `then` runs on the waiting thread once the command has succeeded.
pub fn run(command: &str, wayland_display: &OsStr, then: impl FnOnce() + Send + 'static) {
    debug!(command, "running command");
    let result = Command::new("sh")
        .arg("-c")
        .arg(command)
        .env("WAYLAND_DISPLAY", wayland_display)
        .env_remove("DISPLAY")
        .stdin(Stdio::null())
        .spawn();
    let mut child = match result {
        Ok(child) => child,
        Err(err) => {
            warn!(command, %err, "failed to run command");
            return;
        }
    };

    // Waiting on a thread of its own reaps the process, so it does not
    // linger as a zombie, and reports failures without blocking the event
    // loop.
    let command = command.to_owned();
    let waiter = thread::Builder::new()
        .name("command".into())
        .spawn(move || match child.wait() {
            Ok(status) if status.success() => then(),
            Ok(status) if status.code() == Some(NOT_FOUND) => warn!(
                command,
                "command not found; install it or change it in config.toml"
            ),
            Ok(status) => warn!(command, %status, "command failed"),
            Err(err) => warn!(command, %err, "failed to wait for command"),
        });
    if let Err(err) = waiter {
        warn!(%err, "failed to start thread waiting for command");
    }
}
