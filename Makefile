PREFIX ?= /usr
SYSCONFDIR ?= /etc

BIN_DIR := $(PREFIX)/bin
APPS_DIR := $(PREFIX)/share/applications
APPS_DIRECTORY_DIR := $(PREFIX)/share/desktop-directories
APPS_MENU_DIR := $(SYSCONFDIR)/xdg/menus/applications-merged
METAINFO_DIR := $(PREFIX)/share/metainfo
ICONS_DIR := $(PREFIX)/share/icons
USER_UNIT_DIR := $(PREFIX)/lib/systemd/user
POLKIT_DIR := $(PREFIX)/share/polkit-1/actions

INSTALL_BIN_DIR := $(DESTDIR)$(BIN_DIR)
INSTALL_APPS_DIR := $(DESTDIR)$(APPS_DIR)
INSTALL_APPS_DIRECTORY_DIR := $(DESTDIR)$(APPS_DIRECTORY_DIR)
INSTALL_APPS_MENU_DIR := $(DESTDIR)$(APPS_MENU_DIR)
INSTALL_METAINFO_DIR := $(DESTDIR)$(METAINFO_DIR)
INSTALL_ICONS_DIR := $(DESTDIR)$(ICONS_DIR)
INSTALL_USER_UNIT_DIR := $(DESTDIR)$(USER_UNIT_DIR)
INSTALL_POLKIT_DIR := $(DESTDIR)$(POLKIT_DIR)

build:
	@echo "Building Rust binary..."
	cargo build --release

check:
	cargo fmt --check
	cargo clippy -- -D warnings
	cargo test

# The broker is a systemd user service with socket activation, so nothing here
# installs a system daemon, a D-Bus service, or a kernel module. There is no
# host setup step at all: that is the point of ADR-0004.
install:
	@if [ ! -f target/release/mosaic ]; then \
		echo "Rust binary not found, building..."; \
		cargo build --release; \
	fi
	install -d $(INSTALL_BIN_DIR)
	install -d $(INSTALL_APPS_DIR)
	install -d $(INSTALL_APPS_DIRECTORY_DIR)
	install -d $(INSTALL_APPS_MENU_DIR)
	install -d $(INSTALL_METAINFO_DIR)
	install -d $(INSTALL_ICONS_DIR)/hicolor/512x512/apps
	install -d $(INSTALL_USER_UNIT_DIR)
	install -d $(INSTALL_POLKIT_DIR)

	install -Dm755 target/release/mosaic $(INSTALL_BIN_DIR)/mosaic

	install -Dm644 data/AppIcon.png $(INSTALL_ICONS_DIR)/hicolor/512x512/apps/mosaic.png
	install -Dm644 data/Mosaic.desktop $(INSTALL_APPS_DIR)/Mosaic.desktop
	install -Dm644 data/mosaic.app.install.desktop $(INSTALL_APPS_DIR)/mosaic.app.install.desktop
	install -Dm644 data/mosaic.directory $(INSTALL_APPS_DIRECTORY_DIR)/mosaic.directory
	install -Dm644 data/mosaic.menu $(INSTALL_APPS_MENU_DIR)/mosaic.menu
	install -Dm644 data/id.mosaic.mosaic.metainfo.xml $(INSTALL_METAINFO_DIR)/id.mosaic.mosaic.metainfo.xml

	install -Dm644 systemd/mosaic-broker.socket $(INSTALL_USER_UNIT_DIR)/mosaic-broker.socket
	install -Dm644 systemd/mosaic-broker.service $(INSTALL_USER_UNIT_DIR)/mosaic-broker.service
	install -Dm644 polkit/id.mosaic.allocate-uid.policy $(INSTALL_POLKIT_DIR)/id.mosaic.allocate-uid.policy

uninstall:
	rm -f $(INSTALL_BIN_DIR)/mosaic
	rm -f $(INSTALL_ICONS_DIR)/hicolor/512x512/apps/mosaic.png
	rm -f $(INSTALL_APPS_DIR)/Mosaic.desktop
	rm -f $(INSTALL_APPS_DIR)/mosaic.app.install.desktop
	rm -f $(INSTALL_APPS_DIRECTORY_DIR)/mosaic.directory
	rm -f $(INSTALL_APPS_MENU_DIR)/mosaic.menu
	rm -f $(INSTALL_METAINFO_DIR)/id.mosaic.mosaic.metainfo.xml
	rm -f $(INSTALL_USER_UNIT_DIR)/mosaic-broker.socket
	rm -f $(INSTALL_USER_UNIT_DIR)/mosaic-broker.service
	rm -f $(INSTALL_POLKIT_DIR)/id.mosaic.allocate-uid.policy

.PHONY: build check install uninstall
