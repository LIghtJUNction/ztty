PREFIX ?= $(HOME)/.local
BINARY ?= target/release/ztty
export PREFIX DESTDIR BINARY

.PHONY: build check bench benchmark-terminals install uninstall

build:
	cargo build --locked --release

check:
	sh tools/check.sh

bench:
	cargo run --locked --release -p zterm --example benchmark

benchmark-terminals: build
	python3 tools/benchmark_terminals.py

install: build
	ZTTY_INSTALL_DIR="$$PREFIX/bin" ZTTY_DESKTOP_DIR="$$PREFIX/share/applications" \
		sh install.sh --from-binary "$$BINARY"

uninstall:
	rm -f -- "$$DESTDIR$$PREFIX/bin/ztty" "$$DESTDIR$$PREFIX/bin/ztty.previous" \
		"$$DESTDIR$$PREFIX/bin/ztty.previous.sha256" "$$DESTDIR$$PREFIX/share/applications/ztty.desktop"
