PREFIX ?= $(HOME)/.local

.PHONY: build check bench install uninstall

build:
	cargo build --locked --release

check:
	sh tools/check.sh

bench:
	cargo run --locked --release -p zterm --example benchmark

install: build
	install -Dm755 target/release/ztty $(DESTDIR)$(PREFIX)/bin/ztty
	install -Dm644 assets/ztty.desktop $(DESTDIR)$(PREFIX)/share/applications/ztty.desktop

uninstall:
	rm -f $(DESTDIR)$(PREFIX)/bin/ztty $(DESTDIR)$(PREFIX)/share/applications/ztty.desktop
