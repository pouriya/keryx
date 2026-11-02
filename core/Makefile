TARGET ?= $(shell rustc -vV 2>/dev/null | awk '$$1 == "host:"{print $$2}')
WASM_TARGET=wasm32-unknown-unknown
VERSION=$(shell awk 'BEGIN{FS="[ \"]"}$$1 == "version"{print $$4;exit}' Cargo.toml)
COVERAGE_TABLE=tests/AGENTS.md


all: clippy coverage check-style wasm


version:
	@ test -n "${VERSION}" || { echo "cannot read the version out of Cargo.toml" >&2; exit 1; }
	@ echo ${VERSION}


clippy:
	cargo clippy --all-targets --no-deps -- -D warnings


check-style:
	cargo fmt --check


fmt:
	cargo fmt


lint: clippy check-style


test:
	cargo test --target ${TARGET}


# The browser build. Compiling is the check: it is what breaks when a dependency or an API rules
# WASM out, and the hard constraint in AGENTS.md has no other guard until the web client exists.
wasm:
	cargo check --target ${WASM_TARGET}


# Runs the tests, then rewrites the table between the markers from llvm-cov's summary. Its columns
# are: file, regions, missed, cover, functions, missed, executed, lines, missed lines, cover, ...
# A file missing from the summary has no line of code to measure, which is not the same as not
# measured, so the table lists what the tool reports and nothing else.
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
	cargo clean -p keryx-core


.PHONY: all version clippy check-style fmt lint test wasm coverage clean
