# rustwm

A minimal, keyboard-driven Wayland compositor built on [Smithay](https://github.com/Smithay/smithay). See [spec.md](spec.md) for the full design.

## Motivation

GNOME is a good desktop, it just does a bit more than I need. I never click the icons in the dock that appears when pressing Super, and it cannot snap windows to the corners of the screen out of the box.

My day is mostly switching between full-screen applications: several IntelliJ windows, several Ghostty terminals, Chrome and more. Now and then I snap a few to halves or corners to see them side by side. rustwm is built for exactly that and little else. The [vision in the spec](spec.md#vision) has more.

## What it does

Every application opens filling the screen. Snap windows to halves and quarters from the keyboard, switch between them with Alt+Tab, and tap Super for an overview of your windows with a search field for starting applications and opening files.

## Keys

| Keys | Action |
| --- | --- |
| `Super` (tap) | Open the [overview](#overview), or close it if it is open |
| `Alt+Tab` | Switch to the previous window (most recently used first). Keep Alt held to see thumbnails of your windows; Tab/Shift+Tab or the arrow keys move the highlight, releasing Alt switches, Escape cancels |
| `Super+Left` / `Right` / `Up` / `Down` | Snap the focused window to that half |
| `Super+Left` then `Up` (Super held) | Snap to a quarter; any horizontal plus vertical pair works |
| `Super+<direction the window is already in>` | Back to full screen. A grouped window zooms instead: the next `Super+arrow` puts it back in its tile |
| `Super+g` | Group the snapped windows on screen, so they come into view together and share one Alt+Tab entry. Windows joining a group are added to it |
| `Super+Shift+g` | Take the focused window out of its group |
| `Super+Tab` / `Super+Shift+Tab` | Focus the next / previous window of the focused window's group |
| `Super+Return` | Open a terminal: `xdg-terminal-exec`, else the first entry of `~/.config/xdg-terminals.list`, else foot |
| `Super+f` | Open the file manager on your home folder |
| `Super+b` | Open the default web browser |
| `Super+q` or `Alt+F4` | Close the focused window (the app may ask to save first) |
| `Super+1` … `Super+9` | Focus the first … ninth window, in the order they were opened (dialogs not counted) |
| `Super+l` | Lock the screen (`swaylock -f -c 000000 --indicator-idle-visible`) |
| `Super+Shift+s` | Screenshot of an area you drag out with `slurp`, saved to `~/Pictures/Screenshots` |
| `Ctrl+Alt+Backspace` | Quit rustwm |
| `Ctrl+Alt+F1`…`F12` | Switch virtual terminal (hardware backend only) |
| Brightness up / down | `brightnessctl set +5%` / `5%-`; repeats while held |
| Volume up / down | `rustwm-volume up 5` / `down 5`: 5% steps up to 100%, keeping the left/right balance; repeats while held |
| Mute / microphone mute | Toggle mute: `wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle`, or `@DEFAULT_AUDIO_SOURCE@` for the microphone |
| `Print` | Save a screenshot to `~/Pictures/Screenshots` with `grim` |

The brightness, volume and Print keys work with any modifiers held, and their commands can be changed in the [configuration](#configuration). After each brightness or volume press a small panel near the bottom of the screen shows the new level, read back with `wpctl get-volume` or `brightnessctl -m`, and whether it is muted.

Dialogs (windows with a parent, such as file pickers) open centred at their own size. Snapping a dialog treats it as full screen first.

## Overview

A Super tap shows what GNOME's overview shows without the dock and the app grid: the panel stays at the top, under it is a search field, and the open windows are laid out as live previews, most recently used first. The arrow keys, Tab or the pointer move the highlight; Return or a click brings that window to the front. Typing searches the installed applications, and GNOME Settings' panels such as Sound or Displays, by name, generic name and keywords, and files through an outside command (`plocate` over your home directory by default, see `overview.file_search` in the [configuration](#configuration)). Return or a click starts the highlighted application with `gio launch`, or opens the file with `xdg-open`.

The search field edits like any text field, with a blinking caret: Left and Right move it, `Ctrl+Left` and `Ctrl+Right` by a word, Home and End to the ends, Backspace and Delete remove a character, `Ctrl+Backspace`, `Ctrl+w` and `Ctrl+Delete` a word, `Ctrl+u` the whole line. Escape clears the text, or closes the overview when there is none; another Super tap closes it too. Held keys repeat at the keyboard's repeat rate, so holding Down walks the results. While the overview is open, no key reaches a window; the brightness and volume keys, `Ctrl+Alt+Backspace` and the virtual terminal keys still work.

The windows stay where they are underneath; the overview is only a picture of them, drawn by the compositor from the same textures it shows the windows with, so nothing is copied and no other program is involved.

## Status

Done:

- Nested backend (runs as a window inside your current session)
- Hardware backend (DRM/KMS with libseat, libinput and GBM): one GPU and one monitor, the first connected one; VT switching; a drawn cursor (XCURSOR_THEME, or a built-in arrow); tap-to-click enabled on touchpads
- xdg-shell windows and popups, wlr-layer-shell (for the panel and notifications), clipboard and primary selection, linux-dmabuf, viewporter, fractional scale, cursor shape (the compositor draws the cursors apps ask for by name, at one size), server-side decoration negotiation (the compositor draws no decorations)
- Full-screen initial placement, dialogs floating centred, half and quarter layouts
- Most-recently-used Alt+Tab with live window thumbnails, application icons and names
- Window groups: snapped windows grouped with `Super+g` come into view together over the full-screen app underneath, and Alt+Tab shows each group as one screen-shaped thumbnail
- Super tap opens the overview: window previews, the date and a search for applications and files; Super used with another key or a click does not
- X11-only apps through [xwayland-satellite](https://github.com/Supreeeme/xwayland-satellite): rustwm starts it on a free display when it is installed, stops it on exit, and gives apps started from the overview that display as `DISPLAY`
- xdg-activation: an application can bring another forward, for example a terminal opening a link in the running browser, or a clicked notification bringing up the app that sent it. Only the focused application, or the one just clicked, can hand out a request, right after that input event, and it expires after 10 seconds
- A panel (Waybar) with the time, tray, volume, brightness, network and battery, at the top or bottom, auto-hidden until the pointer touches that edge or always shown. Auto-hidden, it slides in and out. Windows are laid out beside it whenever it is on screen, so it pushes them aside rather than covering them
- [Notifications](#notifications) through mako, styled like the switcher and panel
- Screen locking and idle: ext-session-lock (swaylock), ext-idle-notify and idle-inhibit (swayidle, video players keeping the screen on), and wlr-output-power-management (`wlopm` turns the monitor off). The screen fades to black before the idle lock and lights up again on a key or the touchpad. See [Locking the screen](#locking-the-screen)
- [Screenshots and screen sharing](#screenshots-and-screen-sharing): wlr-screencopy and ext-image-copy-capture, into shared memory or dmabuf buffers, for `grim` and `xdg-desktop-portal-wlr`

Not done yet:

- Multiple monitors (optional per the spec). The code keeps each window's output and computes layouts from the output's position, so it can be added later.

## Fedora dependencies

Nested development needs only Rust and Mesa, which Fedora Workstation has already:

```sh
sudo dnf install cargo rust rustfmt clippy mesa-libEGL-devel libxkbcommon-devel wayland-devel
sudo dnf install foot   # a simple terminal to test with
sudo dnf install plocate   # file search in the overview; glib2 (gio) and xdg-utils, for starting what it finds, come with GNOME
sudo dnf install xwayland-satellite   # optional: runs X11-only apps
sudo dnf install brightnessctl # brightness keys; wpctl (wireplumber) and pactl (pulseaudio-utils) for the volume keys are installed with GNOME
sudo dnf install swaylock swayidle wlopm   # screen locking, idle timeout, monitor off
sudo dnf install waybar network-manager-applet   # the panel, and wifi in its tray
sudo dnf install mako   # notifications
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

Start a first application with a Super tap and the search field. If something goes wrong, see `logs/rustwm.log`.

#### D-Bus bus and keyring

`resources/rustwm-session` decides which D-Bus session bus rustwm runs on. `start.sh`, `scripts/nested.sh` and the login session all go through it.

- **Next to another graphical session** (logged in to GNOME on another VT, or nested inside it): rustwm gets a private bus from `dbus-run-session`. Many apps (GTK apps, Ghostty) run as a single instance: on launch they look for a running copy on the D-Bus session bus and hand the request over. On GNOME's bus the window would open in GNOME instead. The keyring GNOME unlocked at login only answers on GNOME's bus, so `rustwm-session` also starts `resources/rustwm-secrets-relay`, which owns `org.freedesktop.secrets` on the private bus and passes every call through to GNOME's keyring daemon. Apps that store passwords (browsers, IDEs, credential helpers) find it unlocked, with no prompt. The relay needs Python with GObject introspection (`python3-gobject-base`, installed with GNOME).
- **As the only graphical session** (logged in from GDM, or from a console while nobody is logged in to GNOME): rustwm uses the shared user bus, where PAM unlocked the keyring at a GDM login, and points the D-Bus and systemd activation environment (`WAYLAND_DISPLAY`, `XDG_CURRENT_DESKTOP`) at itself. It then starts `graphical-session.target` through its own `rustwm-session.target` (written to `$XDG_RUNTIME_DIR/systemd/user/`, nothing to install), which `xdg-desktop-portal` needs before it starts, and stops it on exit. After a console login the keyring is still locked, and the first app that needs it shows one unlock prompt.

Starting a second `gnome-keyring-daemon` on the private bus does not help: `--start` finds the running daemon through its per-user control socket in `$XDG_RUNTIME_DIR/keyring/`, initialises that one and exits, and the running daemon stays on GNOME's bus only. `--replace` would take the keyring away from GNOME.

### As a login session

Once it works from a console, you can add it to the GDM session list:

```sh
make
sudo make install
```

`make` builds the release binary as you; `sudo make install` only copies it, the helper scripts and the Waybar config to `/usr/local` (set `PREFIX` to change that), and the session and portal files to `/usr/share`. Run both again after pulling changes. `sudo make uninstall` removes everything.

Log out and pick "rustwm" from the gear menu on the login screen. Its output goes to the journal (`journalctl -b _UID=$(id -u)`) instead of `logs/rustwm.log`.

## Locking the screen

rustwm draws no lock screen of its own; [swaylock](https://github.com/swaywm/swaylock) does, and [swayidle](https://github.com/swaywm/swayidle) runs it after a while without input. Start swayidle with rustwm through [autostart](#configuration) in `config.toml` (the sample config has this line):

```toml
[autostart]
commands = [
    "swayidle -w timeout 300 'swaylock -f -c 000000 --indicator-idle-visible' timeout 600 'wlopm --off \"*\"' resume 'wlopm --on \"*\"' before-sleep 'swaylock -f -c 000000 --indicator-idle-visible'",
]
```

This locks after 5 minutes, turns the monitor off after 10, and locks before the laptop suspends (`-w` makes the suspend wait until the lock screen is up). To lock right away, run `swaylock -f -c 000000 --indicator-idle-visible`, or press `Super+l`. Any key or mouse movement turns the monitor back on. `wlr-randr --off` does not work, because rustwm has no output-management protocol; use `wlopm`.

As in GNOME, the lock does not come as a surprise: over the last 10 seconds before swayidle's timeout the screen fades to black, and any key or pointer movement during the fade brings it back and restarts the timeout. rustwm times this from the idle timeouts swayidle registers (the fade precedes the first one that is longer than the fade itself), so no second number has to match swayidle's. `lock.fade_ms` in the [configuration](#configuration) sets the length, `0` turns it off. `Super+l` and the lock before suspend fade to black in half a second.

While locked:

- The screen is black until a key is pressed or the pointer moves, which fades the lock screen in; that is why the default swaylock command shows its ring all along (`--indicator-idle-visible`) rather than only while typing. Before the next swayidle timeout (the monitor turning off) the screen fades to black again.
- Only the lock screen is drawn; windows, the overview and the Alt+Tab switcher are hidden, and all keyboard and mouse input goes to the lock screen.
- The window keys are off, and so is `Ctrl+Alt+Backspace`, which would otherwise quit past the lock. `Ctrl+Alt+F1`…`F12` (VT switching) and the brightness and volume keys still work.
- If swaylock crashes, the screen stays black and locked. From another console than rustwm's (a free text console, or GNOME's terminal if it runs alongside), run `WAYLAND_DISPLAY=wayland-1 setsid -f swaylock -f` (the `ready` line in `logs/rustwm.log` names the socket) to get a lock screen back, then switch back to rustwm and unlock.
- Screen captures are refused: a screen share pauses on its last frame from before the lock, and screenshots (including Print) fail.
- Unlocking returns focus to the window that had it.

A video player or browser playing video can keep the screen from locking and turning off (idle-inhibit) while its window is shown.

## Notifications

GNOME Shell shows notifications in GNOME; in rustwm, [mako](https://github.com/emersion/mako) does. rustwm starts `resources/rustwm-notifications` before the panel and the autostart commands, so mako owns `org.freedesktop.Notifications` before apps send anything. Notifications from apps (Chrome, Signal, `notify-send`) and from the desktop portal all go there.

They show in the bottom-right corner above windows, at most three at a time, for 15 seconds unless the app asks otherwise; critical ones stay until dismissed. Click one to open it (the app's default action, which brings the app forward), right-click to dismiss it. `makoctl restore` brings back the last one that went away, `makoctl dismiss --all` clears them all, and `makoctl mode -t do-not-disturb` turns hiding them on and off.

The look is in [resources/mako/config](resources/mako/config). Copy it to `~/.config/rustwm/mako/config` to change it (see `man 5 mako`) and run `makoctl reload`. To use another daemon, set `notifications.command` to it, or to `""` and start it yourself.

## Configuration

Settings live in `~/.config/rustwm/config.toml`. Copy [resources/config.toml](resources/config.toml) there as a starting point. Everything is optional. Saving the file applies it right away: pointer and touchpad settings, the keyboard layout and repeat, the output scale, commands and the indicator. A file with a mistake is reported in the log and the current settings stay. The panel's `command`, `position` and `mode`, `notifications.command` and `[autostart]` apply the next time rustwm starts.

| Setting | Default | Meaning |
| --- | --- | --- |
| `output.scale` | guessed from pixel density | Output scale, e.g. `1.25` |
| `input.pointer_speed` | `0.0` | Pointer acceleration, `-1.0` to `1.0` |
| `input.natural_scroll` | `false` | Touchpad content follows the fingers |
| `input.tap_to_click` | `true` | Tap the touchpad to click |
| `input.click_method` | `"fingers"` | `"fingers"`: pressing the touchpad with two fingers right-clicks, three middle-clicks. `"areas"`: the bottom-right corner right-clicks |
| `keyboard.layout`, `variant`, `model`, `options` | the system keyboard (`localectl`) | XKB keymap, e.g. `layout = "us,dk"`, `options = "grp:alt_shift_toggle"` |
| `keyboard.repeat_delay`, `repeat_rate` | `500`, `25` | Milliseconds before a held key repeats, and repeats per second |
| `commands.brightness_up`, `brightness_down` | `brightnessctl set +5%`, `… 5%-` | Brightness keys |
| `commands.volume_up`, `volume_down`, `volume_mute` | `rustwm-volume up 5`, `rustwm-volume down 5`, `wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle` | Volume keys |
| `commands.mic_mute` | `wpctl set-mute @DEFAULT_AUDIO_SOURCE@ toggle` | Microphone mute key |
| `commands.screenshot` | `mkdir -p ~/Pictures/Screenshots && grim ~/Pictures/Screenshots/Screenshot-$(date +%Y-%m-%d-%H%M%S).png` | Print key |
| `commands.screenshot_area` | `area="$(slurp)" && … grim -g "$area" …` | Area screenshot for `Super+Shift+s` |
| `commands.terminal` | `rustwm-terminal` | Terminal for `Super+Return` |
| `commands.file_manager` | `xdg-open "$HOME"` | File manager for `Super+f` |
| `commands.browser` | `gtk-launch "$(xdg-settings get default-web-browser)"` | Browser for `Super+b` |
| `commands.lock` | `swaylock -f -c 000000 --indicator-idle-visible` | Screen locker for `Super+l` |
| `lock.fade_ms` | `10000` | How long the screen takes to fade to black before an idle timeout longer than that, such as swayidle's lock, fires, in milliseconds. `0` leaves the screen as it is until the lock |
| `osd.enabled` | `true` | Show the brightness and volume level after a key press; turn off if you run your own indicator (swayosd, wob) |
| `autostart.commands` | none | Commands started once rustwm is ready, such as swayidle; they stop when rustwm exits |
| `panel.position` | `"top"` | `"top"` or `"bottom"` |
| `panel.mode` | `"auto-hide"` | `"auto-hide"`: slides in when the pointer touches the panel's screen edge and out again after it leaves, pushing the windows aside while it shows. `"always"`: always shown, windows laid out beside it |
| `panel.command` | `rustwm-panel` | The bar to start; `""` for no panel. `rustwm-panel` runs Waybar with the config in `resources/waybar/`, or `~/.config/rustwm/waybar/` if you copy it there |
| `panel.hide_delay_ms` | `1000` | In auto-hide mode, how long the panel stays after the pointer leaves it, in milliseconds |
| `panel.slide_in_ms`, `slide_out_ms` | `200`, `200` | In auto-hide mode, how long the panel takes to slide in and out, in milliseconds. `0` moves it at once |
| `panel.namespace` | `"waybar"` | The layer-shell namespace of the bar's surface, for auto-hide with another bar |
| `notifications.command` | `rustwm-notifications` | The notification daemon to start; `""` for none. `rustwm-notifications` runs mako with the config in `resources/mako/`, or `~/.config/rustwm/mako/` if you copy it there |
| `overview.file_search` | `plocate -i -- "$HOME/" "$1" \| grep -v '/\.' \| head -20` | What the overview's search runs to find files, with `sh -c` and the typed text as `$1`, one path per line. The default searches the plocate database (updated daily by `plocate-updatedb.timer`) under your home directory, leaving out hidden directories. `""` searches applications only |

Commands run with `sh -c`, so quoting, `&&` and `$HOME` work as in a terminal. An empty command (`""`) passes the key on to the focused application.

`resources/rustwm-volume` scales every channel of the default speaker by the same factor, so a left/right balance set in the sound settings survives the volume keys (`wpctl set-volume` would set all channels to one level). It stops one step above silence and mutes there instead, since at 0% the balance is lost. `rustwm-session` puts the directory holding it on `PATH`.

Each keyboard field without a setting comes from its `XKB_DEFAULT_*` variable, or else from the system keyboard (`localectl set-x11-keymap dk`, also chosen during installation). A layout that does not exist is logged and replaced by `us`.

### Default apps

Apps started by rustwm get `XDG_CURRENT_DESKTOP=rustwm`. GNOME's default apps (Loupe for images, Papers for PDFs, Text Editor, ...) live in `gnome-mimeapps.list`, which GIO only reads under GNOME, so without it Files opens images in whichever app claims them first. Point rustwm at GNOME's list:

```sh
mkdir -p ~/.local/share/applications
ln -s /usr/share/applications/gnome-mimeapps.list ~/.local/share/applications/rustwm-mimeapps.list
```

Choices you made yourself in `~/.config/mimeapps.list` (Open With → Set as default) still come first.

GNOME Settings does not work here: it refuses to start outside GNOME, and its display, mouse and keyboard pages configure GNOME's compositor, not rustwm. Settings that apps read themselves (dark style, fonts, text size) are stored in dconf and still apply when you change them from GNOME.

### Dark style

Apps follow the dark style in `org.gnome.desktop.interface color-scheme`, set from GNOME or with:

```sh
gsettings set org.gnome.desktop.interface color-scheme prefer-dark
```

GTK4, libadwaita, Firefox, Chromium and Electron apps and Qt 6 read it through the desktop portal, which only runs in a graphical session (see [D-Bus bus and keyring](#d-bus-bus-and-keyring)). GTK3 apps ignore it, in GNOME too, and only change with their theme: `gsettings set org.gnome.desktop.interface gtk-theme Adwaita-dark`, or `adw-gtk3-dark` from the `adw-gtk3-theme` package for the libadwaita look.

Flatpak apps do not see the themes installed on the host, so a GTK3 or Electron app from Flathub (Signal, for example) falls back to the light Adwaita until the theme is installed as a Flatpak extension too:

```sh
flatpak install flathub org.gtk.Gtk3theme.adw-gtk3 org.gtk.Gtk3theme.adw-gtk3-dark
```

### Environment variables

| Variable | Purpose |
| --- | --- |
| `RUST_LOG` | Log filter, default `info,smithay=warn`. `RUST_LOG=rustwm=debug` shows window placement |
| `XKB_DEFAULT_LAYOUT`, `XKB_DEFAULT_VARIANT`, `XKB_DEFAULT_MODEL`, `XKB_DEFAULT_OPTIONS` | Keyboard layout when `config.toml` sets none, e.g. `XKB_DEFAULT_LAYOUT=dk` |
| `XCURSOR_THEME`, `XCURSOR_SIZE` | Cursor theme and size on the hardware backend |

## Screenshots and screen sharing

rustwm lets outside tools read the screen through the wlr-screencopy and ext-image-copy-capture protocols. It has no capture interface of its own.

Screenshots use [grim](https://gitlab.freedesktop.org/emersion/grim), and [slurp](https://github.com/emersion/slurp) to select an area:

```sh
grim screen.png               # the whole screen
grim -g "$(slurp)" area.png   # drag to select an area
```

Print saves the whole screen and `Super+Shift+s` a selected area to `~/Pictures/Screenshots`, through the `commands.screenshot` and `commands.screenshot_area` [settings](#configuration).

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
- No built-in XWayland, workspaces or window decorations (out of scope by design).
- Screen sharing shares the whole screen; single windows cannot be chosen. Nested, the pointer is not in captures (the host draws it).

## Code layout

| File | Contents |
| --- | --- |
| `src/layout.rs` | Layout geometry and snap rules (no Smithay, unit tested) |
| `src/focus.rs` | Most-recently-used focus history and Alt+Tab (no Smithay, unit tested) |
| `src/group.rs` | Window groups: which windows belong together (no Smithay, unit tested), grouping, raising and cycling |
| `src/keyboard.rs` | Keybinding state machine, including the Super tap (no Smithay, unit tested) |
| `src/keymap.rs` | Picks the keyboard layout: config, `XKB_DEFAULT_*`, then the system keyboard |
| `src/state.rs` | Compositor state and focus/layout plumbing |
| `src/window.rs` | Managed window and its configure logic |
| `src/input.rs` | Routes input events to bindings and clients |
| `src/handlers/` | Smithay protocol handlers (compositor, xdg-shell, layer-shell, seat, clipboard, session lock, idle, output power, ...) |
| `src/fade.rs` | The screen fading to black before the lock and back on input |
| `src/switcher.rs` | Alt+Tab switcher: thumbnails, icons and text (fontdue, font from fontconfig) |
| `src/overview.rs` | The Super tap overview: window previews, date, and the application and file search |
| `src/apps.rs` | Application names and icons from desktop files and icon themes, and the list of installed applications |
| `src/session.rs` | The environment of started apps, the D-Bus session bus and the systemd graphical session |
| `src/xwayland.rs` | Starts xwayland-satellite for X11 apps |
| `src/commands.rs` | Runs the brightness, volume and Print key commands |
| `src/capture/` | Screen capture protocols (wlr-screencopy, ext-image-copy-capture) for screenshots and screen sharing; drawn in `src/handlers/capture.rs` |
| `src/osd.rs` | On-screen brightness and volume indicator |
| `src/reload.rs` | Applies the config file again when it is saved |
| `src/panel.rs` | Starts the panel, slides it in and out in auto-hide mode and lays windows out beside it while it shows |
| `src/winit.rs` | Nested backend and rendering |
| `src/udev.rs` | Hardware backend: session, GPU, monitor, libinput, rendering |
| `src/cursor.rs` | Cursor images from the theme for the hardware backend |
