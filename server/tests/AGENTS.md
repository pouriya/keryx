# server/tests

## ⚠️⚠️⚠️ KEEP THE COVERAGE TABLE BELOW UPDATED ⚠️⚠️⚠️

**EVERY CHANGE TO `server/src/` OR `server/tests/` ENDS WITH THE TABLE IN THIS FILE
MATCHING THE CODE. A CHANGE WITH A STALE TABLE IS NOT FINISHED.**

- **Run `make coverage`.** It runs the tests and rewrites the table between the
  markers. `make all` runs it too, so passing the gate leaves the table current.
- **Never edit the numbers by hand.** If the numbers look wrong, fix the measurement.
- **Commit the table with the change that moved it.** A diff in this file is how a
  reviewer sees what the change did to coverage.

## Coverage

Lines measured by `cargo-llvm-cov`, per file and in total.

<!-- coverage:start -->
| File | Lines | Covered | Coverage |
|------|------:|--------:|---------:|
| `src/api.rs` | 1833 | 1717 | 93.67% |
| `src/captcha.rs` | 138 | 131 | 94.93% |
| `src/chat.rs` | 569 | 535 | 94.02% |
| `src/main.rs` | 265 | 258 | 97.36% |
| `src/storage/sqlite.rs` | 1366 | 1280 | 93.70% |
| **Total** | **4171** | **3921** | **94.01%** |
<!-- coverage:end -->

The tool is `cargo install cargo-llvm-cov`, plus
`rustup component add llvm-tools-preview`. The rewrite lives in the Makefile, not in a
script, because the file list is closed.

## Rules

- **No unit tests. No `#[cfg(test)]` in `src/`, ever.** All tests live here.
- **Tests go through the public surface.** A test needing a private item with no public
  path is deleted, not kept inline and not exposed through a new `pub`.
- **Naming:** module `x` → `tests/x.rs`; submodule `a::b` → `tests/a_b.rs`. Helpers
  shared by test files go in `tests/common/mod.rs`. The ones only `api.rs` and `chat.rs` use, a person
  with keys and a device, go in `tests/common/person.rs`, because every other test file would
  otherwise fail clippy on dead code.
- **A test that needs a server starts a real one**, on a free loopback port, with its
  own temporary directory and `--config`. The listener is bound before the helper returns, so a first
  request waits in the backlog instead of being refused.
- **`tests/tls/` holds a self-signed certificate and its private key, for the tests only.** They name
  `localhost` and `127.0.0.1`, and run out in 2036, when the TLS tests start failing. Replace them with:
  `openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -keyout tests/tls/key.pem
  -out tests/tls/cert.pem -days 3650 -subj /CN=localhost
  -addext subjectAltName=DNS:localhost,IP:127.0.0.1 -addext basicConstraints=critical,CA:FALSE
  -addext extendedKeyUsage=serverAuth`
- **`make clippy` covers this directory.** No `#[allow(...)]` here either.
- **Adding a test file is not adding a module.** A `tests/x.rs` for a module on the
  closed list needs no approval. Anything else does.
