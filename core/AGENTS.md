# keryx core

The Rust crate `keryx-core`, shared by every client: the CLI (inside `server/`), the
browser (WASM) and Android (FFI). Read the root [AGENTS.md](../AGENTS.md) first, above
all its rule to **ask the author before deciding anything**.

## Same rules as the server

**[server/AGENTS.md](../server/AGENTS.md) binds this crate too.** That covers no new
files, inlining, `make` rather than `cargo`, `make all` before reporting finished, the
code style, lint, logging format, dependency and vendoring sections, and the codec and
frame-parsing exception.

**Its make targets differ from the server's.** It is a library with no binary, so it has no `dev`,
`release` or `start-dev`, and no `build/` directory. It has `all`, `version`, `clippy`,
`check-style`, `fmt`, `lint`, `test`, `coverage`, `wasm` and `clean`.

**`make all` is its gate: `clippy` + `coverage` + `check-style` + `wasm`.** `make wasm` compiles this
crate for `wasm32-unknown-unknown`, so a change that breaks the browser build fails at once,
before the web client exists.

**Words and key material are wiped from memory when dropped**, with `zeroize`. In WASM this is a
best effort, since copies cannot be ruled out.

## Layout

```
src/lib.rs        The modules
src/protocol.rs   Wire types: what WebSocket and REST carry
src/seed.rs       Words to keys
src/mls.rs        Groups, and the per-epoch keys exported for messages, files and calls
src/wrap.rs       Wrapping epoch keys to accounts and unwrapping them with the words
src/queue.rs      The client's outgoing priority queue, the same for every client
src/ulid.rs       Ulid, the primary key everywhere. The server uses this one too
tests/            Integration tests. See tests/AGENTS.md
Makefile          Every build and check entry point
```

## Hard constraints

- **Same words, same keys, on every platform.** A derivation that differs by one byte
  between the CLI, WASM and Android locks a person out of their own history. Any change
  to it is a break for every existing account, so ask first.
- **It must build for WASM and for Android as well as for the CLI.** A dependency or
  API that rules one out needs the author's approval first.
- **`keryx serve` uses the wire types and `verify_*` functions from this crate, and nothing else of
  its crypto.**
- **Never log, print or persist a word, a key or plaintext.** Handing key material back
  to the caller is the whole job of `seed.rs`, so it does that and nothing else with it.
- **Words are generated from the operating system's random source, never chosen by a
  person and never derived from anything a person typed.**

## Testing

**No unit tests. Integration tests only, in `tests/`.** See
[tests/AGENTS.md](tests/AGENTS.md), including its rule about the coverage table.
