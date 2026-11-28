//! `rustwmctl night-light toggle|status`: asks the running rustwm over its
//! control socket and prints the reply. The panel's night light module
//! runs it.
//!
//! A program of its own rather than arguments to `rustwm`, so a bar config
//! that is newer than the installed compositor fails with "command not
//! found" instead of starting a second compositor.

use std::{
    env,
    io::{Read, Write},
    os::unix::net::UnixStream,
    time::Duration,
};

use anyhow::{Context, anyhow, bail};
use rustwm::ipc;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    let request = args.join(" ");
    if !ipc::is_request(&request) {
        bail!(ipc::USAGE);
    }
    let display = env::var("WAYLAND_DISPLAY")
        .context("WAYLAND_DISPLAY is not set; run this inside rustwm")?;
    let runtime_dir = env::var_os("XDG_RUNTIME_DIR").context("XDG_RUNTIME_DIR is not set")?;
    let path = ipc::socket_path(&runtime_dir, &display)
        .ok_or_else(|| anyhow!("WAYLAND_DISPLAY {display:?} names no socket"))?;

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
    if let Some(error) = reply.strip_prefix(ipc::ERROR_PREFIX) {
        bail!("{error}");
    }
    println!("{reply}");
    Ok(())
}
