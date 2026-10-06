# Using rustwm

## Overview

A Super tap shows what GNOME's overview shows without the dock and the app grid: the panel stays at the top, under it is a search field, and the open windows are laid out as live previews, most recently used first. The arrow keys, Tab or the pointer move the highlight; Return or a click brings that window to the front. The highlighted window has a round close button (×) at its preview's top right corner; clicking it, or a middle click on any preview, asks that window to close, as `Super+q` does, and the overview stays open. A window that asks to save first keeps its preview until it is gone, and its dialog shows up in the overview as a window of its own. Typing searches the installed applications, and GNOME Settings' panels such as Sound or Displays, by name, generic name and keywords, and files through an outside command (`plocate` over your home directory by default, see `overview.file_search` in the [configuration](configuration.md)). Return or a click starts the highlighted application with `gio launch`, or opens the file with `xdg-open`.

The search field edits like any text field, with a blinking caret: Left and Right move it, `Ctrl+Left` and `Ctrl+Right` by a word, Home and End to the ends, Backspace and Delete remove a character, `Ctrl+Backspace`, `Ctrl+w` and `Ctrl+Delete` a word, `Ctrl+u` the whole line. Escape clears the text, or closes the overview when there is none; another Super tap closes it too. Held keys repeat at the keyboard's repeat rate, so holding Down walks the results. While the overview is open, no key reaches a window; the brightness and volume keys, `Ctrl+Alt+Backspace` and the virtual terminal keys still work, and so do the Super shortcuts. The app launchers (`Super+Return`, `Super+f`, `Super+b`, `Super+l`, `Super+Shift+s`) and `Super+1`…`9` close the overview first. The window shortcuts (`Super+arrows`, `Super+q`, `Super+g`, `Super+Shift+g`) act on the highlighted window rather than the focused one and leave the overview open; snapping or grouping a window also brings it to the front, as the focused window is on the desktop; with a search result highlighted they do nothing.

The windows stay where they are underneath; the overview is only a picture of them, drawn by the compositor from the same textures it shows the windows with, so nothing is copied and no other program is involved.

## Switching windows

Alt+Tab walks the windows, most recently used first, and releasing Alt brings the highlighted one to the front; Shift+Tab, Left and Right step back and forth, Up and Down move a row, Escape cancels. A quick Alt+Tab toggles between the two most recent windows without the switcher showing. While it shows, the thumbnail under the pointer is highlighted and clicking it brings that window to the front at once; moving the pointer off the thumbnails puts the highlight back on the window Alt+Tab picked, and releasing Alt still picks that one, which keeps a fainter highlight while another thumbnail is hovered. When more windows are open than fit across the screen, they wrap onto more rows, filled as evenly as possible, reading left to right and then top to bottom; when the rows that fit are not enough, the thumbnails shrink. Only past the smallest size does the grid scroll by whole rows to keep the highlight in view, with an arrow at either end and a count such as "+4" for the windows hidden before and after. With `switcher.wrap = false` in the [configuration](configuration.md) the switcher stays one row of full-size thumbnails that scrolls a window at a time. Clicking an arrow selects the nearest hidden window, and the mouse wheel or a touchpad scroll over the switcher moves the selection one window at a time, stopping at the first and last.

## Dialogs

A dialog opens centred over the window it belongs to and moves with it when that window is snapped. A modal dialog, one its window waits on (marked through `xdg-dialog`, as GTK 4 does), also stays above its window and takes the focus whenever the window would get it: clicking the window, picking it in the overview or with Alt+Tab brings the dialog forward with its window behind it.

## Locking the screen

rustwm draws no lock screen of its own; [swaylock](https://github.com/swaywm/swaylock) does, and [swayidle](https://github.com/swaywm/swayidle) runs it after a while without input. Start swayidle with rustwm through [autostart](configuration.md) in `config.toml` (the sample config has this line):

```toml
[autostart]
commands = [
    "swayidle -w timeout 300 'swaylock -f -c 000000 --indicator-idle-visible' timeout 600 'wlopm --off \"*\"' resume 'wlopm --on \"*\"' before-sleep 'swaylock -f -c 000000 --indicator-idle-visible'",
]
```

This locks after 5 minutes, turns the monitor off after 10, and locks before the laptop suspends (`-w` makes the suspend wait until the lock screen is up). To lock right away, run `swaylock -f -c 000000 --indicator-idle-visible`, or press `Super+l`. Any key or mouse movement turns the monitor back on. `wlr-randr --off` does not work, because rustwm has no output-management protocol; use `wlopm`.

As in GNOME, the lock does not come as a surprise: over the last 10 seconds before swayidle's timeout the screen fades to black, and any key or pointer movement during the fade brings it back and restarts the timeout. rustwm times this from the idle timeouts swayidle registers (the fade precedes the first one that is longer than the fade itself), so no second number has to match swayidle's. `lock.fade_ms` in the [configuration](configuration.md) sets the length, `0` turns it off. `Super+l` and the lock before suspend fade to black in half a second.

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

The look is in [resources/mako/config](../resources/mako/config). Copy it to `~/.config/rustwm/mako/config` to change it (see `man 5 mako`) and run `makoctl reload`. To use another daemon, set `notifications.command` to it, or to `""` and start it yourself.

## Bell

Applications that ring the bell through the system bell protocol (`xdg-system-bell`), such as foot with `bell.system = yes`, get a visual bell instead of a sound: the window that rang flashes white for a quarter of a second, only where it can be seen, or the whole screen when no window did. A menu rings for its window. Rings closer together than 0.4 seconds are let go, so a program ringing nonstop cannot make the screen strobe. Nothing flashes while the screen is locked or the overview is open.

## Screenshots and screen sharing

rustwm lets outside tools read the screen through the wlr-screencopy and ext-image-copy-capture protocols, and a single window through ext-image-copy-capture, with the windows listed through ext-foreign-toplevel-list. It has no capture interface of its own. Any app connected to rustwm can use these protocols, as on other wlroots-style desktops: it can read window titles, and capture a window's contents even while other windows cover it. Flatpak apps cannot: Flatpak (1.15.6 and later) connects them through a socket of its own (`security-context`), and rustwm hides screen capture, the window list, the screen lock, monitor power, idle notification and layer-shell from every app that comes in that way. They share the screen through the desktop portal, which asks first. A Flatpak app allowed X11 (`--socket=x11`) runs through xwayland-satellite instead, where, as on any X server, it can watch the other X11 apps, though not the Wayland ones.

Screenshots use [grim](https://gitlab.freedesktop.org/emersion/grim), and [slurp](https://github.com/emersion/slurp) to select an area:

```sh
grim screen.png               # the whole screen
grim -g "$(slurp)" area.png   # drag to select an area
```

Print saves the whole screen and `Super+Shift+s` a selected area to `~/Pictures/Screenshots`, through the `commands.screenshot` and `commands.screenshot_area` [settings](configuration.md).

Screen sharing in browsers, Meet, Slack, Zoom and OBS goes through the desktop portal (`xdg-desktop-portal`), which hands the ScreenCast and Screenshot requests to `xdg-desktop-portal-wlr` and streams the screen through PipeWire:

1. Install `xdg-desktop-portal-wlr` (see [Fedora dependencies](../README.md#install)).
2. Tell the portal to use it when `XDG_CURRENT_DESKTOP=rustwm`, with [resources/rustwm-portals.conf](../resources/rustwm-portals.conf): system-wide in `/usr/share/xdg-desktop-portal/` (see [As a login session](../README.md#install)), or for yourself in `~/.config/xdg-desktop-portal/`. The other portals (file chooser and so on) stay with GTK's.
3. Choose what to share, the screen or one window, from a Fuzzel menu run by `rustwm-share-chooser`. `sudo make install` sets this up with [resources/xdg-desktop-portal-wlr/rustwm](../resources/xdg-desktop-portal-wlr/rustwm) in `/etc/xdg/xdg-desktop-portal-wlr/`. A copy of that installed file in `~/.config/xdg-desktop-portal-wlr/rustwm` replaces it for you (the copy in the repository names the script without its path, which the portal only finds after `make install`); to always share the whole screen without asking, set `chooser_type=none` there. Without either, `xdg-desktop-portal-wlr` asks with slurp when an app wants a monitor, else with the first dmenu-style menu it finds. If a Fuzzel menu is already open, such as a launcher, close it first: a second one cannot open, and the share is declined.

A shared window is captured on its own at its own size, also while other windows cover it. The stream pauses while the window is hidden, and stops when it closes. When the window changes size, such as when it is snapped or the auto-hide panel pushes it aside, the stream switches to the new size, which some apps show as a brief flicker.

rustwm puts `WAYLAND_DISPLAY` and `XDG_CURRENT_DESKTOP=rustwm` in the D-Bus activation environment at startup (see [D-Bus bus and keyring](development.md#d-bus-bus-and-keyring)), so the portals it starts connect to rustwm. On the shared user bus, a portal that was already running for another desktop keeps that desktop's settings until it restarts: `systemctl --user restart xdg-desktop-portal`.

`xdg-desktop-portal-wlr` 0.8.4 (Fedora 44) only streams over ext-image-copy-capture; over wlr-screencopy it stops after the first frame (fixed upstream after 0.8.4). rustwm offers both. Nested, the shared stream pauses while the host hides the rustwm window, and a shared screen arrives upside down in apps that ignore the stream's transform, because the nested output is flipped. A shared window arrives upright.

