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
- **Next, chosen by the author: calls, then Android for files and calls.** Both clients stay as they
  are and grow only what testing needs; standard screens and a full redesign come after.

Delete this section when it stops being true.

## Layout

```
AGENTS.md       This file: what applies to the whole repository
Cargo.toml      The workspace: server and core, with one Cargo.lock and one target/
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

**`web/wasm/` is its own crate, not a module in `core/`**, so `core/` holds no JavaScript glue and
its native coverage is not pulled down by code its tests cannot run. **`android/ffi/` is its sibling
for Android, the same way.** It shares the workspace's one `Cargo.lock`.

**`core/` exists because keys must come out byte-identical everywhere.** The CLI,
the browser (WASM) and Android (FFI) all turn the same words into the same keys
and read the same MLS state. One implementation means one thing to audit.

**The workspace is what keeps crypto versions identical** between the server and
the WASM build, because there is one `Cargo.lock`. Make targets stay inside
`server/`, `core/`, `web/`, `android/` and `moq/`.

## What has been decided

| Part | Decision |
|------|----------|
| Topology | One self-hosted server. One instance is one community. No federation |
| Encryption | End-to-end, unless a message or chat is explicitly unencrypted. Unencrypted is governed by roles and permissions and flagged in the `message` table, so the client knows. In an unencrypted chat, action messages are unencrypted too. For encrypted content the server stores ciphertext only |
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
| Web delivery | Built on its own, copied into the server's Docker image, and served from `[www] directory = "..."` in `server.toml` through `tower-http`'s ServeDir, so keryx does not own a path check. A path with no file gets `index.html`, so `/setup#token` works. Not embedded in the binary |
| No production yet | **No migrations, no backward compatibility and no `ALTER`s until there is a production server, chosen by the author.** The schema, the wire and stored formats change in place, and a change means deleting the database and the clients' stored state |
| Database | SQLite in **one `.db` file, with no `.wal`**, using its classic `DELETE` journal: a `-journal` file exists only during a write, so a crash mid-write rolls back. One writer and one reader connection, each behind a lock, which take turns. If that becomes too slow, the answer is Postgres |
| v1 scope | Direct and group chats, channels, calls in all three, encrypted attachments, local message search, all four MCP roles |
| Web stack | React, Vite and TypeScript, with all crypto done by `keryx-core` compiled to WASM. The rest is in [web/AGENTS.md](web/AGENTS.md) |

### RBAC

- **Custom roles at two scopes, server and per chat.** Named permissions are grouped
  into roles. Admin, moderator and member are the built-in defaults, and operators and
  chat owners can define more. A new feature adds permissions, not a new mechanism.
- **The server enforces, and MLS guards.** The server knows who holds which role and
  rejects forbidden actions, so it sees roles but never content. Membership is guarded
  by the clients: they reject any device not stamped by the master key of an account
  already in the chat, or added by a member's signed commit. A malicious server could
  block actions or lie about roles, but could not read messages.
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
  kind: `chat.create_direct`, `directory.search`, `account.create`, and in a chat
  `message.send.text` and so on through every kind, `message.send.unencrypted`,
  `message.commit`, `message.edit.own` and `message.delete.own`. The closed list comes from
  activeplano. Member, moderator and admin hold all of
  them, except that `account.create`, `message.send.unencrypted` and `sticker.manage` are admin's alone. A 1:1 chat
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
  video 5, doc 6, file 7, call 8, media_bundle 9, question 10, answer 11 and sticker 12.
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
  one sealed blob under the chat's current key. Whoever adds a member re-encrypts it, since
  a new member has only the current key. **Someone invited but not yet joined cannot see the
  name.** It cannot go inside MLS itself, because the server stores `group_info` in the
  clear so new devices can join.
- **An unencrypted group or channel's profile is readable by the server.** Switching a
  chat's mode rewrites its profile.
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
  a photo in the chat list opens that person's profile, not the chat, chosen by the author.** **It is
  drawn in `tmp/design/profile.html` and approved as drawn, chosen by the author:** the menu button opens
  the profile; each line opens its own editor, saved with a check in the bar; the name and @username show
  under the photo and as lines; a taken username shows on save as "That username is taken."; a link's
  address starts with `https://` or `http://`, its name is at most 32 characters and its address 512, and
  tapping one opens the browser; someone with no photo keeps the letter, and one still loading shows its
  BlurHash; Remove photo asks nothing; the photo uploads when Done is tapped, under a turning ring.

### Files

**Files are encrypted, each with its own random AES-GCM key, in chunks** because a browser's
AES cannot stream one large file. The file's name, type and key are inside the encrypted
message or profile, so a new device with only the words gets files too, and the server sees
an unnamed blob, its **exact size**, and who uploaded it. In an unencrypted chat, files are
not encrypted.

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
`seen`, `received`, `typing`, `sync`, `group_info`, `wraps`, `profile_keys`, `call_event`, `times`, `draft` and `ping`. The server sends `features`, `sent`, `done`,
`refused`, `message`, `chats`, `welcome`, `typing`, `marks`, `revoked`, `call_event`, `times`, `draft`, `profile` and `profile_keys`. `group_info` and `wraps` are not
messages, so they take no sequence id and stay out of history, and what has no sequence id is
answered with `done`. Binary fields are standard base64 with padding, through `base64ct`, which is
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
- **A message's times are server times only, chosen by the author.** A `times` frame {ref, chat, seq} is
  answered with `times` {ref, chat, seq, member_list: [{account, received_at?, seen_at?}]}, and REST has the same
  at `GET /api/v1/chats/{id}/messages/{seq}/times`. It needs a registered device and membership, with no new
  named permission. It lists every member but the sender, and hides `seen_at` for an account with receipts off.
  An unknown message is `not_found`, and a stranger gets `not_member`.
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
  moves the chat to the answer. **Direct chats only for now**: groups, with many answers, are designed later.
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
- **Who can see seen times depends on the chat type and role.** Direct: the other
  person. Group: anyone with a "view seen" permission, by default each message's sender
  and admins. Channel: a permission for owners and admins, while subscribers see counts
  only.
- **A person may turn receipts off, default on.** That hides their times from others,
  and they then cannot see other people's. A group's owner can forbid it by permission.
  The server still stores the times either way.
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
  positions too, so a reload draws the marks at once. Someone with receipts off shows no seen
  position, under the rule above. **The entry's field is `mark_list`, chosen by the author:**
  `[{account, received_seq, seen_seq}]`, one per other member, the same fields as `marks`, with no
  `seen_seq` for someone with receipts off, and empty in a channel. No `marks` go out in a channel
  either.
- **Each `chats` entry carries its newest content message, chosen by the author**, as a full message
  frame, still ciphertext, so one request gives every preview. Its field is `last_content`.
- **Each `chats` entry carries the chat's current `epoch`, chosen by the author:** the epoch of the
  last accepted commit, which the server already keeps for ordering commits. A device back from
  being away compares it with its own and catches up or rejoins at once, rather than noticing only
  when its own commit loses. It is an optional field, so the version does not change.
- **What a sender sees, as drawn by the web client.** A message in the queue, or sent but not yet
  answered with its sequence id, shows as waiting. In a direct chat it gets one mark with its
  sequence id, two once the other person's device has received it, and both marks change colour
  once they have seen it. In a group it gets one mark, which changes colour at the first `seen`. In a
  channel only the waiting sign goes away.

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

### Unencrypted messages and chats

- **A chat has one of three settings.** Private: everything encrypted, and unencrypted is
  refused. Mixed: encrypted, but permitted people may send unencrypted messages, such as
  announcements. Open: everything unencrypted. New chats start as private.
- **Who decides:** in a direct chat, both members must accept before unencrypted things
  can be sent. In a group or channel, an owner decides and can change it at any time.
- **A chat can switch between encrypted and unencrypted after creation.** Messages
  already sent stay as they were.
- **That decision is signed inside MLS, so a server cannot downgrade a chat.** The
  chat's owner accounts are recorded in the MLS group, and a switch is a change signed by
  an owner's device, or by both members in a direct chat. Clients refuse anything
  else. Transferring ownership must be signed too. This is the one place a server
  cannot simply lie about a role.
- **Unencrypted messages are always signed by the sending device**, the same as
  encrypted ones, so clients detect a server altering or forging them.
- **Every chat always has an MLS group, even while unencrypted.** Switching on is
  instant and there is one code path. A large unencrypted channel pays the full MLS and
  key-wrapping cost for encryption it is not using.
- **The server may store and deliver unencrypted content, and whoever holds a named
  permission may read it**, including through admin MCP and server search. Encrypted
  content stays unreadable to the server either way. Clients must make clear when a
  chat or message is not private.

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
POST /api/v1/challenge              username, purpose (password_reset | device_login) -> challenge
POST /api/v1/password/reset         username, challenge, new_password, signature

Any session
GET  /api/v1/sessions               -> session_list
DEL  /api/v1/sessions/{id}
PUT  /api/v1/account/keys           master_public, account_public, account_stamp -> self_chat
GET  /api/v1/accounts/{id}          -> id, username, display_name, and the three keys
GET  /api/v1/devices                -> device_list, reduced if the session has no device
POST /api/v1/devices/id             -> device, the id its stamp signs
POST /api/v1/devices                device, public_key, stamp, platform, os, os_version,
                                    web_platform?, web_platform_version?
POST /api/v1/devices/{id}/terminate signature
POST /api/v1/devices/{id}/logout    signature

A named permission
POST /api/v1/accounts               account.create: username, admin?, display_name? -> account, link
GET  /api/v1/accounts?search=       directory.search -> account_list

A registered device, under the limit
POST /api/v1/key_packages           body_list -> count
POST /api/v1/accounts/{id}/key_packages/claim  -> key_package_list{device, body}
POST /api/v1/chats                  chat.create_direct: account -> chat, created. Your own id gives
                                    your self chat
GET  /api/v1/chats/{id}/group_info  member -> epoch, group_info
GET  /api/v1/chats/{id}/devices     member -> device_list, with ended devices and their statements
GET  /api/v1/chats/{id}/wraps       member -> wrap_list{epoch, account, wrapped}
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
  draft = "64KiB" # an account's sealed draft in one chat. Not a message

  [limits.upload]           # one file's encrypted bytes, by the type its uploader declares
  voice = "50MiB"
  image = "20MiB"
  music = "200MiB"
  video = "2GiB"
  doc = "100MiB"
  file = "2GiB"
  sticker = "256KiB"        # one sticker's WebP. Not encrypted: everyone on the server sees it
  photo = "1MiB"            # one profile photo, sealed. An account's file, not a chat's

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
  HPKE's `DeriveKeyPair`. Another key later is just another label. Wallet-style SLIP-0010 paths
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
  the device's master stamp, so a server cannot invent one. Clients accept this one kind of unencrypted event in a self chat whatever its mode, and it holds
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
- **Exact message lengths are visible**, so the server and a stolen database can tell a "yes"
  from a paragraph, because plaintext is not padded.
- **First contact trusts the server, unless people compare safety numbers.** A server handing
  out a fake master key the first time is caught only that way. A changed key afterwards warns
  loudly.
- **The server can see what kind of thing each person sends**, even in an encrypted chat:
  text, voice, reaction, image, call and so on. So can a stolen database. It cannot read
  the content, but this is a detailed record of how each person uses each chat.
- **The server keeps a reading history per person**, for as long as the messages last.
  Turning receipts off hides it from other people, not from the server or a stolen
  database.
- **Device logs are a location and activity history**, kept without limit unless
  configured, and readable by the server and from a stolen database.
- **A client can fake its IP when it reaches the server directly**, by sending the proxy
  headers itself. A thief could make their device look like it is somewhere else, so
  the IP in the device list and log is a hint, not evidence, unless a proxy overwrites
  those headers.
- **Whoever can read a browser profile can read that person's keys**, since IndexedDB
  holds them unprotected between visits.
- **The server operator can change the web client.** The browser runs whatever the
  server sends, so a malicious operator could ship code that steals words. E2EE in
  the browser protects against a breached database, not against the party serving
  the page.
- **Nothing that reads encrypted content can run on the server.** Searching it, and
  any MCP that reads it, must run on the client, where the keys are.

### MCP has four roles

| Role | Where it runs | Sees content |
|------|---------------|--------------|
| Agents as participants: an agent with its own identity in the chats it was added to | With keys, on the client side | Yes, its own chats |
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
- **Android's down button can count a message that is already on screen.** In the stickers and questions
  phone test, it showed 1 after 15 messages had arrived. Not chased yet.
- **Android's chat list can show an unread count for the chat that is open.** In the same test, it showed 1
  while that chat was open and at its end. Not chased yet.
- **`a_frame_over_call_frame_closes_its_socket_as_a_leave` is flaky.** Once under `make coverage` it saw
  `(1, "")` where it wanted `(4403, "frame_too_large")`, and passed on the next run. Not chased yet.

**Not yet tested on the phone**

- **Receiving a sticker from another account**, and **a deleted question's answer showing "Message
  unavailable"**. `server/examples/drive.rs` cannot send a sticker or delete a message yet, so two commands
  for those come with the test.

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
- Docker: Dockerfile location, base images, and how the web build reaches the image

**Crypto and identity**

- The safety number's format
- Changing words: what happens to keys wrapped to the old account key
- Key wrapping in large channels, where 64 bytes per member adds up
- Agent identities: how they get and store their words
- Someone over the limit who lost their words cannot terminate anything: decide this
  together with what changing words does
- The RBAC permissions for reading another person's devices and logs

**Accounts**

- Rate limits. None in the first build: operators who expose a server turn on `[captcha]`, which
  runs before Argon2, and a server without one can be guessed against at Argon2's speed
- RBAC: the management UI, and what stops a non-admin who may manage roles from granting themselves
  more. Decide them together
- RBAC: what moderator holds beyond member, once groups exist. Activeplano's per-organization
  override, stored as added and removed names, may fit a group changing a role for itself

**Chat**

- What the REST fallback looks like: how a client receives without a socket
- Profiles of groups and channels: who may change them, and their limits. Accounts' are decided above
- Files: quotas, by the author's choice none for now
- A call chosen as recorded when neither device can encode H.264: the client should say so
  before it starts
- Mute and pin
- The client queue on disk, so unsent messages and unfinished uploads survive a restart: where it is
  kept on each platform and how it is locked
- Whether a group or channel can be found and joined from outside, and how
- What a channel is compared with a group

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
- Local search: engine, whether borhan's normalizer is reused, and where it lives.
  **No module in the closed lists holds it yet**, and tantivy on WASM is unproven

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
