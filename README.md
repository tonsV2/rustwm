# rustwm

A minimal, keyboard-driven Wayland compositor built on [Smithay](https://github.com/Smithay/smithay). See [spec.md](spec.md) for the full design.

Every application opens filling the screen. Snap windows to halves and quarters from the keyboard, switch between them with Alt+Tab, and start applications with [Fuzzel](https://codeberg.org/dnkl/fuzzel).

## Keys

| Keys | Action |
| --- | --- |
| `Super` (tap) | Open Fuzzel, or close it if it is open |
| `Alt+Tab` | Switch to the previous window (most recently used first). Keep Alt held to see thumbnails of your windows; Tab/Shift+Tab or the arrow keys move the highlight, releasing Alt switches, Escape cancels |
| `Super+Left` / `Right` / `Up` / `Down` | Snap the focused window to that half |
| `Super+Left` then `Up` (Super held) | Snap to a quarter; any horizontal plus vertical pair works |
| `Super+<direction the window is already in>` | Back to full screen |
| `Ctrl+Alt+Backspace` | Quit rustwm |
| `Ctrl+Alt+F1`…`F12` | Switch virtual terminal (hardware backend only) |

Dialogs (windows with a parent, such as file pickers) open centred at their own size. Snapping a dialog treats it as full screen first.

## Status

Done:

- Nested backend (runs as a window inside your current session)
- Hardware backend (DRM/KMS with libseat, libinput and GBM): one GPU and one monitor, the first connected one; VT switching; a drawn cursor (XCURSOR_THEME, or a built-in arrow); tap-to-click enabled on touchpads
- xdg-shell windows and popups, wlr-layer-shell (needed by Fuzzel), clipboard and primary selection, linux-dmabuf, viewporter, server-side decoration negotiation (the compositor draws no decorations)
- Full-screen initial placement, dialogs floating centred, half and quarter layouts
- Most-recently-used Alt+Tab with live window thumbnails, application icons and names
- Super tap launches Fuzzel; Super used with another key or a click does not

Not done yet:

- Multiple monitors (optional per the spec). The code keeps each window's output and computes layouts from the output's position, so it can be added later.

## Fedora dependencies

Nested development needs only Rust and Mesa, which Fedora Workstation has already:

```sh
sudo dnf install cargo rust rustfmt clippy mesa-libEGL-devel libxkbcommon-devel wayland-devel
sudo dnf install fuzzel foot   # launcher, plus a simple terminal to test with
```

The hardware backend additionally needs:

```sh
sudo dnf install libseat-devel libinput-devel mesa-libgbm-devel libdrm-devel systemd-devel
```

## Build and test

```sh
cargo build
cargo test        # layout, focus history and keybinding logic
```

## Running nested (safe)

This opens rustwm in a window inside your normal session. Nothing about your desktop is replaced.

```sh
scripts/nested.sh
```

GNOME uses Super, Alt+Tab and Super+arrows for itself, so the nested window never sees them. On GNOME, the script clears those shortcuts while rustwm runs and restores them on exit, including on Ctrl+C. If the script is killed hard (for example `kill -9`), restore them by hand:

```sh
gsettings reset org.gnome.mutter overlay-key
gsettings reset org.gnome.desktop.wm.keybindings switch-windows
gsettings reset org.gnome.desktop.wm.keybindings switch-applications
gsettings reset org.gnome.desktop.wm.keybindings maximize
gsettings reset org.gnome.desktop.wm.keybindings unmaximize
gsettings reset org.gnome.mutter.keybindings toggle-tiled-left
gsettings reset org.gnome.mutter.keybindings toggle-tiled-right
```

The log prints the socket name. Start clients on it from another terminal:

```sh
WAYLAND_DISPLAY=wayland-1 foot
```

Closing the nested window stops the compositor.

## Running on the hardware

rustwm picks its backend automatically: inside a desktop session (`WAYLAND_DISPLAY` or `DISPLAY` set) it runs nested, otherwise it takes over the screen.

1. Switch to a free text console with `Ctrl+Alt+F3` and log in.
2. Run `./start.sh` from the checkout. It builds a release binary and starts it on its own D-Bus bus, logging to `logs/rustwm.log` (the previous run's log is kept as `logs/rustwm.log.old`).
3. `Ctrl+Alt+Backspace` quits and returns to the console. `Ctrl+Alt+F2` (or whichever VT GNOME is on) switches back to your desktop while rustwm keeps running.

Start a first application with a Super tap (Fuzzel). If something goes wrong, see `logs/rustwm.log`.

`dbus-run-session` matters while you are also logged in to GNOME. Many apps (GTK apps, Ghostty) run as a single instance: on launch they look for a running copy on the D-Bus session bus and hand the request over. Without a bus of its own, rustwm shares GNOME's, and the window opens in GNOME instead. `scripts/nested.sh` does the same for the nested backend.

### As a login session

Once it works from a console, you can add it to the GDM session list:

```sh
sudo install -m 755 target/release/rustwm /usr/local/bin/rustwm
sudo install -m 644 resources/rustwm.desktop /usr/share/wayland-sessions/rustwm.desktop
```

Log out and pick "rustwm" from the gear menu on the login screen.

## Configuration

Settings live in `~/.config/rustwm/config.toml`. Copy [resources/config.toml](resources/config.toml) there as a starting point. Everything is optional, and changes take effect when rustwm restarts.

| Setting | Default | Meaning |
| --- | --- | --- |
| `output.scale` | guessed from pixel density | Output scale, e.g. `1.25` |
| `input.pointer_speed` | `0.0` | Pointer acceleration, `-1.0` to `1.0` |
| `input.natural_scroll` | `false` | Touchpad content follows the fingers |
| `input.tap_to_click` | `true` | Tap the touchpad to click |

GNOME Settings does not work here: it refuses to start outside GNOME, and its display, mouse and keyboard pages configure GNOME's compositor, not rustwm. Settings that apps read themselves (dark style, fonts, text size) are stored in dconf and still apply when you change them from GNOME.

### Environment variables

| Variable | Purpose |
| --- | --- |
| `RUST_LOG` | Log filter, default `info,smithay=warn`. `RUST_LOG=rustwm=debug` shows window placement |
| `RUSTWM_LAUNCHER` | Launcher command instead of `fuzzel` |
| `XKB_DEFAULT_LAYOUT`, `XKB_DEFAULT_VARIANT`, `XKB_DEFAULT_OPTIONS` | Keyboard layout, e.g. `XKB_DEFAULT_LAYOUT=dk` |
| `XCURSOR_THEME`, `XCURSOR_SIZE` | Cursor theme and size on the hardware backend |

## Known limitations

- The hardware backend uses one monitor at a time, in its preferred mode.
- Only the primary GPU is used.
- Client requests to maximize or go full screen (such as F11 or full-screen video) are ignored, because the keyboard decides placement.
- An application whose minimum size is larger than its slot draws past the slot's edge.
- X11-only apps do not run (no XWayland). JetBrains IDEs have a Wayland mode: add `-Dawt.toolkit.name=WLToolkit` via Help → Edit Custom VM Options. They also run as a single instance per user, so close the IDE in any other session first.
- No screen locking, XWayland, workspaces or window decorations (the last three are out of scope by design).

## Code layout

| File | Contents |
| --- | --- |
| `src/layout.rs` | Layout geometry and snap rules (no Smithay, unit tested) |
| `src/focus.rs` | Most-recently-used focus history and Alt+Tab (no Smithay, unit tested) |
| `src/keyboard.rs` | Keybinding state machine, including the Super tap (no Smithay, unit tested) |
| `src/state.rs` | Compositor state and focus/layout plumbing |
| `src/window.rs` | Managed window and its configure logic |
| `src/input.rs` | Routes input events to bindings and clients |
| `src/handlers/` | Smithay protocol handlers (compositor, xdg-shell, layer-shell, seat, clipboard, ...) |
| `src/switcher.rs` | Alt+Tab switcher: thumbnails, icons and text (fontdue, font from fontconfig) |
| `src/apps.rs` | Application names and icons from desktop files and icon themes |
| `src/launcher.rs` | Spawns Fuzzel |
| `src/winit.rs` | Nested backend and rendering |
| `src/udev.rs` | Hardware backend: session, GPU, monitor, libinput, rendering |
| `src/cursor.rs` | Default cursor image for the hardware backend |
