# agent/tests

## ⚠️⚠️⚠️ KEEP THE COVERAGE TABLE BELOW UPDATED ⚠️⚠️⚠️

**EVERY CHANGE TO `agent/src/` OR `agent/tests/` ENDS WITH THE TABLE IN THIS FILE MATCHING THE CODE. A
CHANGE WITH A STALE TABLE IS NOT FINISHED.** The rule is the server's: see
[server/tests/AGENTS.md](../../server/tests/AGENTS.md).

- **Run `make coverage`.** It runs the tests and rewrites the table between the markers. `make all`
  runs it too, so passing the gate leaves the table current.
- **Never edit the numbers by hand.** If the numbers look wrong, fix the measurement.
- **Commit the table with the change that moved it.**

## Coverage

Lines measured by `cargo-llvm-cov`, per file and in total.

<!-- coverage:start -->
| File | Lines | Covered | Coverage |
|------|------:|--------:|---------:|
| `src/connect.rs` | 1601 | 1119 | 69.89% |
| `src/index.rs` | 839 | 663 | 79.02% |
| `src/llm.rs` | 166 | 147 | 88.55% |
| `src/main.rs` | 307 | 295 | 96.09% |
| `src/normalize.rs` | 188 | 156 | 82.98% |
| `src/run.rs` | 1432 | 1239 | 86.52% |
| `src/schedule.rs` | 119 | 119 | 100.00% |
| `src/script.rs` | 3526 | 2494 | 70.73% |
| `src/store.rs` | 1041 | 886 | 85.11% |
| **Total** | **9219** | **7118** | **77.21%** |
<!-- coverage:end -->

## Rules

- **No unit tests. No `#[cfg(test)]` in `src/`, ever.** All tests live here.
- **Tests go through the public surface:** the binary, or a module `lib.rs` exposes. A test needing a
  private item with no public path is deleted, not kept inline and not exposed through a new `pub`.
- **Naming:** module `x` → `tests/x.rs`, and the binary's own tests, its commands and `agent.toml`, in
  `tests/main.rs`. Helpers shared by test files go in `tests/common/mod.rs`.
- **The loop is tested against the real keryx server and a fake LLM**, as Testing in
  [agent/AGENTS.md](../AGENTS.md) says, each test with its own temporary directory and `--config`.
- **`make clippy` covers this directory.** No `#[allow(...)]` here either.
- **Adding a test file is not adding a module.** A `tests/x.rs` for a module on the closed list needs no
  approval. Anything else does.
