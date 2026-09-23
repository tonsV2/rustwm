//! Runs the outside commands bound to the brightness, volume and Print
//! keys.
//!
//! The compositor has no audio, power or screenshot logic of its own. It
//! starts the configured command (`brightnessctl`, `wpctl`, `grim`, ...)
//! and lets it do the work.

use std::{
    process::{Command, Stdio},
    thread,
};

use tracing::{debug, warn};

use crate::launcher::ChildEnv;

/// Exit status of `sh -c` when the command is not on `PATH`.
const NOT_FOUND: i32 = 127;

/// Runs `command` through `sh -c`, without waiting for it to finish.
/// `then` runs on the waiting thread once the command has succeeded.
pub fn run(command: &str, child_env: &ChildEnv, then: impl FnOnce() + Send + 'static) {
    run_with(command, child_env, &[], then);
}

/// Like [`run`], with extra environment variables.
pub fn run_with(
    command: &str,
    child_env: &ChildEnv,
    envs: &[(&str, &str)],
    then: impl FnOnce() + Send + 'static,
) {
    debug!(command, "running command");
    let mut sh = Command::new("sh");
    sh.arg("-c").arg(command).stdin(Stdio::null());
    child_env.apply(&mut sh);
    sh.envs(envs.iter().copied());
    let result = sh.spawn();
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
