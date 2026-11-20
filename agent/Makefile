TARGET ?= $(shell rustc -vV 2>/dev/null | awk '$$1 == "host:"{print $$2}')
VERSION=$(shell awk 'BEGIN{FS="[ \"]"}$$1 == "version"{print $$4;exit}' Cargo.toml)
BUILD_DIR=build
CARGO_TARGET_DIR ?= ../target
COVERAGE_TABLE=tests/AGENTS.md


# The server's gate, chosen by the author: the tests run the coverage build's binary, and
# `make start-dev` builds the dev one.
all: clippy coverage check-style


version:
	@ test -n "${VERSION}" || { echo "cannot read the version out of Cargo.toml" >&2; exit 1; }
	@ echo ${VERSION}


dev:
	cargo build -p keryx-agent --target ${TARGET}
	@ mkdir -p ${BUILD_DIR}
	@ cp ${CARGO_TARGET_DIR}/${TARGET}/debug/keryx-agent ${BUILD_DIR}/keryx-agent-${VERSION}-${TARGET}-dev


release:
	cargo build -p keryx-agent --release --target ${TARGET}
	@ mkdir -p ${BUILD_DIR}
	@ cp ${CARGO_TARGET_DIR}/${TARGET}/release/keryx-agent ${BUILD_DIR}/keryx-agent-${VERSION}-${TARGET}


# The configuration has no default, here as everywhere: make start-dev CONFIG=/absolute/agent.toml
start-dev: dev
	@ test -n "${CONFIG}" || { echo "give the configuration: make start-dev CONFIG=/absolute/agent.toml" >&2; exit 1; }
	${BUILD_DIR}/keryx-agent-${VERSION}-${TARGET}-dev --config ${CONFIG} --debug run


clippy:
	cargo clippy -p keryx-agent --all-targets --no-deps -- -D warnings


check-style:
	cargo fmt -p keryx-agent --check


fmt:
	cargo fmt -p keryx-agent


lint: clippy check-style


# TEST picks tests by name, as cargo's filter does. Empty runs them all.
TEST ?=

# The loop's tests start the real server and play the owner with drive.rs, so both are built first and
# named to the tests by path.
HELPERS=KERYX_BIN=$(abspath ${CARGO_TARGET_DIR})/${TARGET}/debug/keryx KERYX_DRIVE=$(abspath ${CARGO_TARGET_DIR})/${TARGET}/debug/examples/drive

helpers:
	cargo build -p keryx --bin keryx --example drive --target ${TARGET}

test: helpers
	${HELPERS} cargo test -p keryx-agent --target ${TARGET} ${TEST}


# Runs the tests, then rewrites the table between the markers from llvm-cov's summary, as the
# server's does. It first removes every workspace crate's old coverage build, which all crates share.
coverage: helpers
	@ cargo llvm-cov clean --workspace
	@ ${HELPERS} cargo llvm-cov -p keryx-agent --summary-only --target ${TARGET} > target-coverage.txt || { cat target-coverage.txt; rm -f target-coverage.txt; exit 1; }
	@ awk 'BEGIN { print "<!-- coverage:start -->"; print "| File | Lines | Covered | Coverage |"; print "|------|------:|--------:|---------:|" } \
		/^[a-z_\/]+\.rs / { lines=$$8; covered=$$8-$$9; name=$$1; sub(/^src\//, "", name); percent=(lines ? covered*100/lines : 100); printf "| `src/%s` | %d | %d | %.2f%% |\n", name, lines, covered, percent } \
		/^TOTAL / { lines=$$8; covered=$$8-$$9; percent=(lines ? covered*100/lines : 100); total=sprintf("| **Total** | **%d** | **%d** | **%.2f%%** |", lines, covered, percent) } \
		END { print total; print "<!-- coverage:end -->" }' target-coverage.txt > target-coverage.md
	@ awk 'FNR==NR { table = table $$0 "\n"; next } \
		/<!-- coverage:start -->/ { printf "%s", table; skipping=1; next } \
		/<!-- coverage:end -->/ { skipping=0; next } \
		!skipping { print }' target-coverage.md ${COVERAGE_TABLE} > target-coverage.new
	@ mv target-coverage.new ${COVERAGE_TABLE}
	@ rm -f target-coverage.txt target-coverage.md
	@ sed -n '/coverage:start/,/coverage:end/p' ${COVERAGE_TABLE}


clean:
	cargo clean -p keryx-agent


dist-clean: clean
	rm -rf ${BUILD_DIR}


.PHONY: all version dev release start-dev clippy check-style fmt lint helpers test coverage clean dist-clean
