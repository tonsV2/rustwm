# Configuration

Settings live in `~/.config/rustwm/config.toml`. Copy [resources/config.toml](../resources/config.toml) there as a starting point. Everything is optional. Saving the file applies it right away: pointer and touchpad settings, the keyboard layout and repeat, the output scale, commands and the indicator. A file with a mistake is reported in the log and the current settings stay. The panel's `command`, `position` and `mode`, `notifications.command` and `[autostart]` apply the next time rustwm starts.

| Setting | Default | Meaning |
| --- | --- | --- |
| `output.scale` | guessed from pixel density | Output scale, e.g. `1.25` |
| `input.pointer_speed` | `0.0` | Pointer acceleration, `-1.0` to `1.0` |
| `input.natural_scroll` | `false` | Touchpad content follows the fingers |
| `input.tap_to_click` | `true` | Tap the touchpad to click |
| `input.click_method` | `"fingers"` | `"fingers"`: pressing the touchpad with two fingers right-clicks, three middle-clicks. `"areas"`: the bottom-right corner right-clicks |
| `keyboard.layout`, `variant`, `model`, `options` | the system keyboard (`localectl`) | XKB keymap, e.g. `layout = "us,dk"`, `options = "grp:alt_shift_toggle"` |
| `keyboard.repeat_delay`, `repeat_rate` | `500`, `25` | Milliseconds before a held key repeats, and repeats per second |
| `commands.brightness_up`, `brightness_down` | `brightnessctl -e -n1 set +5%`, `… 5%-` | Brightness keys |
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
| `switcher.wrap` | `true` | Whether the Alt+Tab switcher wraps windows onto more rows, shrinking the thumbnails when even those are not enough, so every window shows at once. `false` keeps one row of full-size thumbnails that scrolls |
| `overview.file_search` | `plocate -i -- "$HOME/" "$1" \| grep -v '/\.' \| head -20` | What the overview's search runs to find files, with `sh -c` and the typed text as `$1`, one path per line. The default searches the plocate database (updated daily by `plocate-updatedb.timer`) under your home directory, leaving out hidden directories. `""` searches applications only |

Commands run with `sh -c`, so quoting, `&&` and `$HOME` work as in a terminal. An empty command (`""`) passes the key on to the focused application.

`resources/rustwm-volume` scales every channel of the default speaker by the same factor, so a left/right balance set in the sound settings survives the volume keys (`wpctl set-volume` would set all channels to one level). It stops one step above silence and mutes there instead, since at 0% the balance is lost. `rustwm-session` puts the directory holding it on `PATH`.

Each keyboard field without a setting comes from its `XKB_DEFAULT_*` variable, or else from the system keyboard (`localectl set-x11-keymap dk`, also chosen during installation). A layout that does not exist is logged and replaced by `us`.

## Default apps

Apps started by rustwm get `XDG_CURRENT_DESKTOP=rustwm`. GNOME's default apps (Loupe for images, Papers for PDFs, Text Editor, ...) live in `gnome-mimeapps.list`, which GIO only reads under GNOME, so without it Files opens images in whichever app claims them first. Point rustwm at GNOME's list:

```sh
mkdir -p ~/.local/share/applications
ln -s /usr/share/applications/gnome-mimeapps.list ~/.local/share/applications/rustwm-mimeapps.list
```

Choices you made yourself in `~/.config/mimeapps.list` (Open With → Set as default) still come first.

GNOME Settings starts from the overview, and its Sound, Bluetooth, Wi-Fi and similar panels work. Its display, mouse and keyboard panels configure GNOME's compositor, not rustwm; use `config.toml` for those. Settings that apps read themselves (dark style, fonts, text size) are stored in dconf and apply from either.

## Dark style

Apps follow the dark style in `org.gnome.desktop.interface color-scheme`, set from GNOME or with:

```sh
gsettings set org.gnome.desktop.interface color-scheme prefer-dark
```

GTK4, libadwaita, Firefox, Chromium and Electron apps and Qt 6 read it through the desktop portal, which only runs in a graphical session (see [D-Bus bus and keyring](development.md#d-bus-bus-and-keyring)). GTK3 apps ignore it, in GNOME too, and only change with their theme: `gsettings set org.gnome.desktop.interface gtk-theme Adwaita-dark`, or `adw-gtk3-dark` from the `adw-gtk3-theme` package for the libadwaita look.

Flatpak apps do not see the themes installed on the host, so a GTK3 or Electron app from Flathub (Signal, for example) falls back to the light Adwaita until the theme is installed as a Flatpak extension too:

```sh
flatpak install flathub org.gtk.Gtk3theme.adw-gtk3 org.gtk.Gtk3theme.adw-gtk3-dark
```

## Environment variables

| Variable | Purpose |
| --- | --- |
| `RUST_LOG` | Log filter, default `info,smithay=warn`. `RUST_LOG=rustwm=debug` shows window placement |
| `XKB_DEFAULT_LAYOUT`, `XKB_DEFAULT_VARIANT`, `XKB_DEFAULT_MODEL`, `XKB_DEFAULT_OPTIONS` | Keyboard layout when `config.toml` sets none, e.g. `XKB_DEFAULT_LAYOUT=dk` |
| `XCURSOR_THEME`, `XCURSOR_SIZE` | Cursor theme and size on the hardware backend |
