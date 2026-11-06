TARGET ?= $(shell rustc -vV 2>/dev/null | awk '$$1 == "host:"{print $$2}')
WASM_TARGET=wasm32-unknown-unknown
ROOT=$(abspath ..)
# The wasm-bindgen crate in Cargo.lock, which the CLI must match exactly.
BINDGEN=$(shell awk '$$0 == "name = \"wasm-bindgen\"" { getline; gsub(/[^0-9.]/, ""); print; exit }' ../Cargo.lock)


all: check build


install:
	npm install


# Builds the glue and writes its JavaScript and TypeScript into src/wasm/, which is git-ignored.
wasm:
	@ test "$$(wasm-bindgen --version 2>/dev/null | awk '{print $$2}')" = "${BINDGEN}" || { echo "wasm-bindgen CLI $$(wasm-bindgen --version 2>/dev/null | awk '{print $$2}') does not match the crate's ${BINDGEN}: cargo install wasm-bindgen-cli --version ${BINDGEN} --locked" >&2; exit 1; }
	cargo build -p keryx-wasm --target ${WASM_TARGET} --release
	wasm-bindgen --target web --out-dir src/wasm ../target/${WASM_TARGET}/release/keryx_wasm.wasm


check: wasm
	npx biome check .
	npx tsc --noEmit
	cargo clippy -p keryx-wasm --target ${WASM_TARGET} --no-deps -- -D warnings
	cargo fmt -p keryx-wasm --check


fmt:
	npx biome check --write .
	cargo fmt -p keryx-wasm


build: wasm
	npx vite build


# Vite with hot reload at http://127.0.0.1:5173, and keryx serve behind it on 7778 with dev.toml.
dev: wasm
	$(MAKE) -C ../server dev
	@ mkdir -p ../tmp/dev
	@ sed 's|@ROOT@|${ROOT}|g' dev.toml > ../tmp/dev/server.toml
	@ ../target/${TARGET}/debug/keryx --config ${ROOT}/tmp/dev/server.toml serve & server=$$!; \
		trap "kill $$server" EXIT INT TERM; \
		npx vite


clean:
	rm -rf dist src/wasm


.PHONY: all install wasm check fmt build dev clean
