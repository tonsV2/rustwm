mod apps;
mod bell;
mod capture;
mod commands;
mod config;
mod control;
mod cursor;
mod dialog;
mod fade;
mod focus;
mod gamma;
mod group;
mod handlers;
mod input;
mod keyboard;
mod keymap;
mod layout;
mod night_light;
mod osd;
mod overview;
mod panel;
mod reload;
mod rfkill;
mod security;
mod session;
mod state;
mod switcher;
mod udev;
mod window;
mod winit;
mod xwayland;

use std::io::IsTerminal;

use anyhow::Context;
use smithay::reexports::{
    calloop::{
        EventLoop,
        signals::{Signal, Signals},
    },
    wayland_server::Display,
};
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
    stop_on_signals(&event_loop)?;
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
    let graphical_session =
        session::export_to_dbus(&state.socket_name, x_display.as_deref(), nested);
    // Kept until the end of main, so it stops when rustwm exits. On another
    // desktop's bus that desktop runs its own.
    let _rfkill = session::bus_is_ours(nested)
        .then(rfkill::Daemon::start)
        .flatten();
    // Kept until the end of main; dropping it removes the socket file.
    // Bound before anything is started, so a panel or autostart command
    // asking for the night light's status finds it.
    let _control = state.start_control_socket();
    let notifications = &state.config.notifications.command;
    if !notifications.is_empty() {
        info!(command = notifications, "starting notifications");
        commands::run(notifications, &state.child_env(), || {});
    }
    let battery = &state.config.battery.command;
    if !battery.is_empty() {
        info!(command = battery, "starting battery watcher");
        commands::run(battery, &state.child_env(), || {});
    }
    for command in &state.config.autostart.commands {
        info!(command, "autostart");
        commands::run(command, &state.child_env(), || {});
    }
    state.start_panel();
    state.watch_config();
    state.start_night_light();

    let result = event_loop
        .run(None, &mut state, State::after_dispatch)
        .context("event loop failed");

    // Drop the compositor now rather than at the end of main: the backend
    // untints the screen and Smithay restores the display state it found,
    // before stopping the session target below waits on systemd, during
    // which on logout the display may be taken away or SIGKILL arrive.
    //
    // The signals stay blocked until the event loop drops at the very end,
    // so one arriving now cannot kill rustwm part way through the shutdown.
    // A shutdown that hangs still ends with logind's SIGKILL.
    drop(state);

    if graphical_session {
        session::stop_session_target();
    }
    info!("rustwm exiting");
    result
}

/// Stops the event loop on SIGTERM (logout, `systemctl stop`), SIGINT
/// (Ctrl+C in the terminal that started it) and SIGHUP, so `main` returns
/// and everything is dropped properly: the night light's tint comes off and
/// the console comes back. Without this the signals would kill rustwm on
/// the spot.
///
/// This blocks the signals for the thread and reads them from a file
/// descriptor instead, so it must run before any other thread starts;
/// threads started later inherit the block and leave the signals to this
/// one. Programs rustwm starts get an empty mask again from Rust's
/// `Command`.
fn stop_on_signals(event_loop: &EventLoop<'static, State>) -> anyhow::Result<()> {
    let signals = Signals::new(&[Signal::SIGTERM, Signal::SIGINT, Signal::SIGHUP])
        .context("failed to watch for signals")?;
    event_loop
        .handle()
        .insert_source(signals, |event, _, state| {
            info!(signal = ?event.signal(), "stopping on signal");
            state.loop_signal.stop();
        })
        .map_err(|err| anyhow::anyhow!("failed to add signals to the event loop: {}", err.error))?;
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
