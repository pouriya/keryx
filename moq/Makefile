VERSION=$(shell awk 'BEGIN{FS="[ \"]"}$$1 == "version"{print $$4;exit}' Cargo.toml)


# The gate, chosen by the author: keryx-moq has no tests of its own. The server's integration tests
# use it as their call client.
all: clippy check-style


version:
	@ test -n "${VERSION}" || { echo "cannot read the version out of Cargo.toml" >&2; exit 1; }
	@ echo ${VERSION}


clippy:
	cargo clippy -p keryx-moq --all-targets --no-deps -- -D warnings


check-style:
	cargo fmt -p keryx-moq --check


fmt:
	cargo fmt -p keryx-moq


lint: clippy check-style


clean:
	cargo clean -p keryx-moq


.PHONY: all version clippy check-style fmt lint clean
