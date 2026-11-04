# android

The Android client. The root [AGENTS.md](../AGENTS.md) binds here too, including its loud rule:
**ask the author before deciding anything.** This file holds what the author has decided for the
Android client, and what is still open.

**It follows the web client's design decisions, chosen by the author.** What a screen shows and how
it behaves is decided once, in [web/AGENTS.md](../web/AGENTS.md), and both clients build it. Only
what is Android's own is decided here.

**It is loose for now, like the web client, by the author's choice.** Kotlin in one package, one file
per screen, settled after the first slice works. ktlint, Android lint and clippy are the only fixed
rules. **`ffi/` is not loose:** it follows core's strict Rust rules in [core/AGENTS.md](../core/AGENTS.md).

**Nothing moves in from `/p/test/android`, chosen by the author.** It starts clean here. Its MoQ call
code is looked at again when calls are built.

## What has been decided

| Part | Decision |
|------|----------|
| Stack | **Kotlin and Jetpack Compose**, chosen by the author: state in, screen out, as the web's React, and right-to-left for Persian built in |
| Oldest Android | **Android 8, API 26**, chosen by the author, so older phones still in use run it |
| Package id | **`io.github.pouriya.keryx`**, chosen by the author: tied to the repository, so it cannot clash. It can never change once installed |
| Binding | **Hand-written JNI, chosen by the author**, in `ffi/`, a workspace crate beside `web/wasm/` holding only the glue around `keryx-core`'s `client.rs`. No UniFFI or other generator. It uses the `jni` crate's safe wrappers, not raw `jni-sys` |
| What crosses JNI | **JSON text, chosen by the author.** One `Client` with `step(input, now)`, as on the web: the input goes in as JSON text and the outputs come back as JSON text, in core's own serde shapes, the same the web sees. Kotlin keeps its own data classes for them, read with kotlinx.serialization, and they are kept in step with core by hand |
| Threads | **One background thread holds core's `Client`, the WebSocket and the timer**, like the web's worker. Sockets and timers stay on the platform side, as the root AGENTS.md says |
| Lifetime | **The connection lives while the app is open, for now, chosen by the author.** When the app is closed nothing runs yet. Checking for messages while it is closed is open below |
| Network | **OkHttp** for the WebSocket and REST, chosen by the author |
| Session | **A Bearer token, chosen by the author.** The app logs in with `/api/v1/login/token`, as root AGENTS.md says non-browser clients do. Core hands the token out once with `Token`, and takes it back in `Loaded`, so a closed app comes back logged in. It is kept with the snapshot and never logged |
| Device state | **Two snapshot slots in the app's private files, encrypted with an AES key in the Android Keystore, chosen by the author.** The key never leaves the phone's secure hardware, so a copied backup or a file dump cannot be read. The web cannot do this, which the root AGENTS.md lists as a cost. The token is kept the same way. As on the web, each username has its own, and the one left behind is kept |
| Server | **The first screen asks for the server's address, chosen by the author**, such as `https://chat.example.com`, checks it with `GET /api/v1/public/features`, and remembers it. The web never asks, since the server serves it. While developing, the phone reaches this machine's `keryx serve` through `adb reverse tcp:7777 tcp:7777`, at `http://127.0.0.1:7777` |
| Build | **`android/Makefile`, wrapping a committed Gradle wrapper and `cargo-ndk`, chosen by the author.** `ffi/` is built for `arm64-v8a`, almost every phone in use, and `x86_64`, so it also runs in an emulator. Everything goes through `make`, as elsewhere |
| Toolchain | **What the moq experiment in `/p/test/android` already built with on this machine, as the author pointed to:** Gradle 8.9 from its cache, the Android Gradle plugin 8.7.3, Kotlin 2.0.21 and Android 15 (API 35). The NDK is r30. `dl.google.com` refuses this network, so Google's Maven and the SDK come through `redirector.gvt1.com`, as the experiment found |
| Icon | The approved logo from `web/public/favicon.svg` as an adaptive icon: ink on white, as the web's home-screen icons keep their white background |
| `make check` | **ktlint, Android lint with warnings as errors, and clippy with `-D warnings` and `cargo fmt --check` on `ffi/`**, chosen by the author |
| Testing | **On the author's own phone over adb, chosen by the author.** Taps and typing through adb, the screen read through uiautomator's layout dump, and screenshots. No test framework for now |
| Order | **The web's order, chosen by the author.** First `ffi/` and a bare screen that logs in and sends one encrypted message on the phone, checked against the web client. Then the account screens, then the chat list, each stopping for the author to see it on the phone. Then the chat screen, directory search and the device list, as decided in [web/AGENTS.md](../web/AGENTS.md) |

## Testing

**⚠️ Never load a screenshot into context at full size. Resize it to 25% first, then look at it.**

## Open: do not decide these, ask

- **Checking for messages while the app is closed.** The author wants something like a check every
  5 minutes, or another way that uses less battery than a connection kept open. Android's own
  scheduler, WorkManager, runs periodic work at most every 15 minutes, not 5. The usual low-battery
  way is a push service that wakes the app: Google's FCM, which tells Google when a message arrives,
  or a self-hosted one such as UnifiedPush
- **Plain HTTP and backups, done for now and not decided.** The app allows plain HTTP, so it can
  reach a server without TLS such as the one on this machine, and turns Android's backups off, since
  the locked files could not be opened anywhere else anyway. Whether a release keeps plain HTTP is
  to ask
- The folder layout and code style, once the first slice works
- Release signing, the store, and how the app reaches people
