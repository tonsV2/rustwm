# Builds rustwm and installs it as a login session.
#
#   make                 build the release binary (as yourself)
#   sudo make install    copy it, the helper scripts and the session file
#   sudo make uninstall  remove them again
#
# install never builds: run as root, cargo would leave root-owned files in
# target/. PREFIX (default /usr/local) holds the binaries, the Waybar and
# mako configs and the private D-Bus bus config. The session and portal files
# go to /usr/share, where GDM and xdg-desktop-portal look for them, and the
# xdg-desktop-portal-wlr settings to /etc/xdg, the only system-wide place it
# reads them from.

PREFIX ?= /usr/local
BINDIR = $(PREFIX)/bin
WAYBARDIR = $(PREFIX)/share/rustwm/waybar
SESSIONDIR ?= /usr/share/wayland-sessions
PORTALDIR ?= /usr/share/xdg-desktop-portal
WLRPORTALDIR ?= /etc/xdg/xdg-desktop-portal-wlr

BINARY = target/release/rustwm
SCRIPTS = rustwm-session rustwm-bus-relay rustwm-volume rustwm-terminal rustwm-panel rustwm-notifications rustwm-share-chooser
WAYBAR = config.jsonc style.css
MAKODIR = $(PREFIX)/share/rustwm/mako
DBUSDIR = $(PREFIX)/share/rustwm/dbus

.PHONY: all build install uninstall

all: build

build:
	cargo build --release

install:
	@test -x $(BINARY) || { echo "$(BINARY) is missing; run make first" >&2; exit 1; }
	install -D -m 755 -t $(DESTDIR)$(BINDIR) $(BINARY) $(addprefix resources/,$(SCRIPTS))
	install -D -m 644 -t $(DESTDIR)$(WAYBARDIR) $(addprefix resources/waybar/,$(WAYBAR))
	install -D -m 644 -t $(DESTDIR)$(MAKODIR) resources/mako/config
	install -D -m 644 -t $(DESTDIR)$(DBUSDIR) resources/dbus/session.conf
	install -D -m 644 -t $(DESTDIR)$(DBUSDIR)/services $(wildcard resources/dbus/services/*.service)
	rm -f $(DESTDIR)$(BINDIR)/rustwm-secrets-relay  # replaced by rustwm-bus-relay
	install -d $(DESTDIR)$(SESSIONDIR)
	sed 's|^Exec=.*|Exec=$(BINDIR)/rustwm-session|' resources/rustwm.desktop >$(DESTDIR)$(SESSIONDIR)/rustwm.desktop
	chmod 644 $(DESTDIR)$(SESSIONDIR)/rustwm.desktop
	install -D -m 644 -t $(DESTDIR)$(PORTALDIR) resources/rustwm-portals.conf
	install -d $(DESTDIR)$(WLRPORTALDIR)
	sed 's|^chooser_cmd=.*|chooser_cmd=$(BINDIR)/rustwm-share-chooser|' resources/xdg-desktop-portal-wlr/rustwm >$(DESTDIR)$(WLRPORTALDIR)/rustwm
	chmod 644 $(DESTDIR)$(WLRPORTALDIR)/rustwm

uninstall:
	rm -f $(DESTDIR)$(BINDIR)/rustwm $(addprefix $(DESTDIR)$(BINDIR)/,$(SCRIPTS)) $(DESTDIR)$(BINDIR)/rustwm-secrets-relay
	rm -rf $(DESTDIR)$(PREFIX)/share/rustwm
	rm -f $(DESTDIR)$(SESSIONDIR)/rustwm.desktop
	rm -f $(DESTDIR)$(PORTALDIR)/rustwm-portals.conf
	rm -f $(DESTDIR)$(WLRPORTALDIR)/rustwm
