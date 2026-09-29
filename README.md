# rustwm

A minimal, keyboard-driven Wayland compositor for Fedora, built on [Smithay](https://github.com/Smithay/smithay).

Every app opens full screen. Snap windows to halves and quarters from the keyboard, switch with Alt+Tab, group snapped windows so they come into view together, and tap Super for an overview with window previews and a search for apps and files. It keeps what I use from GNOME and leaves out the rest.

## Install

```sh
sudo dnf install cargo mesa-libEGL-devel libxkbcommon-devel wayland-devel libseat-devel libinput-devel mesa-libgbm-devel libdrm-devel systemd-devel
sudo dnf install waybar network-manager-applet mako swaylock swayidle wlopm grim slurp xdg-desktop-portal-wlr fuzzel brightnessctl plocate xwayland-satellite
make
sudo make install
```

Log out and pick "rustwm" from the gear menu on the login screen. Run `make && sudo make install` again after pulling; `sudo make uninstall` removes it. To try it first in a window inside your current session, run `scripts/nested.sh`.

## Keys

| Keys | Action |
| --- | --- |
| `Super` (tap) | Overview: window previews and search |
| `Alt+Tab` | Switch window, most recently used first; click a thumbnail to pick it |
| `Super+arrow` | Snap to a half; two arrows with Super held snap to a quarter; the same arrow again goes back to full screen |
| `Super+g` / `Super+Shift+g` | Group the snapped windows on screen / take the focused window out of its group |
| `Super+Tab` / `Super+Shift+Tab` | Next / previous window in the group |
| `Super+1` … `Super+9` | Focus a window by the order it was opened |
| `Super+Return` / `Super+f` / `Super+b` | Terminal / file manager / browser |
| `Super+q` or `Alt+F4` | Close the window |
| `Super+l` | Lock the screen |
| `Print` / `Super+Shift+s` | Screenshot of the screen / of an area, to `~/Pictures/Screenshots` |
| `Ctrl+Alt+Backspace` | Quit rustwm |

The brightness and volume keys work too; the volume keys keep your left/right balance.

## Configuration

Copy [resources/config.toml](resources/config.toml) to `~/.config/rustwm/config.toml`. Saving it applies the change right away. See [docs/configuration.md](docs/configuration.md) for every setting.

## More

- [Using rustwm](docs/usage.md): the overview, locking, notifications, screenshots and screen sharing
- [Configuration](docs/configuration.md): settings, default apps, dark style
- [Development](docs/development.md): building, running nested or from a console, the D-Bus session
- [Code layout](docs/code-layout.md)
- Planned features and bugs are tracked in [GitHub issues](https://github.com/tonsV2/rustwm/issues)

## Known limitations

- One monitor, the first connected one ([#7](https://github.com/tonsV2/rustwm/issues/7)), and only the primary GPU.
- An app whose minimum size is larger than its slot draws past the slot's edge.
- X11 apps need `xwayland-satellite`. Some Java apps show a blank window until `_JAVA_AWT_WM_NONREPARENTING=1` is set; JetBrains IDEs can run on Wayland with `-Dawt.toolkit.name=WLToolkit` (Help → Edit Custom VM Options).
