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
| Oldest Android | **Android 10, API 29**, chosen by the author for calls: Android's MediaCodec encodes Opus only from there, so no codec library is bundled. It was Android 8, API 26 |
| Package id | **`io.github.pouriya.keryx`**, chosen by the author: tied to the repository, so it cannot clash. It can never change once installed |
| Binding | **Hand-written JNI, chosen by the author**, in `ffi/`, a workspace crate beside `web/wasm/` holding only the glue around `keryx-core`'s `client.rs` and, for calls, `keryx-moq`'s MoQ client. No UniFFI or other generator. It uses the `jni` crate's safe wrappers, not raw `jni-sys` |
| What crosses JNI | **JSON text, chosen by the author.** One `Client` with `step(input, now)`, as on the web: the input goes in as JSON text and the outputs come back as JSON text, in core's own serde shapes, the same the web sees. Kotlin keeps its own data classes for them, read with kotlinx.serialization, and they are kept in step with core by hand |
| Call keys | **`ffi/` catches the call's key in Rust, chosen by the author.** Its `step()` takes core's `JoinCall` and rekey outputs, which hold the SFrame base secret and the Bearer token, and hands them straight to `keryx-moq`, kept beside the `Client`. Kotlin gets only `{chat, call_seq}`, to start capture and show the call. A JVM string cannot be zeroed and the GC may copy it, so keys never cross JNI. Core stays as it is |
| Media across JNI | **Raw bytes for media, JSON for the rest, chosen by the author.** Kotlin calls `audio(ts, ByteArray)` and `video(ts, ByteArray, keyframe)`, and its receive thread gets device, timestamp, kind and bytes from a blocking `next()`. State, text, call events and `Closed` stay JSON in core's shapes. At about 50 audio and 15 video frames a second each way, base64 and a JSON parse per frame would cost a third more bytes and CPU on every phone |
| Live audio | **Opus, mono, 48 kHz, 24 kbps, 20 ms frames, chosen by the author**, the same as recordings, through MediaCodec's encoder from API 29. Each frame is its own MoQ group, so a late one is just dropped. Capture is `AudioRecord` with `VOICE_COMMUNICATION` and the platform's echo canceller and noise suppressor, and playback is `AudioTrack` in communication mode. Fixed in the client for now, like the phone test's video |
| Camera | **Camera2, drawing straight into MediaCodec's `createInputSurface()`, chosen by the author**, with a second Surface for the local preview. No copies or YUV conversion in Kotlin and exact control of size and frame rate, so the lowest CPU on low-end phones, for about 150 lines of setup. CameraX would need a custom SurfaceProvider, close to Camera2 anyway, or frames copied and converted by hand |
| Call in the background | **A foreground service while in a call, chosen by the author.** Joining starts it, of type microphone, plus camera while video is on, with an ongoing "In call" notification and a hang-up action, and it stops when the call ends. Since Android 14 the microphone and camera stop in the background without one. The manifest asks for `FOREGROUND_SERVICE_MICROPHONE` and `FOREGROUND_SERVICE_CAMERA` |
| Ring | **A card on the bare screen with the default ringtone and vibration, chosen by the author:** who calls, video or voice from the call's body, Answer and Decline. It stops on a join from another device, an end or a decline. No notification, since the app does nothing while closed |
| Playback | **A small audio buffer, and video on arrival, chosen by the author.** Audio goes through a jitter buffer that starts at 60 ms and grows to at most 200 ms when gaps appear, then `AudioTrack`. Video is decoded by MediaCodec into a `SurfaceView` as each frame arrives, held back only while it is ahead of the same sender's audio by their sealed timestamps. Lip sync stays within a frame or two at low latency, for about 100 lines |
| Audio route | **Speaker when a call starts with video, the earpiece for voice, chosen by the author**, moving to a wired or Bluetooth headset when one is connected: `AudioManager`'s communication device from API 31, the older calls on 29 and 30. A speaker button on the call screen switches it, as phones' own dialers do |
| Permissions | **Asked when first needed, chosen by the author:** `RECORD_AUDIO` at the first call started or answered, and `CAMERA` when video is on or turned on. A refused one keeps that sense off, and the person still watches, listens and types, which a call allows anyway, with the `state` track saying so. This is Android's own guidance |
| Call test | **Two phones, chosen by the author:** the same APK on a second phone, logged in as a second account, in a direct chat. Two real cameras, microphones and networks |
| Call screen | **A minimal set, chosen by the author:** the other person's video full screen with the local preview in a corner, their name and the time in the call, and buttons for mute, video on or off, switching camera, speaker and hang up. A text field and the call's text below it, since text in a call is part of the design. Material 3 only |
| Threads | **One background thread holds core's `Client`, the WebSocket and the timer**, like the web's worker. Sockets and timers stay on the platform side, as the root AGENTS.md says |
| Lifetime | **The connection lives while the app is open, for now, chosen by the author.** When the app is closed nothing runs yet. Checking for messages while it is closed is open below |
| Network | **OkHttp** for the WebSocket and REST, chosen by the author |
| Session | **A Bearer token, chosen by the author.** The app logs in with `/api/v1/login/token`, as root AGENTS.md says non-browser clients do. Core hands the token out once with `Token`, and takes it back in `Loaded`, so a closed app comes back logged in. It is kept with the snapshot and never logged |
| Device state | **Two snapshot slots in the app's private files, encrypted with an AES key in the Android Keystore, chosen by the author.** The key never leaves the phone's secure hardware, so a copied backup or a file dump cannot be read. The web cannot do this, which the root AGENTS.md lists as a cost. The token is kept the same way. As on the web, each username has its own, and the one left behind is kept |
| Server | **The first screen asks for the server's address, chosen by the author**, such as `https://chat.example.com`, checks it with `GET /api/v1/public/features`, and remembers it. The web never asks, since the server serves it. While developing, the phone reaches this machine's `keryx serve` through `adb reverse tcp:7777 tcp:7777`, at `http://127.0.0.1:7777` |
| Build | **`android/Makefile`, wrapping a committed Gradle wrapper and `cargo-ndk`, chosen by the author.** `ffi/` is built for `arm64-v8a`, almost every phone in use, and `x86_64`, so it also runs in an emulator. Everything goes through `make`, as elsewhere |
| Toolchain | **What the moq experiment in `/p/test/android` already built with on this machine, as the author pointed to:** Gradle 8.9 from its cache, the Android Gradle plugin 8.7.3, Kotlin 2.0.21 and Android 15 (API 35). The NDK is r30. `dl.google.com` refuses this network, so Google's Maven and the SDK come through `redirector.gvt1.com`, as the experiment found |
| Look | **Simple for now, chosen by the author:** Material 3's own components and colours, with nothing of the web's look copied. The Android client first has to be good enough to test text, files and calls, and a full redesign comes after that. What screens show and how they behave still follow [web/AGENTS.md](../web/AGENTS.md) |
| Server screen | **When the screens are built, its own first screen, chosen by the author:** only the address, checked with `GET /api/v1/public/features` and remembered. The log-in screen shows the server with a Change link back to it |
| Setup link | **Pasted, chosen by the author.** The server screen offers "I have a setup link": the app takes the server and the token from `https://server/setup#token`. Nothing has to be hosted for it, and it works over plain HTTP. Opening such a link straight into the app is not built |
| Texts | **Android string resources, chosen by the author:** `values/` for English and `values-fa/` for Persian, with Android's plurals, and lint catches one missing. They are copied from the web's `en.json` and `fa.json` by hand |
| Navigation | **Our own state switch, chosen by the author:** one value says which screen shows, and the system back button pops it. No library, as the web has its own router |
| Icon | The approved logo from `web/public/favicon.svg` as an adaptive icon: ink on white, as the web's home-screen icons keep their white background |
| `make check` | **ktlint, Android lint with warnings as errors, and clippy with `-D warnings` and `cargo fmt --check` on `ffi/` and `../moq/`**, chosen by the author |
| Testing | **On the author's own phone over adb, chosen by the author.** Taps and typing through adb, the screen read through uiautomator's layout dump, and screenshots. No test framework for now |
| Order | **Files and calls first, chosen by the author.** The bare screen that logs in and sends an encrypted message works on the author's phone, checked against the web client. It stays bare and grows only what testing text, files and calls needs: no standard screens until then. A full redesign comes after, and the account screens, the chat list, the chat screen, directory search and the device list follow [web/AGENTS.md](../web/AGENTS.md) then |

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
