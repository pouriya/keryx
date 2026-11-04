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
crate for `wasm32-unknown-unknown` with its `wasm` feature on, chosen by the author, so a change that
breaks the browser build, or a type `tsify` cannot describe, fails here at once and not only in
`web/`'s build.

**The `wasm` feature turns on `tsify` and `wasm-bindgen`, chosen by the author.** The derive that
writes the TypeScript types has to sit on the type itself, so `client.rs`'s and `protocol.rs`'s types
carry `#[cfg_attr(feature = "wasm", ...)]`. It is off for native builds and the tests, so the server
and this crate's coverage see no change, and there is one list of types for Rust and TypeScript.

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
src/client.rs     A device's whole flow, the same for every client
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

## The client

**`src/client.rs` holds a device's whole flow, so web and Android share one implementation, chosen
by the author.** That covers logging in with the device's signed challenge, starting a chat, joining
by external commit, checking wraps, sealing and opening messages, and driving the queue. Android
reuses it unchanged, and it is audited once.

**It does no I/O.** Its functions are plain and synchronous. A command, a server frame, a REST reply
or the time goes in. Frames to send, REST requests to make, events to show and "save this snapshot"
come out. The platform does the fetching, the socket and the storage. So `core/` needs no async
runtime and no networking crate, and behaves the same on every platform.

**One door in and one out, chosen by the author.** One `Client` value with one `step(input, now)`
function. The input is one enum: a command from the person, a server frame, a REST reply or a timer.
The output is a list of frames to send, REST requests, each with an id its reply comes back with,
events to show, and "save this snapshot". So the WASM and Android glue each wrap one function, and a
test can feed it anything.

**The first step covers the whole first slice, chosen by the author:** setting the account keys,
registering a device, topping up key packages, logging in with the device's signed challenge,
starting a chat, joining by Welcome or external commit, checking wraps, send, edit and delete, `seen`
and `received`, history pages, acting on `revoked`, logout and terminate.

**Its commands and events are what the page and the web worker exchange**, chosen by the author, and
Android uses the same list. The first list, which grows as the code needs:

- Commands, from the person: sign up, log in, a new captcha, `/setup` with its token, generate words (12 or 24), set
  words, restore, the chat list, open a chat, load older, start a chat, search the directory, send
  text, edit, delete, seen, typing, the device list, terminate (with the words), log out, and "the
  network is back".
- Events, to show: the words (shown once), words needed, signed in, the chats, a message, an edit, a
  delete, pending, sent with its sequence id, marks, typing, search results, the device list, online,
  offline, what `GET /api/v1/public/features` said, a captcha image, and an error with its code and text. The features
  read goes out when the person is asked to log in, and again after a logout, and sign up is offered
  only when `registration` is set.
- The platform's own, in: a server frame, a REST reply with its id, connected, closed, the timer,
  saved, save failed, and the two loaded snapshot slots. Out: a frame to send, a REST request with an
  id, connect, disconnect, save to a slot, and wake at a time.

**How it logs in is set when the `Client` is made, chosen by the author:** cookie or token. With a
token, it keeps it in memory and puts it on its requests as `Bearer`. With a cookie, it never sees
one, so a browser can never be sent down the token route by accident. **Loading a snapshot takes
`resume`.** A reload resumes, and a cookie client connects at once. A login for one username loads
that username's snapshot with `resume` false, so the socket stays down until that login and another
account's device is not sent with the password.

**It keeps no timeline, chosen by the author.** It holds only what it needs to work: the chat list's
state, each chat's last sequence id, the queue and the MLS state. Each opened message, edit or delete
goes out as an event, and the platform keeps the timeline, so the text sits in memory once.

**It asks to be woken, chosen by the author.** One of its outputs is "call me again at time T", and
the platform keeps one timer and calls `step` with a timer input when it fires. So the `ping` frame,
topping up key packages and retrying a rejected commit are timed the same way on every platform.

**A commit waits for its snapshot to be saved, chosen by the author.** `client.rs` outputs "save
this snapshot", with the bytes and the slot, and holds the commit until the platform calls `step` with
"saved" or "save failed". On failure the commit is dropped and an error event goes out, so the rule
stays inside `client.rs` for every platform.

**REST bodies are typed, chosen by the author.** Each route's body and reply get a struct in
`protocol.rs`, beside the frames, and the server reads them with the same structs, so a renamed field
breaks both sides at compile time.

**`serde_json` is a real dependency, chosen by the author**, for the plaintext and REST bodies.

**`serde_bytes` marks the snapshot slots as bytes, chosen by the author**, on `Loaded` and `Save`, so
they reach JavaScript as a `Uint8Array` rather than an array of numbers. It was already in
`Cargo.lock`, and JSON comes out the same.

**A history page is 50 messages, chosen by the author**, fixed as `PAGE` in `client.rs`: about two
screens of bubbles, so a chat opens fast and scrolling up loads the rest. The server's `history_page`
is the most it allows.

**The self chat is started like any other chat, chosen by the author:** the client claims its own
account's key packages, which uses up one of this device's, and the next connect's top-up replaces
it. No special case in the client or the server.

**Key packages are topped up on each connect, chosen by the author:** an empty post learns the count,
then enough are uploaded to reach 10. No server change.

**A dead connection is retried after a growing wait, chosen by the author:** 1 s, then 2, 4 and 8, up
to 30 s, each with a little randomness so clients do not retry together. It resets once a connection
works, and the platform can say "the network is back" to retry at once. The numbers are fixed here,
so every platform behaves the same.

**It sends `received` when the device holds the message, chosen by the author:** when a message frame
arrives live, or a page holding it is fetched and opened. A higher last sequence id in the chat list
does not count, so the second mark means the text really reached a device of that person.

### Messaging flows and failures, chosen by the author

**Missed messages are fetched forward.** Each chat keeps the last sequence id the client handled
with nothing missing below it. On every reconnect, each chat this session already knew is paged
forward with `after_seq` from there to the latest, commits applied in order and content opened. A
live frame that skips ahead starts the same fill. Live frames arriving during a fill are kept and
handled after it, in order. After a reload there is no last id. The list carries this member's seen
position, and updating fetches from there, so the content messages they have not seen are opened
again. The unread count is those messages. A reaction does not count, and neither does a message
from this account, sent from any of its devices: core marks it `mine`, chosen by the author, so every
client counts the same way. The device is still there
and the name is not, so the page hears `signed_in`
when this account's name comes back.

**Unanswered sends are matched by their exact signature** in those same pages, and resent only when
absent. The old check looked at the newest 50 by send time and kind, and could duplicate or
mismatch.

**Requests the client makes by itself retry; those the person starts do not.** On no answer
(status 0), a 5xx or a 429, background requests retry with the connection's backoff, 1, 2, 4, 8, 16,
then every 30 seconds with jitter, for as long as the device runs: account keys, the device id and
registration, key packages, account info, chat devices, group info, wraps, claims and the chat start.
Sign up, log in, setup, search, the device list, terminate and log out fail at once with an error
event, code `offline` when nothing answered, and the person retries. **The words' keys are dropped
only once every chat really restored**, so a network error cannot cost history.

**What needs the socket is held while it is down.** Seen and received keep the highest per chat, and
go again on connect until answered with `done`. Open chat and load older are remembered per chat and
run on connect. The chat list folds into the sync connect already does. Group info and wrap repairs
are kept and resent on connect. Typing while offline is dropped, since it would be stale.

**A message that cannot be shown properly still appears**, with no text and a `problem`:
`bad_signature`, `unknown_device` or `malformed`, so the timeline has no hole. An unknown device is
decided after one fresh device list, never in a loop. If the server refuses that list, what waited
for it shows as `unknown_device` rather than staying held.

**Any commit or Welcome that fails to process makes the device rejoin that chat** by external commit
from the server's group info, after dropping the chat's group but keeping every epoch key it holds, so
old messages stay readable. openmls removes the device's old leaf in the same commit. This covers a
device that fell back to the older slot.

**A commit or Welcome refused for what it brings in does not rejoin.** Those are an outsider joining,
a stamp that does not verify, a changed master key, or a credential that cannot be read. Rejoining
would take the server's group info, which already includes that commit, so the device would accept
what it just refused. It shows `bad_commit` or `bad_welcome` and stays where it is. Chosen by the
author.

**A commit that removes this device keeps what it could read and stops, chosen by the author.** keryx
only ever removes an ended device, or a whole person from a group, so a running device sees this when
its person was removed, or when a broken or hostile member removed it alone. The device drops the
group and keeps every epoch key, so a later rejoin shows what it had before, and emits `ChatLeft`. It
does not rejoin by itself. If its account is still in the chat, the next chat list joins it again, as
for any chat it holds no group for. Hiding the chat and rejoining a group are decided with groups.

**A send waits rather than fails** while its chat is being joined or rejoined, or a key change for it
is pending, including a rotation falling due while it waits. It goes under the new key, and fails
only if the join or the change finally fails. At most one rotation is planned per chat at a time.

**A start whose group info is refused fails, and starting again begins anew:** the device drops the
group it made, since none of it reached the server, so a second start does not find a group and call
the chat ready with nobody added. **A start whose first wraps are refused goes on** to add the other
devices, with an error event: that commit makes a new key and wraps it again, and nothing was sealed
under the first. Failing there would leave the server a group the other person could never join.

**The new-device notice is checked** once its device is in the self chat's device list: its signature
is the device's stamp, which must verify against this account's pinned master key. One that fails
gets `problem` `bad_signature`.

**The same message can be emitted more than once**, as a page and a live frame can overlap. Events
for one chat and sequence id replace each other, so the platform keys its timeline by sequence id.
The client keeps no timeline, so it cannot know what the platform shows.

**A failing state save is retried with the connection's backoff**, with one error event when it starts
failing and nothing more until it succeeds. Commits stay blocked meanwhile.

**Refusals that mean the session is gone are acted on:** a REST 401 asks to log in again,
`device_ended` wipes the device, `device_limit` stops it until the person terminates a device, after
which it reconnects. A `no_device` on the key-package count sent as the socket opens returns to the
login screen and is not shown: that request goes out before the server has closed a session that
has no device, and the person did not do it. Set words after a half-finished set-up skips putting the keys when the server already holds
these exact ones.

**The server's tests drive it against a real `keryx serve`,** replacing their own client code built
from `keryx-core`'s pieces, so one client is proven end to end. Every test that acts as a person uses
it, and the refusal tests keep their hand-made frames. `tests/` also feeds it hand-made
frames for the error cases, so this crate's coverage table counts it.

**A text message's plaintext is `{"text": "..."}`, and an edit carries the same object with its new
text.** Later features add optional fields, such as a reply's target, so older clients still read
newer messages. Unknown fields are ignored. This is permanent once messages exist.

## Testing

**No unit tests. Integration tests only, in `tests/`.** See
[tests/AGENTS.md](tests/AGENTS.md), including its rule about the coverage table.
