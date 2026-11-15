# keryx

A messenger: **chat, calls, MCP and end-to-end encryption**, on one self-hosted
server. Chat runs over WebSocket with a REST fallback. Calls run over Media over
QUIC (MoQ), relayed inside the server binary.

## ⚠️ ASK THE AUTHOR BEFORE DECIDING ANYTHING

**FOR EVERY PART OF KERYX AND FOR EVERY THING IN IT, QA THE AUTHOR BEFORE YOU
DECIDE. NO DEFAULTS. NO "SENSIBLE CHOICE". NO "I'LL PICK ONE AND MENTION IT".**

- **If this file set does not state a decision, it has not been made.** That
  covers crates, protocols, wire formats, schemas, routes, config keys, limits,
  defaults, files, user-visible behaviour and client design. Unsure whether
  something counts as a decision? It does. Ask.
- **Ask before writing the code, not after.** Do not build one option and offer
  to change it. Bring the options, the cost of each and your recommendation.
- **Starting a part means asking about it first.** Picking up calls, groups,
  attachments or search begins with its QA, even where the Open list below looks
  short. That list is what is known to be unanswered, not everything to ask.
- **Write the answer down in the same change.** Each answer goes into the right
  AGENTS.md with its reason, and comes off the Open list.
- **An answer is about what was asked.** It does not settle neighbouring
  questions, and it does not carry over to a different part.

## Status

The first build is committed. **`core/` and the first server build are written, and both pass `make all`.**
The server has the first slice plus delete and edit in 1:1 chats, `received` and `marks`, driven in its
tests by `core/src/client.rs` as a real client. **The account screens replaced the bare page and run in the author's Chromium**
through `web/wasm/`. On the bare page, two people on two origins signed up, set their words, started a
1:1 chat and exchanged encrypted text both ways, with `received` marks, and a reload reopened it from
IndexedDB.
`core/`'s own tests have not run under WASM. **Running its tests under a WASM runtime, and covering its remaining
error paths, are still to do:** see [core/tests/AGENTS.md](core/tests/AGENTS.md).

- **Proven:** `openmls` 0.9.2 runs in WASM under Node: adding a member, a message, an external
  commit, and all three members exporting the same key. It now runs in Chromium too, through the bare page.
- **Done on the web:** `core/src/client.rs` driven by the server's tests, `web/wasm/` and the bare worker,
  the account screens and the chat list, all checked in the author's Chromium.
- **Done on Android:** `android/ffi/` and a bare screen that signed up, set its words, started a chat
  with a web account and sent it encrypted text, read on the web, on the author's phone.
- **Files are done in core and the server, and on Android's bare screen:** an image, a video, a PDF,
  a text file, a 3-chunk file, an image-and-video bundle and an MP3 as music, with its title, artist,
  duration and cover's BlurHash, went up from the author's phone and came back byte-identical, with
  their BlurHashes, opened in other apps. Another account opening them waits
  for the second phone.
- **Calls are done in core, the server and `keryx-moq`** for direct chats: ringing, decline, joins,
  leaves, the drop grace, media, state and text through the relay, tested end to end in the server's
  tests with `keryx-moq` as the client. Android's call screen is next.
- **Folders are done in core, the server and Android**, each with its own pins, replacing the pinned
  chats. On the author's phone: up to 20 folders and the 21st refused, the sheet's create, rename,
  remove, default and drag, move, copy and remove from a folder, pins per folder, unread counts per tab,
  and an arrangement saved offline reaching the server on reconnect.
- **Presence and the per-account settings (`kv`) are done in core and the server**, tested by the server's
  tests driving core. **Android's dots, Active and Connected lines, `lang` and `theme` and the stickers'
  Recent row passed the author's phone test**, with `server/examples/drive.rs` as the other person and as a
  second device of the phone's own account: away, online and offline, `theme` and `lang` from another device
  live and after the app was stopped, and Recent kept across a restart. The red dot of a call is not tested
  yet, as `drive.rs` cannot join a call.
- **Reactions are done in core, the server and Android, and passed the author's phone test**, with
  `drive.rs` as the other person: the row under a message's options with Recent first, chips added up from
  the archive, tapping a chip to add or remove, twenty different emoji on one message wrapping in its bubble,
  the + picker's strip, search and skin tones, the chat list's "Reacted" preview with no unread count, and
  the other person's reaction and its removal arriving live. See [android/AGENTS.md](android/AGENTS.md).
- **Message search is done in core and Android, and passed the author's phone test**, with `drive.rs` as
  the other person: hits newest first with the match tinted, "10" finding "۱۰", a hit opening its chat at
  the message, the tabs and their counts, See all and Show older, and a message deleted on another device
  gone from the archive at once.
- **Groups and channels are built in core, the server and Android, and their phone test is under way**,
  with `drive.rs` as the other people. What it has not covered yet is under To fix, Not yet tested on the
  phone.
- **Next, chosen by the author: calls, then Android for files and calls.** Both clients stay as they
  are and grow only what testing needs; standard screens and a full redesign come after.

Delete this section when it stops being true.

## Layout

```
AGENTS.md       This file: what applies to the whole repository
Cargo.toml      The workspace: server and core, with one Cargo.lock and one target/
Dockerfile      The server's image, built by `make docker` in server/
.dockerignore   Keeps everything the Dockerfile does not copy out of its context
server/         The `keryx` binary: `keryx serve`, the CLI and TUI, local MCP
  AGENTS.md     Binding rules for server/: layout, style, logging, make
  tests/        Integration tests
    AGENTS.md   The rule about the coverage table, and the table
core/           Rust shared by every client: wire types, words, MLS, key wrapping
  AGENTS.md     Binding rules for core/
  tests/        Integration tests
    AGENTS.md   The rule about the coverage table, and the table
web/            The browser client: the account screens and the chat list, on hold
  AGENTS.md     The web client's decisions and rules
  wasm/         A workspace crate holding only the wasm-bindgen glue around keryx-core
android/        The Android client, in Kotlin and Jetpack Compose
  AGENTS.md     The Android client's decisions and rules
  ffi/          A workspace crate holding only the hand-written JNI glue around keryx-core
moq/            A workspace crate, keryx-moq: the native MoQ client for calls, over WebSocket, sealing its frames
  AGENTS.md     keryx-moq's layout, dependencies and checks
agent/          A workspace crate, keryx-agent: an LLM agent living in keryx as its own account. Planned
  AGENTS.md     keryx-agent's decisions, what is drafted for the author's approval, and its open questions
```

**`server/` and `core/` follow the strict rules.** The file lists are closed, code
style and logging are fixed, everything goes through `make`, and all tests are
integration tests. **`web/` is loose for now, by the author's choice:** see
[web/AGENTS.md](web/AGENTS.md). **`android/` is loose the same way, by the author's choice**, except
`android/ffi/`, which follows core's strict Rust rules: see [android/AGENTS.md](android/AGENTS.md).
**`moq/` follows core's strict Rust rules the same way, chosen by the author**, and is checked by clippy
and fmt. The server's integration tests use it as their call client, so it has no tests or coverage
table of its own. **It is one file with its own Makefile and AGENTS.md, chosen by the author**: see
[moq/AGENTS.md](moq/AGENTS.md). The server's `make all` runs its `make lint` too, since its tests use it.
**`agent/` follows core's strict Rust rules too, chosen by the author:** see [agent/AGENTS.md](agent/AGENTS.md).

**`web/wasm/` is its own crate, not a module in `core/`**, so `core/` holds no JavaScript glue and
its native coverage is not pulled down by code its tests cannot run. **`android/ffi/` is its sibling
for Android, the same way.** It shares the workspace's one `Cargo.lock`.

**`core/` exists because keys must come out byte-identical everywhere.** The CLI,
the browser (WASM) and Android (FFI) all turn the same words into the same keys
and read the same MLS state. One implementation means one thing to audit.

**The workspace is what keeps crypto versions identical** between the server and
the WASM build, because there is one `Cargo.lock`. Make targets stay inside
`server/`, `core/`, `web/`, `android/`, `moq/` and `agent/`.

## What has been decided

| Part | Decision |
|------|----------|
| Topology | One self-hosted server. One instance is one community. No federation |
| Encryption | End-to-end, unless a message is explicitly unencrypted. Unencrypted is governed by roles and permissions and flagged in the `message` table, so the client knows. For encrypted content the server stores ciphertext only |
| E2EE protocol | MLS (RFC 9420) via `openmls`, suite `MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519`, the RFC's mandatory one. Each device is a group member. MLS manages membership and keys; see Restoring history. **The backend is `openmls_rust_crypto`:** in WASM under Node it took 5.3 ms against libcrux's 12.8 ms for the same flow, at 645 KB against 727 KB gzipped, and its crates can serve keryx's own formats too. It is not formally verified, which was libcrux's advantage |
| Ciphers | **AES everywhere, so there is one cipher to audit.** Calls use SFrame (RFC 9605) with AES-GCM and its MLS exporter key scheme. Messages, voice notes and files use AES-GCM with MLS-derived keys. Browsers use their hardware AES for large data. Low-end phones without AES hardware will be slower on video; revisit that with Android |
| Chat transport | WebSocket, with a REST fallback |
| Wire format | JSON envelopes with ciphertext as base64, the same on WebSocket, REST and MCP. Readable in logs and devtools, at about 33% overhead on ciphertext. Large attachments go separately as binary |
| Versioning | REST under `/api/v1`, and WebSocket negotiates a `keryx.v1` subprotocol. New optional fields do not bump the version, breaking changes do. A mismatch fails at connect time, with a clear message |
| Calls | MoQ, embedded in the server binary. Not a separate server. See Calls |
| Call transport | **MoQ over `qmux` on a WebSocket only for now, with no QUIC, chosen by the author.** REST carries no media |
| Clients | CLI and TUI in the server binary, web, Android |
| TLS | Optional `[tls]` certificate and key paths in `server.toml`, handled by rustls in the binary through `axum-server`, with the ring provider, which is what borhan's rustls uses. Without them it serves plain HTTP, for local development or behind a reverse proxy. Calls run over the same WebSocket port, so the same certificate covers them |
| Shutdown | On SIGINT or SIGTERM the server stops accepting connections, closes each WebSocket with code 1001, lets each chat's task finish what it already received, closes open device log rows, then exits. **The wait is at most 8 seconds, fixed in code**, under Docker's default 10 before SIGKILL. A client only counts a message as sent once it has its sequence id, so one cut off here is resent |
| Passwords | Argon2id through `argon2` 0.6.0, at 19 MiB, 2 passes and 1 lane: the crate's defaults and OWASP's minimum. The settings are stored with each hash, so they can be raised later |
| Retention | How long the server keeps ciphertext is configured in `server.toml`. **Unset means unlimited.** It can also be set per chat, where the client offers that. **The shortest limit wins:** a chat can shorten the server's limit but never exceed it, and setting it is a permission. Deleting removes the server's copy only |
| Authorization | **A complete RBAC, with many features built on it.** See below |
| Docker | **One root `Dockerfile`, built by `make docker` in `server/`, chosen by the author after activeplano's.** A static musl binary from `rust:alpine` on `alpine` 3.23, the dependencies built first against empty sources so a source change reuses their layer, with a `DOCKER_REGISTRY` prefix for the base images, empty for Docker Hub. **The server only for now:** the web client is on hold. **Everything lives in one volume, `/var/lib/keryx`:** `server.toml`, which the image does not carry, the database, `files/` and an empty `www/`. `ENTRYPOINT` is `keryx --config /var/lib/keryx/server.toml` and `CMD` is `serve`. Tagged `keryx:<version>` and `keryx:latest`, with no `HEALTHCHECK` and no compose file yet |
| Web delivery | Built on its own, copied into the server's Docker image once the web client is off hold, and served from `[www] directory = "..."` in `server.toml` through `tower-http`'s ServeDir, so keryx does not own a path check. A path with no file gets `index.html`, so `/setup#token` works. Not embedded in the binary |
| No production yet | **No migrations, no backward compatibility and no `ALTER`s until there is a production server, chosen by the author.** The schema, the wire and stored formats change in place, and a change means deleting the database and the clients' stored state |
| Database | SQLite in **one `.db` file, with no `.wal`**, using its classic `DELETE` journal: a `-journal` file exists only during a write, so a crash mid-write rolls back. One writer and one reader connection, each behind a lock, which take turns. If that becomes too slow, the answer is Postgres |
| v1 scope | Direct and group chats, channels, calls in all three, encrypted attachments, local message search, all four MCP roles |
| Web stack | React, Vite and TypeScript, with all crypto done by `keryx-core` compiled to WASM. The rest is in [web/AGENTS.md](web/AGENTS.md) |

### RBAC

- **Custom roles at two scopes, server and per chat.** Named permissions are grouped
  into roles. Admin, moderator and member are the server's built-in defaults, owner, moderator and
  member a chat's, as Groups and channels says, and operators and chat owners can define more. A new feature adds permissions, not a new mechanism.
- **The server enforces, and MLS guards.** The server knows who holds which role and
  rejects forbidden actions, so it sees roles but never content. Membership is guarded
  by the clients: they reject any device not stamped by the master key of an account
  already in the chat, or added by a member's signed commit, or joining with an invitation
  signed by a device in the chat. A malicious server could block actions or lie about roles,
  but could not read messages.
- **The core is in the first slice:** permissions, roles and one check that every
  operation in `api.rs` passes through, using only the defaults, with no management
  UI. **No operation exists without its check.**
- **The admin passes every check, and everyone else needs roles and permissions.** Whoever holds
  the built-in server `admin` role can do and see anything a check guards: all metadata, device
  logs and IPs, unencrypted content, and all management. **Ciphertext is never handed to them.**
  The first admin comes from `keryx user add <username> --admin`, which prints the usual one-time
  link, so the operator never learns the password. **The operator's token counts as admin:** see The
  command line.
- **Permission names are one closed list in `server/src/api.rs`, with unknown names refused,** so
  a typo in a role cannot silently grant nothing. They are dot-separated, with one name per content
  kind: `direct.create`, `directory.search`, `account.create`, and in a chat, under its type's prefix
  as Groups and channels says, `direct.message.send.text` and so on through every kind,
  `direct.message.send.unencrypted`, `direct.message.commit`, `direct.message.edit.own` and
  `direct.message.delete.own`. The closed list comes from activeplano. Each role holds only its own
  scope's names, and member, moderator and admin hold all of them, except that `account.create`, every `*.message.send.unencrypted` and `sticker.manage` are
  admin's alone, and the group and channel ones Groups and channels gives to owners and moderators. A 1:1 chat
  has no chat-role rows: both people hold the built-in member role there.
- **The check reads the role tables on every operation**, so a role change applies at once.
- **`seen`, `typing` and reading history need chat membership and no named permission.** Being admin
  does not open history, since that would hand over ciphertext.
- **Membership is not a permission, so the admin needs it too.** An admin sends into a chat only as a
  member. A non-member holds no keys for it, so what they sent would be unreadable junk under a
  sequence id.
- **There is no audit table.** Refusals and management actions are log lines, under the logging
  rules in `server/AGENTS.md`. A table would be one more lasting record of who did what, and a
  stolen database would expose it.

### Conversations and messages

- **One table for conversations and one table for messages**, not a table per
  conversation. In SQLite, per-conversation tables gained no read or write speed in
  measurement, while schema reloads reached 1.1 s per connection at 50,000 chats, and
  adding a column to all of them was estimated at about 17 hours.
- **Each conversation has a folder** for its contents.
- **A conversation row holds its type (self, direct, group, channel) and its last state**:
  last message id and so on. Writing a message updates it, and so does removing or
  editing a chat's last messages.
- **Only content moves a chat up the list:** content including reactions, and the new-device notice,
  set `last_at`. Edits, deletes and commits do not, so a key rotation never reorders anyone's list. A
  call's own messages do not either: the call is the preview, and the chat's time stays where it was.
  When the newest content is deleted, `last_at` walks back with the
  preview, to the chat's creation time if nothing is left.
- **A message either has content or is an action, and may carry a pointer.** A delete
  or edit points at its target, a reply at the message replied to, and a reaction at
  the message it is on.
- **A reply is a property of a message, not a kind:** a content message with a pointer.
- **Every content message has a `content_kind`:** text, voice, reaction, image, music, video, doc,
  file, `call` (one per call, saved, and can be replied to: see Calls), and `media_bundle` (several
  images and videos in one message, built with files, chosen by the author). The server can read
  `content_kind`, so roles can allow or refuse each one. **Music is an MP3, apart from voice, which
  is a recorded note, chosen by the author.** It is numbered 4, right after image, and every kind
  from video on moved up one, so the numbers are text 0, voice 1, reaction 2, image 3, music 4,
  video 5, doc 6, file 7, call 8, media_bundle 9, question 10, answer 11, sticker 12, suggestion 13 and
  ack 14 and activity 15.
- **A reaction is content.** It does not affect the unread count, but it brings the chat up in
  the sorted list and shows as the preview, like "Sara reacted 👍", until the next message.
- **A recording, when it is built, is saved at the lowest quality for now:**
  360p and 12 fps, with a voice codec every device supports. The server cannot decode encrypted
  media, so clients produce it. **Both ways exist:** each participant records their own stream,
  or one device records everyone.
- **Recorded video is H.264**, because a stored recording cannot be converted later and has to
  play on any device. AV1 would be about 30% smaller, but cannot play on most iPhones and Macs.
- **Recorded video targets about 200 kbps**, about 1.5 MB per minute. This is an estimate, to be
  checked on a real talking-head clip before it is fixed.
- **A device that cannot encode H.264 does not record**, and the other person's device does.
  This avoids the patent question of bundling a software encoder.
- **Recorded voice is Opus for now:** mono at about 24 kbps, with libopus bundled for devices that
  cannot encode it natively (Firefox for Android, Safari before 26, Firefox before 130, Android 9
  and below).
- **After a call, clients show the recordings that exist, and both people can remove one or two
  of them.** This is the one case where someone can remove what another device uploaded.
- **The ringing screen says the call will be recorded**, and the person can answer or decline.
- **A call's outcome (missed or answered) is readable**, so the list can show which. A call does not
  count in the unread number. Any recording's file key stays encrypted.
- **The server can read delete and edit pointers only**, because it removes targets,
  updates conversation state and checks ownership. Reply and reaction pointers stay
  encrypted, so neither the server nor a stolen database holds the reply and reaction
  graph. `kind` is always readable, because RBAC needs it.
- **Each user has their own list of conversations, worked out by the server.** The
  client never sends the list. It shows the list, updates state with the latest for
  all of them, then fetches pages in the background from the last id it has to the
  latest. **How, chosen by the author: sync by server time, for messages and calls together.** The
  client sends `sync` with `since`, the server time in milliseconds of the last thing it received, and
  optionally one `chat`. The server goes 1 second back and answers with the `chats` list plus every
  message and call row saved after that, in all the account's chats or the one named, and the client
  drops what it already has by `(chat, seq)` and `(chat, call_seq, n)`. One request after any
  reconnect, not one per chat. **Sequence ids stay, chosen by the author:** they are the order, the
  pointer and the read position, whatever the clock does, and their run 1, 2, 3 lets a client see
  that a chat is complete. Time only finds where to start. **No `since` means everything from 0, chosen
  by the author**, so a new device holds the whole archive. **Answers come in pages with a cursor,
  chosen by the author:** at most `[limits] history_page` rows, ordered by server time, chat, seq and
  `n`, with `more` and the last row's position, and the next request continues exactly after it with
  no step back, so a burst in one millisecond never loops. **The client saves each page, and only once
  the save is confirmed asks for the next**, and only then does its `since` move. **`history` and the
  call replay route are gone, chosen by the author:** `sync` is the one read path, scrolling reads the
  client's own archive, and core's commit catch-up uses `sync`. The unread count is the content
  messages after this member's seen position in the archive. A reaction does not count, a call does not
  count, and neither does one of this account's own messages, which core marks `mine`. See [core/AGENTS.md](core/AGENTS.md).
- **Pinned chats, chosen by the author, are kept per folder, below**, which replaced the first build's
  `pin` place on each member row, its `pins` frame and `PUT /api/v1/pins`. What carried over: a device
  sends the whole order at once, so a reorder never lands half done, and **an order made offline shows at
  once and is kept in the snapshot until the server takes it**, going first on the next connection, so it
  wins over what another device did meanwhile; one the server refuses is dropped and reported. The server
  learns which chats a person pinned, and in what order.
- **Folders, all chosen by the author, built in the server, core and Android; designs in
  `tmp/design/folders.html`.** Each account
  arranges its chats into folders, kept on the server and following it to every device.
  - **`all`** holds every chat. It can be made default, never renamed or removed. **It is always first,
    and the server refuses an arrangement with `all` anywhere else as `bad_body`, chosen by the author**
    when the tabs moved to the top, so every client agrees; it used to be movable. Its row is written when
    the account is made, and clients show it as any folder from the server, minus rename, remove and
    moving. A new chat goes into `all` only.
  - **A chat can be in several folders**, moved or copied between them. **Each folder has its own pins**,
    under `all`'s rules: new pins go under the pinned, an unpinned chat goes back by its time, handles
    show at two or more pinned.
  - **One folder is the default**, the tab a device opens on. A new account's is `all`, and removing the
    default makes `all` the default.
  - **Names are sealed** like a draft: AES-GCM under the profile key, associated data `keryx v1 folder
    name`, account id and folder id, plaintext the bare UTF-8 name. The server sees only that folders
    exist, their order and their chats.
  - **Ids are asked for first**, as a device's is, so the name can be sealed with it:
    `POST /api/v1/chat_folders/id` -> `{folder}`, or a `folder_id` frame `{ref}` answered with
    `folder_id` `{ref, folder}`. Both need `folder.create`, and the server keeps the id for that account
    for 5 minutes.
  - **The whole arrangement travels at once**, replacing `pins`: a `chat_folders` frame
    `{ref, folder_list: [{id, name?, chat_list?, pin_list}], default}`, folders in tab order, `all` the
    one with no name or `chat_list`, `default` a folder id, answered with `done`. `PUT
    /api/v1/chat_folders` takes the same body, and the first page of `chats` carries it as
    `chat_folders` `{folder_list, default}`. An unchanged name is sent back as the same sealed bytes, so
    the server can tell a rename.
  - **Pushed live**: saved, then sent to the account's other online devices as a `chat_folders` frame
    `{folder_list, default}`; offline ones get it on connecting. **One made offline wins**, as pins did:
    it goes first on the next connection and drops chats the account has left. **One naming a folder
    another device removed is refused whole, chosen by the author**, so a removed folder never comes back.
  - **One permission per kind of change**: `folder.create`, `folder.rename`, `folder.remove`,
    `folder.reorder`, `folder.default`, `folder.chats` (move, copy, remove from a folder) and `chat.pin`
    (pins in any folder, so pinning stops being permission-free). **All are server permissions, `chat.pin`
    too, chosen by the author.** Member, moderator and admin hold all. The server compares with what it
    keeps and checks each change. **Refused in any part is refused whole**: the device goes back to the
    server's arrangement and says why.
  - **Refusals, chosen by the author**: a change not permitted is `no_permission`; more than 20 folders
    `too_many_folders`; an id that is neither a folder of this account nor one issued to it
    `unknown_folder`; a sealed name over its limit `body_too_large`; a broken shape (`all` missing,
    twice or not first, an id or a chat twice, a pin outside its folder, a default that is not a folder) `bad_body`.
  - **Core, chosen by the author**: `FolderId` asks for a new folder's id and hands it back as
    `Event::FolderId {folder}`, so it needs a connection, and saves nothing; `Folders {folder_list,
    default}` takes plain names, seals a new folder's name and reseals only changed ones;
    `Event::Folders` hands the arrangement back with names opened. **A new folder is saved only with
    the arrangement it is part of, chosen by the author**, so a client's edit session that is thrown
    away leaves only an unused id, which the server drops after 5 minutes. A session saved more than 5
    minutes after its id was issued is refused whole with `unknown_folder`.
  - **Limits**: `[limits] folders = 20`, not counting `all`; a name of 1 to 32 characters, checked by
    clients; the sealed name capped by `[limits.body] folder_name = "256B"`.
  - **Storage**: one `folder` table, `all` included: `id`, `account_id`, `place` (tab order from 0),
    `name` (sealed, NULL only for `all`), `chats` (chat ids in order, 16 bytes each, NULL for `all`),
    `pins` (top first) and `is_default`, with one default and one `all` per account. **`member.pin`
    goes**, with each `chats` entry's `pin`, the `pins` frame and `PUT /api/v1/pins`.
  - **When an account stops being in a chat**, by leaving, removal, the chat's deletion or a direct chat
    removed for yourself, the server takes it out of that account's folders and pins in the same step and
    pushes the result. The client sends nothing more. None of these is built yet, so this comes with the
    first of them.
  - **Tabs sit at the top of the chat list, under its title row, chosen by the author**, moved from the
    bottom: All stays fixed at the row's start and the rest scroll beside it. Each counts its unread chats
    up to 99, then shows +99. The server's check that `all` is first is built.
  - **Building**: existing files only: the server's `api.rs`, `storage.rs`, `storage/sqlite.rs`,
    `sqlite.sql` and `guide.md`, and core's `protocol.rs`, `mls.rs` and `client.rs`, replacing pins
    there. Server and core first, tested by the server's integration tests driving core, then Android,
    tested on the author's phone. **Pins went at once with no compatibility, chosen by the author.**
- **Search, all chosen by the author, built in core and Android and tested on the author's phone; designs
  in `tmp/design/search.html`.**
  - **Tabs All, Accounts and Messages**, All the default, at the bottom like the folders'. Accounts: chats
    in the list matched by name, plus the server's directory. Messages: text in this device's archive.
    All: both, people first.
  - **Messages are opened and scanned per search**, newest first, stopping at a page of hits, so no
    plaintext index is written. An index may come later, as an open question.
  - **The scan runs in core, chosen by the author**, so every client matches the same way, and the
    platform only reads its archive's rows for it: see [core/AGENTS.md](core/AGENTS.md).
  - **It matches text, captions and file names, chosen by the author**, with a bundle's descriptions and
    music's title and artist, and the newest text of an edited message. A deleted message is never found.
  - **Case and Persian forms are folded, chosen by the author:** Arabic ي, ى and ك match Persian ی and ک,
    Arabic and Persian digits match ASCII ones, and ZWNJ and the short vowel marks are ignored.
  - **A page is 20 hits, chosen by the author**, and Show older goes on from there.
  - **A hit shows two lines around the match**, under its chat's name. While scanning, the search field's
    circle is a ring filling 0 to 100 with the percent and % inside.
  - **No recent searches for now.**
- **Saved server times never go back, chosen by the author.** Every one is the larger of the clock and
  the last one written, server-wide, seeded at startup from the newest row. A clock that steps back,
  from NTP or a moved VM, could otherwise hide a row from "after time T" and from the page cursor.
- **Each client keeps a local archive, chosen by the author:** every message and call row exactly as
  the server sent it, ciphertext, header and signature, opened when shown with the epoch keys its
  snapshot already holds. The phone's copy is no easier to read than the server's, and a signature can
  be checked again at any time. Core stays without I/O: it hands each page to the platform to save,
  and the platform confirms. Android keeps it in SQLite and the web in IndexedDB: see their
  AGENTS.md.

### Removal

- **How a delete runs.** The client sends a `delete` message with `pointer_seq` set to the
  target, plus the message's file ids, waiting in the queue if the target has no sequence id
  yet. The chat's task checks permission and that the files belong to the chat. In one
  transaction, the target and its edits are wiped to their `seq`, the delete gets the next
  `seq`, files are marked removed and conversation state updates, walking the preview back
  if needed. File bytes are deleted after the commit, and a sweep finishes any a crash
  interrupted. Then the sender gets the sequence id, and online devices get the delete.
- **Removing a reaction is a delete pointing at it.** An edit lists the files it drops the
  same way. Reactions on a deleted message stay as ciphertext until retention, because their
  pointers are encrypted, and clients hide them.
- **Only the account that sent a message can edit it**, from any of its devices, with no time limit.
  Roles can never edit someone else's words, because an edit would then look like that person wrote
  it. A reaction cannot be edited: remove it and add it again.
- **A person can remove their own messages, only for everyone.** That goes for selecting
  several messages too. There is no "delete for me" on a message. **In groups and channels, a
  role in that conversation can remove others' messages**, through RBAC.
- **A direct chat can be removed only for yourself.** It stays hidden until the next message,
  which brings everything back, except messages already removed. A reaction does not count as
  that next message, so one on an old message does not restore a chat. **If both people removed it,
  all its messages go, and the next message is like a new chat.**
- **An owner removing a group or channel removes everything, everywhere.** Coming back, a
  member sees nothing about it. Clients wipe any chat missing from the server's list.
- **A group or channel can have several owners, and ownership can be transferred.**
- **When the last owner leaves, the group or channel is removed for everyone.** So a group
  always has an owner, and the last member to leave is always its owner.
- **Leaving a group or channel means not seeing it** until joining again. **On rejoining, a
  person sees what they had before and everything from then on.** Messages from while they
  were away stay unreadable, because no key was wrapped for them.
- **A self chat cannot be removed, but all its messages can**, except that for its first 7 days
  a new-device notice cannot be removed by that device or any newer one. Older devices can
  remove it at any time, and retention never touches it.
- **A removed message keeps only its `seq`**, whether it was deleted or expired. Every other
  field is wiped, so pages have no gaps, and a client knows something was there, not what.
  Rows are never dropped, so one small row per removed message stays for as long as the chat.
  **Its server time becomes the time it was wiped, chosen by the author**, so sync returns it as an
  empty row and every device drops its copy, an expiry too, which has no delete message. It reveals
  only when the removal happened, which a delete row shows anyway.
- **Deleting also removes the message's edits**, because an edit holds its newer text.
- **A device wipes a deleted message from its archive as soon as the server confirms the delete, chosen
  by the author:** by the `sent` reply to its own delete, or by the delete arriving from another device,
  and not only at the next `sync`. Only a device that is offline keeps it until the delete reaches it.
- **In a group or channel a delete hides a message from the chat, not from its owners and moderators,
  chosen by the author:** the server keeps its original in the history, as Groups and channels says, until
  the chat's retention.
- **The notice's 7 days give a thief a way out:** after that, stolen words can erase it. The
  device list still shows the device, and contacts saw the join.
- **One owner can destroy a whole group's history**, for everyone, with no undo, and a sole
  owner does it just by leaving. Clients must warn before that happens.
- **Hiding a direct chat is not privacy:** one message from the other person brings it all back.
- **A deletion cannot reach** copies already delivered, screenshots or backups. The delete
  message itself stays until it expires, so in a direct chat, where only the sender can
  delete, it shows who sent the target.

### Profiles

**Accounts, groups and channels all keep profile info: an image, a bio, and links as
key and value.** Groups and channels also have a name, and accounts their display name.
Direct and self chats have no profile of their own.

- **An encrypted group or channel's name, bio, links and image info are encrypted**, as
  one sealed blob under the chat's own profile key: see Groups and channels. **Someone invited but
  not yet joined sees only the name and the photo's BlurHash their invitation carries.** It cannot go
  inside MLS itself, because the server stores `group_info` in the clear so new devices can join.
- **A findable group or channel's name and bio are readable by the server**, as Groups and channels
  says.
- **An account's display name stays readable**, because the directory searches it. **Its
  image, bio and links are encrypted to people who share a chat with that account:** one
  sealed blob per account, with its key shared per chat, so changing a profile rewrites one
  blob. Strangers see only a name.
- **A profile key does not rotate when someone leaves a chat.** The server refuses to send a
  profile to anyone who no longer shares a chat with that person. Someone who left could
  still read later changes only by getting the blob from a breached server.
- **Channel subscribers do not share profile keys**, so they see only each other's display
  names. Sharing would cost about 6 MB per new subscriber in a 100,000-member channel.

**An account's profile, chosen by the author.** This part builds it for accounts, with a page to edit your
own and others' shown wherever a person is. Groups and channels come with them.

- **Both the display name and the username can be changed, chosen by the author.** **A display name is 1 to
  64 characters after trimming spaces, chosen by the author**, long enough for a full Persian name. **A
  freed username can be taken by anyone at once, chosen by the author**, since keryx has no mentions or
  profile links yet that could point at the wrong person. **Changing the username asks for the password,
  chosen by the author**, as it is what the person logs in with, so a stolen session cannot rename the
  account and leave its owner not knowing what to type. Removing the password, under First phase, has to
  replace this check.
- **The account changes its own names, and admin can change anyone's, chosen by the author**, from the
  command line, to fix an offensive or fake name. The photo, bio and links are encrypted, so admin can
  remove the photo and never see it.
- **Two new permissions, chosen by the author:** `profile.edit` for the photo, bio, links and display name,
  and `account.rename` for the username, so a server can let people set a photo but not change their
  username. Member, moderator and admin hold both.
- **The photo is one square JPEG of 640x640, chosen by the author**, about 50 to 150 KB, which every phone
  and browser can make and show, with a BlurHash. It is limited to 1 MiB, encrypted under its own random
  key like any file, with that key, its size and BlurHash inside the sealed profile. **The phone picks the
  square on a crop screen, chosen by the author**, dragging and pinching to zoom. **There is one photo,
  chosen by the author:** a new one replaces the old, whose file is deleted.
- **A bio is at most 140 characters, with at most 3 links, chosen by the author**, short like Telegram's.
- **Its bytes live under `[files] directory`, in `accounts/<account id>/`, chosen by the author**, beside
  the per-chat folders and `stickers`, with no new config key.
- **The profile key comes from the words, chosen by the author:** one more HKDF label, `keryx v1 profile
  key`, giving 16 bytes, like the master and account keys. Every device of the account has it at once, with
  no sharing and no race between two devices making different keys, and running devices keep it in their
  snapshot after dropping the words' other keys. It changes only if the words do.
- **Whoever adds a member shares every member's profile key with them, inside the commit, chosen by the
  author:** a `profile_key_list` of `{account, sealed}` beside its wraps, saved in the same step, so a new
  member sees everyone's photo at once and the keys can never be lost between two requests. Each is sealed
  under the chat's key at the commit's epoch, with the label `keryx v1 profile key`, the chat, the account
  and the epoch as associated data. **A member whose own key is missing from a chat shares it with a
  `profile_keys` frame** `{ref, chat, epoch, profile_key_list}`, answered with `done`, like `wraps`: the
  person a chat was started with, and the self chat. The server keeps the newest per chat and account.
- **The sealed profile is `{"bio"?, "link_list"?: [{"name", "url"}], "photo"?: {"id", "key", "size",
  "blurhash"}}`**, AES-GCM under the profile key with `keryx v1 profile` and the account as associated
  data. The photo is sealed as one chunk in the file format, under its own key.
- **The routes, chosen by the author.** Usernames follow the sign-up rules, and admin may break the length
  limit, as `keryx user add` can.

  ```
  GET  /api/v1/accounts/{id}/profile   yourself, or sharing a chat -> username, display_name, version,
                                       profile?, photo?
  PUT  /api/v1/accounts/{id}/profile   profile.edit, yourself: profile?, photo? -> version. An old photo
                                       no longer named is deleted
  PUT  /api/v1/accounts/{id}/names     display_name? (profile.edit), username? and password
                                       (account.rename); admin: anyone's, with no password
  POST /api/v1/accounts/{id}/photos    profile.edit, yourself: the sealed JPEG's raw bytes -> photo
  GET  /api/v1/photos/{id}             yourself, or sharing a chat with its account -> the bytes, immutable
  DEL  /api/v1/accounts/{id}/photo     admin
  ```

  The photo's limit is `[limits.upload] photo`, 1 MiB. **The operator's commands, chosen by the author,** are
  `keryx user rename <username> [--username <new>] [--display-name <name>]` and `keryx user photo remove
  <username>`, over REST like `user add`.
- **Changes go out live, chosen by the author:** the server sends a `profile` frame `{account, version,
  username, display_name}` to the online devices of everyone sharing a chat with the account, and its own.
  Each `chats` entry carries `profile_key_list: [{account, epoch, sealed, version}]` for its members, so a
  phone that was offline fetches a profile whose version moved when it reconnects.
- **Shared profile keys are pushed too, chosen by the author: what changes reaches online devices live, and
  offline ones get it when they come back, which is the rule for everything.** Once a `profile_keys` frame
  or a commit carrying keys is saved, the chat's other online devices get a `profile_keys` server frame
  `{chat, profile_key_list}`, with the same items as a `chats` entry, sent after the commit itself so a
  device has reached its epoch. Core opens them and shows any profile that was waiting for its key, and
  holds keys for a chat it has not joined yet until that chat is ready, since a commit's keys reach a new
  member just before its Welcome. A device that joins a chat by Welcome before any list names it shares
  its own key at once: it is the person the chat was started with, whose key nobody else holds. A device
  that was offline gets them in its next `chats` list.
- **Android's profile page, chosen by the author:** the photo, display name, bio and links, the username
  shown with tap to copy, and Devices and Log out, which move there from the chat list's menu. Others'
  photos show in the chat list and the chat's header, and tapping the header opens their profile. **Tapping
  a photo in the chat list opens that person's profile, not the chat, chosen by the author.** **Saved
  messages shows your own photo, in the list and its header, and tapping it opens your profile, chosen by
  the author.** **The page is a big photo, chosen by the author from `tmp/design/profile-redesign.html`, its
  B:** the photo fills the width, 300 dp high, under the status bar, with the name and the state over its
  foot, and the rest in rounded cards with a title over each. **Once the photo scrolls away, a solid bar with
  back and the name fades in, chosen by the author.** Someone else's page has Message, Voice call and Video
  call under the photo, then their bio, links and @username; your own has Account (name, username, bio,
  links), Settings (Language, Theme, Devices) and Log out, with "Add a bio" and "Add a link" where nothing is
  set yet.
  **Tapping the photo stays as it was, chosen by the author:** your own opens the photo sheet. **Tapping
  someone else's photo opens it whole, chosen by the author:** as large as fits on black, with a
  back arrow, and no zoom; Back or the arrow returns to their profile. **It is
  drawn in `tmp/design/profile.html` and approved as drawn, chosen by the author:** the menu button opens
  the profile; each line opens its own editor, saved with a check in the bar; your own name and username are
  also lines in Account, as before; a taken username shows on save as "That username is taken."; a link's
  address starts with `https://` or `http://`, its name is at most 32 characters and its address 512, and
  tapping one opens the browser; someone with no photo keeps the letter, and one still loading shows its
  BlurHash; Remove photo asks nothing; the photo uploads when Done is tapped, under a turning ring.
- **The other side of the phone test gets its profile from `server/examples/drive.rs`, chosen by the
  author:** `bio <text>`, `link <name> <url>` and `photo <file.jpg>`, through core's own commands, so one
  phone sees someone else's photo, bio and links. The photo's BlurHash there is a flat colour, since
  `drive.rs` decodes no images.

### Files

**Files are encrypted, each with its own random AES-GCM key, in chunks** because a browser's
AES cannot stream one large file. The file's name, type and key are inside the encrypted
message or profile, so a new device with only the words gets files too, and the server sees
an unnamed blob, its **exact size**, and who uploaded it. An unencrypted message's files are
not encrypted. **An upload declares it, chosen by the author:** the create-file body's `encrypted`, true
by default, and false needs `*.message.send.unencrypted`, as an unencrypted message does.

**On a delete or edit, the client sends the file ids, and the server removes them** if the
asker is the file's uploader, or holds that permission in the chat, such as a group's or
channel's owner or moderator. The server checks that each file belongs to that chat. It does
not know which message names which file until that message is deleted or edited. A file id is
never reused between messages, so forwarding uploads a new copy, otherwise deleting one message
would break the other.

**A file has its own expiry**, because nobody names the files of a message that expires.

**An upload stays only once a saved message says so.** A message can be lost on a bad connection after
its file went up, and the file would otherwise be kept forever. So the message a client sends carries a
`persist` list of its file ids, and the chat's task marks those files to stay in the same step that saves
the message and gives it its sequence id. If the message never lands, the file stays unmarked. **It is an
optional list of file ids, chosen by the author:** `persist: ["01J..."]`. The chat's task refuses the send
unless each file belongs to this chat, was uploaded by this account's device, is complete and is not yet
used. **A delete or edit names the files it drops in `remove: [...]`, chosen by the author**, beside
`persist`.

**Files uploaded but never marked are removed when the chat's task loads, and at startup.** There
is no job running on a timer. Only unmarked files older than a set age go, so an upload in progress is
safe. An idle chat's unmarked files stay until it next loads or the server restarts. **That age is 1 day,
chosen by the author**, fixed in code: long enough for a slow upload of a large video on a bad network.

**How a file is built, chosen by the author:**

- **Chunks are 1 MiB of plaintext**, each AES-128-GCM under the file's own random 16-byte key, the same
  strength as messages. A phone or browser holds a few in memory, and a 2 GiB video is 2048 chunks.
- **A chunk's nonce is its number, and its associated data says whether it is the last.** The key
  encrypts one file only, so counting is safe, and a server that reorders, swaps or cuts chunks makes
  the file fail to open. Each chunk is its ciphertext and 16-byte tag, with no nonce stored.
- **One request per chunk, in order.** Chunk n is accepted only after 0 to n-1, and sending the last
  one again is fine. The server keeps one count, so after a dropped connection the client asks how many
  arrived and goes on from there.
- **The upload's `size` is the encrypted bytes**, what the server stores, so it checks every chunk's
  length and the limits apply to what it holds. The real size is inside the encrypted message for
  other clients.
- **Chunks can be read only once a sent message has marked the file to stay.** Nobody fetches a
  half-upload.
- **Limits are per declared type under `[limits.upload]`, chosen by the author:** voice 50 MiB, image
  20 MiB, music 200 MiB, video 2 GiB, doc 100 MiB and file 2 GiB, each changeable. Music's 200 MiB
  holds about 80 minutes of MP3 at 320 kbps, so an album or a long mix goes as one file. A file's real type is encrypted, so
  the server trusts what a client declares, and a client that lies gets the largest limit.
- **There are no quotas for now**, only the per-file limits. They stay open, as rate limits do.
- **The bytes live under `[files] directory` in `server.toml`**, an absolute path like `[www]`, with a
  folder per chat named by its id and a file per upload named by its id.
- **The encrypted message describes its file:** its id, name, type, real size, key and chunk count, and
  for images and videos the width, height and a BlurHash in its standard text, so a chat draws the
  right space before downloading. **BlurHash is made and read by core with the `blurhash` crate**, the
  platform handing it small RGBA pixels, so there is one implementation. **A file message may carry an
  optional caption.**
- **Music is one MP3, of type `audio/mpeg`, and core refuses anything else with `bad_files`, chosen by
  the author**, since the server cannot see a file's type. A modified client could still mislabel one,
  and receivers open files by their type, so that does no harm. **Music carries optional `title`,
  `artist` and `duration` in milliseconds from its tags, and its cover as a BlurHash with the cover's
  `width` and `height`, chosen by the author.** Core keeps `title`, `artist` and `duration` on music
  only. Music never goes in a bundle.
- **`media_bundle` holds images and videos only, at most 20, fixed in core**, each with its own upload
  limit by its type. **Each image and video may carry its own optional description, chosen by the
  author**, and the bundle one optional caption. Its body limit is 64 KiB, which holds all of them. **An edit may remove files from
  a bundle**, naming them in `remove`, and never adds any, so an edit never uploads.
- **The code lives in two new files, chosen by the author:** `core/src/file.rs` seals and opens chunks
  and describes a file inside a message; `server/src/file.rs` keeps the bytes on disk, writes and reads
  chunks, and removes unused uploads. Routes stay in `api.rs` and rows in storage.
- **Testing, chosen by the author:** the server's integration tests drive core as a real client, and the
  bare Android screen picks a file with Android's picker, sends it, and another account opens it.

### Calls

**A call is one room where each person chooses what they send, chosen by the author.** Anyone in it may
have video and voice on, turn either off, or only watch and listen and answer in text. Calls are for
direct chats, groups and channels. **The first build is direct chats only, with no recording**, designed
so groups and channels fit later; recording comes after the phone test.

- **A call is one chat message with one sequence id, chosen by the author:** content kind `call`, which
  replaced `voice_call`, `video_call` and `text_call`, since what each person sends is their own choice.
  Its body limit is `[limits.body] call`, and sending one needs `message.send.call`. **The message row
  holds nothing more about the call, chosen by the author:** no outcome and no end time, there or in its
  frame. **Everything else lives in one `call` table, with many rows per call, chosen by the author:**
  joins, leaves, declines, the text typed in it and its end, keyed by chat, `call_seq` and `n`, the
  call row pointing at its message. None of them count as unread or move the preview. **The call's
  message is the list preview, chosen by the author:** its status, missed or not, and voice or video.
  The unread number does not include it.
- **Starting a call sends that message, and its delivery over the chat's WebSocket is the ring.** Joining
  needs membership only, like `seen`. **Its encrypted body says what the caller has on, chosen by the
  author:** `{"video": true|false, "voice": true|false}`, so the ring can say video or voice call.
- **The server saves what happens inside a call as it goes, chosen by the author**, in the `call`
  table: chat, `call_seq`, `n` (1, 2, 3 inside the call), device, kind, and for client events the body
  and client time, and always the server time. Clients get them through `sync`, beside the messages,
  into their archive. **Client events use the normal message encryption**, with
  the call's sequence id as `call_seq` in the signed header, so a new device with only the words reads
  them too.
- **Join and leave are written by the server**, as readable events with device and time, because it
  admits every connection to the call's MoQ and sees it leave. They hold nothing it did not already
  know, a modified client cannot skip them, and a phone that dies still leaves.
- **Declining is a `call_event` frame over the chat's WebSocket**, answered with `done`, since the person
  has not joined the call's MoQ. The server saves it inside the call and sends it as `call_event` to the
  chat's online devices. **In a direct chat a decline ends the call at once, chosen by the author:** a
  decline from any of the other account's devices is followed by `end` as missed, and the caller's
  socket closes with `call_ended`, so nobody hears ringing for a minute after a no. The outcome stays
  missed or answered, with no declined value. Groups, later, keep ringing the rest. **The `end` that
  follows a decline names the decliner's device, chosen by the author**, as the device whose action
  ended the call. **A decline for a call that is not open is refused with `call_ended`, chosen by the
  author**, the same word the call socket's close uses. **A decline after
  the call was answered is refused with `call_answered`, and nothing is saved, chosen by the author**:
  a device that missed the join learns the call was taken and stops ringing, and the call its person
  is in can never be ended from another device.
- **Every call event but text also goes to the chat's online devices, chosen by the author:** `join`,
  `leave`, `decline` and `end` are sent as `call_event` on the chat's WebSocket as well as on the
  `events` track. A device that rang stops when its person joins on another device, and a chat can show
  who is in a call. Text stays inside the call, on `events` and in `sync`.
- **Text typed in a call travels through MoQ, chosen by the author.** Each device sends it on its own
  `text` track. The relay, in the same binary, hands it to the chat's task, which saves it with the next
  `n` inside the call and republishes it on the `server` broadcast's `events` track, which everyone in
  the call reads, so all see one order. Joins, leaves and the end go out on that track too.
- **A call ends when its last device leaves, and an ended call cannot be joined again**; a new call is
  started instead. **If nobody else joins within 60 seconds, fixed in code, the server ends it as
  missed.** The call's `end` row carries the readable outcome, missed or answered, and its server time
  is when the call ended. **An open call is one with no `end` row yet.**
- **A dropped connection gets 10 seconds, fixed in code, before it counts as a leave, chosen by the
  author.** A clean close of the call's MoQ is a leave at once. A device that reconnects within the 10
  seconds writes no leave and no new join, so a blip from the caller while it rings does not end the
  call as missed. Without it, the last device's blip would end the call for good.
- **Tests shorten both timers with test-only keys, chosen by the author**, as captcha's `test_answer`
  does: `[calls] test_ring` and `test_grace`, left out of the template, and the server warns at startup
  whenever either is set. Production keeps 60 and 10 seconds fixed in code, and the server's tests, which
  run the real binary, do not wait a minute for a missed call.
- **A call is answered only by the other account, chosen by the author.** In a direct chat, only a join
  from a device of the account that did not start the call makes it answered and stops the 60-second
  timer. The caller's own other device may join, and the call still rings. Only the other account's
  devices may decline: a decline from the caller's own account is refused with `bad_message`. Clients do
  not ring for their own account's call, which core marks `mine`.
- **Calls are refused in a self chat, chosen by the author**, with a new code, `call_not_allowed`, since
  the first build is direct chats only. A self chat's call could reach only the caller's own devices,
  with no rule for answered or missed. Groups and channels get the same refusal until their calls are
  designed.
- **A chat has at most one open call, chosen by the author.** Starting another while one has not ended
  is refused with `call_open`, naming the open call's sequence id, so the client joins that one. Two
  people calling each other at once meet in one room, not two that each ring. **The id goes in an
  optional `call_seq` on the refusal, chosen by the author**, on the WebSocket's `refused` and in REST's
  error body, set only for `call_open`. It is a new optional field, so `keryx.v1` stays, and the client
  can join at once even before the winning call's message reaches it.
- **The chat's task does not idle out while its chat has an open call, chosen by the author.** It holds
  the call's state in memory: who is in it, the next `n`, the 60-second missed timer and the 10-second
  drop timers. A voice-only call sends the task nothing, so `[chat] idle_after` would otherwise end it
  mid-call. Once the call ends, the idle timeout counts again. Calls still open at startup are ended,
  since nobody can be in them after a restart, so the state is never rebuilt from the database.
- **Turning video or voice on or off is live only**, never saved: each device's encrypted `state` track
  says what it has on.

**How a call's media travels, chosen by the author:**

- **The relay is keryx's own, in `server/src/moq.rs`**, on `moq-net` 0.2.22 for the fan-out and `qmux`
  0.5 for MoQ over the WebSocket: its `Upgraded` takes over a WebSocket axum already accepted, and
  `keryx-moq` uses its `ws::Client`. **`moq-net` stays on 0.2, chosen by the author**, because it takes
  `qmux` 0.5's session directly; 0.3 takes only a lower-level interface, whose adapter lives in
  `moq-tokio`. Moving to 0.3 means writing or taking that adapter. `moq-relay` was rejected: it forces `aws-lc-rs` beside
  `ring` and brings clustering, `reqwest` and its own config, which one server does not need. **So was
  `moq-tokio`, chosen by the author**: it brings `aws-lc-rs` back through its certificates, and
  `tracing-subscriber`, `notify`, `x509-parser` and its own config, none of which WebSocket alone needs.
- **WebSocket only for now, with no QUIC**: MoQ over `qmux` on a WebSocket, on the same port as the API at
  `GET /api/v1/calls/{chat}/{call_seq}/moq`, with the same cookie or Bearer token as the chat's
  WebSocket. A device is admitted only if it is a member of the chat and the call has not ended. It may
  publish only under its own device id, and may read everyone in that call. **A refused device is
  answered as the chat's WebSocket answers, chosen by the author:** the upgrade is accepted, then closed
  with code 4403 and the refusal's code as the reason, so browsers, which cannot read a failed upgrade's
  reply, and native clients get one rule. **`keryx-moq` reports it as every other ending, chosen by the
  author:** `Call::connect` returns once the WebSocket is upgraded, and a refusal arrives from `next()`
  as `Closed{4403, code}`, beside `1000 call_ended` and `1000 replaced`. A socket that ends with no close
  frame is `Closed{1006}`, as the WebSocket standard says. Since an admitted socket sends nothing at
  once, `connect` could not know how long to wait for a refusal.
- **The call socket's subprotocol is `qmux-02.moq-lite-05` only, chosen by the author:** qmux's own
  form, its newest draft, and moq-lite 05. Both ends offer and accept exactly this one, so a stock MoQ
  client, such as a later browser one, can connect. keryx's own version stays in the `/api/v1` path.
- **`keryx-moq` opens the WebSocket itself, then hands it to `qmux`, chosen by the author.** `qmux`
  0.5's own `ws::Client` sends no custom headers, so it cannot carry the Bearer token, and secrets never
  go in a URL. It also turns a close frame into a bare `Closed`, losing 4403 and its reason. So
  `keryx-moq` connects with `tokio-tungstenite` 0.29, the version axum already pulls in, sending the
  Bearer header and the subprotocol. It wraps the stream in a thin adapter that records a close frame's
  code and reason, and gives it to `qmux`'s `Upgraded::with_alpn(..).connect()`. Stock `qmux`, no fork.
  **`connect` waits at most 10 seconds for the upgrade, fixed in code, chosen by the author**, the same
  as a dead socket takes to be noticed, so a hung server never freezes a call screen or a test.
  **It speaks `ws://` only for now, chosen by the author:** no rustls and no root store, as the tests use
  plain HTTP and the phone test reaches `127.0.0.1:7778` through `adb reverse`. How it trusts a `wss://`
  server's certificate is on the Open list.
- **The server keeps axum's `WebSocketUpgrade` and adapts its socket for `qmux`, chosen by the author.**
  So the `Origin` check, cookie or Bearer, subprotocol choice, the `[tcp]` socket wrapper and the 4403
  close work as on the chat's WebSocket. A small Stream and Sink adapter in `moq.rs` maps axum's
  `Message` to tungstenite's and back, both on tungstenite 0.29, with no new dependency.
- **Each device publishes into its own origin, and the relay copies, chosen by the author.** If every
  session shared one origin, `moq-net` would forward frames without keryx seeing them, so neither
  `call_frame` nor text going to the chat's task could hold. Each device's origin is scoped with
  `moq-net`'s `scope()` to its own device id. Per track, a relay task reads each frame, whose size is
  declared before its bytes, so one over `call_frame` closes that device before it is buffered. Allowed
  `state`, `audio` and `video` frames are written into the call's shared origin, which everyone reads,
  and any other track name is refused. `text` is not forwarded: it goes to the chat's task, which saves
  it and republishes it on `events`. This costs one in-memory copy and one task per track.
  **A device that breaks a call rule is closed with 4403 and a code, and that is a leave at once,
  chosen by the author:** `frame_too_large` for a frame over `call_frame` or text over `[limits.body]
  text`, and `bad_frame` for text that is not call text's JSON, the chat socket's rule for a broken
  frame. The server ended it, so there is no 10-second grace, and the device may join again.
- **The relay's own close frame goes straight on the WebSocket, and qmux's application close is never
  sent.** A device's `moq-net` closes its session locally the moment it reads qmux's close, which
  hides the code and reason it carried, so `call_ended`, `replaced` and the 4403 codes travel only in
  the WebSocket's close frame, after whatever was already written.
- **Both ends speak moq-lite 05 only, and every frame's `moq-net` timestamp is zero, chosen by the
  author.** moq-lite 05 writes each frame's timestamp in the clear, and 04 writes none. `keryx-moq`
  writes `Timestamp::ZERO` and the relay writes zero when it copies, so the real clock stays inside
  SFrame and only track names are in the clear. In `moq-net` 0.2.22 timestamps only convert between
  time scales: cached groups expire by the relay's own clock at last access, and its own tests write
  zero. It costs a byte per frame. 05 keeps `TRACK_INFO`, so audio's priority over video, ordering and
  the latency window travel with each track, and it is the version a later browser MoQ client and
  `moq-net` 0.3 are most likely to speak.
- **`keryx-moq` runs its own tokio runtime and gives its callers a plain handle, chosen by the author.**
  Both callers are synchronous: `android/ffi/` has no tokio and drives core's no-I/O commands, and the
  server's tests block. So each call gets one current-thread runtime on its own thread. Sends never
  block, as they queue to that runtime, and incoming frames come from a blocking `next(timeout)`. One
  thread per call, and a device has one call at a time.
- **`keryx-moq`'s API is typed per track, chosen by the author:** `Call::connect(url, bearer, key,
  index_list)`, then `audio(ts, bytes)`, `video(ts, bytes, keyframe)`, where a keyframe starts a new
  MoQ group, `state(State)`, `text(CallText)` and `rekey(key, index_list)`. One incoming `Event`:
  `Audio{device, ts, bytes}`, `Video{device, ts, bytes}`, `State{device, State}`, `CallEvent` and
  `Closed{code, reason}`. Track names and grouping live in one place, so no caller can name a wrong
  track. **Each `state` and `text` frame is its own MoQ group, chosen by the author**, as each audio frame
  is: the relay keeps the newest group, so a joiner reads a sender's current state at once, and a late
  frame never holds up a newer one. **A frame that will not open is dropped silently, chosen by the
  author:** an unknown key, a sender index that does not match its broadcast, a device missing from the
  index list, or bad state JSON. A frame sealed under a key not yet received is normal for a moment
  during a rekey, and a malicious one gets nothing. **Up to 256 events wait for `next()`, chosen by the
  author:** past that, new audio, video and state are dropped, and call events and `Closed` are always
  kept, so a stalled decoder cannot grow memory. **`keryx-moq` logs nothing of its own, chosen by the
  author**, like core: every ending is an `Event` or a `Result`. `moq-net`'s and `qmux`'s own `tracing`
  reaches whatever subscriber its caller installs.
- **The relay's cache is `[limits] call_cache`, 16 MiB by default, chosen by the author.** It is
  `moq-net`'s process-wide byte budget for cached groups, unbounded unless a relay sets it. **It never
  evicts a track's newest group, so it does not limit how many calls run:** it is the room for older
  groups a receiver that fell behind catches up from. A direct call at the phone test's 500 kbps holds
  about one 2-second group of video per device, about 122 KiB, plus a little audio. Counted in both the
  device's origin and the shared one, that is about 0.5 MiB of older groups per call, so 16 MiB serves
  **about 32 direct calls at once** with full catch-up room: about 64 if the copy is charged once, and
  about 16 if two groups sit in the window. Past that, older groups go first, so a receiver that falls
  behind skips to the newest keyframe sooner. Calls keep running, and memory still grows by the newest
  groups, about 0.25 MiB per extra call.
- **Track priority and windows are fixed in code, chosen by the author:** `events` before `audio` before
  `state` before `video`, so voice survives a weak link. **A device's `text` track has the same priority
  and 2-second window as `events`, chosen by the author**, since it becomes events: it is small and rare,
  so going first costs voice nothing, and typed words never wait behind video. `latency_max` is 500 ms for `audio`, 2 seconds
  for `video`, one group at the keyframe interval, and 2 seconds for `state` and `events`. The newest
  group is always kept, so a joiner gets the current state.
- **A call socket's keep-alive is `qmux`'s, set from the chat socket's keys, chosen by the author.** Once
  `qmux` owns the WebSocket, keryx's own ping timer cannot send on it. So `qmux`'s `KeepAlive` pings
  every `[limits] ping_after` and closes when nothing arrives within `ping_after` plus `ping_timeout`,
  about 10 seconds, as on the chat socket. Browsers answer WebSocket pings themselves. A dead phone then
  leaves after about 20 seconds: 10 to notice and the 10-second drop grace.
- **A device's new connection to a call replaces its old one, chosen by the author.** The relay closes
  the old one with 1000 and reason `replaced` and moves the device's broadcast to the new one, writing
  no `leave` or `join`, as within the drop grace. A phone that switched networks before the server
  noticed its old socket was dead is never locked out of its own call.
- **When the server ends a call with devices still in it, it says so first, chosen by the author.** The
  chat's task writes `end` and publishes it on `events` and as `call_event` on the chat's WebSocket,
  then the relay closes each device's call socket with 1000 and reason `call_ended`. This covers a call
  that rang out with its caller still there. **Deleting an open call ends it this way first**, then wipes
  it with its events, so devices see the end before the message goes.
- **keryx's own layout on MoQ, not `hang`'s:** each device's broadcast, named by its device id, has
  `state`, `audio`, `video` and `text` tracks, and the server's broadcast `server` has `events`. Every
  media frame is SFrame-encrypted with its timestamp inside, and a video keyframe starts a new MoQ
  group. Only track names are in the clear; `hang`'s catalog would show codecs and sizes.
- **SFrame is the `sframe` crate 2.1 on its RustCrypto backend**, with AES-128-GCM and its MLS key
  scheme, keyed from the chat's MLS exporter per epoch. It carries the RFC's test vectors and builds to
  WASM. Its backend is `aes-gcm` 0.11, beside the 0.10 openmls and core use, and core's `hkdf`.
- **A frame's key id is 16 bits of sender index and 8 bits of epoch, with context 0, chosen by the
  author**: room for 65,536 members, and frames from up to 255 key changes back are recognised. A
  receiver checks that the sender index belongs to the device whose broadcast the frame came on.
- **A media frame's plaintext is `[timestamp, 8 bytes big-endian microseconds][codec bytes]`, chosen by
  the author.** The `state` track's is JSON `{"video": bool, "voice": bool, "width"?, "height"?,
  "rotation"?: 0|90|180|270}`, sealed the same way. **The codec bytes are H.264 in Annex B, with its
  SPS and PPS in front of every keyframe, and Opus as bare packets, chosen by the author.** Annex B is
  what MediaCodec writes and WebCodecs reads, and any keyframe starts a decoder from nothing, so a
  joiner needs no setup sent apart. Opus carries no `OpusHead`: every receiver builds its own from the
  fixed settings, mono at 48 kHz.
- **Every call event is the same JSON, chosen by the author**, on the `events` track, in the WebSocket's
  `call_event` and in `sync`'s call rows: `{chat, call_seq, n, kind, device, server_at, client_at?,
  epoch?, signature?, body?, outcome?}`, with kind `join`, `leave`, `decline`, `text` or `end`, and
  `outcome` on `end` only. **A decline has no
  body**, since the server sees who sent it anyway. **Text is encrypted and signed like a message**, with
  header kind content, content kind text and `call_seq` set, so it can never pass for a chat message. A
  device sends text on its `text` track as `{client_at, epoch, signature, body}`.
- **One frame on a call's tracks is at most `[limits] call_frame`, 256 KiB by default, chosen by the
  author**, for `state`, `audio` and `video`. Text on the `text` track uses `[limits.body] text`. A
  larger frame closes that device's connection to the call. The relay forwards and keeps each track's
  current group, so without a limit one member could fill the server's memory.
- **Live video for the phone test is 640x360 at 15 fps and 500 kbps, chosen by the author**, fixed in the
  Android client for now. **A keyframe comes every 2 seconds, and at once on each `join`, chosen by
  the author:** each sender forces one when a join arrives on the `events` track, which everyone in the
  call already reads, so a joiner gets video at once with no new request in the protocol.
- **Live calls are H.264 video and Opus audio**, the same as recordings. **Live H.264 is Constrained
  Baseline, chosen by the author:** every hardware decoder and browser plays it, and with no B-frames
  frames leave in capture order with no reordering delay, at about 10 to 15% less efficiency than High. Android encodes both through
  MediaCodec, which has Opus from Android 10, so the Android client starts at API 29.
- **The code, chosen by the author:** `core/src/call.rs` has no I/O and holds SFrame sealing and
  opening, the media frame layout, the `state` track and call events, driven by `client.rs`. A new
  workspace crate `moq/` (`keryx-moq`) holds the native MoQ client over `qmux`'s WebSocket, used by
  `android/ffi/` and by the server's tests. **It seals and opens frames itself, chosen by the author:**
  it depends on `keryx-core`, takes the `CallKey` and the members' MLS indexes, and its callers hand it
  plain codec frames and get plain frames back. So `android/ffi/` and the tests share one path, and
  keys never leave Rust. On Android it lives inside keryx's own bridge, so Kotlin only captures,
  encodes and decodes.

### Sending a message

1. **The client sends the message, with a new ULID as the request's `ref`.**
2. **A per-chat tokio task serializes the chat's events**, in `server/src/chat.rs`, where chat
   logic lives. It stores the message under its sequence id. **Direct, group and channel tasks all end after the same idle
   timeout**, set in `server.toml`, and restart on the next event by reading the last
   sequence id. One lifetime rule, and memory does not grow with old chats.
3. **The server tells the sender the sequence id.** It does not check the message's signature: clients
   must check it anyway, because they cannot trust the server, and a bad one only hurts its sender.
4. **The server publishes the message, with that sequence id, to all online devices**, except the
   device that sent it, which already has the `sent` reply.

**A WebSocket refused after its upgrade closes with code 4403 and the refusal's code as the reason**,
since a browser cannot read a failed upgrade's reply.

**A message's body is limited by its content kind**, under `[limits.body]`. An edit uses the limit of the
kind it edits, since it holds that message's newer text. A delete and a commit have only
`websocket_frame`, because a commit grows with the group.

**The task's idle timeout defaults to 5 minutes**, as `[chat] idle_after` in `server.toml`.
**The task never waits on a connection.** Each device has a bounded outgoing buffer, and when it
fills, that connection is closed. The device reconnects and fetches what it missed from its last
sequence id, so nothing is lost, because every message is saved.

**Both sides ping, and only silence ends a connection.** The server pings a client after `ping_after`
(5 s) with nothing from it, then every `ping_every` (15 s), and each ping has `ping_timeout` (5 s) to be
answered, so a client gone silent closes at about 10 s. The connection closes with no close frame, since
the peer is probably gone, and the session stays, so the device reconnects with the same login. Only
what the client sends holds pings back, because only that proves it is alive; a dead client in a busy
chat still gets pinged. A pong or a client's ping answers a waiting ping but does not hold back the next.
A browser cannot send a WebSocket ping or see one, so it sends a `ping` frame and gets `done`, answered
on the socket's own loop so a slow request cannot delay it. Other clients may send real pings. **Each
connection has one timer, and frames only move times it reads when it wakes**, so a busy connection
costs no timer work per frame. A shared clock would save little, as a quiet connection's task has to
wake to send its ping anyway. Each of the three must be above zero, or startup stops.

**The first frame on every connection is `features`:** the server's `ping_after`, `ping_every` and
`ping_timeout`, in seconds and under the config's names, sent before anything else can be. It reaches
exactly the clients it applies to and is never out of date, as a login reply could be. Later features
are new optional fields.

**`[tcp]` configures every connection,** WebSocket and REST, through `socket2`, as far as staying
cross-platform allows, with Linux the main target. **`send_timeout` (15 s) closes a connection when the
client has taken no bytes for that long**, while a slow one still taking data stays. It is a wrapper
around each socket whose timer exists only while a write is stuck, since the operating system's own send
timeout does nothing on a non-blocking socket. A full outgoing buffer or a shutdown still ends a stuck
connection at once. `user_timeout` is Linux's `TCP_USER_TIMEOUT`, unset by default, and elsewhere startup
warns and runs without it. `nodelay` is on by default. `keepalive_idle`, `keepalive_interval`,
`keepalive_retries`, `send_buffer` and `receive_buffer` are unset, leaving the operating system's values.
`backlog` is 1024, what the server used before it was configurable, as it has no operating system default.

**Every envelope frame has a `type`, and a request's `ref` is a ULID the client makes.** The server
holds it in memory only for that request and sends it back in its answer, so requests and replies
match over one socket: a `send` gets its sequence id this way, and a `sync` its pages. **The
server never stores a client's ULID, and it is not in the signed header.** What is saved goes by
its sequence id, from 1. REST carries the same bodies. A client sends `send`,
`seen`, `received`, `typing`, `sync`, `group_info`, `wraps`, `profile_keys`, `call_event`, `times`, `draft`, `folder_id`, `chat_folders`, `presence`, `presence_of`, `kv`, `kv_set` and `ping`. The server sends `features`, `sent`, `done`,
`refused`, `message`, `chats`, `welcome`, `typing`, `marks`, `revoked`, `call_event`, `times`, `draft`, `folder_id`, `chat_folders`, `profile`, `profile_keys`, `presence`, `presence_of` and `kv`. `group_info` and `wraps` are not
messages, so they take no sequence id and stay out of history, and what has no sequence id is
answered with `done`. **`typing` and `presence` are events, not requests, chosen by the author:** they
carry no `ref` and get no answer. Binary fields are standard base64 with padding, through `base64ct`, which is
already in the tree and constant-time. Kinds and modes are the schema's numbers. **A commit is a `send` with kind 3 plus its wraps, welcomes,
epoch and group info**, so there is one path into the task. A separate type would add a second path, for
something stored as a message anyway. Later parts add their own types.

**Not replaying a duplicate is the client's job, and the server does not deduplicate.**
If the connection drops before the sequence id arrives, the client reconnects and gets
the updates, which include this message if it was stored. It finds a match for its
pending message in the queue, by client send time and contents, and does not send
again. If there is no match, it resends. A message still in flight on the old connection can
arrive after the client looked, and then be stored twice under two sequence ids. **Removing such a
duplicate is a client's job and is not built now.** If bad networks show it happening, it gets fixed
then: a copy has the same device, send time and body.

**A message stores both the client's send time and the server's save time.** The
sequence id decides order, not the client's clock.

**A conversation has one sequence, assigned by the server, starting at 1 and growing.**
Whatever is saved to show later gets the next number. Whatever is not saved gets none.

### Replies, times, drafts, questions and stickers

- **A reply is a property inside the encrypted plaintext, chosen by the author:** `{"text": "...", "reply": <seq>}`.
  The signed header's `pointer_seq` stays empty for content, so the server never sees the reply graph. The queue
  holds a reply to a pending message until that message has its sequence id.
- **A reaction's plaintext is `{"emoji": "👍", "on": <seq>}`, chosen by the author**, its pointer inside
  the encryption like a reply's. **Core refuses an emoji the `emojis` crate does not know**, when sending
  and when opening, so a reaction is always one real emoji. **A person may put several on one message,
  chosen by the author**, each its own message, removed by a delete pointing at it. **Clients add them up
  from their own archive, chosen by the author:** core hands each reaction over opened, and the platform
  groups them by `on`, leaving out removed ones, **one chip per exact emoji, chosen by the author**, so 👍
  and 👍🏽 are two.
- **A message's times are server times only, chosen by the author.** A `times` frame {ref, chat, seq} is
  answered with `times` {ref, chat, seq, member_list: [{account, received_at?, seen_at?}]}, and REST has the same
  at `GET /api/v1/chats/{id}/messages/{seq}/times`. It needs a registered device and membership, and lists
  every member but the sender, keeping only what the asker's permissions show: see Groups and channels'
  four times permissions. Someone who may see neither time on that message gets `no_permission`. An unknown
  message is `not_found`, and a stranger gets `not_member`. **The answer comes in pages of 50 members, chosen by
  the author**, so a large group or channel never answers with every member at once: **ordered seen first, newest
  first, then received only, newest first, then neither, each by account id within the same time, chosen by the
  author.** **The next page is asked with `after`, the last account listed, chosen by the author:** `times {ref,
  chat, seq, after?}` and `GET .../times?after=<account>`, the server working out that account's place from its
  own times on the message, and the answer gains `more`. Core's `Command::Times` takes `after?` and
  `Event::Times` carries `more`. Android's times sheet in a group or channel is one row per member and asks for
  the next page when scrolled to its end. **In a channel, subscribers keep the `.own` pair and owners and
  moderators the `.others` pair, as in a group, chosen by the author** when paging was asked.
- **Seen is sent only while the list is at its end, chosen by the author.** A message arriving while the
  reader is scrolled up does not move the view or send `seen`: a down button with a count shows it.
- **A draft belongs to the conversation, not to messages, chosen by the author.** It is one blob per account
  and chat, sealed by the client under the chat's key at the epoch the blob names, so the server never reads
  it. It sits in three columns of `member` (`draft`, `draft_epoch`, `draft_at`) and syncs to the account's
  devices. It holds text, plus the message being replied to or edited. **It travels both ways, chosen by the
  author:** a `draft` frame over the WebSocket, answered with `done`, and `PUT /api/v1/chats/{id}/draft`, with
  none to clear it. The server sends the other online devices of the account a `draft` frame, and every `chats`
  entry carries the account's own in `draft`. It needs membership only, takes no sequence id and no chat task,
  and is limited by `[limits.body] draft`, 64 KiB. **A device sends it 5 seconds after typing stops, chosen by
  the author**, and clears it when the message is sent or the text emptied. A draft is best effort: core queues
  nothing, so one that never reached the server is replaced by the next change. Its associated data is the label
  `keryx v1 draft`, the chat and the epoch, so it cannot pass for a message, and it carries no signature.
- **Two new content kinds, `question` (10) and `answer` (11), chosen by the author**, each with its own body
  limit and permission, `message.send.question` and `message.send.answer`. **Both permissions work like every
  other kind's, chosen by the author:** member, moderator and admin hold them. Clients simply have no way to
  send a question, now or later: an AI or agent sends them. Nothing refuses one from a client. Both are
  encrypted and signed like text, and both count as unread and move the chat up, like text.
- **A question message is one tree, chosen by the author.** Each question is a choice (at most 8 options,
  single or multi-select) or a number (with a range), and every question also gets a "write your own" option.
  Which question shows next depends on the picked options, a number's range, or the written-in answer.
  Skipping, for the whole form or for some questions, is itself an answer.
- **Its plaintext, chosen by the author:** `{"text"?, "first", "nodes": [...]}`. `text` is an optional intro and
  `first` names the starting node. A node is `{"id", "text", "choice": {"multi", "options": [{"id", "text"}]},
  "next"}` or the same with `"number": {"min", "max", "step"?, "unit"?}` instead of `choice`. `next` is a list
  of rules tried in order, and the first match wins: `{"picked": [...], "to"}` for a choice's options,
  `{"range": [low, high], "to"}` for a number, `{"own": true, "to"}` for written-in text, `{"skipped": true,
  "to"}` for a skip, and `{"to"}` alone for anything. No match, or `"to": null`, ends the form. Core refuses a
  tree with a cycle, an unknown id, or a node nothing reaches.
- **An answer's plaintext, chosen by the author:** `{"question": <seq>, "answers": [...]}`, the pointer
  encrypted like a reply's. `answers` follows the path actually taken: `{"node", "picked"?: [...], "own"?,
  "number"?}`, or `{"node", "skipped": true}` for a skipped question, whose path goes on by a rule with
  `skipped` or one with no condition. Skipping the whole form is `{"question": <seq>, "skipped": true}` with no
  answers. **Any question can be skipped**, since a skip is an answer.
- **Limits, fixed in core, chosen by the author:** at most 32 nodes, a path at most 16 long, 1 to 8 options
  per choice, a question's and an option's text at most 500 characters, written-in text at most 2,000. A number's
  `min` and `max` are required, `step` is optional with 1 as its default and 0.5 allowing halves, `unit` is
  optional, and all are decimals. `[limits.body] question` is 64 KiB and `answer` 16 KiB.
- **An answer points at its question and is one live answer per account, chosen by the author.** Answering
  again means removing the old answer, which is an ordinary delete. An answer can be edited. The answer
  message always shows the question and each answer. On a question, tapping it shows a "show answer" key that
  moves the chat to the answer. **In groups and channels each account answers once**, as Groups and channels says.
- **A question with an answer cannot be edited, enforced by clients only, chosen by the author**, so the
  pointer stays encrypted and the server learns no answer graph. It can be deleted, and then its answer shows
  "Message unavailable" for the question.
- **Previews, chosen by the author:** a question shows its intro, or its first question without one, an answer
  shows "Answered" and a sticker "Sticker".
- **Android answers one question at a time, chosen by the author.** The question bubble shows its intro and an
  Answer button, which opens a sheet with one question at a time along the branches: options as checkboxes or
  radios, a number field with its range and unit, "Write your own", and Skip, Skip all, Back and Next. The last
  step shows a summary and Send. Once answered, the question's menu has Show answer, which scrolls to it.
  **Editing an answer reopens the same sheet filled in**, and Send sends an edit.
- **A question reaches the phone for testing from `server/examples/drive.rs`, chosen by the author:** a small
  program driving core as a real client, committed, with a command that logs in as a test account and sends a
  sample tree to a chat, until an agent exists.

### Stickers

- **A sticker is content kind 12, and its message names one sticker kept on the server, chosen by the
  author.** Its plaintext is `{"sticker": "<id>"}`, sealed and signed like text, so neither the server nor a
  stolen database sees which sticker was sent; the server still sees the kind, and could guess from who fetches
  which image. A reply goes inside as with text. It counts as unread and moves the chat up like text, and
  **cannot be edited**, like a reaction: the server refuses with `cannot_edit`. Its body limit is
  `[limits.body] sticker`, 1 KiB, and sending one needs `message.send.sticker`, which member, moderator and
  admin hold.
- **The server keeps stickers in named packs, which everyone with a registered device can list and use,
  chosen by the author.** A new server permission, `sticker.manage`, which only admin holds by default, adds and
  removes packs and stickers. A sticker carries one optional emoji, for search later and unused for now.
- **A sticker is a static or animated WebP of at most 512x512 and `[limits.upload] sticker`, 256 KiB, chosen
  by the author.** It is not encrypted, since everyone on the server can see it. The server checks its RIFF and
  WebP header and reads its size from the VP8, VP8L or VP8X chunk in `server/src/file.rs`, never decoding its
  pixels, and refuses anything else with `bad_sticker`. The bytes live under `[files] directory`, in a
  `stickers` folder, a file per sticker named by its id.
- **Its routes, chosen by the author.** A registered device lists and fetches; `sticker.manage` adds and
  removes. A sticker's bytes never change, as an id is never reused, so they are sent as immutable:

  ```
  GET  /api/v1/stickers                      device or sticker.manage -> pack_list{id, name, sticker_list{id, emoji?, width, height, size}}
  GET  /api/v1/stickers/{id}                 device -> the WebP bytes
  POST /api/v1/sticker_packs                 sticker.manage: name -> pack
  DEL  /api/v1/sticker_packs/{id}            sticker.manage: the pack and its stickers
  POST /api/v1/sticker_packs/{id}/stickers   sticker.manage: the raw WebP, ?emoji= -> sticker
  DEL  /api/v1/stickers/{id}                 sticker.manage
  ```

- **Operators add stickers from the command line, chosen by the author:** `keryx sticker pack add <name>`,
  `keryx sticker add <pack id> <file.webp> [--emoji]`, `keryx sticker list`, and the matching remove commands,
  over REST like `user add`. **`GET /api/v1/stickers` also admits `sticker.manage`**, which the operator's
  token holds, so `list` can print each pack's id and name with its stickers' ids and emoji. **A pack's name is
  unique on the server, 1 to 64 characters, chosen by the author**, so two packs never look the same in a
  picker, and a taken one is refused with `name_taken`.
  **Android sends and shows them, and has no management screens:** a sticker button by the text box opens the
  packs, a sticker shows in the chat at 160 dp, and one removed from the server shows "Sticker unavailable".
- **A `Default` pack is bundled inside the server's source and served to every client, chosen by the
  author.** It is **Bit**, a pixel robot in 15 poses by Godspeed Mission Control, from
  `github.com/MichaelZelbel/hermes-telegram-stickers` under CC0, so no credit is owed, picked by the author
  from free sets after getstickerpack.com's `stickthing-2` could not be downloaded. They are 512x512 PNGs
  **converted once to lossless WebP, about 1.3 KB each, and committed in `server/src/stickers/`, chosen by
  the author**, compiled in with `include_bytes!`. **Each has a fixed ULID written in `server/src/file.rs`,
  and so does the pack, chosen by the author**, never to change, since a sticker message names its sticker
  by id. **Each carries the first of the two emoji its source gives, chosen by the author.**
  - **It lives in memory only, never in the database, chosen by the author:** `GET /api/v1/stickers` adds
    it **after the operator's packs, named `Default`, chosen by the author**, and `GET /api/v1/stickers/{id}`
    answers its ids from memory. `sticker.manage` cannot add to it or remove it or its stickers, which are
    answered `no_such_pack` and `no_such_sticker` as an unknown id is. **No operator's pack may be named
    `Default`, whether the bundled one is on or off, chosen by the author:** `name_taken`.
  - **`[stickers] default_pack = false` stops listing it, chosen by the author**; unset means listed.
    **Turned off, its bytes are still served, chosen by the author**, so old messages keep showing their
    stickers while nobody can send new ones.
  - **The server checks each one at startup with the same check an upload gets, chosen by the author**,
    `[limits.upload] sticker` included, and a failure stops startup, naming the file.
  - **`keryx sticker list` marks it `(built in)`, chosen by the author**, so the operator knows its ids
    cannot be removed.

### Suggestions

**An agent can suggest what to do with a chat, and the person accepts or declines it, all chosen by the
author.** The first action is a folder: an agent that makes a group for a task suggests the folder it
belongs in, so it arranges the person's folders under the person's own permissions. Clients show
suggestions and cannot build them, as with questions.

- **Two new content kinds, `suggestion` (13) and `ack` (14), chosen by the author**, each with its own
  permission, `message.send.suggestion` and `message.send.ack`, which member, moderator and admin hold, as
  for questions, in groups too. Both are encrypted and signed like text. Their limits are `[limits.body]
  suggestion = "4KiB"` and `ack = "1KiB"`. **Neither can be edited, chosen by the author:** the server
  refuses with `cannot_edit`, as for stickers, so an ack always agrees to what it saw. Deleting your ack,
  which is always allowed, brings the buttons back, and does not undo the folder change. **Android does not
  offer it, chosen by the author:** see below.
- **A suggestion is general, with one action per message, chosen by the author:** a later action is a new
  key beside `folder`, not a new kind. **Its plaintext, chosen by the author:** `{"text"?, "folder":
  {"name", "move"}}`, exactly one action key. `text` is an optional message of at most 500 characters,
  shown above the buttons. `name` is 1 to 32 characters, the clients' limit for a folder's name, which core
  checks. **A suggestion always means the chat it is sent in, chosen by the author.**
- **An action this client does not know is opened, with no buttons, chosen by the author:** core hands it
  over with its text and an unknown action, and the client says it needs a newer keryx.
- **Copy adds the chat to the folder, creating it when needed. Move also takes it out of every other
  folder but `all`, chosen by the author.** **Pins follow the folders' rules, chosen by the author:** a
  move drops the chat's pins in the folders it leaves, it goes into the target folder unpinned, by its
  time, and a chat already there keeps its pin there.
- **The folder is found by its name folded as search folds, chosen by the author:** trimmed, case
  ignored, and Arabic and Persian forms, digits, ZWNJ and the short vowel marks matched as in Search, so an
  agent writing Arabic ي does not make a twin of a Persian folder. When several match, the first in tab
  order is used.
- **A folder made by accepting goes last among the tabs, chosen by the author**, as one made from the
  sheet does, so the rest of the arrangement never shifts.
- **The ack's plaintext, chosen by the author:** `{"suggestion": <seq>, "accepted": true|false,
  "created"?: bool, "left"?: <count>}`, the pointer encrypted like an answer's. **It says what changed,
  chosen by the author:** `created` when the folder was made for it, and `left` the number of folders a
  move took the chat out of, never their names, since in a group every member reads it. **A decline holds
  only `suggestion` and `accepted`, chosen by the author**, since it changed nothing.
- **Declining sends an ack too, chosen by the author**, so every device of the account and the agent know
  it was handled, and the buttons go everywhere.
- **Accepting runs in core, chosen by the author**, so every client does it the same way:
  `AcceptSuggestion {chat, seq, suggestion}`, with the suggestion as the platform shows it, as `SendAnswer`
  takes its question, chosen by the author, finds the folder, builds the new arrangement, saves it as `Folders` does
  and waits for its `done`, then sends the ack; `DeclineSuggestion {chat, seq}` sends the ack alone. A new folder needs its id
  from the server first, so accepting one that makes a folder needs a connection. A
  folder change that is refused, such as `too_many_folders`, sends no ack, and the client shows why with
  the buttons still there. Matching by name makes accepting again change nothing, so an ack lost before
  it was sent costs only a second tap.
- **The buttons show only on suggestions another account sent, and go once this account has an ack,
  chosen by the author**, as Answer does.
- **In a group each account acks for itself, chosen by the author**, and every member sees each ack. Groups
  are not built yet, so this runs in direct chats until they are.
- **A suggestion counts like text, and an ack is quiet, chosen by the author:** a suggestion counts as
  unread, moves the chat up and is the preview, its text or what it suggests. An ack does not count, does
  not move the chat and does not change the preview, so a group is not bumped by each person's ack.
- **Android folds the ack into the suggestion's bubble, chosen by the author:** no bubble of its own, and
  under the suggestion's text its state, such as "Accepted: moved to Apollo" or "You declined", in a group
  yours and a count of the others. Drawn in `tmp/design/suggestions.html` before it is built.
- **The bubble's label is "Suggested folder", in Persian پوشهٔ پیشنهادی, chosen by the author** over
  "Folder suggestion", since it names the folder written right under it. An action this client does not
  know is labelled "Suggestion".
- **The design is approved as drawn in `tmp/design/suggestions.html`, chosen by the author:** a top row
  with the folder icon, the label and the time; the text; the folder's name with a "Copy" or "Move" pill;
  Decline and Accept in one row, which becomes the state at the same height: "Copied to Apollo", "Moved to
  Apollo" or "You declined", with a clock until the server has the ack. Its Persian texts are as drafted there.
  **Your own decline reads "You declined", in Persian «شما رد کردید», and the other person's, on a
  suggestion your account sent, reads "Babak declined", «بابک رد کرد», chosen by the author** after the
  phone test showed a bare "Declined".
  **A move's state does not say how many folders the chat left, chosen by the author**, keeping it on one
  row. **Tapping the state does nothing of its own, chosen by the author**, as any bubble's text.
- **Once you accepted, the folder's pill says Open and tapping the folder's row opens the chat list on
  that folder's tab, as tapping it in the tab row does; once you declined, the row is the folder's name
  alone and a tap does what tapping the bubble does, chosen by the author** after the phone test showed
  Copy and Move still there on handled suggestions. A suggestion your own account sent keeps its Copy or
  Move pill, since the folder is the other person's. The pill's word sits in its middle.
- **Accepting a move asks first only when the chat would leave other folders, chosen by the author:** a
  move that takes it out of no folder but `all` stays one tap. **Its words, chosen by the author as drawn:**
  "Move this chat to Lunch?", "It leaves your other folders and stays in All.", Cancel and Move, in Persian
  «این گفتگو به Lunch منتقل شود؟», «از پوشه‌های دیگرتان بیرون می‌رود و در «همه» می‌ماند.», لغو and انتقال.
- **Android tells whether the chat would leave other folders with core's own fold, over JNI, chosen by the
  author:** `Core.fold`, so the question and core always agree on which folder is meant.
- **A refusal other than too many folders or no connection shows the folder sheet's own words, chosen by
  the author:** its lines for `no_permission`, `unknown_folder` and any other refusal. The red line stays
  until Accept or Decline is tapped again, or the chat closes.
- **Android does not offer deleting an ack, chosen by the author:** once accepted or declined, the state
  stays. **The bubble's menu is a question's, chosen by the author:** Reply, and Delete on a suggestion of
  your own, with no Copy and no Edit.
- **Built server and core first, tested by the server's integration tests driving core, then Android on
  the author's phone, chosen by the author**, with `server/examples/drive.rs`'s `suggest move|copy
  <folder> [text]` sending one and printing the ack it gets back.

### Groups and channels

**Being asked now, part by part.** What Removal, Profiles, Read state and Unencrypted messages already say
about groups and channels stands; this section adds what the author decides as the Q/A goes.

- **Groups and channels are designed and built together, chosen by the author**, so what a channel is
  compared with a group is settled in this Q/A.
- **Creating a group needs a new server permission, now `group.create`, chosen by the author**, held by
  member, moderator and admin as creating a direct chat is, so an operator can turn it off for some people.
- **A person gets into a group by being added by a member, or by asking to join and being let in, chosen by
  the author.** No invite links for now.
- **A group has at most `[limits] group_members = 200` member accounts, chosen by the author**, changeable
  in `server.toml`, which keeps a key change's wraps and every device's wrap check small until larger
  groups are measured.
- **A channel has at most `[limits] channel_members = 1000` member accounts, chosen by the author**,
  encrypted like a group, so a key change's wraps stay about 64 KB; larger channels wait for a measured
  design.
- **An owner can mark a group or channel findable, chosen by the author:** its name and bio then become
  readable to the server, and it shows in the directory search beside accounts, where anyone can ask to
  join it. One not marked stays hidden, its profile sealed as Profiles says. **Changing it is pushed to
  the chat's online devices as a `findable` frame `{chat, findable, name?, bio?}`, chosen by the author**,
  and offline ones read it in their next `chats` list. **Core hands it on as `Event::Findable {chat,
  findable?: {name, bio?}}`, chosen by the author**, the item a `chats` entry carries, none meaning no
  longer findable.
- **Adding people and letting join requests in is a chat permission, `group.member.add` and `channel.member.add`, held by the chat's
  owner and moderator roles and not by plain members, chosen by the author**, so owners can change it per
  chat later.
- **Groups and channels have threads of both kinds, built in this same build, chosen by the author:**
  topics, which split a chat into named message lists, and per-message threads, where any message can
  open a small chat under it, shown in the main list with its reply count.
- **A channel is a broadcast whose people talk in threads, chosen by the author:** only owners and those
  with a post permission write in the main list; subscribers read, react, and write only inside a post's
  thread. In a group every member writes in the main list too.
- **A thread's pointer is readable, chosen by the author:** a message in a thread carries the seq of the
  message that opened it in the signed header, as delete and edit pointers are, so the server enforces a
  channel's rule, counts replies and can sync one thread. It learns which messages have threads and how
  busy they are, never their text. Reply and reaction pointers stay encrypted.
- **A message's topic is readable, chosen by the author:** the topic's id is in the signed header, so the
  server keeps a seen position and unread count per topic and can limit a role to some topics. Topic names
  stay sealed under the chat's key.
- **Topics are switched on or off per group and per channel, chosen by the author:** `server.toml` holds
  the default for new ones, the client lets the creator choose when making one, and it can be turned on or
  off later. **Channels have topics too, chosen by the author**, each a broadcast list with its own threads.
  **The default is off, `[chat] topics = false`, chosen by the author.**
- **A topic is like a small group inside its group, chosen by the author:** its own profile, settings and
  moderators, each answered as follows. **The group's switch turning topics on and off stays, chosen by the
  author.**
  - **Its profile is a photo, a bio of at most 140 characters and up to 3 links, as a group's, chosen by the
    author**, the photo shown in place of its coloured letter. **It is one sealed blob per topic under the group's
    profile key, with its own frame and route, chosen by the author**, so a topic's moderator edits it without
    rewriting the group's profile; topic names stay in the group's sealed topic list.
  - **Each topic has its own value for reactions, stickers, files, slow mode, who may write and the four times
    permissions, starting from the group's, chosen by the author**; anything else, such as adding people, stays
    group-wide. **Who may write is everyone, or only moderators in the topic's main list, members still reacting
    and talking in threads, as in a channel, chosen by the author.**
  - **A topic can be closed, which freezes it, chosen by the author:** nobody but moderators writes, reacts or
    replies in its threads, it stays readable, and the list shows a lock.
  - **A topic can be hidden, chosen by the author:** only group owners, group moderators and that topic's
    moderators still see it, and the server stops delivering and syncing its messages to everyone else until it
    is shown again.
  - **Each topic has its own moderators, appointed by group owners, chosen by the author;** a topic moderator
    moderates that topic only, and group owners and moderators stay moderators in every topic.
  - **A topic's moderators, group moderators and group owners change its settings, chosen by the author:** its
    options, closing, hiding, its photo, bio and links.
  - **A group or channel and each of its topics have their own rules, chosen by the author**, useful once agents
    take part: at most 2,000 characters, counted as Unicode characters as a bio is, never bytes, checked by core.
    **They sit in the same sealed profile as the bio, as `rules`, chosen by the author**, so the server never reads
    them and whoever may edit the bio edits them.
  - **General gets all of it but hiding, chosen by the author**, as it cannot be removed either.
  - **Each topic has its own history, chosen by the author:** everything done in it, its profile, options,
    closing, hiding, its moderators, and every delete and edit of a message in it. **Its moderators and the
    group's owners and moderators read it, chosen by the author.** **The group's own history page shows
    group-wide rows only, and a topic's rows show on that topic's page, chosen by the author**, so neither
    list is cluttered.
  - **The wire, schema and permissions, approved by the author:**
    - `topic` gains `closed`, `hidden`, `write` (0 everyone, 1 moderators only in the main list), `slow_mode` and the
      topic's sealed profile: `sealed`, `signature`, `photo_id`, `updated_by`, `version`, `updated_at`. **General's
      settings live in a `topic` row under the all-zero ULID, made with the chat**, while its messages still carry no
      topic in their header.
    - `topic_permission (conversation_id, topic, permission, allowed)` overrides a topic on top of the chat's own
      override, for the reaction, sticker and file kinds and the four times permissions; `topic_moderator
      (conversation_id, topic, account_id)`.
    - **`group.topic.edit` and `channel.topic.edit`**, held by the owner and moderator roles, cover a topic's profile,
      options, closing and hiding; a topic's moderators hold them in that topic only, with deleting others' messages,
      writing when it is closed, the slow-mode exemption and seeing everyone's times there. Appointing them is
      `member.role`, owners'.
    - Frames, each answered with `done`, pushed live and caught up through `chats`: `topic_profile {ref, chat, topic,
      sealed, signature, photo?}` and `PUT /chats/{id}/topics/{topic}/profile`, sealed under the group's profile key with
      `keryx v1 topic profile`, the chat and the topic as associated data, its photo one of the chat's files;
      `topic_options {ref, chat, topic, allow, deny, slow_mode, write, closed, hidden}` and `PUT
      /chats/{id}/topics/{topic}/options`; `topic_role {ref, chat, topic, account, role}` and `PUT
      /chats/{id}/topics/{topic}/members/{account}/role`.
    - Each `chats` entry gains `topic_list: [{topic, closed, hidden, write, slow_mode, allow, deny, mod_list,
      profile_version}]`, leaving hidden topics out for those who may not see them; showing a hidden topic again makes
      core sync that chat, as removing one does.
    - Refusals: `topic_closed` for writing, reacting or replying in a closed topic, `moderators_only` for writing in a
      moderators-only topic's main list, and a hidden topic answered as one that does not exist.
    - Each change writes a history row, and none writes a membership line.
    - Core: `TopicProfile`, `TopicPhoto`, `TopicPhotoGet`, `TopicOptions`, `TopicRole`; events `TopicProfile`,
      `TopicPhoto`, `TopicOptions`. `drive.rs`: `topicbio`, `topicphoto`, `topicopts`, `topicmod`.
    - **As built, settled while building and open to the author's change:** each `chats` entry's list is
      `topic_setting_list`, General's first, and carries each topic's sealed profile itself rather than a
      version; a change is pushed as one `topic` frame `{chat, topic, ...options, mod_list, profile?}`, and
      to those who may not see a hidden topic as `{chat, topic, hidden: true}` alone; **it goes to the
      device that made the change too, chosen by the author** after the phone test showed a closed
      topic with no lock on the phone that closed it, so that device shows the server's own copy; a reaction carries
      its message's `topic` and `thread_seq` in its signed header, so a closed topic and a topic's own
      reaction rule can be enforced; `chat_event` gains `topic_id`, the topic whose history a row is in;
      when a topic shows again, core syncs the chat from its start and opens that topic's withheld
      messages and every edit, though they sit below the last sequence id it knew; a topic's rules are
      checked by core at 2,000 characters, as a chat's are. `drive.rs`'s `topicbio <text> [-- rules]`
      and `chatprofile <name> [-- rules]` set rules.
    - **Creating a topic shows on the group's History page as "added a topic" and on the topic's own page
      as "created the topic", and the topic's first profile, set by the account that added it, writes no
      "edited" row, chosen by the author** after the phone test showed "pari edited the topic" on a topic
      New topic had just made. A first profile set later by someone else still writes "edited".
    - **A topic's slow mode is "same as group", off, or one of the group's steps, chosen by the author**
      after the phone test showed a topic could not turn slow mode off while its group had it on:
      `topic_options`' `slow_mode` is none for the group's own and 0 for off.
    - **The history's copies sit in one `copy` column on `chat_event`, chosen by the author** over a table
      of their own: the JSON list of message frames the wire carries, NULL for every other row, removed
      with its row. `ChatEvent` gains `copy_list`, and action 20, edited; core opens and checks each copy and hands it on in `Event::ChatEvent`'s `copy_list`, never
      the ciphertext. A deleted file message's copy keeps its name, type and size, while its bytes still
      go with the delete. **A History page read from the archive has core open its copies, chosen by the author:**
      `Command::OpenHistory {chat, event_list}`, the platform handing over the rows it read and core
      answering with one `Event::ChatEvent` per row, its copies opened, so nothing opened is ever written
      to disk.
  - **Built as before, chosen by the author:** the wire, schema and permission names written here for the
    author's approval first, the topic list and page drawn in `tmp/design/groups.html`, approved as drawn by the author, then the
    server and core with their tests and `server/examples/drive.rs`, then Android.
- **Turning topics off shows every message in one list by its seq, and each keeps its topic id, chosen by
  the author**, so turning them on again puts everything back where it was.
- **Turning topics on or off, and making, renaming and removing topics, is a chat permission,
  `group.topic.manage` and `channel.topic.manage`, held by the owner and moderator roles, chosen by the author.**
- **Removing a topic deletes it and all its messages and threads for everyone, after a warning, chosen by
  the author**, wiped as a delete wipes. General cannot be removed. **No delete is sent for the wiped
  messages, so core syncs that chat as soon as a topic is gone, chosen by the author:** when a pushed
  `topics` list drops one, and on the remover's own device once its `topics` frame is answered, so the
  wiped rows come back empty and every online archive drops them at once.
- **Threads are one level deep, chosen by the author:** only a main-list message opens a thread, and inside
  one, replies quote as they do today.
- **Each thread keeps its own seen position, chosen by the author:** the message that opened it shows the
  thread's unread count, and the chat's unread number counts main-list messages only.
- **This build has the built-in chat roles only, chosen by the author:** owner, moderator and member, with
  owners making someone a moderator or an owner. Custom roles and a screen to manage them stay open.
- **A moderator holds, beyond a member, chosen by the author:** `member.add` and `topic.manage`, removing
  members who are not owners, deleting others' messages except owners', and posting in a channel's main
  list. **Owners and moderators see a history of who did what, chosen by the author.**
- **Every delete and edit in a group or channel goes into the history with what it removed or changed,
  chosen by the author**, people's own included, since a row saying only "someone removed a message" means
  nothing. **The server keeps the original, chosen by the author:** before a delete wipes a message, it
  copies the message and its edits, ciphertext, header and signature, into the history row, and an edit's
  row holds the message and every edit so far, so before and after both show. Owners and moderators open
  them on their devices with the chat keys they hold, and the author's signature proves who wrote them, so
  nobody can fake what was removed. **Direct chats stay as they are, chosen by the author:** they have no
  history, and a delete there still wipes for good.
- **The history is rows the server writes, chosen by the author:** each add, removal, role change, delete
  of someone else's message and topic change, with the actor, the action, the target account or seq and
  the time, readable as a call's joins are, so a modified client cannot leave one out. Only owners and
  moderators can read them. **They are kept under the chat's own retention, chosen by the author**, so
  unset means for as long as the chat. This is a chat's history, apart from server-wide refusals and
  management, which stay log lines with no audit table.
- **Every member sees membership changes as lines in the chat, chosen by the author:** added, removed,
  left, let in by request, made moderator or owner, such as "Sara added Babak". Deletes of others'
  messages are in the history only.
- **A membership line is a message, chosen by the author:** the chat's task writes it in the same step as
  the action, taking the chat's next seq, readable and written by the server as the self chat's new-device
  notice is. So it can be replied to, reacted to, removed and opened as a thread like any message.
  **Owners and moderators remove it, through `message.delete.others`, chosen by the author**, since it is
  nobody's own. **It counts as unread, moves the chat up and is the preview, like text, chosen by the
  author**, except for the account whose action it was, as its own messages are. **It carries no topic,
  so it shows in General, chosen by the author.** `chat_event` keeps only the history and lines for one
  account, such as the inviter told of a decline. **It is a message kind of its own, `Line` = 5 beside
  `DeviceNotice` = 4, chosen by the author:** unencrypted, with no content kind, so no permission and no
  body limit, and a client sending one is refused with `server_only`, as the notice is. **Clients trust
  the server for it, chosen by the author**, as for roles, leaving and removals: it carries no signature,
  and its device is the actor's. A breached server could write a false line, but could not really add
  anyone, since MLS still guards who gets in. **One action writes one line, however many people it names,
  chosen by the author**, such as "Sara added Babak and Mina". **Its body is readable JSON with the action
  as a word, chosen by the author:** `{"action", "account_list"?}`, the action one of `created`, `added`,
  `let_in`, `joined`, `removed`, `left`, `made_owner`, `made_moderator` and `made_member`, the actor being
  its device's account. **Every one of those writes a line, "created" too, chosen by the author**, so a
  new group's first message says who made it. **Channels get no lines, chosen by the author:** a channel's
  main list holds posts only, and its joins, leaves and role changes go to the history alone. **Each
  action also writes its history row, chosen by the author**, which owners and moderators read, so
  removing a line never erases the history. **Built in the server and core:** core hands each line on as
  `Event::Line`.
- **A join request may carry a short note, and the person asking is told the outcome, chosen by the
  author.** Anyone holding `member.add` sees the pending requests and lets each in or declines it. A
  declined person may ask again.
- **Making a group or channel needs only a name, chosen by the author:** a photo, a bio, up to 3 links,
  the people in it and the chat's options are optional.
- **A chat's options in this build, chosen by the author:** whether reactions are allowed, whether
  stickers are allowed, whether files and media are allowed, and whether plain members may add people.
  **Each is kept as a per-chat override of the member role, chosen by the author:** the chat stores
  permission names added to and removed from its member role, as activeplano's per-organization override,
  so the server's one check enforces every option and a later one is just a permission name. **Changing
  them after the chat is made is a chat permission, `group.options.edit` and `channel.options.edit`, held by owners only, chosen by the
  author**, since options change what moderators and members may do.
- **Unencrypted is a property of a message, not of a chat, chosen by the author**, in groups and channels
  as everywhere: see Unencrypted messages.
- **Clients trust the server for roles, leaving and removals, and guard only who gets in, chosen by the
  author** over keeping roles inside MLS, which would have been about half of core's MLS work for groups.
  Owners and moderators live on the server alone, as `member.is_owner` and the member roles, and changing
  one is a plain frame, not a key change. Clients accept any member's add or removal commit, since the
  server checks its permission, and could cut anyone off by not delivering anyway. **The one check kept:**
  an account not already in the group gets in only through a member's add commit, or by an external join
  carrying an invitation signed by a device already in the group, so a breached server cannot slip in an
  account of its own to read a group or channel.
- **Commits are sent as MLS public messages and the server checks what each adds and removes, chosen by
  the author**, so a plain member's modified client cannot quietly take someone out of the group, who
  could then neither read nor rejoin by themselves. Only the handshake goes in the clear: message content
  is keryx's own AES-GCM anyway, and the update path stays HPKE-encrypted. The server, which already links
  openmls through `keryx-core` and keeps each epoch's group info, maps each added and removed leaf to its
  device and refuses a commit whose adds are not the action's accounts or devices of accounts already in
  the chat, or whose removals are not the action's accounts, ended devices or accounts that left. It
  learns nothing it did not already know. It costs one parse per commit in the chat's task.
  **The server keeps openmls's public group for each chat, chosen by the author:** it is built once from
  epoch 0's group info, while every key in it is new, and moved on by each accepted commit, in a
  `public_group` column on `mls_group`. It is never rebuilt from group info, because openmls checks
  every member's key lifetime when it builds one, at most 12 weeks, and a member quiet for longer would
  fail that. openmls also checks each commit's signature and shape. It costs about 0.3 KB per member,
  rewritten on each commit. **A refused commit gets `bad_commit`, chosen by the author**, a 403 of its own.
  **Every chat type's commits are checked, chosen by the author**, so there is one path. **A device
  joining by itself must be the device sending the commit, chosen by the author**, so a device only
  ever joins as itself. Epoch 0's group info that is not the chat's own is `bad_message`.
- **Content sealed under an older key than the chat's current one is refused with `stale_epoch`, in every
  chat type, chosen by the author:** core applies the commits it missed and seals it again. Nothing new
  is ever sealed under a key a removed person still holds, and someone who joins reads everything after
  their join, at the cost of one more round trip during the race.
- **Leaving takes effect on the server at once, and members commit the MLS removal, chosen by the
  author:** no more delivery, out of the account's folders, and a "left" line, then the next online
  member's device commits the removal, the first winning, as a terminated device is removed today, with
  no signed statement, since clients trust the server for removals.
- **Whoever adds someone, or lets a request in, does the MLS work on their own device, chosen by the
  author:** a key package claimed for each of the new account's devices, the add committed with its wraps
  and the chat's profile key, as starting a direct chat does. A device with no key package left joins by
  itself later.
- **A group's or channel's profile is sealed under its own profile key, chosen by the author**, so
  someone joining by invitation reads it, and every later change, with nobody else online. The key is 16
  random bytes its creator makes, kept in each device's snapshot, and **it never changes, chosen by the
  author**, as an account's profile key does not: the server stops sending the profile to someone who
  left, so they could read later changes only from a breached server's copy. The profile is AES-GCM under
  it with `keryx v1 chat profile` and the chat as associated data, signed by its device over those and
  the sealed bytes. **The key itself travels sealed at an epoch**, under that epoch's key with `keryx v1
  chat profile key`, the chat and the epoch as associated data: at epoch 0 with the first profile, in
  each add commit at the epoch it moves to, and in the commit of a device joining by invitation, which
  read the key in its invitation. The server keeps the newest share, which every member holds the key
  of, since every way in shares it again, and hands it out beside the profile.
- **The first profile, its key and the options travel inside epoch 0's `group_info`, and an add's key
  share inside its commit, chosen by the author**, each saved in the same step, so a crash between two
  requests can never leave new members, or a whole new group, without a name. After a commit carrying
  a share, the server pushes the profile with the new share, as it pushes profile keys, and to the
  joining device too when it is a join. It costs about 45 bytes per add commit.
- **The claim refuses first, chosen by the author:** claiming key packages for an add names the chat,
  and the server answers `invite_needed` for an account whose switch says nobody before handing out any
  of its packages, so an invitation-only account's one-use packages are never wasted.
- **Every member sees the member list, channel subscribers too, chosen by the author** over hiding a
  channel's subscribers from each other: every device is a leaf in the channel's MLS group, and the tree
  every member and joiner holds names each leaf's account and device, so hiding it would only pretend.
  "This is the thing for security, and people using secure messaging should accept it." The apps show it.
- **The member list with roles comes from its own route, chosen by the author:** `GET
  /api/v1/chats/{id}/members` and a `members` frame `{ref, chat}`, answered with `members` `{ref, chat,
  member_list: [{account, role}]}`, for any member. Core's `Command::Members {chat}` hands back
  `Event::Members`, and the platform keeps it current from the role and membership lines. It is asked
  only when the screen opens, so `sync` stays light.
- **Questions work in groups and channels, one answer per account, chosen by the author:** each member
  answers for themselves, the question shows how many answered, and its sender sees each answer.
- **Android's screens are drawn first in `tmp/design/groups.html` and approved before any Android code,
  chosen by the author, and approved as drawn by the author:** the create sheet, the chat with topics and threads, the members and requests
  screens and the history, in English and Persian.
- **Built server and core first, tested by the server's integration tests driving core with
  `server/examples/drive.rs` as the other side, then Android on the author's phone, chosen by the
  author.**
- **`server/examples/drive.rs` gets a command for everything groups and channels do, chosen by the
  author**, so it can be every other side of the phone test.
- **A person can refuse to be added straight in, chosen by the author:** a switch in their account says
  whether others may add them. With it off, being added becomes an invitation they see inside the chat
  they are invited to, and accepting it joins. **Its choices are Everyone and Nobody, Everyone by default,
  chosen by the author:** Everyone puts an added person straight in, and Nobody turns every add into an
  invitation. **It is `kv` keys the server reads, left unsealed because the server acts on them, chosen by
  the author**, one for groups and one for channels, named below, as the `kv` design foresaw, synced to every device like `lang` and `theme`.
- **An invitation shows who invited them and the group's name and photo, sealed to the invited account,
  chosen by the author:** the inviter's device seals them so the server still sees neither.
- **It is sealed to a new invite key, chosen by the author**, since a running device drops the account
  key and could never open anything sealed to it: one more HKDF label from the words, `keryx v1 invite
  key`, giving an X25519 key that running devices keep in their snapshot, as the profile key is kept. The
  master key stamps its public half, over `keryx v1 invite key`, the account id and the key, and it is
  published with the account's keys through `PUT /api/v1/account/keys` and `GET /api/v1/accounts/{id}`. A
  stolen phone can open invitations only, never history. One HPKE seal per invitation, with the label,
  the chat and the account as its info.
- **The invitation shows the photo's BlurHash only, chosen by the author:** its sealed part is the group's
  name and the photo's BlurHash, and the photo itself shows once they join, so who may read a chat's
  files does not change. **It also carries the chat's profile key, chosen by the author**, which opens
  the whole profile once they join: JSON `{name, blurhash?, key, note?}`, HPKE-sealed as its X25519 KEM output
  then its ciphertext. The inviter checks the invite key's stamp against the pinned master key first,
  and an invitation waits for the account's keys when they are not known yet. **An invitation may carry
  a note from the inviter, sealed in that part, chosen by the author**, so the server never reads it, at
  most 500 characters, checked by core. `AddMembers` takes one `note?` for every invitation it turns
  into, and `Event::Invitation` shows it.
- **Accepting an invitation lets the invited device join by itself, by external commit stamped by its
  master key, chosen by the author:** the server checks the invitation, clients check that the inviting
  device was in the group, and nobody else must be online.
- **A chat keeps one sequence across its topics and threads, chosen by the author:** every message takes
  the chat's next seq, so pointers, sync, deletes and the archive stay as they are, and a topic or thread
  is a filter on them, with gaps inside one topic normal.
- **A topic's id is a ULID the server issues first, as a folder's is, and General has none, chosen by the
  author**, so its sealed name can name it, and a chat with topics off needs nothing in its headers.
- **Seen positions per topic and per thread are sparse, chosen by the author:** written only when a
  member reads there, so a thread nobody opened costs nothing. They are rows in the existing `seen` and
  `received` tables, as the schema below says.
- **`sync` still returns the whole chat, every topic and thread, chosen by the author**, so the archive
  holds all of it and search finds thread text too. Topics and threads are views on the archive.
- **Someone removed by an owner or moderator sees what someone who left sees, chosen by the author:** the
  chat leaves their list, and rejoining later shows what they had before.
- **Who sees when each member got and saw a message is four chat permissions per chat type, chosen by the
  author:** `message.view_received.own` and `message.view_seen.own` for your own messages, and
  `message.view_received.others` and `message.view_seen.others` for everyone else's, under `direct.`,
  `group.` and `channel.`, replacing `message.view_seen`. **By default, as the rule was, chosen by the
  author:** every role holds the `.own` pair in every chat type; in a direct chat members hold the
  `.others` pair too, and in groups and channels owners and moderators do. A chat's options can add or
  take away any of the four for its members. A `times` answer keeps only the times the asker may see, and
  someone who may see neither on that message is refused with `no_permission`.
- **Mentions and slow mode are in this build, chosen by the author**, pinned messages not.
- **A mention lives inside the ciphertext, chosen by the author:** `@username` in the encrypted text, with
  the mentioned accounts listed inside too, so the server never learns who was mentioned. The client marks
  a message mentioning you, and the chat list shows an @ badge.
- **Slow mode is a chat option under `options.edit`, enforced by the server, chosen by the author:** off,
  10 s, 30 s, 1 m, 5 m, 15 m or 1 h between one member's messages in the main list or a topic, not in
  threads. Owners and moderators are exempt. An earlier send is refused with `slow_mode` and the seconds
  left. **Each topic, General included, keeps its own last-sent time per member, chosen by the author**
  after the phone test, so a topic's slow mode counts only messages in that topic. **Core hands the seconds on in `Event::Failed`'s optional `seconds`, chosen by the author**, so an
  app can count down.
- **Typing shows names in a group, in its topic or thread, such as "Sara is typing" or "Sara and 2
  others", and a channel shows typing only inside a thread, chosen by the author.**
- **Every permission used inside a chat is split by chat type, chosen by the author:** `direct.*`,
  `group.*` and `channel.*`, such as `direct.message.send.text`, `group.message.send.text` and
  `channel.message.send.text`, for every in-chat one, the existing `message.*` included. Server-wide ones,
  `folder.*`, `profile.edit`, `presence.*`, `sticker.manage` and the like, stay as they are. **The names,
  chosen by the author:** each of `direct.`, `group.` and `channel.` has `message.send.text` through
  `message.send.ack`, `message.send.unencrypted`, `message.commit`, `message.edit.own` and
  `message.delete.own`, `message.view_received.own`, `message.view_seen.own`,
  `message.view_received.others` and `message.view_seen.others`; `group.` and `channel.` also have
  `member.add`, `member.remove`, `member.role`, `message.delete.others`, `topic.manage`, `profile.edit`
  and `options.edit`; and
  `channel.message.post` is writing in a channel's main list. So `group.member.add` and
  `channel.member.add` are what this section calls `member.add`, and so on.
- **Creating chats is `direct.create`, `group.create` and `channel.create`, chosen by the author**, each
  held by member, moderator and admin; `chat.create_direct` is renamed and `chat.create_group` becomes
  `group.create`.
- **Only owners make or unmake moderators and owners, another owner included, chosen by the author**, and
  the last owner cannot step down without making someone else owner first.
- **The "may others add me" switch is two `kv` keys, `privacy.add.group` and `privacy.add.channel`,
  chosen by the author**, each `"everyone"` or `"nobody"`, everyone by default.
- **Direct chats stay as they are, chosen by the author:** topics, threads, mentions, slow mode, options
  and roles are for groups and channels only. Sections above still write the unprefixed names, such as
  `message.send.call`; in a chat they mean that chat type's prefixed one.
- **A channel's main-list post shows the channel's name, chosen by the author**; owners and moderators
  still see who posted in the history, and subscribers' thread comments show their own names.
- **A group or channel has at most `[limits] topics = 100`, General not counted, chosen by the author**,
  and a topic's name is 1 to 32 characters, as a folder's.
- **The chat list's row for a chat with topics adds up every topic's unread main-list messages, and its
  preview is the newest message in any topic with its topic's name before it, chosen by the author.**
- **Any owner may remove a whole group or channel for everyone, with no named permission, chosen by the
  author**, the server checking `is_owner`. Admin can too, as admin passes every check.
- **A findable chat's photo stays sealed, chosen by the author:** the directory shows its name's letter
  until the person joins.
- **A pending join request is a row in the asker's chat list, chosen by the author:** the chat's name and
  "Request sent", which they can open to cancel it. It becomes the chat when they are let in, or says
  declined. **Asking and taking it back push `join_request` to every device of the asker, the asking one
  too, chosen by the author**, so the asking device's row gets the name the server keeps. **Removing the
  chat deletes its join requests and invitations and pushes `join_request` with no request, and
  `invitation` with none, to each asker's and invitee's devices, chosen by the author**, so the row goes;
  offline ones no longer find it in `asked_list` or `invitation_list`.
- **Owners and moderators who were offline learn of pending join requests from a count on each chat, chosen by
  the author:** each `chats` entry gains `request_count`, set only for accounts that may let people in, and the
  requests themselves are fetched when the Requests page opens. Online ones already get each `join_request` live.
  Built in the server, with 0 sent too, so a device back from being away knows when none wait.
- **Invitations and join requests expire after `[limits] invitation_lifetime = "30d"`, chosen by the
  author**, changeable in `server.toml`. An expired one goes from both sides, and the person can be invited
  or ask again. **It is a duration, not a count of days, and 0 stops startup, chosen by the author**, as
  the ping keys do: a count of days let 0 mean every invitation expires the moment it is saved, and a
  duration lets the tests use `"1s"` with no test-only key.
- **Core shows an invitation or a request gone when a first `chats` page leaves it out, chosen by the
  author:** it keeps those it showed, in the snapshot too, and sends `Invitation` with none and `Asked`
  gone for each missing one, so one that expired, or whose chat was removed while the device was away,
  never stays on screen. An invitation being accepted stays, as its join still carries the signature.
  **Nothing tells an online device the moment one expires, chosen by the author:** the server has no
  timer, so the row goes at the next reconnect, and accepting it meanwhile is refused with
  `no_invitation`.
- **When an invited person declines, the inviter alone sees a line, such as "Babak declined your
  invitation", chosen by the author**; other members see nothing.
- **The signed header gains two optional fields after `call_seq`, `topic` (a 16-byte ULID) and
  `thread_seq`, each with the presence byte the others use, chosen by the author.** No version moves, as
  there is no production, and `core/tests/protocol.rs` pins the new bytes. Mentions stay inside the
  ciphertext.
- **Creating one extends `POST /api/v1/chats`, chosen by the author:** `{type: "group"|"channel",
  topics?, findable?}` -> `{chat}`. The creator's device builds the MLS group and uploads epoch 0's group
  info as a direct chat's start does, then adds the chosen people in one add commit, with the sealed
  profile and options.
- **A membership change that touches MLS is a commit carrying a readable action, chosen by the author:**
  a commit (`send`, kind 3) gains `action` `{add|remove, account_list}`, letting a request in
  being an add. A commit that only finishes in MLS what a leave or an ended device already did carries
  none. The chat's task checks the permission and the targets, saves the commit, updates the
  member rows and writes the history row and the line in the same step, so there is one path, as commits
  already have.
- **The history and the membership lines were one `chat_event` table, chosen by the author**, until lines
  became messages, above: a row per
  action, with the chat, `n`, the actor, the action, the target, the seq where one applies, the server
  time and whether everyone or only owners and moderators see it, sent live as a `chat_event` frame and
  through `sync` as call rows are. Clients draw lines from the ones they may see, and retention is the
  chat's.
- **A join request's note is plain text the server can read, at most 140 characters, chosen by the
  author**, and the client says so under the box, as it is meant for outsiders like a findable chat's
  name.
- **Topic names live inside the chat's sealed profile, chosen by the author:** one blob with the name,
  bio, links, photo and the topic list `{id, name, place}`, under the chat's profile key, so a new member
  reads every topic name. Changing topics rewrites the blob.
- **Actions that do not touch MLS each have a frame answered with `done` and a matching REST route,
  chosen by the author**, as folders and `kv` do: leaving, a join request and its answer, an invitation
  and its answer, role changes, topic changes, options and findable. The server pushes the result to the online devices
  it concerns, and offline ones get it at `sync`.
- **A mention's plaintext is `{"text", "mentions": [account]}`, chosen by the author:** the text holds
  `@username` as typed and `mentions` the account ids, so a later username change still points at the
  right person. Core checks that each is a member.
- **The frames and routes, chosen by the author as listed.** Each frame is answered with `done` unless
  noted, with its REST route beside it, all under `/api/v1`:

  ```
  POST /chats {type, topics?, findable?} -> chat
  chat_remove {ref, chat}                     DEL  /chats/{id}                       owners
  leave {ref, chat}                           POST /chats/{id}/leave
  chat_role {ref, chat, account, role}        PUT  /chats/{id}/members/{account}/role   owners
  join_request {ref, chat, note?}             POST /chats/{id}/requests
  join_cancel {ref, chat}                     DEL  /chats/{id}/requests
  join_decline {ref, chat, account}           POST /chats/{id}/requests/{account}/decline
                                              GET  /chats/{id}/requests  member.add -> request_list
  invite {ref, chat, account, sealed}         POST /chats/{id}/invitations
  members {ref, chat} -> members {ref, chat, member_list}   GET /chats/{id}/members
  invite_answer {ref, chat, accept}           POST /chats/{id}/invitation
  topic_id {ref, chat} -> topic_id {ref, chat, topic}   POST /chats/{id}/topics/id
  topics {ref, chat, on, topic_list}          PUT  /chats/{id}/topics
  chat_profile {ref, chat, sealed, signature, photo?}  PUT  /chats/{id}/profile
  chat_options {ref, chat, allow, deny, slow_mode}      PUT /chats/{id}/options
  findable {ref, chat, findable, name?, bio?} PUT  /chats/{id}/findable
  ```

  Letting a request in is a commit with action `add`. An add naming someone whose switch is `nobody` is
  refused with `invite_needed`, and the client sends `invite` instead. `GET /api/v1/accounts?search=`
  also lists findable chats. The server pushes `chat_event`, `topics`, `chat_profile`, `chat_options`,
  `join_request` and `invitation`.
- **The schema changes, chosen by the author**, with no migrations, the database deleted and made anew:
  `conversation` loses `mode` and gains `topics_on`, `findable`, `find_name`, `find_bio` and `slow_mode`
  (seconds, NULL for off); `message` gains `topic_id` and `thread_seq`. **Slow mode's last-sent times are a
  table, `slow_sent (conversation_id, account_id, topic_id, at)`, General under the all-zero ULID as its
  `topic` row is, chosen by the author** in place of `member.last_sent_at`, so each topic keeps its own. New tables: `topic (conversation_id, id, issued_at, removed)`, its names living in the sealed
  profile; `chat_permission (conversation_id, permission, allowed)`, the member role's per-chat override;
  **no table of its own for topics' and threads' positions, chosen by the author:** the existing `seen`
  and `received` tables gain `kind` and `place` columns, empty for the main list, so a topic's or
  thread's marks are rows in the same two tables and its positions are read from them; `join_request (conversation_id, account_id, note, at)`;
  `invitation (conversation_id, account_id, inviter, sealed, at)`; and `chat_event (conversation_id, n,
  actor, action, target?, seq?, audience, viewer?, server_at)`. `is_owner` stays, and a moderator is a
  `member_role` row.
- **Core's commands and events, chosen by the author as listed**, core doing the MLS work, sealing and
  queueing, and the platforms showing it: `CreateChat {type, name, bio?, link_list?, photo?,
  account_list, topics, findable, options}`, `AddMembers {chat, account_list, note?}`, which sends `invite`
  itself for anyone refused with `invite_needed`, `RemoveMembers {chat, account_list}`, `SetRole {chat,
  account, role}`, `Leave`, `RemoveChat`, `AskToJoin {chat, note?}`, `CancelAsk`, `LetIn {chat,
  account}`, `DeclineAsk {chat, account}`, `Requests {chat}`, `AnswerInvite {chat, accept}`, `TopicId`,
  `Topics {chat, on, topic_list}`, `ChatProfile`, `ChatPhoto`, `ChatPhotoGet`, `ChatOptions`, `Findable` and `Members`, with
  `SendText` and the other sends gaining `topic?`, `thread?` and `mentions?`. Events: `ChatEvent`,
  `Requests`, `Invitation {chat, invitation?: {type, inviter, name?, blurhash?, note?, at}}`, with no
  `invitation` once it is gone and no `name` when its sealed part does not open, `Topics`,
  `ChatProfile`, `ChatPhoto`, `ChatOptions`, `Members` and `Asked {chat, name?, state, at}`, kept as built by the author, and
  `chats` entries gain the type, this account's role, `topics_on` and unread per place.
- **Changing a group's or channel's name, photo, bio and links is a chat permission, `group.profile.edit` and `channel.profile.edit`,
  held by the owner and moderator roles, with an account's limits, chosen by the author:** a name of 1 to
  64 characters after trimming, a bio of at most 140, at most 3 links, and one 640x640 JPEG of at most
  1 MiB. **An edited profile is pushed to the device that saved it too, chosen by the author** after the
  phone test showed Edit group's saved rules missing from the saving phone until it restarted, so that
  device shows the server's copy, as a topic's change does.
- **Calls stay refused in groups and channels in this build, chosen by the author**, with `call_not_allowed`;
  group calls get their own Q/A after it.
- **The chat-scope built-in roles become owner, moderator and member, chosen by the author**, replacing
  the chat-scope admin. Every member of any chat holds member, a `member_role` row adds moderator, and
  `is_owner` gives owner. `chat_permission` overrides the member role only.
- **Roles hold the names of their own scope, chosen by the author:** server roles only server-wide names,
  such as `group.create`, `folder.*` and `profile.edit`, and chat roles only the `direct.`, `group.` and
  `channel.` ones. Admin still passes every check.
- **`conversation_profile` keeps only `sealed`, `key`, `key_epoch`, `photo_id`, `signature`, `updated_by`,
  `version` and `updated_at`, the key being the newest share of the chat's profile key, and
  `conversation_link` goes, chosen by the author**, since no chat is unencrypted any more. A findable chat's
  readable name and bio sit on `conversation`.
- **A group's or channel's photo goes up as one of the chat's files, chosen by the author:** `POST
  /api/v1/chats/{id}/files` with kind image, kept by the `chat_profile` that names it in `persist`, the old
  one removed once no profile names it, under `[limits.upload] photo`.
- **Creating with a photo uploads it before people are added, chosen by the author**, so the first
  profile everyone gets already names it. **If it fails, the group is still made without it, and the
  person is only shown the error, chosen by the author**, `photo_failed`; they set it again later from the
  group's settings, with `Command::ChatPhoto {chat, jpeg, thumbnail}`, which mirrors the account's
  `SetPhoto`. A photo the server refuses at the start, such as one over `[limits.upload] photo`, counts
  as failing too: core seals the first profile again without it and sends epoch 0 again.
- **Apps get a group's or channel's photo as they get an account's, chosen by the author:** the opened
  profile shows `photo {id, blurhash}`, and `Command::ChatPhotoGet {chat}` fetches its one chunk from
  `GET /api/v1/files/{id}/chunks/0`, opens it in core and answers with `Event::ChatPhoto {chat, photo,
  bytes}`, so the photo's key never leaves core.
- **An invitation is signed, chosen by the author:** the inviter's device signs `keryx v1 invite`, the
  chat and the invited account, and the joiner puts it in its external commit's authenticated data, JSON
  like every plaintext: `{"invite": {account, device, signature}}`. Clients accept that external join from
  an account not yet in the chat only when the signing device is in the group at that epoch.
- **A `chat_event` row has an `audience` and a `viewer`, chosen by the author**, replacing `owners_only`:
  0 everyone, 1 owners and moderators, 2 the one account in `viewer`, such as the inviter told of a
  decline. **0 is now for live frames only, never saved, chosen by the author:** a whole chat removed, and
  each membership change, pushed to the chat's online devices and the person removed, so every device, a
  channel's subscribers too, keeps its member list current; offline ones catch up from the chats list and
  the device list.
- **Pending requests and invitations come in the first `chats` page, chosen by the author:** `asked_list`
  `[{chat, name, at, state}]` and `invitation_list` `[{chat, inviter, sealed, signature, at}]`, and live as
  `join_request` and `invitation`.

### Read state

- **`seen` is not per message.** A client sends one at some point in time, and it covers
  everything from the previous `seen` up to that point.
- **A `seen` carries "seen up to N", and the server keeps the highest.** Without it, a
  message stored just before the `seen` arrives would be marked seen though nobody saw
  it, and a late `seen` from another device would do the same.
- **Only a `seen` moves the seen position, chosen by the author.** Sending does not: a reply from a
  notification, or an agent sending for a person, is not reading the chat.
- **Every chat type keeps one position per member**: `member(chat, user, joined_seq,
  seen_seq)`, instead of a row per message per member. Since a `seen` always clears
  everything up to a point, one number holds the same information. Who has not seen
  message N is the members with `seen_seq < N` and `joined_seq <= N`, except its sender.
  In measurement, per-message rows held SQLite's single writer for 162 ms per post in a
  100,000-member channel, stalling every chat, and grew by idle members times messages.
- **Every member's seen time is available for every message.** A fixed time shared by
  many messages is fine, so one time per `seen` is enough. **One table holds it for
  direct, group and channel chats**: one row per `seen`, with chat, user, device, "up
  to N" and the times. A member saw message N at the time on their first row reaching N. A `seen`
  uses no sequence number and stays out of history pages. Rows cost about 30 bytes, do
  not grow with messages or idle members, and are deleted once their messages expire.
  **A member who leaves keeps their seen times**, so older messages do not lose them.
- **A `seen` stores the client's time and the server's time**, like messages do. Show
  the client's time unless it is later than the server's or far from it, so reading
  offline is accurate and a lying client stays detectable.
- **Who can see received and seen times is four permissions per chat type**, as Groups and channels
  says: by default, in a direct chat both people see all of them, and in a group or channel each
  sender sees them on their own messages, and owners and moderators on everyone's.
- **There is no switch to turn receipts off, chosen by the author.** Who sees the times is the
  permissions' alone, and the server stores them either way.
- **Delivery works like `seen`, chosen by the author for the web client's ticks.** A device that now
  holds messages sends `received` with "received up to N", and the server keeps the highest per
  member, beside `seen_seq`. It is real delivery, not the server handing a message to a socket, and
  it survives a reload. **Received times are kept like seen times**, one row per `received` with the
  client's and the server's time, so a message's info shows every timing detail to whoever RBAC
  lets read it. That is one more record of when each person was reachable. **It mirrors `seen`, chosen
  by the author:** a `received_seq` column on `member`, a `received` table with the same columns as
  `seen`, and a `received` frame {ref, chat, up_to_seq, client_received_at} answered with `done`.
- **Senders learn positions through `marks`, chosen by the author.** A client sends `received` with
  its chat and "up to N", as it does `seen`. When a member's received or seen position rises, the
  server sends `marks` with the chat, the account, `received_seq` and `seen_seq` to the online
  devices of that chat's other members. Each entry of the `chats` list carries the other members'
  positions too, so a reload draws the marks at once. **The entry's field is `mark_list`, chosen by
  the author:** `[{account, received_seq, seen_seq}]`, one per other member, the same fields as
  `marks`, and empty in a channel. No `marks` go out in a channel either.
- **Each `chats` entry carries its newest content message, chosen by the author**, as a full message
  frame, still ciphertext, so one request gives every preview. Its field is `last_content`. **A self
  chat's new-device notice counts as its last content, chosen by the author** after the phone test showed
  a later commit in Saved messages leaving "No messages yet", since a notice already moves `last_at`.
- **Each `chats` entry carries the chat's current `epoch`, chosen by the author:** the epoch of the
  last accepted commit, which the server already keeps for ordering commits. A device back from
  being away compares it with its own and catches up or rejoins at once, rather than noticing only
  when its own commit loses. It is an optional field, so the version does not change.
- **What a sender sees, as drawn by the web client.** A message in the queue, or sent but not yet
  answered with its sequence id, shows as waiting. In a direct chat it gets one mark with its
  sequence id, two once the other person's device has received it, and both marks change colour
  once they have seen it. **In a group it is drawn as in a direct chat, chosen by the author:** two marks
  once the first other member's device has received it, and both change colour at the first other
  member's `seen`. **In a channel it gets one mark, saying the server has it, shown to its sender only,
  chosen by the author**, as a channel has no `marks`.

### Devices

- **Devices are kept, and each gets its own ULID.** Messages and `seen` rows record
  which device they came from. Old signatures stay checkable because the device stays
  on record.
- **Each device stores:** platform (desktop or mobile), OS, OS version, optionally the
  web platform and its version, its first and last IP address, and a log of when it
  went online and offline.
- **The owner sees all of it. Others see what RBAC and their roles allow.**
- **The log has one row per session:** device, online time, offline time and IP.
  Reconnects within 60 seconds, fixed in code, extend the same row, so a flaky phone stays
  readable. At startup the server closes rows left open by a crash.
- **How long the log is kept is configured in `server.toml`. Unset means unlimited.**
  First and last IP stay on the device row regardless.
- **The IP comes from `X-Real-IP`, then the first address in `X-Forwarded-For`, falling back to the
  socket address.** These are what nginx and Caddy commonly set.
- **A device on the REST fallback counts as online** through its own endpoint, and when
  its message fetches and sends fall near each other, within a window set in
  `server.toml`.
- **There is a maximum number of devices.** At the limit, a new device is refused with a
  specific reason and the maximum. The client then loads the devices and
  terminates one or some of them. **Lowering the limit is enforced the same way:** with
  a limit of 8, a person with 8 devices, and the limit changed to 4, that person gets
  the same error on arriving with any device, and has to terminate 4 from an existing
  device or 5 from a new one.
  - **The limit is `max_devices` in `server.toml`**, and unset means unlimited. A role
    can override it through RBAC, and the highest of a person's roles applies.
  - **A blocked device still gets a token.** Username and password alone give it a
    session. WebSocket and REST messaging reject that session because of the limit, but
    with the token it can load the devices.
  - **Terminating a device also needs the words, from any device. Beyond setup and password
    reset, nothing else does.** So this works with no other device online,
    and a stolen password or session cookie cannot terminate anything. The words are
    proven by signing, and never sent.
  - **A password-only session sees a reduced device list:** each device's platform and
    last-active day, enough to choose what to terminate. IP addresses and session logs
    need a registered device, so a stolen password does not reveal location or activity
    history.
  - **Termination is a signed revocation that members carry out.** The server stops
    delivering to the device at once, and publishes a statement signed by the master
    key. Any member's client that comes online verifies it and removes the device from
    each chat, with the first commit winning. No particular device has to be online, and
    the server cannot fake one. Logout uses the same path, signed by the device's own
    key. Until a member removes it, a stolen device keeps its keys, so it could read new
    messages only by getting them outside normal delivery, such as a stolen copy of the
    server's data.
  - **It is enforced at the next connect.** Open connections carry on, so nobody is cut
    off mid-call, and an over-limit device can stay online as long as its connection
    lasts.
  - **Logging out terminates the device, and only works if the server confirms it.**
    Keys are wiped only after that confirmation, so a device that cannot reach the server
    stays logged in with its keys. A logged-out device can never be used again and stops
    counting towards the limit. Its record stays, for old signatures and the log.
    Devices lost to cleared browser data or closed private windows still have to be
    terminated by hand.
- **Client-reported values are claims.** Platform, OS and versions come from the device
  and can be faked, and browsers hide or freeze some of them. So can the IP, unless a
  proxy overwrites those headers: see What this costs.
- **Each device keeps two times, chosen by the author: last connected and last in the app.** Last connected
  is when it last had a live connection. Last in the app is when its person last had keryx open on screen,
  which only the device can say, since Android keeps the connection while the app is in the background. A
  person shows the newest of their devices.
  - **Two server permissions, `presence.in_app` and `presence.connected`, held by member, moderator and
    admin, chosen by the author.** `presence.in_app` covers the state too: green, red and orange. **Orange is
    shown under `presence.in_app` as well, chosen by the author**, though it tells that a device is connected.
    `presence.connected` covers only the Connected time. **Whoever holds one sees anyone's on the server, not
    only people sharing a chat, chosen by the author.** **There is no switch to hide your own for now, chosen
    by the author.**
  - **Kept in two columns on `device`, `last_connected_at` and `last_in_app_at`, chosen by the author**, not
    worked out from `device_session`, whose rows the log's retention deletes.
  - **XMPP's way, chosen by the author:** a connection counts for nothing until its device sends a `presence`
    frame `{in_app}`. **It is an event, like `typing`, with no `ref` and no answer, chosen by the author.**
    **Before that the device is not there to others, chosen by the author**, and last connected is written at
    that first frame and again when the connection ends. After the first one the server sends **one
    `presence` frame per person sharing a chat, chosen by the author**, and tells those people's announced
    devices about this one. Later frames, on every app open and close, are pushed to them the same way. **When a direct chat is made, each of its two people is sent the other's `presence` the same way, chosen by
    the author**, since they share a chat from then on and neither may change for hours. **A connection that drops while the app is open sets last in the app to when the
    server noticed, chosen by the author.** Red comes from the call socket the server already admits.
  - **`PUT /api/v1/devices/{id}/presence {in_app}` does the same over REST and answers with the list,
    chosen by the author:** `presence_list`, everyone sharing a chat.
  - **One answer per account, chosen by the author:** `presence {account, state, in_app_at?, connected_at?}`,
    `state` `online`, `call`, `away` or `offline`, the server merging the devices (in a call over online over
    away, the newest times), and leaving out what the viewer's permissions hide. It never shows how many
    devices someone has.
  - **Someone sharing no chat is asked about once, chosen by the author:** a `presence_of {ref, account}`
    frame answered with `presence_of {ref, presence}`, and `GET /api/v1/accounts/{id}/presence`. It is not
    updated live.
  - **Core sends the first `presence` on every connect, chosen by the author**, with the in-app state the
    platform last gave it, and hands others' back as `Event::Presence`.
  - **A dot on the person's photo shows their state, in the chat list and a direct chat's header, chosen by
    the author:** green while online, red while in a call, orange while away
    (connected with the app in the background, or stepped away), and no dot while offline, when the time
    they were last here or connected shows instead. **It is 10 dp on the list's 54 dp photo, 9 dp on the
    header's 44 dp, with a thin ring of the background's colour, chosen by
    the author**, growing with the photos when the author made the list and the header a little larger. **On a phone the app in the background is away; how long without input counts as away on a
    desktop or browser is decided with that client, chosen by the author.** **The profile page has no dot
    since its big photo, chosen by the author with that design:** the state's word sits under the name, over
    the photo.
  - **A direct chat's header shows only last in the app; the profile page shows both, chosen by the
    author.** Drawn in `tmp/design/presence.html`. **The words are short, chosen by the author:** online, in
    a call, away, and offline "active 4m ago", "active 09:12", "active yesterday 21:40", "active 3 Nov
    18:05", **the time always shown, in the header too, chosen by the author**, with the profile's line
    called Active, chosen by the author over "seen". **Last connected is not shown when last in the app is
    shown and is the same or later, chosen by the author**, since it then says nothing more.
  - **A fifth state, yellow "having trouble", chosen by the author in the agent Q/A of 2026-11-16:** wire
    state `trouble`, which any device may send, and the `presence` frame gains `trouble` and `text`. **Phones
    do not send it for now**; agents send it on their own, as agent/AGENTS.md says. **It ranks in a call,
    having trouble, online, away, offline, chosen by the author**, so a problem shows over a healthy
    device but never hides a call. **Its dot is yellow with a small dark !, chosen by the author**, so it
    differs from orange by shape and not by colour alone.
  - **Presence may carry a text, as XMPP's status does, chosen by the author:** at most 140 characters,
    sealed under the account's profile key, so only people sharing a chat with the account read it and
    the server, a stolen database and a `presence_of` answer do not. **A person's text goes to everyone
    who may see their presence, and an agent's text, its trouble reason, to its owner alone, chosen by
    the author**; others see yellow and "having trouble" only. Everyone sharing a chat with an agent
    holds its profile key, so that the reason reaches the owner alone rests on the server not delivering
    it, as profiles after leaving do.

### Settings kept per account (`kv`)

**Each account has a key-value store on the server, chosen by the author**, so every device shows the same
settings.

- **It holds `lang`, `theme`, the recent stickers and the recent reactions to begin with, chosen by the
  author:** `lang` `"en"` or `"fa"`, `theme` `"light"`, `"dark"` or `"system"`, `recent.stickers` sticker
  ids newest first, at most 20, and `recent.reactions` emoji newest first, at most 8. **Any key name of
  `a-z`, `0-9`, `_` and `.`, 1 to 64 long, chosen by the author**, with a count limit: `[limits] kv_keys =
  64`, and a value at most `[limits.body] kv_value = "4KiB"`.
- **Values are bytes to the server, and the client decides what is sealed, chosen by the author.** A later
  key the server must act on is one its code knows by name. **Core seals these four one by one with AES-GCM
  under the profile key, chosen by the author**, associated data `keryx v1 kv`, the account id and the key
  name, so a value cannot be moved to another key or account. The plaintext is JSON.
- **A version number, not a hash, chosen by the author**, as the cheaper check: the server bumps one
  counter per account on every change. **A device asks with its own frame on every connect, chosen by the
  author:** `kv {ref, version?}`, answered with `kv {ref, version}` when equal, and with the whole set,
  `kv_list: [{key, value}]`, and the version when not. `GET /api/v1/kv?version=N` does the same.
- **A change names only what changed, chosen by the author:** `kv_set {ref, version, set: [{key, value}],
  remove: [key]}`, or `PATCH /api/v1/kv` with the same body, answered with the new version. A key named
  twice, or broken, is `bad_body`, a value over `kv_value` `body_too_large`, and a change leaving more than
  `kv_keys` keys `too_many_keys`.
- **The server wins, chosen by the author:** a change based on an older version is refused with
  `kv_stale`, **and the refusal carries the whole set and its version, chosen by the author**, as
  `call_open` carries `call_seq`, so the device takes the server's set in one round trip.
- **The account's other online devices get only the change, chosen by the author:** `kv {from, version,
  set, remove}`. A device on `from` applies it, and one on another version asks with `kv {version}`.
- **Kept in a `kv` table, `account_id`, `key` and `value`, with `kv_version` on `account`, chosen by the
  author**, written in one transaction.
- **Core keeps the set and its version in the snapshot and asks on every connect, chosen by the author**,
  so platforms call a set command and get `Event::Kv`.
- **No permission, like drafts, chosen by the author:** any registered device of the account reads and
  changes its own.
- **After login the account's language wins, chosen by the author.** The Welcome screen's choice covers only
  the screens before login, and a new account takes it as its first value.
- **It is called `kv` on the wire and in the schema, chosen by the author.**
- **Built server and core first, tested by the server's integration tests, then Android, chosen by the
  author:** dots in the list, header and profile, the Active and Connected lines, `lang` and `theme`
  through `kv`, and a Recent row in the sticker and reaction pickers.

### The client queue

**Client SDKs keep a priority queue for outgoing messages, with priority based on
message type.** Some types go first so they reach the connection quickly:

| Level | What | Order |
|-------|------|-------|
| 0 | MLS commits, key changes | First |
| 1 | Is typing, stop typing | Next |
| 2 | Content of any kind, edit, delete | In the order sent, so no action can overtake its target |

Attachments upload outside this queue. **The queue's code is one Rust implementation in
`core/src/queue.rs`**, so every client orders messages the same way. Sockets and timers
stay on the platform side and call into it.

- **Is typing and stop typing are not saved, so they get no sequence id.**
- **A pointer holds the target's sequence id, never a ULID.** Actions stay sorted in the
  queue behind their target, and use its sequence id if the client has it, or wait to
  get it. So anything pointing at a pending message waits for that message's reply, and
  **an encrypted reply or reaction can only be encrypted when it leaves the queue**,
  since its pointer is inside the ciphertext.
- **The queue is memory only, for now.** It is lost when the tab closes, so a client must show a
  message as pending until the sequence id arrives, never as sent. A message sent just
  before closing or crashing can vanish. **A queue kept on disk is wanted later, chosen by the
  author**, so messages not yet sent are retried after the app restarts; it is on the Open list.
- **A file's message joins the queue only once its bytes are all up, chosen by the author.** Text
  sent while a large file uploads goes at once, and the file's message lands after it, at its own
  sequence id. One big upload never blocks a chat.

### Unencrypted messages

- **Unencrypted is a property of a message, not of a chat, chosen by the author.** Every chat takes
  encrypted and unencrypted messages, and each message's flag says which, so clients know. There is no
  private, mixed or open setting on a chat, no `mode` column and no signed switch. It is useful because,
  with it, the server can write messages on behalf of people.
- **Sending one needs `message.send.unencrypted`, admin's alone by default, chosen by the author**, in
  groups and channels as everywhere.
- **Unencrypted messages are always signed by the sending device**, the same as
  encrypted ones, so clients detect a server altering or forging them.
- **Every chat always has an MLS group**, so there is one code path.
- **The server may store and deliver unencrypted content, and whoever holds a named
  permission may read it**, including through admin MCP and server search. Encrypted
  content stays unreadable to the server either way. Clients must make clear when a
  message is not private.

### The first build's REST routes

WebSocket carries messages. Every route calls a `run_*` function, and secrets go only in bodies. The
directory search uses a query string, because a search term is not a secret. **Every reply is
wrapped:** success is 2xx and `{"ok": true, "data": {...}}`, and failure is 4xx or 5xx and
`{"ok": false, "code": "...", "message": "..."}`, with the same `code` and `message` as the
WebSocket's `refused`. The CLI's `--json` prints the same object.

**Who may call what.** Anyone: the guide and the sign-in routes. Any session, even one with no device:
account keys, reading an account, the device routes and sessions. A registered device under the limit:
everything that touches chats, and the WebSocket. Bodies below; binary fields are base64 and ids are
ULID text. The cookie is called `keryx_session`.

```
Anyone
GET  /api/v1                        the guide, as plain text
GET  /api/v1/public/features        -> registration, captcha, and name when one is set. Anyone, before an account.
                                    The login screen reads it and offers sign up only when
                                    registration is true
GET  /api/v1/captcha                -> id, image
POST /api/v1/register               username, password, display_name?, captcha? -> account
POST /api/v1/login                  username, password, captcha?, device?{id, challenge, signature}
                                    -> account, device?, and sets the cookie
POST /api/v1/login/token            the same -> account, device?, token, and no cookie
POST /api/v1/setup                  token, password -> account, username
POST /api/v1/challenge              username, purpose (password_reset | device_login) -> challenge, account
POST /api/v1/password/reset         username, challenge, new_password, signature

Any session
GET  /api/v1/sessions               -> session_list
DEL  /api/v1/sessions/{id}
PUT  /api/v1/account/keys           master_public, account_public, account_stamp, invite_public,
                                    invite_stamp -> self_chat
GET  /api/v1/accounts/{id}          -> id, username, display_name, and the keys with their stamps
GET  /api/v1/devices                -> device_list, reduced if the session has no device
POST /api/v1/devices/id             -> device, the id its stamp signs
POST /api/v1/devices                device, public_key, stamp, platform, os, os_version,
                                    web_platform?, web_platform_version?
POST /api/v1/devices/{id}/terminate signature
POST /api/v1/devices/{id}/logout    signature
PUT  /api/v1/devices/{id}/presence  this session's own device: in_app -> presence_list
GET  /api/v1/kv?version=            -> version, and kv_list when the version differs
PATCH /api/v1/kv                    version, set, remove -> version, or kv_stale with the whole set

A named permission
POST /api/v1/accounts               account.create: username, admin?, display_name? -> account, link
GET  /api/v1/accounts?search=       directory.search -> account_list
GET  /api/v1/accounts/{id}/presence presence.in_app or presence.connected -> account, state?,
                                    in_app_at?, connected_at?

A registered device, under the limit
POST /api/v1/key_packages           body_list -> count
POST /api/v1/accounts/{id}/key_packages/claim  -> key_package_list{device, body}
POST /api/v1/chats                  chat.create_direct: account -> chat, created. Your own id gives
                                    your self chat
GET  /api/v1/chats/{id}/group_info  member -> epoch, group_info
GET  /api/v1/chats/{id}/devices     member -> device_list, with ended devices and their statements
GET  /api/v1/chats/{id}/wraps       member -> wrap_list{epoch, account, wrapped}
POST /api/v1/chat_folders/id       folder.create -> folder, kept for this account for 5 minutes
PUT  /api/v1/chat_folders          a permission per change: folder_list, default, the whole arrangement
POST /api/v1/chats/{id}/files       member: kind, size, chunks -> file
PUT  /api/v1/files/{id}/chunks/{n}  its uploader: the encrypted chunk's raw bytes
GET  /api/v1/files/{id}             member -> size, chunks, received
GET  /api/v1/files/{id}/chunks/{n}  member, once a sent message keeps it -> the raw bytes
GET  /api/v1/ws                     cookie or Bearer, subprotocol keryx.v1
GET  /api/v1/calls/{chat}/{call_seq}/moq  member, until the call ends: MoQ over qmux
```

**The guide sits at the API's root, as in borhan**, so whoever arrives at REST can read it from REST.

### The command line

**`keryx serve` is the server, and every other subcommand is a client of it.** A subcommand tells the
server what to do and shows its reply, and never opens the database or does the work itself. So the
command line is one more client, which can use the same SDK, and everything stays consistent. **Every
operation is one unit of work**, a `run_*` function, so adapting all of them later is simple.

- **`--config <path>` is required on every command**, with no default and no environment variable. The
  log flags are optional. There is no home directory.
- **Where a subcommand connects:** `127.0.0.1:<port>` by default, with the port from `listen` in the
  config. `--remote HOST` connects to `HOST:<port>`, and `--remote HOST:PORT` to that port.
- **The scheme is `https` if the config has `[tls]`, otherwise `http`**, and `--remote` may also say
  `https://host`. On the default `127.0.0.1` the certificate is not checked, since the traffic never
  leaves the machine and a certificate is issued for a name. With `--remote` it always is, unless
  `--insecure` is given.
- **Every command accepts `--insecure`, before or after its name.** It changes nothing where no
  certificate would be checked, so a script can always pass it.
- **A subcommand proves it is the operator with `[operator] token` in `server.toml`**, as borhan does,
  sent as a `Bearer` header and never in a URL. It counts as admin, and without that key there is no
  operator access. **This token sits in plain text, by the author's choice**, so that the first admin
  can exist and no captcha gets in the way: whoever can read the file gets admin, which means accounts
  and metadata and never message content. It is the one token not stored hashed.

### First phase

The web client, then the Android client. **The web client is on hold while Android starts, chosen by
the author**, and Android follows the web client's design decisions. See [android/AGENTS.md](android/AGENTS.md).

**The first working thing is 1:1 encrypted text:** two browsers sign up, generate
their words, and send encrypted text over WebSocket, under keys from MLS. This comes first because
`openmls` in WASM and words-to-keys are the riskiest parts. The REST fallback, groups
and calls come after it.

**The first server build also has delete and edit in 1:1 chats,** with wiping and the preview walking
back. Files, profiles, retention, MCP and calls come after it. Files and calls are built since, so
`mcp.rs` is the one module on the closed list that does not exist yet.

**To do after files and calls, chosen by the author: remove the password.** It unlocks almost nothing
the words or a device key do not already cover: a new device still needs the words to register, and a
returning one already signs a challenge with its own key. It is also a hole, since "lost words: set up
new words" would let a stolen password replace the words and then add any device. Without it the
server stores no password hashes, only public keys, and there is nothing to guess. A new device would
log in by signing with the words, and a returning one by signing with its own key. Until then the
password rules below stand. **Ask the details when it starts:** what sign-up and the setup link become,
whether the captcha stays on sign-up, and how lost words are replaced, by a still-registered device's
signature or by the operator.

**`keryx` has server commands only in this phase:** `serve`, `user add`, and the `sticker` commands,
with no terminal chat. Integration tests drive `keryx-core` directly as their client.

**Multi-device is in from the start, not added later.** A 1:1 chat is an MLS group of
every device of both accounts, so adding and removing devices is part of the first
slice.

**Starting a chat needs a one-use key package for each of the other person's devices,** so it works
while they are offline. Each device keeps 10 on the server and uploads more when it comes online,
and the server hands out one per device and deletes it. A device that has run out is left out, and
joins by itself later, as its account is already in the chat. If all have run out, the chat cannot
start yet, and the person is told so.

**The server makes every id it stores.** A device's stamp signs its id, so a new device asks for one
first: the server remembers it for that account for 5 minutes, and registering uses it up, so a
client can never choose its own. A chat's id comes from `POST /api/v1/chats`, and the client then
builds its MLS group with it. If two people start the same chat at once, both get the same id, and the
first to upload epoch 0's group info wins, while the other waits for its Welcome.

**A self chat is made in the same step as the account's keys**, so a new device's notice always has
somewhere to go. **A Welcome is deleted when it is handed to a device's connection.** If that
connection dies, the device joins by external commit instead.

**A device that ends is pushed to everyone sharing a chat with it.** Their online devices get a
`revoked` frame with the signed statement, and `GET /api/v1/chats/{id}/devices` also lists ended
devices with their statements, so a client that was offline compares it with its MLS members. Whoever
commits the removal first wins.

**A new device adds itself to each chat** with an MLS external commit, proving
ownership with the master key's stamp. This has to work for someone who lost their
only device, so it cannot wait for another member to be online. The server stores each
chat's public group info for this, and other members verify the stamp.

**The server orders commits: first one wins.** It numbers each chat's commits and
accepts the first for each step. The loser gets a rejection, catches up and retries.

### Words, passwords and accounts

- **Registration is configurable in `server.toml`, and off by default**, so a new server is closed
  until its operator opens it. When allowed, a person signs
  up with a **username and password**, with an optional captcha (from a captcha
  crate, with its options in the same file). The first slice's template, with sections for calls
  and other parts added when they are built:

  ```toml
  listen = "127.0.0.1:7777"
  public_url = "http://127.0.0.1:7777"
  # name = "North"       # unset, or blank: a client shows the host, with no port
  # max_devices = 8      # unset: unlimited

  [database]
  type = "sqlite"
  path = "/var/lib/keryx/keryx.db"

  # [operator]
  # token = "..."        # what `keryx user add` and the like send. Unset: no operator access

  [registration]
  allow = false
  password_min_length = 12
  username_min_length = 3

  [session]
  lifetime = "30d"

  [chat]
  idle_after = "5m"

  [www]
  directory = "/var/lib/keryx/www"

  [files]
  directory = "/var/lib/keryx/files"   # a folder per chat, a file per upload

  [stickers]
  default_pack = true       # the bundled pack, Default. false: not listed, old messages still show theirs

  [limits]
  websocket_frame = "1MiB"
  rest_body = "1MiB"        # not file uploads
  history_page = 200
  outgoing_frames = 1024    # full: the connection closes, and the device refetches
  ping_after = "5s"         # quiet this long: the first ping
  ping_every = "15s"        # then one ping this often
  ping_timeout = "5s"       # each ping's time to answer, or the connection closes
  key_packages = 100        # the most one device may keep on the server
  call_frame = "256KiB"     # one frame on a call's state, audio or video track. Larger: that device leaves
  call_cache = "16MiB"      # the relay's room for older groups. Newest groups are always kept
  folders = 20              # an account's folders, not counting all
  kv_keys = 64              # an account's settings

  [limits.body]             # one message's body. A file's bytes upload separately
  text = "64KiB"
  reaction = "1KiB"
  voice = "16KiB"
  image = "16KiB"
  music = "16KiB"
  video = "16KiB"
  doc = "16KiB"
  file = "16KiB"
  call = "16KiB"
  media_bundle = "64KiB"
  question = "64KiB"
  answer = "16KiB"
  sticker = "1KiB"
  suggestion = "4KiB"
  ack = "1KiB"
  draft = "64KiB" # an account's sealed draft in one chat. Not a message
  folder_name = "256B"      # one sealed folder name
  kv_value = "4KiB"         # one setting's value, as the client sent it

  [limits.upload]           # one file's encrypted bytes, by the type its uploader declares
  voice = "50MiB"
  image = "20MiB"
  music = "200MiB"
  video = "2GiB"
  doc = "100MiB"
  file = "2GiB"
  sticker = "256KiB"        # one sticker's WebP. Not encrypted: everyone on the server sees it
  photo = "1MiB"            # one profile photo, sealed: an account's, or a group's or channel's

  [tcp]
  send_timeout = "15s"      # no byte taken for this long: the connection closes. WebSocket and REST
  # user_timeout = "20s"    # Linux only: unacknowledged this long, and the kernel drops the connection
  nodelay = true
  # keepalive_idle = "30s"
  # keepalive_interval = "10s"
  # keepalive_retries = 3
  # send_buffer = "256KiB"  # unset: the operating system decides. Linux doubles what it is given
  # receive_buffer = "256KiB"
  backlog = 1024

  # [tls]
  # certificate = "/path/cert.pem"
  # key = "/path/key.pem"
  ```

  **Every path in it is absolute**, and a relative one stops startup with an error naming the key, so
  nothing depends on where the server was started. **An unknown key stops startup too.** A later
  Postgres is a different `[database]`: `type = "postgres"`, `uri`, `pool_size` and
  `connection_retry_timeout`. Postgres is the answer if SQLite ever becomes too slow. Retention's key
  joins the template when retention is built.
- **`name` is optional, chosen by the author.** Set, it is the name a client shows for this server,
  returned by `GET /api/v1/public/features`. Unset or blank, the client shows the host with no port.
  It is not on the WebSocket `features` frame, which stays the ping settings.
- **There is no setup command.** The operator writes the config, and `serve` creates the database at
  `path` the first time it starts, with its tables and built-in roles.
- **When registration is off, an operator creates accounts** with
  `keryx user add <username>`, which prints a one-time link where that person sets
  their password and then their words. The operator never learns the password. **The token follows a
  `#`**, as in `https://chat.example.com/setup#<token>`, because browsers never send that part to a
  server, so no log can record it, and the client posts it in a body. The address comes from a
  `public_url` key in `server.toml`. A link lasts 7 days, fixed in code. A try says which
  it was: no such link, the link was already used, or it is older than those 7 days and was never
  used. The row stays after the password is set, so a second try can say it was used.
- **Registering and the one-time link only create the account or its password.** The client then logs
  in the normal way, so there is one way into a session, and with `[captcha]` on, a person signing up
  solves two captchas.
- **After creating the account, the person sets up their words.** The client
  generates a random BIP39 phrase, and the person picks its length (**12 or 24
  words, default 12**), writes it down and confirms it. Restoring accepts any valid
  BIP39 length. People do not hand-pick words, because human choices have far less
  randomness and hand-picked phrases are what brain-wallet crackers target. The words
  and their order produce the keys. A shutdown before they are confirmed leaves the account
  and the password. The phrase that was only on screen is not stored, and the next login
  generates a new one.
- **The words are the English BIP39 list only.** BIP39 derives keys from the literal
  text, so a Persian list would be non-standard, permanent, and full of look-alike
  and ZWNJ traps.
- **A browser login is a random session token in an `HttpOnly`, `Secure`,
  `SameSite=Strict` cookie.** Browsers cannot set headers on a WebSocket and secrets
  never go in a URL, and a cookie scripts cannot read is one XSS cannot steal. The
  server stores only the token's hash and checks `Origin` on the WebSocket upgrade against
  `public_url`, whose scheme also decides whether the cookie is `Secure`, so local plain HTTP works.
  Agents use a `Bearer` header. **There are two login routes:** `/api/v1/login` sets only the cookie,
  and `/api/v1/login/token` returns the token and sets no cookie, so a script can never read a
  browser's token. A token is 32 random bytes, stored as SHA-256, since a random token needs no slow
  hash.
- **A password is at least `[registration] password_min_length` characters, 12 by default**, and at most 1,024, so
  a huge one cannot waste server time. Any characters are allowed, and digits or symbols are not
  required, because NIST advises against such rules.
- **A username is a unique lowercase handle:** `a-z`, `0-9` and `_`, at least
  `[registration] username_min_length` characters (3 by default) and at most 32, not starting with a digit.
  `keryx user add` may break the length limit
  and nothing else. Handles are ASCII so look-alike letters cannot impersonate anyone.
  **A display name is separate:** any language including Persian, not unique, and
  visible to the server. The directory searches both.
- **Sessions slide for 30 days, configurable in `server.toml`.** Each use extends one,
  and people can see and revoke theirs. An expired session needs only the password, plus the
  returning device's signature, since the keys are still on the device.
- **The captcha is in the first slice, optional, and its options copy activeplano's**
  (`/p/test/activeplano-R/server/src/captcha.rs`, using the `captcha` crate with
  `default-features = false`). It is drawn and checked by the server, with no outside
  service, and one check uses it up, right or wrong. **It guards registration and
  password login**, and runs before any Argon2 hashing so bots cannot make the server
  burn CPU guessing passwords. Without a `[captcha]` section there is no captcha. Its
  options, with defaults:

  ```toml
  [captcha]
  length = 5
  alphabet = "digits"       # digits (1-9) | letters | mixed
  ttl = "2m"
  difficulty = "easy"       # easy | medium | hard
  style = "random"          # amelia | lucy | mila | random
  case_sensitive = false
  sweep_every = "10m"       # how often old captchas are deleted
  keep_for = "1d"           # captchas expired longer ago than this go
  # For the tests only, never in production: every captcha has this answer.
  # test_answer = "12345"
  # pass_code = "THE CODE" # accepted as well as the drawn answer. Unset: only the drawn one
  ```

  **`test_answer` is how tests pass the captcha simply**, with one static answer. The
  server warns at startup whenever it is set. **`pass_code` is accepted as well as the
  drawn answer**, still against a captcha id, and one check uses that id up either way.
  Case follows `case_sensitive`. The server warns at startup whenever it is set, and
  the code itself is not logged.
- **The password logs in. The words decrypt.** The password only authenticates to
  the server and decrypts nothing. **The words never leave the device.**
- **Words can reset a forgotten password**, by signing a challenge from the server, along with
  SHA-256 of the new password, so a captured signature cannot set a different one. A challenge lasts
  5 minutes and works once. A reset ends all of the account's sessions.
- **A returning device proves which one it is.** The password logs in, and the device also signs a
  server challenge. Otherwise a stolen password could become any device's session, with its logs and
  delivery. A terminated device cannot do this.
- **Lost words:** the person can still log in but must set up new words, and old
  history is gone. No one can recover it, including the operator.
- **How words become keys, which can never change once accounts exist:** the standard BIP39
  seed (PBKDF2 of the words, with no passphrase), then one HKDF-SHA256 step per key. The label
  `keryx v1 master key` gives the Ed25519 master key that stamps devices, and
  `keryx v1 account key` gives the X25519 account key that epoch keys are wrapped to, through
  HPKE's `DeriveKeyPair`. `keryx v1 invite key` gives the X25519 key invitations are sealed to, the same
  way: see Groups and channels. Another key later is just another label. Wallet-style SLIP-0010 paths
  were rejected: more to audit, and keryx has no use for a tree of keys. Words are handled by
  the `bip39` crate rather than by hand, as a one-byte bug here locks people out.
- **The same words work on every device, through one master key.** The words derive
  one master key from the standard BIP39 seed. Each device creates its own random key,
  and the master key signs it as belonging to the account. A stolen device gives up
  that device's key only. Stolen words let an attacker add a device, which contacts
  can be shown. A password alone cannot add one. Device keys are deliberately not
  computed from the words, because that lets stolen words silently recreate any
  device, and a reused device number breaks MLS.
- **A client pins an account's master public key the first time it sees it**, warns loudly if it
  ever changes, and shows a safety number two people can compare in person. A key-transparency
  log was rejected as too much to build, with few outside auditors on a single server.
- **There is no BIP39 passphrase.** The words alone restore everything, so a person has
  one thing to keep safe.
- **Everyone has a chat with themselves, and a new device is announced twice:** as a
  notice in each chat it joins, and in the person's self chat, where **the new device
  cannot remove it**. Stolen words look exactly like a new device, so the real owner
  has to be able to see it even if the attacker is the one holding the new device.
  Removing it follows the 7-day rule under Removal. **The server writes the self chat's notice
  as an unencrypted event**, in the same step that registers the device, so a thief's modified client cannot
  skip it. It reveals nothing new, since the server already knows a device joined. The
  message's sender is the new device, which is how the server applies that rule, and it carries
  the device's master stamp, so a server cannot invent one. Clients accept this one kind of unencrypted event in a self chat, and it holds
  no free text. Each other chat's notice comes from the device's signed MLS join.
- **A browser keeps its keys in IndexedDB between visits, one database per username.**
  The database is named for that username. There is no production data, so a database that has
  no username is deleted and never read. The username is saved when the account is created and
  again after a login. After the words are typed once, the derived device keys and MLS state stay
  there and the words themselves are not stored. More than one account can stay on the browser.
  Switching to another keeps the one left behind. Logging out wipes the account that is logging out.
- **Device state is saved as a whole snapshot, chosen for safety by the author.** openmls is
  synchronous and works in memory. `core/` hands back one blob holding all of a device's MLS
  state, so a save lands completely or not at all, unlike many small writes. Each snapshot has a
  version, a generation number and a checksum. **Saves alternate between two slots**, so a torn
  or failed write cannot destroy the only copy, and loading takes the newest slot that checks
  out. **A client saves before it sends:** a commit stays pending until its snapshot is saved, and
  a failed save means the commit is never sent. Every failure has its own typed error: corrupt,
  unknown version, nothing usable. A device that had to fall back to the older slot may be
  behind, and rejoins by external commit. Writing grows with the number of chats, so revisit
  this if measurement shows it.
- **Users find each other through a searchable directory** of partial usernames.
  Anyone logged in can list the community's users, accepted so people are easy to
  find.

### Restoring history

**A new device gets everything the account's other devices have, using only the
words.** No other device has to approve it, or even be online or still exist. This
is a hard requirement from the author, so losing or keeping history must never depend
on another device's cooperation.

MLS alone cannot do this: a device joining starts at the current epoch, and a message
key is deleted once used. So keryx uses MLS to agree on keys, and the messages table
stays readable through those keys:

- **MLS manages membership and produces one key per epoch**, through its exporter.
  Message content is AES-GCM under that key, signed by the sending device, rather than
  an MLS application message. That is what lets a device decrypt in any order and more
  than once, as calls and files already do.
- **The master key signs the account public key**, over `keryx v1 account key`, account id and
  the key, and clients check that against the pinned master key before wrapping anything.
  Otherwise a malicious server could swap in its own key and be sent every chat's keys.
- **Whoever changes a chat's key encrypts the new one to each member account.** The
  words also derive an account public key. Each wrapped key is about 64 bytes and the
  server stores it. Nothing goes missing even if all of an account's devices were
  offline when the key changed, which a keyring uploaded by devices could not promise.
- **A new device types the words, unwraps every key and reads the messages table.**
  There is no separate per-user archive: the messages table is the one copy.
- **Running devices drop the words' keys after joining** and learn new keys through
  MLS, so a stolen device cannot unwrap history or add a device.
- **"Everything" means everything retention has not deleted.** An old device may show
  messages a new one cannot. Unset retention means unlimited, so by default nothing
  is lost, and whoever sets a limit chooses this knowingly.

This is in the first slice, because the message format cannot be retrofitted.

**The signed statements the server and members check, which are permanent:** each has its own label,
so a signature for one can never be used as another. Terminating is one-way, so repeating it changes
nothing and it needs no time or nonce.

- **Terminating a device**, signed by the master key: `keryx v1 revoke device`, account id, device id.
- **Logging out**, signed by the device's own key: `keryx v1 logout`, account id, device id.
- **Resetting a password**, signed by the master key: `keryx v1 password reset`, account id, the
  server's 32-byte challenge, SHA-256 of the new password.
- **A returning device**, signed by the device's own key: `keryx v1 device login`, account id, device
  id, the server's 32-byte challenge.

**The message format:**

- **The epoch key** is the MLS exporter with label `keryx v1 message key`, the chat id as context,
  and 16 bytes.
- **An encrypted body** is a version byte, a random 12-byte nonce, then AES-128-GCM ciphertext and
  its tag: 29 bytes of overhead. AES-128 matches the MLS suite's strength, and a larger key would
  add nothing while the rest stays at 128 bits. Nonces are random because many devices share one
  key and cannot coordinate counters. That is safe up to about 4 billion messages per key.
- **A fixed binary header is the AES associated data, and the device signs the header plus the
  body.** The header is `keryx v1 message`, chat id, device id, kind, content kind, encrypted
  flag, epoch, `pointer_seq`, `call_seq` and client send time. Clients check the signature before
  decrypting, with public keys only. Unencrypted messages use the same layout.
- **The plaintext is JSON and is not padded.** Simplicity and storage were chosen over hiding
  length, so the server and a stolen database see each message's exact length.
- **One random Ed25519 device key signs both MLS and messages**, kept apart by their labels. The
  master key stamps `keryx v1 device stamp`, account id, device id, device public key. The MLS
  credential is the account id, device id, master public key and stamp, 128 bytes, so any member
  can verify a device with only the pinned master key.
- **Rotation:** a device about to send commits first if the epoch key is over 7 days old, so a leak
  exposes about a week of a busy chat and an idle chat costs nothing. The 7 days are fixed in
  `core/`, because a server setting could be used to turn rotation off, and sealing a message under
  an older key is refused, so rotation cannot be skipped.
- **Pinned master keys and every readable epoch key live in the device's snapshot.**
- **Lifetime checks are skipped when joining**, because a chat idle for 84 days must still be
  joinable. Whether a device belongs is decided by its stamp.
- **A chat's group info goes up inside the commit and is saved in the same step, chosen by the
  author.** It used to go up after the server accepted the commit, and a committer that crashed or
  reloaded in between left the old group info on the server for good: every later external join was
  built on the wrong epoch, lost with `commit_lost` and retried forever. openmls builds the new group
  info with the commit, so this costs no extra work. A start's epoch 0 still goes up as `group_info`
  on its own, and the same bytes sent again are accepted, since that is a resend whose answer was
  lost. Any member may still upload the current epoch's group info.

**Wrapping is repeatable, so a bad wrap is caught at once.** Running devices cannot open a wrap, as
they drop the words' keys, so a bad one could otherwise surface months later on a new device, once
the real key may be gone. Instead the random part of each wrap comes from the epoch's own secret:
another exporter label plus the account id gives an ephemeral X25519 key. So every member device
recomputes every wrap from public keys and compares, and a device finding a bad wrap uploads the
right one. Detecting it only on restore was rejected, because it would make history depend on
another device still holding the key.

### What this costs, accepted knowingly

- **Stolen words expose all stored history, unlock new chat keys too, and take over
  the login.** Going through the server, a thief has to add a device, which shows. But
  someone holding the words and a copy of the server's data can read past and future
  messages without adding anything, so no notice appears. This is the price of
  wallet-style restore.
- **No per-message forward secrecy inside an epoch.** Everyone in a chat shares one
  key per epoch, so how often keys rotate decides how much one leaked key opens.
- **This message format and the repeatable wrapping are keryx's own, on top of MLS**, so they
  need careful review rather than relying on MLS's analysis.
- **A malicious server can drop or reorder messages undetected.** A signature cannot cover the
  sequence id, which is assigned after signing. It cannot change or forge messages. A per-device
  counter was rejected, because a crash between assigning and sending would show as a false gap.
- **Checking wraps costs every device one key operation per member per key change.**
- **Key changes cost about 64 bytes per member account**, so about 64 KB per change in
  a 1,000-member group.
- **The server sees metadata.** E2EE hides content, not who talks to whom, when,
  or how much. For calls, the relay sees track names and timing.
- **The server cannot search, list or show encrypted groups and channels**, including
  through admin MCP, and push notifications cannot carry their names unless the client
  decrypts them first. It can check an encrypted file's size only, not its type or viruses,
  and clients make their own thumbnails.
- **File sizes, counts and timing stay visible**, and a size can hint at what a file is.
- **Account profiles cost more than their blob:** adding a member means re-sharing every
  member's profile key under the new chat key, about 60 bytes each.
- **Nothing written in a group or channel can be taken back from its owners and moderators.** A delete or
  edit keeps the original ciphertext in the history until retention, so a stolen database holds it too, and
  anyone who later becomes a moderator, holding the keys of the epochs they were in, can read it.
- **Exact message lengths are visible**, so the server and a stolen database can tell a "yes"
  from a paragraph, because plaintext is not padded.
- **First contact trusts the server, unless people compare safety numbers.** A server handing
  out a fake master key the first time is caught only that way. A changed key afterwards warns
  loudly.
- **The server can see what kind of thing each person sends**, even in an encrypted chat:
  text, voice, reaction, image, call and so on. So can a stolen database. It cannot read
  the content, but this is a detailed record of how each person uses each chat.
- **The server keeps a reading history per person**, for as long as the messages last.
  The times permissions hide it from other people, not from the server or a stolen
  database.
- **Device logs are a location and activity history**, kept without limit unless
  configured, and readable by the server and from a stolen database.
- **A client can fake its IP when it reaches the server directly**, by sending the proxy
  headers itself. A thief could make their device look like it is somewhere else, so
  the IP in the device list and log is a hint, not evidence, unless a proxy overwrites
  those headers.
- **Anyone who knows a username learns its account id, without logging in**, since the challenge
  answers with it so a phone can find that account's folder. The id is a ULID, so it also shows when the
  account was made. The challenge already said whether a username exists.
- **Whoever can read a browser profile can read that person's keys**, since IndexedDB
  holds them unprotected between visits.
- **The server operator can change the web client.** The browser runs whatever the
  server sends, so a malicious operator could ship code that steals words. E2EE in
  the browser protects against a breached database, not against the party serving
  the page.
- **Nothing that reads encrypted content can run on the server.** Searching it, and
  any MCP that reads it, must run on the client, where the keys are.

### Agent accounts

**An agent is a keryx account run by `keryx-agent`, all chosen by the author in the Q/A of 2026-11-15
and 2026-11-16.** It is MCP's first role, an agent as a participant, and answers how agents get and
store their words. How `keryx-agent` itself works is in [agent/AGENTS.md](agent/AGENTS.md).

- **An agent account has an owner and logs in with a token only**, with no password: a long-lived token
  its owner can renew, which replaces it and ends the old one at once, or revoke. One token per agent.
- **The owner's app makes the agent's words, never the server.** Core makes the words, publishes the
  agent's keys with their stamps, and shows the token and words once, to be put in `agent.toml`. If the
  server made or relayed the words, it and a stolen database could read every chat the agent is in.
- **Making one needs a new server permission, `agent.create`**, held by member, moderator and admin, and
  an owner may have at most `[limits] agents = 5`.
- **The owner ticks which of their own server-wide permissions the agent gets**, at creation and later.
  **It holds the ticked names checked against its owner's current permissions on every operation**, so an
  agent never exceeds its owner and loses a permission the moment its owner does. **Ticks cover
  server-wide permissions only**: inside a chat the agent holds what its chat role gives, like anyone.
- **An admin owner chooses whether the agent is admin.** It is admin only while its owner is. **No agent
  ever creates agents, admin or not:** `agent.create` cannot be ticked and an admin agent's check refuses
  it, so nothing can multiply itself through a prompt.
- **An owner can renew and revoke the token, disable the agent, change its ticks, and edit its display
  name, photo and bio.** The owner's snapshot keeps only the agent's profile key, to seal its photo and
  bio, and never its master key, so a stolen owner phone cannot register a device as the agent.
  **Disabling is done by the server**: the token is revoked, its sessions end, and the agent leaves every
  group and channel as a leave does, members committing its MLS removal, which clients already trust the
  server for. The account and device stay for old signatures. The owner's app cannot read the agent's
  chats unless the owner is in them, but whoever runs the agent's container holds its `agent.db` and
  reads every chat it is in.
- **An agent has one device**, which it replaces itself when its container starts with no data: see
  agent/AGENTS.md. **The server enforces it and tells the owner, chosen by the author:** an agent
  registers a new device only after its old one is ended, and the server writes a notice into the
  owner's self chat, such as "Your agent X registered a new device", carrying the device's stamp as the
  new-device notice does. An agent's own self chat is one its owner never sees, so without this whoever
  copied `agent.toml` could add a device as the agent and nobody would know.
- **No agent is run by the keryx server, chosen by the author**, as asked with an `agent.server.create`:
  the server, and a stolen database, could then read every chat it is in. See agent/AGENTS.md.
- **Anyone may start a direct chat with an agent or add it to a group**, as with any account. The agent's
  wake rules decide whether it acts.
- **Others see an agent badge, and its owner only once the owner chooses to show it**, hidden by default.
- **An agent says what it is doing with `activity` messages, content kind 15, chosen by the author:** each
  step's intention, written for that chat, is one message. **Its rules are a question's, chosen by the
  author:** encrypted and signed, `message.send.activity` under each chat type's prefix, held by member,
  moderator and admin, `[limits.body] activity = "1KiB"`, refused for edits with `cannot_edit`, deletable,
  and clients have no way to send one while nothing refuses one. **It is a stored message, chosen by the
  author:** it counts as unread, moves the chat up and is the preview, its text. In a channel's main
  list it needs `channel.message.post`, as any post does. **Clients fold activity messages in a row from
  the same account into one expandable list, chosen by the author**; another message or another
  sender between them starts a new one, so an agent working while people talk reads activity, message,
  activity by seq. There is no live, unsaved activity event: the message itself arrives as each step
  starts.
- **The server side, approved as drafted:**

  ```
  account   + owner_id (NULL for people), show_owner (false), disabled_at
  agent_token      (account_id PK, hash, created_at)  one per agent
  agent_permission (account_id, permission)  the ticks

  POST /api/v1/agents  agent.create: username, display_name, permission_list (each one the owner holds)
         -> account, token        at most [limits] agents
  GET  /api/v1/agents  the owner's -> agent_list{id, username, display_name, show_owner, disabled,
         permission_list, token_at, last_connected_at}
  PUT  /api/v1/agents/{id}  owner: display_name?, show_owner?, permission_list?
  POST /api/v1/agents/{id}/token    owner: renew -> token
  DEL  /api/v1/agents/{id}/token    owner: revoke
  POST /api/v1/agents/{id}/disable  owner
  POST /api/v1/login/agent  token, device?{id, challenge, signature} -> account, device?, token (a
         session, as /login/token gives, so there is one way in)
  GET  /api/v1/accounts/{id}  + agent, owner? (when shown)
  PUT  /api/v1/accounts/{id}/keys  the owner, once, for its agent: what PUT /account/keys takes
  refusals: not_owner, too_many_agents, not_held (ticking what the owner lacks), agent_disabled
  ```

  **An admin agent is an optional `admin: true|false` on the create and update bodies, chosen by the
  author**, refused with `not_held` unless the owner is admin. An agent cannot log in with a password.
- **Built in this order, chosen by the author:** first the parts of `keryx-agent` that do not touch the
  server or core (cron, `agent.db`, search, Starlark and the LLM against a fake server), since main's
  `server/` and `core/` were being reworked in another session; then, once main settles, the server and
  core, tested by the server's tests driving core; then the rest of `keryx-agent` with its tests, with `server/examples/drive.rs` as the owner through `agent add
  <username> [--admin] [permission ...]`, which prints the token and words once, `agent list`, `agent
  perms <username> [permission ...]`, `agent show <username> on|off`, `agent token renew|revoke
  <username>`, `agent profile <username> [bio] [-- photo.jpg]` and `agent disable <username>`. Android's
  Agents page waits until the agent proves itself, and is drawn first when it comes.

### MCP has four roles

| Role | Where it runs | Sees content |
|------|---------------|--------------|
| Agents as participants: an agent with its own identity in the chats it was added to, `keryx-agent` | With keys, on the client side | Yes, its own chats |
| A person's agent acting for them, through their own client | Their client | Yes, as them |
| Admin and operations: users, rooms, moderation, stats | The server | **Never encrypted content.** Unencrypted content only with a named permission |
| MCP traffic between agents, tunnelled over chats | Both ends, on the client side | The endpoints only |

## To fix

Found in a review of the calls, sync and Android commits after `82cc9a6`, and in the phone test of stickers
and questions. Each comes off this list in the change that fixes it. Fixes that need a decision are asked
first, as everywhere.

**Bugs**

- **A replaced call socket can silence its device.** `moq-net` keeps a second broadcast at the same path
  as a standby and hands over to it when the first finishes, which can take up to 10 seconds while the
  old socket's close frame waits. The new connection's group numbers start again at 0, the relay copies
  them as they are, and `keryx-moq` reads media with `next_group`, which skips any group at or below the
  last it returned. Audio is one group per frame, so the device can stay silent for about as long as its
  old connection ran. Not yet proven by a run: the replacement test checks events only, so a test that
  sends media through a replacement comes with the fix, likely the relay numbering each device's groups
  on across its connections.
- **Android can ring for a call this account is already in.** Core hands out a `sync` page's call rows
  before its messages, so a join from this account's other device is shown while nothing rings yet, then
  the call's message arrives with no end and starts the ring. The same happens when a call's `end` lands
  on the next page. Core should say whether a call rings, once the whole page is handled.
- **A rekey can be lost while a call connects.** `android/ffi/`'s `callConnect` takes the pending join and
  blocks up to 10 seconds. A `CallKey` arriving meanwhile finds no call and no join, and is dropped, so the
  phone seals under the old epoch and a device that joined with only the new key cannot hear or see it.
- **`a_frame_over_call_frame_closes_its_socket_as_a_leave` is flaky.** Once under `make coverage` it saw
  `(1, "")` where it wanted `(4403, "frame_too_large")`, and passed on the next run. Not chased yet.
- **`drive.rs` loses a command sent after it sat idle.** It reads its socket only while a command runs, so
  the server's pings go unanswered and the socket is closed; the next command goes out on the dead socket
  and is lost, though it still prints `saved`. Sending `pump 2000` first works around it.
- **A topic's row in the topics list previews only from messages already loaded on the phone.**
- **A search hit in a thread opens the chat, not the reply inside its thread.**

**Not yet tested on the phone**

- **A deleted question's answer showing "Message unavailable"**. `server/examples/drive.rs` can delete one
  of its own messages with `delete <seq>`, chosen by the author with reactions. Receiving a sticker from
  another account passed, sent by `drive.rs`'s `sticker [n]`, the nth of the Default pack, chosen by the
  author.
- **Groups and channels, what is left of their phone test.** Passed so far: Saved messages' notice as its
  preview, a new group's row tinted, the night switch's blue, the member tabs' counts, rules not shown to
  their writer, Read all only on cut rules, and an edited profile showing at once on the phone that saved
  it. Left: "created the topic" on a topic's History page with no "edited" row; Pick an owner; a topic's
  slow mode picker, "Same as group" first and Off; slow mode counting per topic, with a member who is not a
  moderator; the new row's tint clearing once opened or the list is left; the night text colour
  `#C5D4E8`; setting a topic's photo. The author's "back arrow" note from this test is still to be asked
  about.
- **`make coverage` in `server/` and `core/` has not run since the groups work**, so the coverage tables
  in their `tests/AGENTS.md` are behind.

**Access**

- **The call socket skips the device limit.** `moq.rs` checks `Need::Member` only, while every other chat
  route and the chat's WebSocket also check `Need::Device`, so a device over a lowered limit can still join
  calls.

**Smaller**

- **A failed call timer leaves the call open in the database.** If saving the end fails, the chat's task
  drops the call from memory with no `end` row, its sockets close with 1006, clients see it open until a
  restart, and a second call can start beside it. A call whose room could not be made does the same, though
  its comment says it rings out.
- **Call text's limit applies to its JSON frame**, whose body is base64, so text in a call may be about
  three quarters of `[limits.body] text`, unlike in a chat.
- **Call frames have no replay window.** A relay could replay a member's earlier frames within the epoch.
  RFC 9605 recommends a replay check. A decision for the author: whether, and how large a window.
- **`sync` without a chat sorts every matching row of the account's chats for each page**, which grows
  slow for a new device on a large account. Not measured.
- **`saved_now()` is taken before the writer's lock**, so a row can land with a time earlier than one
  already handed out. The first page's 1-second step back covers a wait under a second, and online devices
  get the row live anyway.

**Docs and comments**

- In `core/src/mls.rs`, `draft_data` sits under the **Permanent** doc comment that belongs to
  `device_statement_bytes`.
- `moq/src/lib.rs`'s `closed` says the server sends `qmux`'s application close, which it never does.
- Calls above: "`keryx-moq` uses its `ws::Client`" contradicts the later decision that `keryx-moq` opens the
  WebSocket itself, and "any other track name is refused" is not what the relay does: it ignores them.
- Status says Android's call screen is next, but it is built, and it does not mention stickers and questions,
  which are built and tested on the phone.
- A question for the author, not a bug: the words screen's Copy could mark the clip sensitive on Android
  13 and later, so the clipboard preview hides it, and the words screen could set `FLAG_SECURE`, so
  screenshots and the recent-apps thumbnail never hold the words.

## Open: do not decide these, ask

**Whole repository**

- The web client's open questions are in [web/AGENTS.md](web/AGENTS.md), and Android's in
  [android/AGENTS.md](android/AGENTS.md)
- CI, release targets and installers
- Docker: how the web build reaches the image, once the web client is off hold; a health check and a
  compose file

**Crypto and identity**

- The safety number's format
- Changing words: what happens to keys wrapped to the old account key
- Key wrapping in large channels, where 64 bytes per member adds up
- Someone over the limit who lost their words cannot terminate anything: decide this
  together with what changing words does
- The RBAC permissions for reading another person's devices and logs

**Accounts**

- The owner's notice of a new agent device: its message kind and body, and who may remove it
- An `activity` message's plaintext, and how core checks its 140 characters
- Android's Agents page: its design, drawn first, once the agent has proved itself
- Rate limits. None in the first build: operators who expose a server turn on `[captcha]`, which
  runs before Argon2, and a server without one can be guessed against at Argon2's speed
- RBAC: the management UI, and what stops a non-admin who may manage roles from granting themselves
  more. Decide them together
- RBAC: custom chat roles. Activeplano's per-organization override, stored as added and removed names,
  may fit a group changing a role for itself

**Chat**

- What the REST fallback looks like: how a client receives without a socket
- Files: quotas, by the author's choice none for now
- A call chosen as recorded when neither device can encode H.264: the client should say so
  before it starts
- Mute
- Presence text: where Android shows it and sets a person's own; whether an agent's reason is sealed to
  its owner alone, with HPKE to the owner's invite key, rather than under the agent's profile key
- The client queue on disk, so unsent messages and unfinished uploads survive a restart: where it is
  kept on each platform and how it is locked

**Calls**

- QUIC for calls: its UDP port and certificates, when WebSocket alone is not enough
- How `keryx-moq` trusts a `wss://` server's certificate: Mozilla's roots compiled in, or the platform's
  own store through `rustls-platform-verifier`. It speaks `ws://` only until a phone has to reach a TLS
  server
- Calls in groups and channels: who may start one, and who may send video or voice in a channel
- Recording: how it is chosen now that each person picks what they send, and who records
- Video size, frame rate and bitrate for live calls beyond the phone test
**Attachments and search**

- Quotas and retention for attachments
- Local search: an index, once scanning per search is too slow, and tantivy on WASM is unproven. The
  scan's folding lives in `core/src/client.rs` for now; whether borhan's normalizer replaces it

**MCP**

- MCP protocol version; the local client's transport
- How tunnelled MCP traffic is framed over chat

## Repository

`https://github.com/pouriya/keryx`, under the MIT license.

## Commits

Short imperative subjects with a type prefix: `feat: …`, `fix: …`, `ref: …`,
`doc: …`, `tests: …`, `ver: …`. One logical change per commit.

## References

| Where | What |
|-------|------|
| `/p/test/borhan` | Where the rules in `server/AGENTS.md` come from |
| `/p/test/android` | MoQ relay, web and Android experiments, `PROGRESS.md`, load test |
| `/p/test/web-transport` | The author's fork of the WebTransport libraries, including `qmux` |
| `tmp/telegram-tt` | Telegram Web A (`Ajaxy/telegram-tt`), a shallow clone of its latest commit, chosen by the author over Web K for the web client's inspiration. Its tags stopped in May 2025. It is GPL-3.0, so ideas only, never its code. `tmp/` is git-ignored |
