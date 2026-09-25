PREFIX ?= /usr
SYSCONFDIR ?= /etc

BIN_DIR := $(PREFIX)/bin
APPS_DIR := $(PREFIX)/share/applications
APPS_DIRECTORY_DIR := $(PREFIX)/share/desktop-directories
APPS_MENU_DIR := $(SYSCONFDIR)/xdg/menus/applications-merged
METAINFO_DIR := $(PREFIX)/share/metainfo
LIBEXEC_DIR := $(PREFIX)/lib/mosaic
ICONS_DIR := $(PREFIX)/share/icons
USER_UNIT_DIR := $(PREFIX)/lib/systemd/user
SYSTEM_UNIT_DIR := $(PREFIX)/lib/systemd/system
TMPFILES_DIR := $(PREFIX)/lib/tmpfiles.d
POLKIT_DIR := $(PREFIX)/share/polkit-1/actions

INSTALL_BIN_DIR := $(DESTDIR)$(BIN_DIR)
INSTALL_APPS_DIR := $(DESTDIR)$(APPS_DIR)
INSTALL_APPS_DIRECTORY_DIR := $(DESTDIR)$(APPS_DIRECTORY_DIR)
INSTALL_APPS_MENU_DIR := $(DESTDIR)$(APPS_MENU_DIR)
INSTALL_METAINFO_DIR := $(DESTDIR)$(METAINFO_DIR)
INSTALL_LIBEXEC_DIR := $(DESTDIR)$(LIBEXEC_DIR)
INSTALL_ICONS_DIR := $(DESTDIR)$(ICONS_DIR)
INSTALL_USER_UNIT_DIR := $(DESTDIR)$(USER_UNIT_DIR)
INSTALL_SYSTEM_UNIT_DIR := $(DESTDIR)$(SYSTEM_UNIT_DIR)
INSTALL_TMPFILES_DIR := $(DESTDIR)$(TMPFILES_DIR)
INSTALL_POLKIT_DIR := $(DESTDIR)$(POLKIT_DIR)

build:
	@echo "Building Rust binary..."
	cargo build --release

check:
	cargo fmt --check
	cargo clippy -- -D warnings
	cargo test

verify-a:
	tools/verify-a.sh "$(BUNDLE)"

# The broker is a systemd user service with socket activation, so there is no
# daemon, no D-Bus service and no kernel module. Two things do land system side,
# and both are limits rather than machinery: the priority limit the framework
# needs, which only the user manager can grant, and the tmpfiles entry for the
# paths Bionic hardcodes.
install:
	@# Never build here. `install` runs under sudo, and a root cargo build uses
	@# root's CARGO_HOME: it re-downloads the registry, recompiles from scratch,
	@# writes root-owned files into the user's target/ -- which stops that user
	@# building afterwards -- and in this project's case made rustc panic outright.
	@# Build as yourself, then install: cargo build --release && sudo make install
	@test -f target/release/mosaic || { \
		echo "target/release/mosaic is missing. Build it first, as yourself:"; \
		echo "    cargo build --release"; \
		echo "then: sudo make install"; \
		exit 1; \
	}
	@# Installing a binary that is older than the tree is silent, and silence here
	@# has cost a whole evening: the fix is committed, the command is run, and the
	@# behaviour does not change because /usr/bin/mosaic is from before the fix.
	@stale=$$(find src -name '*.rs' -newer target/release/mosaic 2>/dev/null | head -1); \
	if [ -n "$$stale" ]; then \
		echo "warning: target/release/mosaic is older than $$stale"; \
		echo "         what you are installing does not have the latest changes."; \
		echo "         As yourself: cargo build --release"; \
	fi
	install -d $(INSTALL_BIN_DIR)
	install -d $(INSTALL_APPS_DIR)
	install -d $(INSTALL_APPS_DIRECTORY_DIR)
	install -d $(INSTALL_APPS_MENU_DIR)
	install -d $(INSTALL_METAINFO_DIR)
	install -d $(INSTALL_ICONS_DIR)/hicolor/512x512/apps
	install -d $(INSTALL_USER_UNIT_DIR)
	install -d $(INSTALL_SYSTEM_UNIT_DIR)/user@.service.d
	install -d $(INSTALL_TMPFILES_DIR)
	install -d $(INSTALL_POLKIT_DIR)
	install -d $(INSTALL_LIBEXEC_DIR)

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
	@# The bundle builder ships with the package so that a user can build a runtime
	@# from the machine's image without a checkout. It needs the build tools, which
	@# is why it is used rather than depended on.
	install -Dm755 tools/bundle/bundle.sh $(INSTALL_LIBEXEC_DIR)/bundle.sh
	install -Dm644 tools/bundle/make-property-area.py $(INSTALL_LIBEXEC_DIR)/make-property-area.py
	install -Dm644 packaging/arch/user@.service.d/mosaic.conf $(INSTALL_SYSTEM_UNIT_DIR)/user@.service.d/mosaic.conf
	install -Dm644 packaging/arch/mosaic.tmpfiles $(INSTALL_TMPFILES_DIR)/mosaic.conf

# Install this checkout on this machine, in the order the two halves require:
# build as the user, install as root, tell the user manager the unit changed. One
# command because doing it by hand has gone wrong every way it can -- a stale
# binary, a build as root, a forgotten daemon-reload.
setup:
	cargo build --release
	sudo $(MAKE) install
	systemctl --user daemon-reload
	systemctl --user restart mosaic-broker.socket
	@echo
	@echo "mosaic is installed. The runtime is next:"
	@echo "    mosaic runtime install <bundle directory | system image>"
	@echo "then:"
	@echo "    mosaic install <app.apk>"

# A6's gate is a system setting, so checking it needs root. This is the one
# command that does it, and it says exactly what is missing when it is not.
verify-priority:
	sudo tools/verify-priority-limit.sh

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
	rm -f $(INSTALL_SYSTEM_UNIT_DIR)/user@.service.d/mosaic.conf
	rm -f $(INSTALL_TMPFILES_DIR)/mosaic.conf
	rm -f $(INSTALL_LIBEXEC_DIR)/bundle.sh $(INSTALL_LIBEXEC_DIR)/make-property-area.py
	rmdir $(INSTALL_LIBEXEC_DIR) 2>/dev/null || true

.PHONY: build check install uninstall setup verify-priority
