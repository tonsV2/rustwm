#!/usr/bin/env bash
# Builds and starts rustwm. Run it from a text console (Ctrl+Alt+F3) to use
# the hardware; inside a desktop session it opens nested in a window.
#
# The log goes to logs/rustwm.log in the project folder; the previous run's
# log is kept as logs/rustwm.log.old.
set -euo pipefail

cd "$(dirname "$0")"

# Rebuilding first means the binary always matches the checked-out code.
cargo build --release

mkdir -p logs
log="logs/rustwm.log"
if [[ -f "$log" ]]; then
    mv "$log" "$log.old"
fi

# rustwm-session picks the D-Bus bus: a private one while GNOME runs on
# another VT, so single-instance apps (GTK, Ghostty) open their windows here,
# with the keyring relayed from the shared bus.
exec resources/rustwm-session ./target/release/rustwm 2>"$log"
