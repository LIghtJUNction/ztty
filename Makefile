PREFIX ?= $(HOME)/.local
BINARY ?=
export PREFIX DESTDIR BINARY

.PHONY: build check bench benchmark-terminals install uninstall

build:
	cargo build --locked --release

check:
	sh tools/check.sh

bench:
	cargo run --locked --release --manifest-path crates/zterm/Cargo.toml --example benchmark

benchmark-terminals: build
	python3 tools/benchmark_terminals.py

install:
	@set -eu; \
	ztty_binary="$$BINARY"; \
	if [ -z "$$ztty_binary" ]; then \
		ztty_build_json=$$(cargo build --locked --release --message-format=json-render-diagnostics); \
		ztty_binary=$$(printf '%s\n' "$$ztty_build_json" | python3 -c 'import json,sys; paths={m["executable"] for m in map(json.loads,sys.stdin) if m.get("reason")=="compiler-artifact" and m.get("executable") and m["target"]["name"]=="ztty" and "bin" in m["target"]["kind"]}; len(paths)==1 or sys.exit("Cargo did not report exactly one ztty executable"); print(paths.pop())'); \
	fi; \
	ZTTY_INSTALL_DIR="$$PREFIX/bin" ZTTY_DESKTOP_DIR="$$PREFIX/share/applications" \
		sh install.sh --from-binary "$$ztty_binary"

uninstall:
	rm -f -- "$$DESTDIR$$PREFIX/bin/ztty" "$$DESTDIR$$PREFIX/bin/ztty.previous" \
		"$$DESTDIR$$PREFIX/bin/ztty.previous.sha256" "$$DESTDIR$$PREFIX/share/applications/ztty.desktop"
