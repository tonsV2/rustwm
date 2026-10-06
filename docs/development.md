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

- **Next to another graphical session** (GNOME on another VT, or nested inside it): rustwm gets a private bus from `dbus-run-session`. Single-instance apps (GTK apps, Ghostty) look for a running copy on the session bus, so on GNOME's bus their windows would open in GNOME. The keyring GNOME unlocked at login only answers on GNOME's bus, and the document portal must not get a second copy: it mounts at `$XDG_RUNTIME_DIR/doc`, the path Flatpak sandboxes and the file paths it hands to apps are built on, and a second portal there unmounts the shared one's mount when it exits, after which no Flatpak app starts (`bwrap: Can't find source path /run/user/1000/doc/by-app/...`) until `systemctl --user restart xdg-document-portal`. So the private bus runs with `resources/dbus/session.conf` (installed under `share/rustwm/dbus`), whose service files have D-Bus start `resources/rustwm-bus-relay` for `org.freedesktop.secrets` and `org.freedesktop.portal.Documents`. The relay owns the name on the private bus and passes calls through to the service on GNOME's bus. It needs `python3-gobject-base`, installed with GNOME. Without these files `rustwm-session` refuses to start a private bus. The document portal sees every relayed call as coming from the unsandboxed relay, so the relay passes on only harmless calls, such as asking where the portal is mounted, from sandboxed apps. On the private bus, Flatpak apps get files from the file chooser, but dragging or pasting files into or out of them does not work.
- **As the only graphical session** (from GDM, or a console while nobody is logged in to GNOME): rustwm uses the shared user bus, where PAM unlocked the keyring at a GDM login, and points the D-Bus and systemd activation environment (`WAYLAND_DISPLAY`, `XDG_CURRENT_DESKTOP`) at itself. It then starts `graphical-session.target` through its own `rustwm-session.target` (written to `$XDG_RUNTIME_DIR/systemd/user/`), which `xdg-desktop-portal` needs, and stops it on exit. After a console login the keyring is still locked, and the first app that needs it asks to unlock it.

A second `gnome-keyring-daemon` on the private bus does not help: `--start` finds the running daemon through its control socket in `$XDG_RUNTIME_DIR/keyring/`, initialises that one and exits, and `--replace` would take the keyring away from GNOME.

## Protocols

xdg-shell, wlr-layer-shell, clipboard and primary selection, linux-dmabuf, viewporter, fractional scale, single-pixel buffers, alpha modifier, content type, cursor shape, xdg-decoration (the compositor draws no decorations), xdg-activation, xdg-system-bell, xdg-dialog, ext-session-lock, ext-idle-notify, idle-inhibit, relative pointer, pointer constraints, security-context, wlr-output-power-management, wlr-screencopy and ext-image-copy-capture. X11 apps run through [xwayland-satellite](https://github.com/Supreeeme/xwayland-satellite), which rustwm starts on a free display when it is installed.
