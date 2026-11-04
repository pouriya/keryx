# keryx server

The `keryx` binary: `keryx serve`, the CLI and TUI, and the local MCP server. Read
the root [AGENTS.md](../AGENTS.md) first, above all its rule to **ask the author
before deciding anything**.

These rules come from borhan (`/p/test/borhan`). Where keryx differs on purpose,
the text says so.

## ⚠️ Read this before writing any code

**DO NOT CREATE NEW FILES UNLESS EXPLICITLY TOLD TO.** The list under Layout is the
whole program and it is closed. Adding `settings.rs`, `utils.rs`, `types.rs` or any
further split is **not** an improvement to make on your own initiative. It changes
the project's shape, and that is the author's decision. Add to the module the code
belongs to and let it grow. If you think a split is needed, say so and wait for an
answer. Do not split and then explain.

**INLINE EVERYTHING UNLESS YOU KNOW IT WILL BE REUSED.** Not "might be reused
later", not "is cleaner as its own function": *known*, concrete, present reuse. One
caller means no function, so paste the body into the call site. This applies to
functions, types, traits, constants and modules alike. Abstraction is earned by a
second caller that exists today, never by anticipation.

Corollary: when a helper's only caller disappears, or two helpers end up in a single
chain used once, collapse them back together.

**Two exceptions, both new in keryx, and they cover nothing else:**

- **Codec and frame-parsing code** may be its own function with one caller when it
  needs fuzzing or benchmarking on its own.
- **The storage trait** exists with one implementation, because Postgres is expected
  later. See Storage.

**USE `make`. DO NOT CALL `cargo` DIRECTLY.** Every check is a make target, and the
bare `cargo` equivalents silently skip some: `cargo build` does not pin `--target`,
and `cargo clippy` without `-D warnings` does not fail.
**Before reporting any task finished, `make all` must pass.** Quote its result. Do
not claim it passed without running it.

## Make targets

| Target | What it does |
|--------|--------------|
| `make all` | **The gate**: `dev` + `clippy` + `coverage` + `check-style` |
| `make dev` | Debug build → `build/keryx-<version>-<target>-dev` |
| `make release` | Release build → `build/keryx-<version>-<target>` |
| `make version` | Prints the `Cargo.toml` version |
| `make start-dev` | `make dev`, then runs `serve` with `--debug` |
| `make clippy` | `cargo clippy --all-targets --no-deps -- -D warnings` |
| `make check-style` | `cargo fmt --check` |
| `make fmt` | Rewrites formatting in place |
| `make lint` | `clippy` + `check-style`, no build |
| `make test` | `cargo test --target …` |
| `make coverage` | Runs the tests under `cargo-llvm-cov` and **rewrites the table** in [tests/AGENTS.md](tests/AGENTS.md) |
| `make clean` / `dist-clean` | Drop `target/` / also `build/` |

**`make start-dev` needs `CONFIG=/absolute/server.toml`,** since the configuration has no default here
either. `CARGO_TARGET_DIR` is honored, for a build directory on another disk.

## Layout

```
src/main.rs       CLI, TUI, clap derive, logging setup, configuration, printers
src/lib.rs        The modules, as a library so that tests/ can reach them
src/api.rs        Every operation, and the axum router: REST and WebSocket
src/captcha.rs    The captcha: drawn, stored with its answer, checked, swept
src/chat.rs       Chat logic: the per-chat task that serializes a chat's events
src/file.rs       Files' bytes on disk under [files] directory: chunk writes and reads, removing unused uploads
src/mcp.rs        Admin MCP on the server, and the local client MCP
src/moq.rs        The embedded relay: QUIC, and qmux over a secure WebSocket
src/storage.rs    The storage trait: accounts, conversations, messages, wrapped keys
src/storage/sqlite.rs  Its SQLite implementation, and the only place SQL lives
src/storage/sqlite.sql The schema, compiled in with include_str!, since SQLite takes text. Every
                  field's comment names the feature that needs it, and a new field must do
                  the same
src/guide.md      The REST guide. Not code: prose, include_str!'d
server.toml       The commented example configuration
tests/            Integration tests. See tests/AGENTS.md
Makefile          Every build and check entry point
```

`guide.md` sits under `src/` because `include_str!` compiles it into the binary, so a binary copied
alone still has its guide. **The web client is the exception:** it is not compiled in. It is served
from `[www] directory` in `server.toml`.

**Every operation is one unit of work, a `run_*` function,** so a command-line adapter over all of them
is simple to add later.

## Hard constraints

- **`keryx serve` never holds a key, a word or the plaintext of an encrypted message.**
  It stores and forwards ciphertext. Messages flagged unencrypted are the only content
  it can read. The binary also contains the CLI, which does use `core/`'s crypto, and
  **no code path under `serve` may derive a key, sign, wrap, seal or open.** It may call only
  `core/`'s `verify_*` functions, which take public keys, so each signed layout stays in one place.
- **The relay forwards encrypted frames and never decrypts them.**
- **Never log or print a word, a key, a password, a token or message content, and never
  store a word or a private key at all.** Passwords and tokens are stored only hashed, except
  `[operator] token` in `server.toml`, by the author's choice. Secrets travel in headers or bodies and never in a URL, because the
  access log records `http_path` and `http_query`.
- **The admin MCP never reaches encrypted content.** The ciphertext is not a tool
  result either. Unencrypted content needs a named permission.

## Storage

The trait in `storage.rs` is what the rest of the server sees, so that a Postgres
implementation can be added later beside `storage/sqlite.rs`.

- **Methods are async and each is one whole operation**, such as `append_message`
  doing the insert and the conversation state update together.
- **No SQL and no transactions cross the trait.**
- **Files stay out of it.** Conversation folders are not part of this trait.

## Surfaces

**REST, WebSocket, MCP and the CLI call the same `run_*` functions in `api.rs`**, so
a tool and its endpoint cannot drift. An operation is never available through only
one of them unless the root AGENTS.md says so.

**Each surface teaches itself and must stand alone.** Whoever arrives has arrived at
exactly one, and has nothing else to read. Guide text lives in `src/guide.md`, in
the tool descriptions in `src/mcp.rs`, and in the clap doc comments in `src/main.rs`.
That duplication is the design, not drift to be factored out.

1. **Add a route, edit `src/guide.md`.** Nothing checks that the two agree, and a
   documented endpoint that does not exist is worse than an undocumented one.
2. **Rename a JSON field, grep all three.**
3. **No surface may need another one to be usable.**

Other rules, all from borhan:

- **A JSON array field's name ends in `_list`.**
- **One way to do one thing.** No two tools or commands that overlap, because a model
  choosing between them picks wrong.
- **A refusal says what is refused and never how to un-refuse it.** The party reading
  the error is the party the refusal is aimed at, and a model reads a named config
  key as a repair instruction and loops on it. A refused operation has no MCP tool,
  not a disabled tool with a note, and the call is checked again when it arrives,
  because a client may cache the tool list.
- **Every HTTP response sets** `Server: keryx/<version> (<repository>)` and
  `X-Keryx-Version` from `Cargo.toml`, and carries `X-Trace-Id`.

## Configuration

**`--config <path>` is required on every command**, with no default and no environment variable, and
there is no home directory. The configuration is read with [`tanzim`](https://docs.rs/tanzim) into a
serde struct, with its validators turning `"5m"` and `"1MiB"` into numbers. Format tanzim's error with `{:#}`: that form carries the source, line, column and caret,
and wrapping it as a `#[source]` throws them away. Unknown keys and relative paths stop startup.

The operator writes the file, from the commented example at `server/server.toml`. Nothing writes it,
and a test parses that example so that it stays true. `serve` creates the database at `path` the first
time it starts, with its tables and built-in roles.

**Only `serve` opens the database. Every other subcommand is a client of the server:** see The command
line in the root AGENTS.md.

## Identifiers

`Ulid` lives in `core/src/ulid.rs`, because clients create them too, and the server
uses it through `keryx-core`.

**Every primary key is a ULID**: 48 bits of milliseconds since the Unix epoch, then
80 random bits, big-endian, stored as `BLOB(16)` and shown as 26 Crockford base32
characters. Byte order is time order and SQLite compares blobs with `memcmp`, so
`ORDER BY id` is chronological and a time range is a contiguous key range.

**A request's `ref` is a ULID the client makes**, held in memory only for that request and sent back
in the answer. It is never stored. What is saved goes by its sequence id, and that is how the server
orders a chat.

Written by hand rather than pulled from the `ulid` crate. **It has no monotonic
factory**, so two ULIDs from the same millisecond sort arbitrarily. Where order
matters inside a millisecond, use an explicit `seq`. `Ulid::parse` is
case-insensitive and does not fold `I`, `L` and `O` onto `1` and `0`.

## Dependencies

- **Every dependency carries its reasoning in `Cargo.toml`, as a comment beside it.**
- **`default-features = false`**, then turn back on what is used, and say why.
- **Set `rust-version`**, so an old toolchain says it is too old rather than printing
  a wall of errors.
- **Adding a dependency is a decision.** Ask first, naming what it adds to the tree.
- **A dependency and its reasoning sit in the `Cargo.toml` of the crate using it**, not in the
  workspace root, so a crate both use is listed twice. The one `Cargo.lock` keeps versions
  identical.

Already agreed: `tokio` (with `signal` and `time`), `axum` (with `ws`), `axum-server` (rustls with the
ring provider), `rusqlite` (`bundled`), `clap` (derive), `tanzim`, `serde`, `serde_json`,
`getrandom`, `thiserror`, `anyhow`, `tracing`, `tracing-subscriber`, `argon2`, `captcha`, `sha2`,
`tower-http` (`fs`), `ureq` (the subcommands are clients), `base64ct` (captcha images and tokens) and
`rustls` (switching on the ring provider), `socket2` (the `[tcp]` settings), all already in the tree, plus `core/`. For tests: `tempfile` and
`tungstenite`. Take the current version of each when adding it.

```toml
[profile.release]
opt-level = 3       # Not borhan's 'z': keryx forwards media and speed beats size.
lto = true
codegen-units = 1
strip = true
# No `panic = 'abort'`, unlike borhan: unwinding keeps one bad frame's panic inside
# that connection's task instead of ending every call on the server.
```

## CLI

- **`--config` and the logging flags are `global = true`**, so they work before or
  after a subcommand.
- **Bare `keryx` prints its long help and exits `0`.** So does any bare command
  group. None has an implicit action, because a verb that quietly does one of nine
  things is a choice the caller did not know they were making.
- **Results go to stdout and nothing else does.** Headers, hints and "no results"
  lines go to stderr, so a pipe reads only data.
- **`--json` prints the same wrapped object the HTTP API returns**, pretty-printed.

## Code style conventions

- **Plain `for` loops over iterator method chains.** Prefer a `for` loop to
  `.map`/`.filter`/`.fold`/`.collect` chains. When you do index a slice, use
  `for x in &xs` or `.iter().enumerate()` to satisfy `needless_range_loop`.
- **`match` over combinators for `Result`.** Use an explicit `match` instead of
  `map_err`, `and_then`, `or_else` and so on.
- **`if let Some(...)` over combinators for `Option`.** Use `if let` or `match`
  instead of `map`, `and_then`, `unwrap_or_else` and so on.
- **Don't extract single-use helpers.** This is the rule at the top of this file. It
  is not negotiable.
- **`anyhow` at the binary boundary, `thiserror` inside modules.**
  `fn main() -> anyhow::Result<()>`, so every fallible call is `?` plus
  `.context(...)` or `.with_context(...)`. Never hand-roll a source-chain walker and
  never convert errors to `String` to propagate them. Module errors are `thiserror`
  enums with `#[source]`.

## Lint and style

- **No `#[allow(...)]` anywhere.** Fix the root cause instead of suppressing a lint.
  (`collapsible_if` wants edition-2024 let-chains: `if let Some(x) = … && cond`.)
- **`make clippy` is the gate.** It runs with `-D warnings`, so warnings in tests fail
  the build too.
  Where it asks for a combinator over a hand-written `match`, such as `manual_map`, it wins over the
  style rule, since `#[allow]` is not an option.
- **`clippy::type_complexity`:** extract a named `pub type` alias rather than spelling
  out nested generics in signatures.
- **`needless_range_loop`:** iterate with `for x in &xs` or `.iter().enumerate()`, not
  `for i in 0..xs.len()`.

## Testing

**No unit tests. Integration tests only, in `tests/`.** See
[tests/AGENTS.md](tests/AGENTS.md), including its rule about the coverage table.

## Logging conventions

`tracing` only: no `log`, no `cfg_if`, no feature gates. The subscriber is installed
in `main` and writes **JSON to stderr**, which leaves stdout free for program output.

| Flag | Level | Extra fields |
|------|-------|--------------|
| `--quiet` | `OFF` | none |
| *(none)* | `WARN`; **`INFO` for `serve`** | none |
| `--info` | `INFO` | none |
| `--debug` | `DEBUG` | target |
| `--trace` | `TRACE` | target, file, line |

`--quiet` wins over `--trace`, which wins over `--debug`, which wins over `--info`.

The `serve` default differs from borhan on purpose: running as a server is keryx's
main mode, and an operator wants the per-operation line there. For a one-shot command
that line is a second copy of what is already on stdout, so it stays off.

### Level guide

| Level | When to use |
|-------|-------------|
| `info` | Important success event |
| `warn` | Intentionally ignored error, or a fallback being taken |
| `error` | Failure the binary recovers from. One it does *not* recover from is returned from `main`, not logged |
| `debug` | Before attempting something important, with the inputs that affect the outcome |
| `trace` | After completing a low-level operation, with detail about what was produced |

A WebSocket client dropping to the REST fallback, or a call dropping to `qmux`, is a
fallback being taken: `warn`.

### Operations and HTTP

Every operation opens a named span (`Send`, `Search`, …) with `op` (lowercase, for
grouping), `trace` (a ULID) and the numeric timings that ran (`total_ms`, …). The same
numbers are on the matching `info` event. Every surface shares this span.

HTTP adds a parent `Http` span (`op = "http"`) and one access-log event
`msg = "HTTP request"` with:

- `http_method`, `http_path` (the raw URI), `http_route` (the matched template, low
  cardinality), `http_query`
- `http_status` (numeric)
- `http_request_bytes`, `http_response_bytes` (`Content-Length`, else 0)
- `total_ms`, `trace`

2xx is `info`, 4xx is `warn`, 5xx is `error`. The CLI talking to `serve` emits the
same fields as `msg = "HTTP client request"` on a `Client` span and records the
server's `trace`, so the two processes join. Span `NEW` and `CLOSE` events are
enabled, so busy and idle time are in the JSON too.

### Format rules

Every log call starts with a `msg` field whose value begins with a capital letter.
Further structured fields follow as `key=value` pairs. Use `?` for `Debug` (paths,
options) and `%` for `Display`.

```rust
tracing::info!(msg = "Created storage directory", directory = ?settings.storage_directory);
tracing::warn!(msg = "Skipped unreadable file", path = ?path, error = %error);
tracing::debug!(msg = "Opened storage", path = ?path);
tracing::trace!(msg = "Committed rows", table = "message", rows = rows.len());
```

Never rely on a log line to tell the user something they must see, because `--quiet`
sets `OFF` and one-shot commands default to `WARN`. Return it from `main` or write it
to stdout.

## Vendored code

**When vendoring, the doc comment must quote the original source, name the crate and
version, and list every deliberate deviation.** Do not vendor silently.
