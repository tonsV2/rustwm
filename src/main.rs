mod apps;
mod capture;
mod commands;
mod config;
mod cursor;
mod focus;
mod handlers;
mod input;
mod keyboard;
mod keymap;
mod launcher;
mod layout;
mod osd;
mod panel;
mod reload;
mod state;
mod switcher;
mod udev;
mod window;
mod winit;
mod xwayland;

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
    let nested = nested();
    let mut state = if nested {
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

    // The socket is listening, so the satellite can connect; it is served
    // once the event loop runs.
    state.xwayland = xwayland::Satellite::start(&state.socket_name);
    let x_display = state.xwayland.as_ref().map(|s| s.display.clone());
    launcher::export_to_dbus(&state.socket_name, x_display.as_deref(), nested);
    for command in &state.config.autostart.commands {
        info!(command, "autostart");
        commands::run(command, &state.child_env(), || {});
    }
    state.start_panel();
    state.watch_config();

    event_loop
        .run(None, &mut state, State::after_dispatch)
        .context("event loop failed")?;

    info!("rustwm exiting");
    Ok(())
}

/// Inside an existing desktop session the compositor runs as a window;
/// from a text console it takes over the hardware.
///
/// This reads the environment rustwm was started with. xwayland-satellite's
/// `DISPLAY` is only given to children, never set in our own environment.
fn nested() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_some()
}
