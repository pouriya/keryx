# keryx-agent

An LLM agent that lives in keryx as its own account. It holds that account's words and keys like any
device, stays connected to one keryx server, keeps everything on disk, and wakes one LLM loop when
configured events arrive. Its one tool is a Starlark script runner.

**Every part below was answered in the author's Q/A of 2026-11-15 and 2026-11-16. Built so far: the
crate, `check` reading `agent.toml`, and the cron parser, passing `make all`.** The server's side, agent accounts, is in the root
[AGENTS.md](../AGENTS.md) under Agent accounts.

## Same rules as core

**[core/AGENTS.md](../core/AGENTS.md) binds this crate too, chosen by the author**, and through it
[server/AGENTS.md](../server/AGENTS.md): no new files beyond the list under Files, inlining unless a
second caller exists today, `make` rather than `cargo`, `make all` passing before anything is reported
finished, the code style (plain `for` loops, `match` and `if let` over combinators, `anyhow` at the
binary's boundary and `thiserror` inside modules), lint with no `#[allow]`, the logging format, the
dependency rules (each with its reasoning in `Cargo.toml`, `default-features = false`, `rust-version`
set, adding one asked first), the vendoring rule, and integration tests only. Where this file differs,
it says so.

## Shape

- **A workspace crate `agent/`, binary `keryx-agent`, in Rust, chosen by the author.** It drives
  keryx-core's `Client` as `server/examples/drive.rs` does, on the workspace's one `Cargo.lock`, so it
  seals and opens exactly as the apps do. It follows core's strict Rust rules: a closed file list,
  everything through `make`, integration tests only.
- **One account per process, chosen by the author.** No multi-account, and one search store rather
  than borhan's many memories.
- **It runs unattended, on a server or in Docker, chosen by the author**, as a long-running process, not
  on the owner's own machine.
- **Its own Docker image, chosen by the author:** `make docker` in `agent/`, a static musl binary on
  alpine as the server's, one volume `/var/lib/keryx-agent` holding `agent.toml`, `agent.db`, the index,
  the files and the skills, with `ENTRYPOINT keryx-agent --config /var/lib/keryx-agent/agent.toml` and
  `CMD run`.
- **Never run by the keryx server, and a binary only, chosen by the author** when asked whether
  someone holding an `agent.server.create` could have the server run one. A hosted agent's words, keys,
  plain index, run logs and memory would sit on the server, so the server and a stolen database or
  backup could read every chat it is in, and every other member of those chats would lose end-to-end
  encryption without choosing to. Its scripts would also reach the server's own network, and a runaway
  script or a panic in starlark or tantivy would take the messenger down for everyone. Revisited only
  once the standalone agent has proved itself, and then as unencrypted participants: a badge nobody can
  hide, a warning before adding one, admin-only by default, each in its own process.
- **Its `lib.rs` exists only so that `tests/` reaches its modules, as the server's does, chosen by the
  author:** the cron parser, `store.rs` and the normalizer are tested directly, and the loop through the
  binary. It is no API, and not the hosting library ruled out above.

### Crates, chosen by the author

| Crate | Version | Why |
|-------|---------|-----|
| `keryx-core` | the workspace's | Sealing, opening, MLS, the queue, as every client |
| `tokio` | 1.54, in the lock | One task drives core's `Client`; others run the socket, timers, schedules and the LLM. Scripts run on blocking threads under their time limit |
| `tokio-tungstenite` | 0.29, in the lock | The keryx WebSocket, `wss://` through rustls |
| `reqwest` | 0.13 | The LLM API and scripts' `http`, on hyper 1.12, already in the lock, with rustls and the ring provider as the server uses |
| `webpki-roots` | 1.0, in the lock | Mozilla's roots compiled in, so a bare alpine image needs no CA files. `insecure=True` skips the check per call |
| `starlark` | 0.14.2 | facebook's Starlark, the newest |
| `tantivy` | 0.26.1 | As borhan pins it, since the normalizer and scoring are copied from borhan and proven against it |
| `rusqlite` | 0.40.2, in the lock | `agent.db` |
| `futures` | 0.3, in the lock | The Stream and Sink traits of tokio-tungstenite's socket, as moq takes them; chosen by the author while building |
| `rust-stemmers` | 1.2 | English stemming in the copied normalizer, as borhan has it, so `errors` finds `error`; chosen by the author while building |
| `unicode-normalization` | 0.1, in the lock | NFC in the copied normalizer, as borhan has it; chosen by the author while building |
| `jiff` | 0.2 | Times, durations and IANA zones, with its zone database bundled so the image needs no tzdata. One time library |
| `tracing`, `tracing-subscriber` | in the lock | Logging, by the server's rules |
| `base64ct` | 1.8, in the lock | `llm.attach`'s images as data URLs, the crate core and the wire use; chosen by the author while building |

**The server's own crates for the CLI, `agent.toml` and errors, chosen by the author:** `clap` (derive),
`tanzim` with its `"5m"` and `"1MiB"` validators, `serde`, `serde_json`, `anyhow` and `thiserror`, all
already in the lock.

**Cron is parsed by keryx-agent's own small parser, chosen by the author:** five fields, minute, hour,
day, month and weekday, with ranges, lists and steps, and jiff finding the next time. **Names `JAN` to
`DEC` and `SUN` to `SAT` in any case, and weekday 7 as Sunday beside 0; no `@` aliases, chosen by the
author.** **With both day and weekday restricted, either matching fires, as Vixie cron and POSIX do,
chosen by the author.** **A time a clock change skips fires right after the jump, and a time a fall-back
repeats fires once, the first time, chosen by the author.**

### Files, chosen by the author

```
agent/Cargo.toml
agent/Makefile
agent/AGENTS.md
agent/Dockerfile
agent/src/main.rs       CLI, agent.toml, logging, startup
agent/src/lib.rs        the modules, as a library only so that tests/ can reach them
agent/src/agent.toml    the commented template, include_str!'d
agent/src/agent.sql     agent.db's schema, include_str!'d by store.rs
agent/src/connect.rs    the socket, driving core, both queues, presence, snapshot and archive saves
agent/src/store.rs      agent.db: schema and every query
agent/src/index.rs      tantivy: messages and memory, search
agent/src/normalize.rs  vendored from borhan, under its vendored-code rule
agent/src/run.rs        the one loop: wakes, rules, context, compaction, limits, checkpoints, the run log
agent/src/llm.rs        Chat Completions over reqwest, backoff
agent/src/script.rs     Starlark: the tool, every built-in, secrets, files, skills, the action list
agent/src/schedule.rs   at, every, cron with its own parser, fires
agent/tests/            integration tests, the fake LLM, and AGENTS.md with the coverage table
```

- **SIGTERM and SIGINT end it at once, chosen by the author while building**, through tokio's `signal`
  feature: it logs and exits with nothing to finish, since every phase of a step is already saved and a
  restart continues from it. Without a handler it would run as PID 1 in Docker, ignore SIGTERM and be
  killed 10 seconds later.
- **Two commands, chosen by the author:** `keryx-agent --config <path> run` starts it, and `check` reads
  `agent.toml`, every wake script and the skills, says what is wrong and exits, so a bad config is caught
  before deploying. **Fine, it prints one line on stdout, such as `agent.toml is fine: 3 wake rules, 2
  skills`, and exits 0; otherwise every problem goes to stderr and it exits 1, chosen by the author.** `--config` is required, as on the server, with no environment variables.
- **Every path in `agent.toml` is absolute, and a relative one stops startup naming its key, chosen by
  the author**, the server's rule, wake scripts and the skills directory included. An unknown key stops
  startup too.
- **The server's logging rules, chosen by the author:** `tracing` JSON to stderr, `--quiet`, `--info`,
  `--debug` and `--trace` as the server's, INFO by default for `run`. **No content is ever logged:**
  message text, prompts, LLM answers, scripts' output, secrets, the words and the token live in
  `agent.db`'s run log only. Lines carry chat ids, seqs, run ids, step numbers, sizes, codes and times.
- **The server's make targets, chosen by the author:** `all` (clippy, coverage, check-style), `dev`,
  `release`, `clippy`, `check-style`, `fmt`, `lint`, `test`, `coverage`, `docker`, `clean`, `version` and
  `start-dev`, with a coverage table in `agent/tests/AGENTS.md`.

## Identity and connection

- **The URL, token and words sit in `agent.toml`, chosen by the author**, since nobody is there to type
  them. Whoever can read that file holds the account, as with the operator token. The words reach the
  owner from their own app when the agent is made: see Agent accounts in the root AGENTS.md.
- **One device, which only its owner's token renew ends, chosen by the author.** The server allows an
  agent one device and refuses ending it with the words, so whoever copied `agent.toml` cannot replace
  the running agent with their own. A container that lost its volume needs the owner to renew the token
  and put the new one in `agent.toml`: the owner types the agent's words into the renew, their core signs
  the old device's termination, and the server ends it so members remove it from MLS. keryx-agent,
  starting with no snapshot, then registers a new device. See Agent accounts in the root AGENTS.md.
- **The token is a JWT, chosen by the author:** keryx-agent reads its `sub` to ask its device-login
  challenge by account, and logs in again with it whenever its session ends.
- **It keeps its connection and retries forever**, as the author's first sketch said.
- **Presence only, chosen by the author:** online while connected, offline when it is down, as any
  device. No typing while it works.
- **Yellow, having trouble, while something needs its owner, chosen by the author:** its LLM calls
  failing and backing off, which replaced away; storage full; storage past `storage_warn`, waiting for
  the owner's answer; and a run ended on the tool-call limit or unable to compact, waiting to be told to
  continue. It goes back to green once the cause clears. **The reason is its presence text, sent to its
  owner alone, chosen by the author:** a code, `llm_failing`, `storage_full`, `storage_warn` or `stuck`,
  then a short detail, such as `llm_failing: HTTP 429 since 12m` or `storage_warn: 8.3 of 10 GiB`. Apps
  translate the code into the owner's language and show the detail as it is. See Presence in the root AGENTS.md.
- **Whoever runs its container reads everything it sees.** The owner's app cannot open the agent's
  chats, but `agent.db`, the index, the words and the snapshot sit in its volume, so whoever holds the
  volume reads every chat the agent is in. Members of a chat with an agent should take it as that
  person's.

## Storage

- **One SQLite file, `agent.db`, with the server's `DELETE` journal, chosen by the author:** both
  snapshot slots, the archive of raw frames as the server sent them, both queues, the run logs,
  schedules and memory notes, so one transaction covers a loop step and its queue changes.
- **Its schema follows the server's rules in `agent/src/agent.sql`, chosen by the author:** ULIDs as
  `BLOB(16)`, unix milliseconds, enums as integers with their values listed, `WITHOUT ROWID` for small
  rows, every field naming the feature that needs it, and `PRAGMA user_version`. **The tables, approved
  by the author:**

  ```
  snapshot    (slot 0|1 PK, bytes, saved_at)            core's two slots
  session     (id 0 PK, token)                          the login core hands back, for Loaded
  message     (chat, seq, kind, content_kind, device, pointer, topic, thread_seq, saved_at, frame)
              PK (chat, seq)                            the archive of raw frames, as Android's
  call_row    (chat, call_seq, n, kind, outcome, frame)
  chat_event  (chat, n, audience, server_at, frame)  PK (chat, n)
                                                        a group's or channel's history rows, as Android's
  event       (id PK, kind, chat?, topic?, thread_seq?, seq?, body JSON, received_at,
               state 0 waiting | 1 taken | 2 deferred, until?, run?, step?)
              index (state, received_at)
  outgoing    (id PK, run?, step?, n?, chat, command JSON, client_at,
               state 0 queued | 1 sent | 2 done | 3 refused, seq?, code?, at)
  run         (id PK, started_at, ended_at?, state 0 running | 1 done | 2 limit | 3 failed)
  step        (run, n, phase 1..6, context JSON, intention_list JSON?, answer JSON?, script?,
               sent_at?, started_at?, ended_at?, cut, tokens_in?, tokens_out?)  PK (run, n)
  step_output (run, step, n, kind 0 print | 1 action, body, at)  PK (run, step, n)
  schedule    (id PK, kind 0 at | 1 every | 2 cron, spec, tz, start?, note, chats BLOB(16 each),
               join, next_at, fired_at?, run?, made_at)  index next_at
  memory      (id PK, chat?, text, run, step, source_chat?, seq?, made_at, updated_at)
  ```

- **`chat_event` keeps a group's or channel's history rows as the server sent them, chosen by the author
  while building:** core's `SaveRows` hands them over and its `ReadRows` asks for them back, and
  `keryx.history_list` reads them, so they last across restarts and a delete's copies stay ciphertext
  on disk.
- **Events are one table and stay once taken, chosen by the author**, pointing at the run and step that
  took them, as the run log's record of what woke it, with no copy in the step.
- **Each print and each action is its own `step_output` row, chosen by the author**, inserted as it
  happens, so a crash loses at most the one being written.
- **An `at` schedule's row is removed in the transaction that saves its fire event, chosen by the
  author**; the event keeps the note, the planned and actual times and why it was late.
- **`keryx.chat_list` and `keryx.chat` answer from core's state, chosen by the author:** its snapshot
  and the `chats` list since connecting, so nothing opened reaches the disk unless `[search] plain` is
  on. Before the first connection they return `offline`.
- **The search index is a tantivy directory, `index/`, beside `agent.db`, chosen by the author.** Messages
  are indexed only when `[search] plain` is on; memory notes always are, since `memory.note_list`
  searches them. The index stores each note's document key so a hit can be resolved to its note.
  The index's last commit names the newest row it covers. Startup rebuilds a missing or
  incompatible index from `agent.db`, preserving the notes and the archive, as the author chose.
- **The plain search database is optional, chosen by the author.** Turning it on means the owner trusts
  the environment to hold plaintext, and gives the LLM rich search. Off, the agent uses core's
  archive scan and continues through older pages until it fills the requested page with matches
  that pass the filters or reaches the end, as the author chose.
- **It follows borhan's design in keryx-agent's own copy, chosen by the author:** tantivy and not FTS5,
  since borhan does not use FTS5, borhan's normalizer vendored, and borhan's query syntax and coverage.
  One session per conversation, with its folders, topics, threads and senders stored too. Unlike borhan
  it edits and deletes per message, since a deleted keryx message must leave at once.
- **A storage cap instead of time-based retention, chosen by the author:** `[limits] storage`, 10 GiB,
  covers `agent.db`, the index, `tmp/` and `chats/`, so the disk, which is what runs out, is what is
  limited. **Past `storage_warn`, 0.8, a `storage` event wakes the loop** with the usage by kind, and the
  model proposes what to remove, asks its owner with a keryx question, and removes what was agreed. **At
  the cap**, run logs keep only their action lists, and downloads, `file.write` and `memory.add` return
  `storage_full`. **The archive and both queues always keep writing**, since losing messages is worse
  than passing the cap. Deferred events stay until taken, as they were put back on purpose.
- **Run logs are kept `[limits] runs_keep = "90d"`, chosen by the author**, and until then removed only
  by `storage.remove_runs`. Older runs go at startup and once a day, each with its steps, outputs, the
  events it took and its sent outgoing rows, in one transaction. Unset keeps them until removed. Memory
  notes keep their source ids when the run is gone.

## Queues and durability

- **Both queues are on disk, chosen by the author.** Everything from the server goes into the incoming
  queue, and everything to the server comes from the outgoing queue.
- **Everything is saved after every phase of a step, so a restart continues where it stopped, chosen by
  the author:**

  ```
  1 input built: the events taken and their context     saved
  2 LLM call sent                                       saved
  3 LLM answer, the tool call                           saved
  4 script started                                      saved
  5 each print and each action, as it happens           saved
  6 script finished: its output and action list         saved
  ```

  On restart, after 1 or 2 the LLM call is sent again; after 3 the script runs; after 4 or 5 the script
  is never run again, and its output is what was saved, marked as cut by a restart, with its actions;
  after 6 the next step is built. An LLM failure at 2 backs off and retries, across restarts too.
- **An unanswered send is matched before it goes again, chosen by the author.** After a restart, a
  message in the outgoing queue with no sequence id is looked for by syncing its chat for this device's
  message with the same client send time and body. Found, it takes that seq; otherwise it is sent.
- **LLM failures back off forever, chosen by the author**, with a cap and honouring `Retry-After`, with
  presence away until a call succeeds.

## The loop

- **One main loop, chosen by the author.** No runs in parallel: events from every chat and schedule feed
  the one loop.
- **Each wake starts fresh, chosen by the author.** The model gets the system message, the waiting
  events and their chat background. It can reach past runs, memory and skills through Starlark.
- **The system message is `agent.toml`'s prompt as written, then a fixed preamble, with no templating,
  chosen by the author:** who it is (`ME`, `OWNER`), the time, that it acts only through `starlark`, the
  tool's rules, and the skills' names and descriptions. Events go in the user message as grouped JSON.
- **Waiting events go in all at once, grouped by chat, then topic, then thread, chosen by the author**,
  each group with its context. **Within each group, events appear in the order they were received,
  chosen by the author**, so a deferred event keeps its place. The fresh event that wakes the run
  still selects its rule, so an older deferred event cannot change the run's overrides.
- **New events are added before each LLM call, chosen by the author**, so the model always sees the
  latest without asking.
- **One background budget per chat, topic and thread group, chosen by the author.** `[context]`
  `around` and `around_bytes` default to 10 messages and 16 KiB of serialized JSON. Both can be
  overridden by `[wake.context]`. The agent takes the nearest earlier messages and stops when
  the next one exceeds the remaining budget, so the model sees the closest exchange in order.
  A thread's starter is included in full when available, even when it exceeds the limit. It gives
  the replies their context and gets the first slot before any other background messages.
- **Messages shown as events or background are tracked through restarts and excluded from later
  background in the same wake until compaction, chosen by the author.**
- **What is shown is taken, chosen by the author.** `inbox.defer(ids, after?)` puts events back, per
  event or per group, for the next wake or a later time. The author first asked for an explicit ack;
  taken-by-default was chosen so a model that forgets to ack cannot cause endless redelivery.
- **Each message is one JSON object, chosen by the author:** seq, sender username and display name,
  time, kind, topic, thread, reply-to, text or caption, and files as name, type and size without their
  bytes.
- **The final text is not sent by default, chosen by the author:** the model acts on the server through
  Starlark only. `send_final` sends it as a reply to the trigger, for models that answer in text anyway.
- **Compaction, chosen by the author:** the config holds a compaction prompt, with a default, and a
  maximum context size in bytes. When the input reaches that size, or the provider answers with a
  context-length error, the loop compacts with that prompt. **A failed compaction call backs off like any
  LLM call; if the result still does not fit, the oldest steps' printed output is replaced by a note,
  keeping their action lists; if even that does not fit, the wake ends through the limit prompt, chosen
  by the author**, and the run log keeps everything.
- **A limit of tool calls per wake, 50 by default, chosen by the author.** On reaching it, a configured
  prompt, with a default, gives the model those steps to produce an outcome, such as telling the chat it
  hit the limit, and the wake ends. A person can then say in the chat to continue or stop, and the next
  wake reads the previous run through Starlark.

## Waking

- **Ordered `[[wake]]` rules, first match wins, chosen by the author**, each matching on any of chat
  ids, chat type, folder, topic, thread, sender usernames, content kind, event kind, mentions me and
  replies to me, with action wake or ignore. No match ignores. A rule may override the prompts and the
  context size.
- **Its keys, chosen by the author:** lists `chat` (chat ids), `chat_type` (`self`, `direct`, `group`,
  `channel`), `folder` (names, folded as search folds), `topic` (topic ids), `sender` (usernames),
  `content_kind` and `event` (`message`, `reaction`, `answer`, `ack`, `added`, `removed`, `invitation`,
  `join_request`, `edit`, `delete`); booleans `thread` (true only in threads, false only in main lists),
  `mentions_me` and `replies_to_me`; `script`; and `action`. Every condition set must hold, any item of a
  list may match, and a rule with none matches everything. **Overrides are `[wake.prompt]` and
  `[wake.context]`, with the keys of the top-level tables.**
- **A rule may carry a Starlark script defining `wake(event)`, returning True or False, chosen by the
  author.** `event` is the JSON the loop would see, with its chat's context. It may read, with keryx's
  reading functions, search and memory. **A failing script or an attempt to write, send or call HTTP
  skips the rule and is always logged, chosen by the author.** Later rules can still match the event.
  **`report_failure = false` is per rule, chosen by the author.** When enabled, each failure sends
  one encrypted text message with the rule number and a short reason to the owner's direct chat.
  The agent opens the chat with its usual permissions if needed. The report carries no chat
  content or script output, so failures in other chats can be reported without sharing their contents.
- **Rules match these events, chosen by the author:** content messages; reactions on its messages;
  answers to its questions and acks of its suggestions; membership, being added, removed or invited,
  or a join request where it may let people in; and edits and deletes. Schedules and the `storage`
  event wake on their own.
- **Membership additions wait up to the existing five-second hold for the group or channel
  name, chosen by the author.** At expiry the wake carries the chat ID and any name available.
- **The agent matches verified delete events to their saved archive rows, chosen by the author.**
  This works when core reports the delete before the row arrives. The saved row supplies the
  sending device and time for the wake rules.
- **Wake scripts have separate background limits, chosen by the author.**
  `[context_limits.wake_script]` has `count = 10` and `bytes = "16KiB"` by default.
- **An invitation wakes the loop to decide, chosen by the author**, which accepts or declines it from
  Starlark.

## Schedules

- **`at`, `every` and `cron`, chosen by the author**, each kept on disk with an id, listed and removed by
  id.
- **A schedule has a note, 0 to N chats and a flag, chosen by the author.** The note is what the LLM left
  for its future self. The flag says whether it may join a running loop at its next step, or waits until
  the loop is idle.
- **Firing gives the loop the note, the planned and actual times, and each named chat's nearby messages,
  chosen by the author**, deduplicated as events' context is. No chats means the note alone.
  `[context_limits.schedule]` independently sets the per-chat background limits. The defaults are
  `count = 10` and `bytes = "16KiB"`.
- **`agent.toml`'s top-level `timezone`, an IANA name, `"UTC"` by default, chosen by the author**, is
  `schedule.cron`'s zone when it names none, and the preamble shows the time in it beside UTC, so the
  model knows its owner's day. Built-ins still take and return UTC.
- **A fire that is late still fires, chosen by the author**, and the loop is told how late and why: the
  agent was down, or the schedule waited for the loop to end.

## LLM

- **OpenAI's protocol, not Claude's, chosen by the author.** Chat Completions first, stateless, sending
  the whole input on each call so the disk log is the only state. The Responses API comes later, behind
  a setting.

## Skills

- **An optional skills folder, chosen by the author:** `[skills] directory`, unset by default. Every
  `.md` file in it has frontmatter with `name` and `description`; their names and descriptions go into
  the preamble, and `skills.read(name, ...)` prints whole skills into the loop. A file without valid
  frontmatter is skipped and logged by `run`, so valid skills remain available. As the author chose,
  `check` reports the invalid file and exits 1 so it can be caught before deploying.
- **With `writable = true`, the model can create, replace and remove skills, chosen by the author**, and
  the preamble tells it to do so only when a person in the chat asks or agrees.

## Starlark

- **facebook's Starlark as the one tool, chosen by the author.** The LLM sees one tool, `starlark`, with
  two arguments, `script` and `intention_list`. Its description holds the `value, err` rule, the naming rules, every built-in's
  signature, the globals and the error codes, built from the code so it never drifts. External MCP
  servers come later.
- **Every step says what it is for, in an `intention_list`, chosen by the author:** `[{"intention":
  "Posting the new meeting time", "chat_list": [team]}, {"intention": "Telling Sara it is done",
  "chat_list": [sara]}]`. At least one entry; each intention at most 140 characters; a `chat_list` item is
  a chat id or `{chat, topic?, thread?}`; a chat in at most one entry, so it never sees two texts for one
  step; an empty `chat_list` is kept in the log only. Each entry is sent to each of its chats as an
  `activity` message, a content kind of its own (see Agent accounts in the root AGENTS.md), when the
  step starts, and the list is saved on the step. **The model writes one text per audience**, since one
  run can work on a private chat and a group at once and a text written for both would carry one into
  the other. **`keryx.activity(text, chat_list)` sends more mid-script, chosen by the author.** **Writes are
  not held to the named chats, chosen by the author:** a chat no entry names simply sees no intention
  for that step.
- **A step's output is everything the script printed and the list of actions it took, chosen by the
  author**, appended to the loop's context and saved. **Every side effect is an action, chosen by the
  author:** each keryx write with its result (seq, queued or err), each HTTP call as method, host, path
  and status with no bodies or secrets, and each schedule, memory, skill, storage and file change, defer
  and attach.
- **Every built-in returns `value, err` as Go does, chosen by the author:** `(value, None)` on success
  and `(None, err)` on failure, bad arguments included. **`err` is a dict, chosen by the author:**
  `{"code", "message"}`, plus keys where they help, such as `seconds` for `slow_mode`.
- **Error codes are the server's and a fixed list of keryx-agent's own, chosen by the author.** A
  refusal from keryx keeps the server's code, such as `no_permission`, `slow_mode` or `not_member`.
  keryx-agent's own are `bad_argument`, `offline`, `timeout`, `not_found`, `host_not_allowed`,
  `secret_not_allowed`, `too_large`, `memory_full`, `storage_full` and `path_not_allowed`.
- **Built-ins are grouped in modules with dots, chosen by the author:** `keryx`, `search`, `log`,
  `memory`, `skills`, `storage`, `http`, `file`, `schedule`, `inbox`, `llm` and `time`, beside `secret()`
  and Starlark's own `json`.
- **Naming, chosen by the author:** a function returning a list ends in `_list` and returns a page,
  `{"<name>_list": [...], "more": bool}`; a function returning one record is singular; one returning a
  dict keyed by id or username is plural. Every list in a result is named `_list` too.
- **Arguments, chosen by the author:** what is required is positional, such as the chat id, and every
  filter or option is a keyword argument with a sensible default, so a model can follow them simply.
  **Functions follow keryx's protocol**, one call per protocol action.
- **Times are UTC ISO-8601 strings and durations are strings such as `"30s"`, `"5m"` and `"7d"`, chosen
  by the author**, as `agent.toml` writes them.
- **Chats, files, schedules, runs and notes are named by ULID; wherever a person is meant, a username or
  an account id is accepted, chosen by the author**, and results give both with the display name.
- **The `keryx` module covers reading and messaging, questions and folder suggestions, and groups and
  channels, chosen by the author.** It has no calls, no sign-up, words, devices or logout, and does not
  manage its own profile, folders or settings.
- **Writes wait for the server, or queue, chosen by the author.** Every write takes `timeout="10s"` and
  `enqueue=True`. It waits up to `timeout` for the server's answer: `{"seq"}`, or `err` with the server's
  code. Offline or past `timeout`, with `enqueue=True` it returns `{"queued": id}` and still goes later,
  its seq arriving as an event; with `enqueue=False` it returns `err` `offline` or `timeout` and nothing
  is kept. Scripts read the global `CONNECTED` first if they care.
- **`seen` is sent when events are shown to the loop, chosen by the author**, up to their seq, since
  shown means taken, so a person's seen mark means the agent got it. Messages no wake rule matched stay
  unseen. `keryx.seen` exists beside it.
- **HTTP accepts an `insecure` flag, chosen by the author**, which skips certificate checks. **`[http]
  allow` unset means any host, and a list, even an empty one, means only those hosts, chosen by the
  author.** **A non-2xx answer is a value with its status, chosen by the author**; `err` is only for the
  network, timeouts, a host not allowed or a body too large.
- **Secrets are opaque handles, chosen by the author.** `[secrets] name = value` in `agent.toml`;
  `secret("name")` prints as `<secret name>`, works only in header values and the URL, and goes only to
  the hosts it is bound to. Its value never reaches the script or the LLM, since any chat member can
  steer the model.
- **Long-term memory is borhan-like and searched only, chosen by the author**, in the same tantivy index
  as its own kind. Each note records its source, the chat, sender and run, and shows it when read back.
  Nothing is put into a wake on its own, since a note planted through a chat would then steer every
  wake. **Besides the global memory, each chat has its own, chosen by the author**, through the same
  functions with `chat=`, and searched only as well. **The preamble hints what is there, chosen by the
  author:** for the global memory and each chat whose events are shown, its note count and its
  `[memory] hint_words = 10` most common words from the index's lexicon, 0 turning the words off, so the
  model knows when to search. **`memory_notes` counts every note together, chosen by the author**, the
  global memory's and every chat's, so the disk use stays bounded; past it `memory.add` returns
  `memory_full` in any memory. **At most `[limits] memory_notes = 10000` of `memory_note = "16KiB"`, chosen by the author**;
  past the count, `memory.add` returns `memory_full`.
- **Files, chosen by the author:** the file functions write and remove only in `tmp/`, and read in
  `tmp/` and in `chats/<chat id>/`, where chat files are kept per conversation. `keryx.send_files` and
  `llm.attach` take paths in either. `..` and absolute paths are refused. `llm.attach(path)` adds an image
  to the next LLM input as Chat Completions image content, for models with vision. Files stay until
  removed, under the storage cap.
- **Limits, in `agent.toml`, with defaults chosen by the author:** a script runs at most 30 s and prints
  at most 64 KiB, cut with a note past that; 50 tool calls per wake; a wake rule's script 1 s; an HTTP
  call 30 s and 10 MiB per answer.

### The built-ins, approved by the author

Each returns `value, err`. Every keryx write also takes `timeout="10s", enqueue=True` and returns
`{seq}` or `{queued}`.

```
# keryx: reading
keryx.chat_list(type=None, folder=None, name=None, limit=50)
  -> {chat_list: [{id, type, name, role, folder_list, topic_list: [{id, name}], unread, last_at,
      preview}], more}                                   newest activity first
keryx.chat(chat) -> {id, type, name, bio, link_list, rules, role, topics_on, topic_list, options,
      slow_mode, findable, folder_list}
keryx.message_list(chat, before=None, after=None, limit=20, topic=None, thread=None)
  -> {message_list, more}                                oldest first; neither before nor after: the newest
keryx.message(chat, seq) -> message + reaction_list, edit_list, thread_count, answer_list
keryx.member_list(chat, role=None, limit=200)
  -> {member_list: [{id, username, display_name, role, agent}], more}  groups and channels only
keryx.request_list(chat) -> {request_list: [{id, username, note, at}], more}
keryx.invitation_list() -> {invitation_list: [{chat, inviter, name, note, at}], more}
keryx.times(chat, seq) -> {username: {received_at, seen_at}}
keryx.history_list(chat, topic=None, limit=50) -> {history_list: [{actor, action, target, seq, at}], more}
keryx.profile(person) -> {id, username, display_name, bio, link_list, agent, owner, presence}
keryx.directory_list(text, limit=20) -> {directory_list: [{kind: account|chat, id, name, ...}], more}
keryx.pack_list() -> {pack_list: [{id, name, sticker_list: [{id, emoji}]}], more}

message = {chat, seq, topic, thread, sender: {id, username, display_name}, at, kind, text, caption,
           reply, mention_list, file_list: [{id, name, type, size}], edited, deleted}

# keryx: messaging
keryx.send(chat, text, topic=None, thread=None, reply=None, mention_list=[])
keryx.send_files(chat, path_list, caption=None, kind=None, description_list=None, topic=None,
                 thread=None, reply=None)
  kind None: from each file's type; several images and videos become one media_bundle; music is MP3
keryx.sticker(chat, sticker, topic=None, thread=None, reply=None)
keryx.react(chat, seq, emoji)
keryx.unreact(chat, seq, emoji)                          deletes its own reaction
keryx.edit(chat, seq, text)
keryx.delete(chat, seq)
keryx.seen(chat, seq)
keryx.download(chat, seq, file=None) -> {path, name, type, size}
  under chats/<chat id>/; file is required when the message has several
keryx.activity(text, chat_list) -> {seqs: {chat: seq}}   one activity message to each chat, as a
  step's intention_list sends; a chat_list item is a chat id or {chat, topic?, thread?}

# keryx: questions and folder suggestions
keryx.question(chat, first, nodes, text=None, topic=None, thread=None, reply=None)
keryx.answer(chat, question, answers=None, skipped=False)
keryx.edit_answer(chat, seq, answers)
keryx.suggest(chat, folder, move=False, text=None, topic=None, thread=None)
keryx.accept_suggestion(chat, seq)
keryx.decline_suggestion(chat, seq)

# keryx: groups and channels
keryx.direct(person) -> {chat}
keryx.create(type, name, person_list=[], bio=None, link_list=[], rules=None, topics=False,
             findable=False, options={}, photo=None, note=None) -> {chat}
keryx.add(chat, person_list, note=None)
keryx.remove(chat, person_list)
keryx.role(chat, person, role)                           owner | moderator | member
keryx.leave(chat)
keryx.remove_chat(chat)
keryx.ask(chat, note=None)           keryx.cancel_ask(chat)
keryx.let_in(chat, person)           keryx.decline_ask(chat, person)
keryx.answer_invite(chat, accept)
keryx.topics(chat, on)
keryx.topic_create(chat, name) -> {topic}
keryx.topic_rename(chat, topic, name)
keryx.topic_remove(chat, topic)
keryx.chat_profile(chat, name=None, bio=None, link_list=None, rules=None, photo=None)
keryx.chat_options(chat, reactions=None, stickers=None, files=None, members_add=None, slow_mode=None)
keryx.topic_profile(chat, topic, bio=None, link_list=None, rules=None, photo=None)
keryx.topic_options(chat, topic, closed=None, hidden=None, write=None, slow_mode=None, ...)
keryx.topic_moderator(chat, topic, person, on=True)
keryx.findable(chat, on, name=None, bio=None)

# search, its own past runs, memory, skills, storage
search.message_list(query, chat=None, folder=None, topic=None, thread=None, sender=None, since=None,
                    until=None, limit=20)
  -> {message_list: [message + {snippet}], more, unknown_list, hint_list}
  plain index on: borhan's query syntax and coverage; off: plain text folded as the apps fold, newest first
log.run_list(query=None, since=None, until=None, limit=20)
  -> {run_list: [{id, started_at, ended_at, outcome, snippet}], more}
log.run(id) -> {id, started_at, ended_at, outcome, event_list,
                step_list: [{n, intention_list, script, output, at}]}
memory.add(text, chat=None) -> {id}                      chat None: the global memory
memory.note_list(query=None, chat=None, limit=20)
  -> {note_list: [{id, chat, text, source: {chat, sender, run}, at}], more}  query None: the newest
memory.note(id)    memory.replace(id, text)    memory.delete(id)
skills.read(name, ...)                                   prints each skill
skills.skill_list() -> {skill_list: [{name, description}], more}
skills.write(name, description, body)    skills.remove(name)               only when writable
storage.usage() -> {total, limit, runs, memory, files, archive, index}
storage.remove_runs(before)    storage.remove_files(path_list)

# http
http.request(method, url, headers=None, body=None, json=None, form=None, timeout="30s", insecure=False)
  -> {status, headers, body, json}                       json set when the answer is JSON
http.get(url, headers=None, timeout="30s", insecure=False)
http.post(url, body=None, json=None, form=None, headers=None, timeout="30s", insecure=False)
secret(name)                                             a handle, printed as <secret name>

# files: write in tmp/, read in tmp/ and chats/<chat id>/
file.write(path, text, append=False) -> {path, size}
file.read(path, offset=0, limit="64KiB") -> {text, more}
file.path_list(dir="") -> {path_list: [{path, size, at}], more}
file.remove(path)

# schedule, inbox, llm, time
schedule.at(time, note, chat_list=[], join=False) -> {id}
schedule.every(duration, note, chat_list=[], join=False, start=None) -> {id}
schedule.cron(expr, note, tz=None, chat_list=[], join=False) -> {id}   tz None: agent.toml's timezone
schedule.schedule_list() -> {schedule_list: [{id, kind, when, next_at, note, chat_list, join}], more}
schedule.remove(id)
inbox.defer(id_list, after=None)                         ids of events or groups
llm.attach(path)
time.now() -> "2026-11-15T09:12:00Z"    time.add(time, duration)    time.diff(a, b) -> duration

# globals
CONNECTED    ME {id, username, display_name}    OWNER {id, username, display_name}
```

**`keryx.member_list` is for groups and channels, chosen by the author.** A direct chat returns
`not_group`. The model can check the chat type in `keryx.chat(chat)` before calling.

**`keryx.create` waits for the new group or channel to appear in the agent's chat view, chosen by the
author.** The next command can use it immediately. Topic edits use the acknowledged topics switch.

## `agent.toml`, approved by the author

```toml
url = "http://127.0.0.1:7777"
token = "..."
words = "..."
directory = "/var/lib/keryx-agent"   # agent.db, index/, tmp/, chats/<chat id>/
timezone = "UTC"                     # IANA name: schedules' default tz, and the preamble's local time

[llm]
protocol = "chat_completions"        # later: "responses"
url = "https://api.openai.com/v1"
key = "..."
model = "..."
backoff_max = "5m"

[prompt]
system = """..."""
# compact = """..."""                # unset: the built-in default
# limit = """..."""                  # unset: the built-in default
send_final = false

[context]
max = "256KiB"                       # input bytes before compaction
around = 10                          # earlier messages per chat/topic/thread group
around_bytes = "16KiB"               # total JSON bytes of that background

[context_limits.schedule]
count = 10
bytes = "16KiB"

[context_limits.wake_script]
count = 10
bytes = "16KiB"

[search]
plain = false

[memory]
hint_words = 10                      # most common words per memory in the preamble. 0: counts only

# [skills]
# directory = "/var/lib/keryx-agent/skills"
# writable = false

[limits]
script_time = "30s"
script_output = "64KiB"
tool_calls = 50
wake_script = "1s"
http_time = "30s"
http_body = "10MiB"
memory_notes = 10000
memory_note = "16KiB"
storage = "10GiB"
storage_warn = 0.8
runs_keep = "90d"                    # run logs older than this go. Unset: kept until removed

# [http]
# allow = ["api.github.com"]         # unset: any host. A list, even empty: only those

# [secrets]
# github = { value = "...", hosts = ["api.github.com"] }

[[wake]]
chat_type = ["direct"]
action = "wake"

# [[wake]]
# chat = ["01J..."]
# sender = ["sara"]
# event = ["message"]
# mentions_me = true
# script = "/var/lib/keryx-agent/wake/sara.star"   # defines wake(event)
# report_failure = false              # true: send each script failure to the owner in a direct chat
# action = "wake"
```

## Testing

- **The real server and a fake LLM, chosen by the author:** integration tests in `agent/tests` start
  the real keryx server and a small fake OpenAI-compatible server answering from a script per test, and
  drive the agent through wakes, Starlark calls, failures and backoff, compaction, checkpoints and
  restarts, schedules, skills and the storage cap. `server/examples/drive.rs` plays the owner. Then a
  real model against the author's phone.

## Open: do not decide these, ask

- The built-in default texts of the compaction prompt, the limit prompt and the preamble, and the
  `starlark` tool's description: written with the code, for the author's approval before they are kept
- The Responses API, and calling external MCP servers from scripts, when they come
