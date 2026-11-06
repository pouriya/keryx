# keryx moq

The Rust crate `keryx-moq`: the native client for a keryx call's MoQ, used by `android/ffi/` and by the
server's integration tests. Read the root [AGENTS.md](../AGENTS.md) first, above all its rule to **ask
the author before deciding anything**. What a call is and how its media travels is decided there, under
Calls; this file holds only what is keryx-moq's own.

## Same rules as core

**[core/AGENTS.md](../core/AGENTS.md) binds this crate too, chosen by the author**, and through it
[server/AGENTS.md](../server/AGENTS.md): no new files, inlining, `make` rather than `cargo`, the code
style, lint, and the dependency rules. **It never logs**, like core: every ending is an `Event` or a
`Result`, and nothing it holds, a key, a token or a frame, goes into an error.

**It has no tests or coverage table of its own, chosen by the author.** The server's integration tests
drive it as their call client against the real relay, which is what it is for.

## Layout

```
src/lib.rs      The whole client: the call's thread and runtime, the WebSocket adapter, the typed API
Makefile        Every check entry point
```

**One file, chosen by the author**: the adapter that records the close frame is about 60 lines and
used only here.

## Make

| Target | Does |
|--------|------|
| `make all` | **The gate**: `clippy` + `check-style` |
| `make version` | Prints the `Cargo.toml` version |
| `make clippy` | `cargo clippy -p keryx-moq --all-targets --no-deps -- -D warnings` |
| `make check-style` | `cargo fmt -p keryx-moq --check` |
| `make fmt` | Rewrites formatting in place |
| `make lint` | `clippy` + `check-style` |
| `make clean` | Drops this crate's build output |

**The server's `make all` runs `make lint` here too, chosen by the author**, since its tests use this
crate, so a change here cannot pass the server's gate unchecked.

## Dependencies

**Agreed by the author:** `keryx-core`, `moq-net` 0.2.22, `qmux` 0.5.3 with only `ws`, `tokio-tungstenite`
0.29 with only `connect`, `tokio` (`rt`, `net`, `time`, `sync`, `macros`), `futures` for the adapter's
Stream and Sink traits, `serde_json`, `thiserror` and `zeroize`. New to the tree through `moq-net` and
`qmux`: `kio`, `web-async`, `web-transport-trait`, `web-transport-proto`, `num_enum` and the `futures`
umbrella. No rustls, no root store and no `aws-lc-rs`.

## How it works

- **`Call::connect` builds the request, starts the call's thread, and waits for the upgrade**, at most
  10 seconds. The request carries `Sec-WebSocket-Protocol: qmux-02.moq-lite-05` and, with a token,
  `Authorization: Bearer`. A server that answers with another subprotocol is an error.
- **This device is the `index_list` entry with the key's own MLS index.** Its broadcast is named by
  its device id and holds `state`, `audio`, `video` and `text`. The broadcast `server` holds `events`.
  Every other device's broadcast is read for `state`, `audio` and `video`, and this device's own is
  never read back.
- **Sealing happens on the caller's thread**, under a lock, so plaintext never crosses into the call's
  thread. Opening happens on the call's thread, which holds every epoch's key and each device's MLS
  index; a rekey replaces both.
- **Media and state read the newest group and each group to its end**, skipping older groups that
  arrive late. **`events` reads every group in the order it arrived**, since none may be lost.
- **Dropping the `Call` leaves with a clean close**: it closes the MoQ session, which sends qmux's
  close and then the WebSocket's close frame, and waits up to 10 seconds for it to go out.
