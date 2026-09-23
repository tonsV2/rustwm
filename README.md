# rustwm

A minimal, keyboard-driven Wayland compositor built on [Smithay](https://github.com/Smithay/smithay). See [spec.md](spec.md) for the full design.

## Motivation

GNOME is a good desktop, it just does a bit more than I need. I never click the icons in the dock that appears when pressing Super, and it cannot snap windows to the corners of the screen out of the box.

My day is mostly switching between full-screen applications: several IntelliJ windows, several Ghostty terminals, Chrome and more. Now and then I snap a few to halves or corners to see them side by side. rustwm is built for exactly that and little else. The [vision in the spec](spec.md#vision) has more.

## What it does

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
| Brightness up / down | `brightnessctl set +5%` / `5%-`; repeats while held |
| Volume up / down | `wpctl set-volume -l 1.0 @DEFAULT_AUDIO_SINK@ 5%+` / `5%-`; repeats while held |
| Mute / microphone mute | Toggle mute: `wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle`, or `@DEFAULT_AUDIO_SOURCE@` for the microphone |
| `Print` | Save a screenshot to `~/Pictures/Screenshots` with `grim` |

The brightness, volume and Print keys work with any modifiers held, and their commands can be changed in the [configuration](#configuration).

Dialogs (windows with a parent, such as file pickers) open centred at their own size. Snapping a dialog treats it as full screen first.

## Status

Done:

- Nested backend (runs as a window inside your current session)
- Hardware backend (DRM/KMS with libseat, libinput and GBM): one GPU and one monitor, the first connected one; VT switching; a drawn cursor (XCURSOR_THEME, or a built-in arrow); tap-to-click enabled on touchpads
- xdg-shell windows and popups, wlr-layer-shell (needed by Fuzzel), clipboard and primary selection, linux-dmabuf, viewporter, server-side decoration negotiation (the compositor draws no decorations)
- Full-screen initial placement, dialogs floating centred, half and quarter layouts
- Most-recently-used Alt+Tab with live window thumbnails, application icons and names
- Super tap launches Fuzzel; Super used with another key or a click does not
- X11-only apps through [xwayland-satellite](https://github.com/Supreeeme/xwayland-satellite): rustwm starts it on a free display when it is installed, stops it on exit, and gives apps started from Fuzzel that display as `DISPLAY`
- xdg-activation: an application can bring another forward, for example a terminal opening a link in the running browser. Only the focused application can hand out a request, right after an input event, and it expires after 10 seconds
- [Screenshots and screen sharing](#screenshots-and-screen-sharing): wlr-screencopy and ext-image-copy-capture, into shared memory or dmabuf buffers, for `grim` and `xdg-desktop-portal-wlr`

Not done yet:

- Multiple monitors (optional per the spec). The code keeps each window's output and computes layouts from the output's position, so it can be added later.

## Fedora dependencies

Nested development needs only Rust and Mesa, which Fedora Workstation has already:

```sh
sudo dnf install cargo rust rustfmt clippy mesa-libEGL-devel libxkbcommon-devel wayland-devel
sudo dnf install fuzzel foot   # launcher, plus a simple terminal to test with
sudo dnf install xwayland-satellite   # optional: runs X11-only apps
sudo dnf install brightnessctl # brightness keys; wpctl for the volume keys comes with wireplumber
sudo dnf install grim slurp xdg-desktop-portal-wlr   # screenshots and screen sharing
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
2. Run `./start.sh` from the checkout. It builds a release binary and starts it through `resources/rustwm-session` (see below), logging to `logs/rustwm.log` (the previous run's log is kept as `logs/rustwm.log.old`).
3. `Ctrl+Alt+Backspace` quits and returns to the console. `Ctrl+Alt+F2` (or whichever VT GNOME is on) switches back to your desktop while rustwm keeps running.

Start a first application with a Super tap (Fuzzel). If something goes wrong, see `logs/rustwm.log`.

#### D-Bus bus and keyring

`resources/rustwm-session` decides which D-Bus session bus rustwm runs on. `start.sh`, `scripts/nested.sh` and the login session all go through it.

- **Next to another graphical session** (logged in to GNOME on another VT, or nested inside it): rustwm gets a private bus from `dbus-run-session`. Many apps (GTK apps, Ghostty) run as a single instance: on launch they look for a running copy on the D-Bus session bus and hand the request over. On GNOME's bus the window would open in GNOME instead. The keyring GNOME unlocked at login only answers on GNOME's bus, so `rustwm-session` also starts `resources/rustwm-secrets-relay`, which owns `org.freedesktop.secrets` on the private bus and passes every call through to GNOME's keyring daemon. Apps that store passwords (browsers, IDEs, credential helpers) find it unlocked, with no prompt. The relay needs Python with GObject introspection (`python3-gobject-base`, installed with GNOME).
- **As the only graphical session** (logged in from GDM, or from a console while nobody is logged in to GNOME): rustwm uses the shared user bus, where PAM unlocked the keyring at a GDM login, and points the D-Bus and systemd activation environment (`WAYLAND_DISPLAY`, `XDG_CURRENT_DESKTOP`) at itself. After a console login the keyring is still locked, and the first app that needs it shows one unlock prompt.

Starting a second `gnome-keyring-daemon` on the private bus does not help: `--start` finds the running daemon through its per-user control socket in `$XDG_RUNTIME_DIR/keyring/`, initialises that one and exits, and the running daemon stays on GNOME's bus only. `--replace` would take the keyring away from GNOME.

### As a login session

Once it works from a console, you can add it to the GDM session list:

```sh
sudo install -m 755 target/release/rustwm resources/rustwm-session resources/rustwm-secrets-relay /usr/local/bin/
sudo install -m 644 resources/rustwm.desktop /usr/share/wayland-sessions/rustwm.desktop
sudo install -m 644 resources/rustwm-portals.conf /usr/share/xdg-desktop-portal/rustwm-portals.conf
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
| `keyboard.layout`, `variant`, `model`, `options` | the system keyboard (`localectl`) | XKB keymap, e.g. `layout = "us,dk"`, `options = "grp:alt_shift_toggle"` |
| `keyboard.repeat_delay`, `repeat_rate` | `200`, `25` | Milliseconds before a held key repeats, and repeats per second |
| `commands.brightness_up`, `brightness_down` | `brightnessctl set +5%`, `… 5%-` | Brightness keys |
| `commands.volume_up`, `volume_down`, `volume_mute` | `wpctl set-volume -l 1.0 @DEFAULT_AUDIO_SINK@ 5%+`, `… 5%-`, `wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle` | Volume keys |
| `commands.mic_mute` | `wpctl set-mute @DEFAULT_AUDIO_SOURCE@ toggle` | Microphone mute key |
| `commands.screenshot` | `mkdir -p ~/Pictures/Screenshots && grim ~/Pictures/Screenshots/Screenshot-$(date +%Y-%m-%d-%H%M%S).png` | Print key |

Commands run with `sh -c`, so quoting, `&&` and `$HOME` work as in a terminal. An empty command (`""`) passes the key on to the focused application.

Each keyboard field without a setting comes from its `XKB_DEFAULT_*` variable, or else from the system keyboard (`localectl set-x11-keymap dk`, also chosen during installation). A layout that does not exist is logged and replaced by `us`.

GNOME Settings does not work here: it refuses to start outside GNOME, and its display, mouse and keyboard pages configure GNOME's compositor, not rustwm. Settings that apps read themselves (dark style, fonts, text size) are stored in dconf and still apply when you change them from GNOME.

### Environment variables

| Variable | Purpose |
| --- | --- |
| `RUST_LOG` | Log filter, default `info,smithay=warn`. `RUST_LOG=rustwm=debug` shows window placement |
| `RUSTWM_LAUNCHER` | Launcher command instead of `fuzzel` |
| `XKB_DEFAULT_LAYOUT`, `XKB_DEFAULT_VARIANT`, `XKB_DEFAULT_MODEL`, `XKB_DEFAULT_OPTIONS` | Keyboard layout when `config.toml` sets none, e.g. `XKB_DEFAULT_LAYOUT=dk` |
| `XCURSOR_THEME`, `XCURSOR_SIZE` | Cursor theme and size on the hardware backend |

## Screenshots and screen sharing

rustwm lets outside tools read the screen through the wlr-screencopy and ext-image-copy-capture protocols. It has no capture interface of its own.

Screenshots use [grim](https://gitlab.freedesktop.org/emersion/grim), and [slurp](https://github.com/emersion/slurp) to select an area:

```sh
grim screen.png               # the whole screen
grim -g "$(slurp)" area.png   # drag to select an area
```

Print runs the `commands.screenshot` [command](#configuration). To select an area instead, set it to `'grim -g "$(slurp)" ~/Pictures/Screenshots/Screenshot-$(date +%Y-%m-%d-%H%M%S).png'`.

Screen sharing in browsers, Meet, Slack, Zoom and OBS goes through the desktop portal (`xdg-desktop-portal`), which hands the ScreenCast and Screenshot requests to `xdg-desktop-portal-wlr` and streams the screen through PipeWire:

1. Install `xdg-desktop-portal-wlr` (see [Fedora dependencies](#fedora-dependencies)).
2. Tell the portal to use it when `XDG_CURRENT_DESKTOP=rustwm`, with [resources/rustwm-portals.conf](resources/rustwm-portals.conf): system-wide in `/usr/share/xdg-desktop-portal/` (see [As a login session](#as-a-login-session)), or for yourself in `~/.config/xdg-desktop-portal/`. The other portals (file chooser and so on) stay with GTK's.
3. With one monitor there is nothing to choose, so skip the chooser `xdg-desktop-portal-wlr` would otherwise show (slurp, or a Fuzzel menu) with `~/.config/xdg-desktop-portal-wlr/rustwm`:

   ```ini
   [screencast]
   chooser_type=none
   ```

rustwm puts `WAYLAND_DISPLAY` and `XDG_CURRENT_DESKTOP=rustwm` in the D-Bus activation environment at startup (see [D-Bus bus and keyring](#d-bus-bus-and-keyring)), so the portals it starts connect to rustwm. On the shared user bus, a portal that was already running for another desktop keeps that desktop's settings until it restarts: `systemctl --user restart xdg-desktop-portal`.

`xdg-desktop-portal-wlr` 0.8.4 (Fedora 44) only streams over ext-image-copy-capture; over wlr-screencopy it stops after the first frame (fixed upstream after 0.8.4). rustwm offers both. Nested, the shared stream pauses while the host hides the rustwm window, and it arrives upside down in apps that ignore the stream's transform, because the nested output is flipped.

## Known limitations

- The hardware backend uses one monitor at a time, in its preferred mode.
- Only the primary GPU is used.
- Client requests to maximize or go full screen (such as F11 or full-screen video) are ignored, because the keyboard decides placement.
- An application whose minimum size is larger than its slot draws past the slot's edge.
- X11-only apps need `xwayland-satellite` installed; without it they do not run. Some Java apps show a blank window under it until `_JAVA_AWT_WM_NONREPARENTING=1` is set. JetBrains IDEs also have a Wayland mode: add `-Dawt.toolkit.name=WLToolkit` via Help → Edit Custom VM Options. They also run as a single instance per user, so close the IDE in any other session first.
- No screen locking, built-in XWayland, workspaces or window decorations (the last three are out of scope by design).
- Screen sharing shares the whole screen; single windows cannot be chosen. Nested, the pointer is not in captures (the host draws it).

## Code layout

| File | Contents |
| --- | --- |
| `src/layout.rs` | Layout geometry and snap rules (no Smithay, unit tested) |
| `src/focus.rs` | Most-recently-used focus history and Alt+Tab (no Smithay, unit tested) |
| `src/keyboard.rs` | Keybinding state machine, including the Super tap (no Smithay, unit tested) |
| `src/keymap.rs` | Picks the keyboard layout: config, `XKB_DEFAULT_*`, then the system keyboard |
| `src/state.rs` | Compositor state and focus/layout plumbing |
| `src/window.rs` | Managed window and its configure logic |
| `src/input.rs` | Routes input events to bindings and clients |
| `src/handlers/` | Smithay protocol handlers (compositor, xdg-shell, layer-shell, seat, clipboard, ...) |
| `src/switcher.rs` | Alt+Tab switcher: thumbnails, icons and text (fontdue, font from fontconfig) |
| `src/apps.rs` | Application names and icons from desktop files and icon themes |
| `src/launcher.rs` | Spawns Fuzzel |
| `src/xwayland.rs` | Starts xwayland-satellite for X11 apps |
| `src/commands.rs` | Runs the brightness, volume and Print key commands |
| `src/capture/` | Screen capture protocols (wlr-screencopy, ext-image-copy-capture) for screenshots and screen sharing; drawn in `src/handlers/capture.rs` |
| `src/winit.rs` | Nested backend and rendering |
| `src/udev.rs` | Hardware backend: session, GPU, monitor, libinput, rendering |
| `src/cursor.rs` | Default cursor image for the hardware backend |
