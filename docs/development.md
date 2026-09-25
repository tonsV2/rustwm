# Development

See [code-layout.md](code-layout.md) for where things live in `src/`.

## Dependencies

Fedora, for building and running nested:

```sh
sudo dnf install cargo rust rustfmt clippy mesa-libEGL-devel libxkbcommon-devel wayland-devel foot
```

The hardware backend also needs:

```sh
sudo dnf install libseat-devel libinput-devel mesa-libgbm-devel libdrm-devel systemd-devel
```

The programs rustwm starts for the panel, locking, notifications and so on are listed in the [README](../README.md#install).

## Build and test

```sh
cargo build
cargo test        # layout, focus history and keybinding logic
cargo clippy --all-targets
```

## Running nested

`scripts/nested.sh` opens rustwm in a window inside your normal session. The log prints the socket name; start clients on it with `WAYLAND_DISPLAY=wayland-1 foot`. Closing the window stops the compositor.

GNOME uses Super, Alt+Tab and Super+arrows for itself, so on GNOME the script clears those shortcuts while rustwm runs and restores them on exit. If the script is killed hard (`kill -9`), restore them by hand:

```sh
gsettings reset org.gnome.mutter overlay-key
gsettings reset org.gnome.desktop.wm.keybindings switch-windows
gsettings reset org.gnome.desktop.wm.keybindings switch-applications
gsettings reset org.gnome.desktop.wm.keybindings maximize
gsettings reset org.gnome.desktop.wm.keybindings unmaximize
gsettings reset org.gnome.mutter.keybindings toggle-tiled-left
gsettings reset org.gnome.mutter.keybindings toggle-tiled-right
```

## Running on the hardware

rustwm picks its backend automatically: inside a desktop session (`WAYLAND_DISPLAY` or `DISPLAY` set) it runs nested, otherwise it takes over the screen.

1. Switch to a free text console with `Ctrl+Alt+F3` and log in.
2. Run `./start.sh`. It builds a release binary and starts it through `resources/rustwm-session`, logging to `logs/rustwm.log` (the previous run's log is kept as `logs/rustwm.log.old`).
3. `Ctrl+Alt+Backspace` quits. `Ctrl+Alt+F2` (or whichever VT GNOME is on) switches back to your desktop while rustwm keeps running.

Installed as a login session, rustwm logs to the journal instead: `journalctl -b _UID=$(id -u)`.

## D-Bus bus and keyring

`resources/rustwm-session` decides which D-Bus session bus rustwm runs on. `start.sh`, `scripts/nested.sh` and the login session all go through it.

- **Next to another graphical session** (GNOME on another VT, or nested inside it): rustwm gets a private bus from `dbus-run-session`. Single-instance apps (GTK apps, Ghostty) look for a running copy on the session bus, so on GNOME's bus their windows would open in GNOME. The keyring GNOME unlocked at login only answers on GNOME's bus, so `rustwm-session` also starts `resources/rustwm-secrets-relay`, which owns `org.freedesktop.secrets` on the private bus and passes every call through to GNOME's keyring daemon. It needs `python3-gobject-base`, installed with GNOME.
- **As the only graphical session** (from GDM, or a console while nobody is logged in to GNOME): rustwm uses the shared user bus, where PAM unlocked the keyring at a GDM login, and points the D-Bus and systemd activation environment (`WAYLAND_DISPLAY`, `XDG_CURRENT_DESKTOP`) at itself. It then starts `graphical-session.target` through its own `rustwm-session.target` (written to `$XDG_RUNTIME_DIR/systemd/user/`), which `xdg-desktop-portal` needs, and stops it on exit. After a console login the keyring is still locked, and the first app that needs it asks to unlock it.

A second `gnome-keyring-daemon` on the private bus does not help: `--start` finds the running daemon through its control socket in `$XDG_RUNTIME_DIR/keyring/`, initialises that one and exits, and `--replace` would take the keyring away from GNOME.

## Protocols

xdg-shell, wlr-layer-shell, clipboard and primary selection, linux-dmabuf, viewporter, fractional scale, cursor shape, xdg-decoration (the compositor draws no decorations), xdg-activation, ext-session-lock, ext-idle-notify, idle-inhibit, wlr-output-power-management, wlr-screencopy and ext-image-copy-capture. X11 apps run through [xwayland-satellite](https://github.com/Supreeeme/xwayland-satellite), which rustwm starts on a free display when it is installed.
