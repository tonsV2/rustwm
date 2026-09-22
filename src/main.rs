mod config;
mod cursor;
mod focus;
mod handlers;
mod input;
mod keyboard;
mod launcher;
mod layout;
mod state;
mod switcher;
mod udev;
mod window;
mod winit;

use std::io::IsTerminal;

use anyhow::Context;
use smithay::reexports::{calloop::EventLoop, wayland_server::Display};
use tracing::info;
use tracing_subscriber::EnvFilter;

use crate::state::{Backend, State};

fn main() -> anyhow::Result<()> {
    // Logs go to stderr so they can be captured apart from anything a
    // backend prints on stdout.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal())
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,smithay=warn")),
        )
        .init();

    info!("starting rustwm");

    let mut event_loop: EventLoop<'static, State> =
        EventLoop::try_new().context("failed to create event loop")?;
    let display: Display<State> = Display::new().context("failed to create Wayland display")?;

    let config = config::Config::load();
    let mut state = if nested() {
        info!("running nested inside the current session");
        let (winit, winit_loop, dmabuf_state) = winit::init_backend(&display.handle())?;
        let mut state = State::new(
            &mut event_loop,
            display,
            Backend::Winit(winit),
            dmabuf_state,
            config,
        )?;
        winit::run(&mut event_loop, &mut state, winit_loop)?;
        state
    } else {
        info!("running on the hardware (DRM/KMS)");
        let (udev, dmabuf_state) = udev::init_backend(&mut event_loop, &display.handle())?;
        let shm_formats = udev.shm_formats();
        let mut state = State::new(
            &mut event_loop,
            display,
            Backend::Udev(udev),
            dmabuf_state,
            config,
        )?;
        state.shm_state.update_formats(shm_formats);
        udev::scan_connectors(&mut state);
        state
    };

    info!(
        socket = ?state.socket_name,
        "ready; run clients with WAYLAND_DISPLAY={}",
        state.socket_name.to_string_lossy()
    );
    launcher::export_to_dbus(&state.socket_name);

    event_loop
        .run(None, &mut state, State::after_dispatch)
        .context("event loop failed")?;

    info!("rustwm exiting");
    Ok(())
}

/// Inside an existing desktop session the compositor runs as a window;
/// from a text console it takes over the hardware.
fn nested() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_some()
}
