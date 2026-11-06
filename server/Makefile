TARGET ?= $(shell rustc -vV 2>/dev/null | awk '$$1 == "host:"{print $$2}')
VERSION=$(shell awk 'BEGIN{FS="[ \"]"}$$1 == "version"{print $$4;exit}' Cargo.toml)
BUILD_DIR=build
CARGO_TARGET_DIR ?= ../target
COVERAGE_TABLE=tests/AGENTS.md


all: dev clippy moq coverage check-style


version:
	@ test -n "${VERSION}" || { echo "cannot read the version out of Cargo.toml" >&2; exit 1; }
	@ echo ${VERSION}


dev:
	cargo build --target ${TARGET}
	@ mkdir -p ${BUILD_DIR}
	@ cp ${CARGO_TARGET_DIR}/${TARGET}/debug/keryx ${BUILD_DIR}/keryx-${VERSION}-${TARGET}-dev


release:
	cargo build --release --target ${TARGET}
	@ mkdir -p ${BUILD_DIR}
	@ cp ${CARGO_TARGET_DIR}/${TARGET}/release/keryx ${BUILD_DIR}/keryx-${VERSION}-${TARGET}


# The configuration has no default, here as everywhere: make start-dev CONFIG=/absolute/server.toml
start-dev: dev
	@ test -n "${CONFIG}" || { echo "give the configuration: make start-dev CONFIG=/absolute/server.toml" >&2; exit 1; }
	${BUILD_DIR}/keryx-${VERSION}-${TARGET}-dev --config ${CONFIG} --debug serve


clippy:
	cargo clippy --all-targets --no-deps -- -D warnings


check-style:
	cargo fmt --check


fmt:
	cargo fmt


# keryx-moq's own lint, chosen by the author: the tests use it as their call client.
moq:
	$(MAKE) -C ../moq lint


lint: clippy check-style


test:
	cargo test --target ${TARGET}


# Runs the tests, then rewrites the table between the markers from llvm-cov's summary. Its columns
# are: file, regions, missed, cover, functions, missed, executed, lines, missed lines, cover, ...
coverage:
	@ cargo llvm-cov --summary-only --target ${TARGET} > target-coverage.txt || { cat target-coverage.txt; rm -f target-coverage.txt; exit 1; }
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
	cargo clean -p keryx


dist-clean: clean
	rm -rf ${BUILD_DIR}


.PHONY: all version dev release start-dev clippy moq check-style fmt lint test coverage clean dist-clean
