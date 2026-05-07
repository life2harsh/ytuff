.PHONY: all build release clean install uninstall package-deb package-tar strip-binary

PREFIX ?= /usr/local
BINDIR ?= $(PREFIX)/bin
DESKTOPDIR ?= $(PREFIX)/share/applications

BINARY = ytuff
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
	install -Dm644 ytuff.desktop $(DESTDIR)$(DESKTOPDIR)/ytuff.desktop
	@echo "Installed $(BINARY) to $(DESTDIR)$(BINDIR)"

uninstall:
	rm -f $(DESTDIR)$(BINDIR)/$(BINARY)
	rm -f $(DESTDIR)$(DESKTOPDIR)/ytuff.desktop
	@echo "Uninstalled $(BINARY)"

package-tar: release
	mkdir -p dist
	tar -czf dist/$(BINARY)-$(shell cargo pkgid | cut -d# -f2).tar.gz \
		-C $(BUILD_DIR) $(BINARY) \
		-C .. ytuff.desktop README.md
	@echo "Created dist/$(BINARY)-*.tar.gz"

# Debian/Ubuntu package (requires dpkg-deb)
package-deb: release
	mkdir -p dist/ytuff_deb/DEBIAN
	mkdir -p dist/ytuff_deb/usr/local/bin
	mkdir -p dist/ytuff_deb/usr/local/share/applications
	cp $(BUILD_DIR)/$(BINARY) dist/ytuff_deb/usr/local/bin/
	cp ytuff.desktop dist/ytuff_deb/usr/local/share/applications/
	echo "Package: ytuff" > dist/ytuff_deb/DEBIAN/control
	echo "Version: $(shell cargo pkgid | cut -d# -f2)" >> dist/ytuff_deb/DEBIAN/control
	echo "Section: sound" >> dist/ytuff_deb/DEBIAN/control
	echo "Priority: optional" >> dist/ytuff_deb/DEBIAN/control
	echo "Architecture: amd64" >> dist/ytuff_deb/DEBIAN/control
	echo "Depends: libgtk-3-0,libwebkit2gtk-4.0-37" >> dist/ytuff_deb/DEBIAN/control
	echo "Maintainer: YTuff Team" >> dist/ytuff_deb/DEBIAN/control
	echo "Description: A fast and lightweight music player built with Rust" >> dist/ytuff_deb/DEBIAN/control
	@echo "Run: dpkg-deb --build dist/ytuff_deb dist/ytuff.deb"
