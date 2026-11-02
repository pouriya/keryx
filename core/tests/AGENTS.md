# core/tests

## ⚠️⚠️⚠️ KEEP THE COVERAGE TABLE BELOW UPDATED ⚠️⚠️⚠️

**EVERY CHANGE TO `core/src/` OR `core/tests/` ENDS WITH THE TABLE IN THIS FILE
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
| `src/mls.rs` | 1038 | 962 | 92.68% |
| `src/protocol.rs` | 62 | 51 | 82.26% |
| `src/queue.rs` | 107 | 106 | 99.07% |
| `src/seed.rs` | 66 | 60 | 90.91% |
| `src/ulid.rs` | 81 | 77 | 95.06% |
| `src/wrap.rs` | 104 | 99 | 95.19% |
| **Total** | **1458** | **1355** | **92.94%** |
<!-- coverage:end -->

## ⚠️ To do later, marked very important by the author

- **Cover the remaining 7%.** What the tests miss is mostly error paths that are hard to trigger,
  such as a failing random source or an MLS operation failing internally. They are not to be
  written off as untestable: find a way to reach each one. Run `make coverage` for the current
  numbers.
- **Run `core/` in WASM.** `make wasm` only compiles it, so nothing proves that it works there. Find
  a way to run these tests under a WASM runtime. Only an earlier throwaway `openmls` probe has run
  in WASM, under Node and not in a browser, and it did not include any of keryx's own code. **The
  author put it off until after the web client has started.** Decided so far: the tests stay
  exactly as they are and run on `wasm32-wasip1` under wasmtime. WASI is a different target from
  the browser's `wasm32-unknown-unknown`, so this proves keryx's code and its dependencies run in
  WASM, not the browser's own clock and random source. **Still to ask:** whether wasmtime runs them
  as cargo's runner program, which has to be installed, or as a library in dev-dependencies driven
  by a native test that builds the `.wasm` files itself, and whether the run joins `make all`.

## Rules

[server/tests/AGENTS.md](../../server/tests/AGENTS.md) binds this directory too: no unit
tests, public surface only, the naming scheme, and adding a test file for a module on
the closed list needs no approval.
