# Code layout

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
| `src/capture/` | Screen capture protocols (wlr-screencopy, ext-image-copy-capture) for screenshots and sharing the screen or a window; drawn in `src/handlers/capture.rs`, with the window list in `src/handlers/foreign_toplevel.rs` |
| `src/osd.rs` | On-screen brightness and volume indicator |
| `src/reload.rs` | Applies the config file again when it is saved |
| `src/panel.rs` | Starts the panel, slides it in and out in auto-hide mode and lays windows out beside it while it shows |
| `src/winit.rs` | Nested backend and rendering |
| `src/udev.rs` | Hardware backend: session, GPU, monitor, libinput, rendering |
| `src/cursor.rs` | Cursor images from the theme for the hardware backend |
