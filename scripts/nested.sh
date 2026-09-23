#!/usr/bin/env bash
# Runs rustwm nested inside the current desktop session.
#
# GNOME handles Super, Alt+Tab and Super+arrows itself, so the nested window
# would never see them. On GNOME this script clears those shortcuts while
# rustwm runs and restores the previous values on exit, even on Ctrl+C.
#
# Extra arguments are passed to `cargo run`, e.g. `scripts/nested.sh --release`.
set -euo pipefail

cd "$(dirname "$0")/.."

# schema key pairs that clash with rustwm's bindings
clashing=(
    "org.gnome.mutter overlay-key"
    "org.gnome.desktop.wm.keybindings switch-windows"
    "org.gnome.desktop.wm.keybindings switch-applications"
    "org.gnome.desktop.wm.keybindings maximize"
    "org.gnome.desktop.wm.keybindings unmaximize"
    "org.gnome.mutter.keybindings toggle-tiled-left"
    "org.gnome.mutter.keybindings toggle-tiled-right"
)
saved=()

restore() {
    for entry in "${saved[@]}"; do
        read -r schema key value <<<"$entry"
        gsettings set "$schema" "$key" "$value"
    done
}

if [[ "${XDG_CURRENT_DESKTOP:-}" == *GNOME* ]] && command -v gsettings >/dev/null; then
    trap restore EXIT
    for pair in "${clashing[@]}"; do
        read -r schema key <<<"$pair"
        value=$(gsettings get "$schema" "$key")
        saved+=("$schema $key $value")
        if [[ "$key" == overlay-key ]]; then
            gsettings set "$schema" "$key" "''"
        else
            gsettings set "$schema" "$key" "[]"
        fi
    done
    echo "GNOME shortcuts for Super, Alt+Tab and Super+arrows disabled until rustwm exits"
fi

# A private D-Bus session bus keeps single-instance apps (GTK, Ghostty) from
# handing their windows to copies already running in the host session;
# rustwm-session sets it up with the host's keyring relayed onto it.
resources/rustwm-session cargo run "$@"
