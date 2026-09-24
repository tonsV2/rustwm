# Builds rustwm and installs it as a login session.
#
#   make                 build the release binary (as yourself)
#   sudo make install    copy it, the helper scripts and the session file
#   sudo make uninstall  remove them again
#
# install never builds: run as root, cargo would leave root-owned files in
# target/. PREFIX (default /usr/local) holds the binaries and the Waybar
# config. The session and portal files go to /usr/share, where GDM and
# xdg-desktop-portal look for them.

PREFIX ?= /usr/local
BINDIR = $(PREFIX)/bin
WAYBARDIR = $(PREFIX)/share/rustwm/waybar
SESSIONDIR ?= /usr/share/wayland-sessions
PORTALDIR ?= /usr/share/xdg-desktop-portal

BINARY = target/release/rustwm
SCRIPTS = rustwm-session rustwm-secrets-relay rustwm-volume rustwm-terminal rustwm-panel
WAYBAR = config.jsonc style.css

.PHONY: all build install uninstall

all: build

build:
	cargo build --release

install:
	@test -x $(BINARY) || { echo "$(BINARY) is missing; run make first" >&2; exit 1; }
	install -D -m 755 -t $(DESTDIR)$(BINDIR) $(BINARY) $(addprefix resources/,$(SCRIPTS))
	install -D -m 644 -t $(DESTDIR)$(WAYBARDIR) $(addprefix resources/waybar/,$(WAYBAR))
	install -d $(DESTDIR)$(SESSIONDIR)
	sed 's|^Exec=.*|Exec=$(BINDIR)/rustwm-session|' resources/rustwm.desktop >$(DESTDIR)$(SESSIONDIR)/rustwm.desktop
	chmod 644 $(DESTDIR)$(SESSIONDIR)/rustwm.desktop
	install -D -m 644 -t $(DESTDIR)$(PORTALDIR) resources/rustwm-portals.conf

uninstall:
	rm -f $(DESTDIR)$(BINDIR)/rustwm $(addprefix $(DESTDIR)$(BINDIR)/,$(SCRIPTS))
	rm -rf $(DESTDIR)$(PREFIX)/share/rustwm
	rm -f $(DESTDIR)$(SESSIONDIR)/rustwm.desktop
	rm -f $(DESTDIR)$(PORTALDIR)/rustwm-portals.conf
