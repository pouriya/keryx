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
- **Next:** the web client, designed in [web/AGENTS.md](web/AGENTS.md), built in the author's order:
  first `core/src/client.rs`, with the server's tests driving it and `received` and `marks` added to the
  server, all passing `make all`; then `web/wasm/` and a bare worker that logs in and sends a message in
  the author's Chromium, both done; then the account screens (sign up, log in, the setup link and the words), checked in the author's Chromium. The chat list is next. Running `core/`'s tests in WASM waits until after it starts:
  see [core/tests/AGENTS.md](core/tests/AGENTS.md).

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
web/            The browser client: a bare page for now
  AGENTS.md     The web client's decisions and rules
  wasm/         A workspace crate holding only the wasm-bindgen glue around keryx-core
android/        The Android client. Not created yet
```

**`server/` and `core/` follow the strict rules.** The file lists are closed, code
style and logging are fixed, everything goes through `make`, and all tests are
integration tests. **`web/` is loose for now, by the author's choice:** see
[web/AGENTS.md](web/AGENTS.md). Code style in `android/` is undecided.

**`web/wasm/` is its own crate, not a module in `core/`**, so `core/` holds no JavaScript glue and
its native coverage is not pulled down by code its tests cannot run. Android gets a sibling crate
the same way. It shares the workspace's one `Cargo.lock`.

**`core/` exists because keys must come out byte-identical everywhere.** The CLI,
the browser (WASM) and Android (FFI) all turn the same words into the same keys
and read the same MLS state. One implementation means one thing to audit.

**The workspace is what keeps crypto versions identical** between the server and
the WASM build, because there is one `Cargo.lock`. Make targets stay inside
`server/`, `core/` and `web/`.

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
| Calls | MoQ, embedded in the server binary. Not a separate server |
| Call fallback | MoQ over `qmux` on a secure WebSocket when UDP is blocked. REST carries no media |
| Clients | CLI and TUI in the server binary, web, Android |
| TLS | Optional `[tls]` certificate and key paths in `server.toml`, handled by rustls in the binary through `axum-server`, with the ring provider, which is what borhan's rustls uses. Without them it serves plain HTTP, for local development or behind a reverse proxy. The same certificate setup covers QUIC for calls |
| Shutdown | On SIGINT or SIGTERM the server stops accepting connections, closes each WebSocket with code 1001, lets each chat's task finish what it already received, closes open device log rows, then exits. **The wait is at most 8 seconds, fixed in code**, under Docker's default 10 before SIGKILL. A client only counts a message as sent once it has its sequence id, so one cut off here is resent |
| Passwords | Argon2id through `argon2` 0.6.0, at 19 MiB, 2 passes and 1 lane: the crate's defaults and OWASP's minimum. The settings are stored with each hash, so they can be raised later |
| Retention | How long the server keeps ciphertext is configured in `server.toml`. **Unset means unlimited.** It can also be set per chat, where the client offers that. **The shortest limit wins:** a chat can shorten the server's limit but never exceed it, and setting it is a permission. Deleting removes the server's copy only |
| Authorization | **A complete RBAC, with many features built on it.** See below |
| Web delivery | Built on its own, copied into the server's Docker image, and served from `[www] directory = "..."` in `server.toml` through `tower-http`'s ServeDir, so keryx does not own a path check. A path with no file gets `index.html`, so `/setup#token` works. Not embedded in the binary |
| Database | SQLite in **one `.db` file, with no `.wal`**, using its classic `DELETE` journal: a `-journal` file exists only during a write, so a crash mid-write rolls back. One writer and one reader connection, each behind a lock, which take turns. If that becomes too slow, the answer is Postgres |
| v1 scope | Direct and group chats, channels, 1:1 calls, encrypted attachments, local message search, all four MCP roles |
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
  them, except that `account.create` and `message.send.unencrypted` are admin's alone. A 1:1 chat
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
  call's own messages do not either. When the newest content is deleted, `last_at` walks back with the
  preview, to the chat's creation time if nothing is left.
- **A message either has content or is an action, and may carry a pointer.** A delete
  or edit points at its target, a reply at the message replied to, and a reaction at
  the message it is on.
- **A reply is a property of a message, not a kind:** a content message with a pointer.
- **Every content message has a `content_kind`:** text, voice, reaction, image, video, doc,
  file, `video_call` and `voice_call` (saved, and can be replied to), and `text_call` (a
  simulated call: a quick text chat, then hang up). **`media_bundle` is reserved as the last
  value and is not built:** it is in no code, and the schema refuses it for now. The server
  can read `content_kind`, so roles can allow or refuse each one.
- **A reaction is content.** It does not affect the unread count, but it brings the chat up in
  the sorted list and shows as the preview, like "Sara reacted 👍", until the next message.
- **A `text_call`'s messages are saved, belong to that call, travel through the MoQ call,
  and are encrypted.** They do not count as unread and do not move the preview, because it
  was a call.
- **A saved call is one of three things:** a missed call; a call with no contents, showing only
  how long it was, its type and which devices; or a call with its contents.
- **Whether a call has contents is chosen before it starts:** each has its own initiate button.
- **The contents of a voice or video call are a recording, saved at the lowest quality for now:**
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
- **A call's outcome (missed or answered) is readable**, because a missed call counts as unread
  and an answered one does not. Its length, devices and any recording's file key stay in the
  encrypted body.
- **The server can read delete and edit pointers only**, because it removes targets,
  updates conversation state and checks ownership. Reply and reaction pointers stay
  encrypted, so neither the server nor a stolen database holds the reply and reaction
  graph. `kind` is always readable, because RBAC needs it.
- **Each user has their own list of conversations, worked out by the server.** The
  client never sends the list. It shows the list, updates state with the latest for
  all of them, then fetches pages in the background from the last id it has to the
  latest. **How, chosen by the author:** on every reconnect, each chat this session already
  knew is paged forward with `after_seq` from the last sequence id it handled, commits applied in
  order and content opened. After a reload there is no last id. The list carries this member's seen
  position, and updating fetches from there, so the unread count is the content messages decrypted
  in that fetch. A reaction does not count. A chat still opens on its newest page. See [core/AGENTS.md](core/AGENTS.md).

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
the message and gives it its sequence id. If the message never lands, the file stays unmarked. The name
`persist` is the author's; its exact shape and the rest of the upload are still to ask when files are built.

**Files uploaded but never marked are removed when the chat's task loads, and at startup.** There
is no job running on a timer. Only unmarked files older than a set age go, so an upload in progress is
safe. An idle chat's unmarked files stay until it next loads or the server restarts.

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
match over one socket: a `send` gets its sequence id this way, and a history request its page. **The
server never stores a client's ULID, and it is not in the signed header.** What is saved goes by
its sequence id, from 1. REST carries the same bodies. A client sends `send`,
`seen`, `received`, `typing`, `sync`, `history`, `group_info`, `wraps` and `ping`. The server sends `features`, `sent`, `done`,
`refused`, `message`, `chats`, `page`, `welcome`, `typing`, `marks` and `revoked`. `group_info` and `wraps` are not
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

### Read state

- **`seen` is not per message.** A client sends one at some point in time, and it covers
  everything from the previous `seen` up to that point.
- **A `seen` carries "seen up to N", and the server keeps the highest.** Without it, a
  message stored just before the `seen` arrives would be marked seen though nobody saw
  it, and a late `seen` from another device would do the same.
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
- **`history` takes an optional `before_seq`, chosen by the author.** With it, the server returns up
  to `limit` messages just below it, still oldest first, and one past the last sequence id gives the
  newest page. So a chat opens on its newest page and scrolling up loads older ones.
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
- **The queue is memory only.** It is lost when the tab closes, so a client must show a
  message as pending until the sequence id arrives, never as sent. A message sent just
  before closing or crashing can vanish.

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
GET  /api/v1/public/features        -> registration, captcha. Anyone, before an account.
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
GET  /api/v1/ws                     cookie or Bearer, subprotocol keryx.v1
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

The web client. **No Android work in this phase**, and `android/` is not created yet.

**The first working thing is 1:1 encrypted text:** two browsers sign up, generate
their words, and send encrypted text over WebSocket, under keys from MLS. This comes first because
`openmls` in WASM and words-to-keys are the riskiest parts. The REST fallback, groups
and calls come after it.

**The first server build also has delete and edit in 1:1 chats,** with wiping and the preview walking
back. Files, profiles, retention, MCP and calls come after it, so `mcp.rs` and `moq.rs` do not exist
yet.

**`keryx` has server commands only in this phase:** `serve` and `user add`,
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

  [limits]
  websocket_frame = "1MiB"
  rest_body = "1MiB"        # not file uploads
  history_page = 200
  outgoing_frames = 1024    # full: the connection closes, and the device refetches
  ping_after = "5s"         # quiet this long: the first ping
  ping_every = "15s"        # then one ping this often
  ping_timeout = "5s"       # each ping's time to answer, or the connection closes
  key_packages = 100        # the most one device may keep on the server

  [limits.body]             # one message's body. A file's bytes upload separately
  text = "64KiB"
  reaction = "1KiB"
  voice = "16KiB"
  image = "16KiB"
  video = "16KiB"
  doc = "16KiB"
  file = "16KiB"
  video_call = "16KiB"
  voice_call = "16KiB"
  text_call = "64KiB"

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

## Open: do not decide these, ask

**Whole repository**

- Android, when its phase comes: whether anything moves in from `/p/test/android`,
  and its stack
- Android code style and its own AGENTS.md. The web client's open questions are in
  [web/AGENTS.md](web/AGENTS.md)
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
- Profiles: who may see and change them, image size and format limits, limits on bios and
  links, and where an account's folder lives
- Files: chunk size, quotas, and how old an unused upload must be before it goes. Upload limits are
  to be per declared type under `[limits.upload]`, such as voice 50 MiB, image 20 MiB, video 2 GiB,
  doc 100 MiB and file 2 GiB. A file's real type is encrypted, so the server can only trust what a
  client declares, and a client that lies gets the largest limit
- Calls: how a `text_call`'s MoQ messages reach the message table. The relay is in the same
  binary, so it could pass them to the chat's task. They need the normal message encryption
  rather than SFrame, so a new device can read them
- A call chosen as recorded when neither device can encode H.264: the client should say so
  before it starts
- Mute and pin
- Whether a group or channel can be found and joined from outside, and how
- What a channel is compared with a group

**Calls**

- Which MoQ crates (`moq-net`, `moq-tokio`, `hang`, `qmux`) and versions
- Codecs for live calls, as only recordings are decided; the SFrame cipher variant
- UDP port, TLS certificates, how a call is set up over WebSocket
- Group calls: **not supported for now and not decided.** Nothing here is designed for them

**Attachments and search**

- Quotas, retention and size limits for attachments
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
