.PHONY: all build release clean install uninstall package-deb package-tar strip-binary

PREFIX ?= /usr/local
BINDIR ?= $(PREFIX)/bin
DESKTOPDIR ?= $(PREFIX)/share/applications

BINARY = rustplayer
BUILD_DIR = target/release

all: build

build:
	cargo build

release:
	cargo build --release

strip-binary: release
	strip $(BUILD_DIR)/$(BINARY)
	@ls -lh $(BUILD_DIR)/$(BINARY)

clean:
	cargo clean
	rm -rf dist/

install: release
	install -Dm755 $(BUILD_DIR)/$(BINARY) $(DESTDIR)$(BINDIR)/$(BINARY)
	install -Dm644 rustplayer.desktop $(DESTDIR)$(DESKTOPDIR)/rustplayer.desktop
	@echo "Installed $(BINARY) to $(DESTDIR)$(BINDIR)"

uninstall:
	rm -f $(DESTDIR)$(BINDIR)/$(BINARY)
	rm -f $(DESTDIR)$(DESKTOPDIR)/rustplayer.desktop
	@echo "Uninstalled $(BINARY)"

package-tar: release
	mkdir -p dist
	tar -czf dist/$(BINARY)-$(shell cargo pkgid | cut -d# -f2).tar.gz \
		-C $(BUILD_DIR) $(BINARY) \
		-C .. rustplayer.desktop README.md
	@echo "Created dist/$(BINARY)-*.tar.gz"

# Debian/Ubuntu package (requires dpkg-deb)
package-deb: release
	mkdir -p dist/rustplayer_deb/DEBIAN
	mkdir -p dist/rustplayer_deb/usr/local/bin
	mkdir -p dist/rustplayer_deb/usr/local/share/applications
	cp $(BUILD_DIR)/$(BINARY) dist/rustplayer_deb/usr/local/bin/
	cp rustplayer.desktop dist/rustplayer_deb/usr/local/share/applications/
	echo "Package: rustplayer" > dist/rustplayer_deb/DEBIAN/control
	echo "Version: $(shell cargo pkgid | cut -d# -f2)" >> dist/rustplayer_deb/DEBIAN/control
	echo "Section: sound" >> dist/rustplayer_deb/DEBIAN/control
	echo "Priority: optional" >> dist/rustplayer_deb/DEBIAN/control
	echo "Architecture: amd64" >> dist/rustplayer_deb/DEBIAN/control
	echo "Depends: libgtk-3-0,libwebkit2gtk-4.0-37" >> dist/rustplayer_deb/DEBIAN/control
	echo "Maintainer: RustPlayer Team" >> dist/rustplayer_deb/DEBIAN/control
	echo "Description: A fast and lightweight music player built with Rust" >> dist/rustplayer_deb/DEBIAN/control
	@echo "Run: dpkg-deb --build dist/rustplayer_deb dist/rustplayer.deb"
