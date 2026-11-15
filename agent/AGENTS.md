# keryx-agent

An LLM agent that lives in keryx as its own account. It holds that account's words and keys like any
device, stays connected to one keryx server, keeps everything on disk, and wakes one LLM loop when
configured events arrive. Its one tool is a Starlark script runner.

**Planned, nothing built yet; every part below was answered in the author's Q/A of 2026-11-15 and
2026-11-16**, so building can start. The server's side, agent accounts, is in the root
[AGENTS.md](../AGENTS.md) under Agent accounts.

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
  hide, a warning before adding one, admin-only by default, each in its own process. Splitting the
  crate into a library then is mechanical, so there is no `lib.rs` now.

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
| `jiff` | 0.2 | Times, durations and IANA zones, with its zone database bundled so the image needs no tzdata. One time library |
| `tracing`, `tracing-subscriber` | in the lock | Logging, by the server's rules |

**Cron is parsed by keryx-agent's own small parser, chosen by the author:** five fields, minute, hour,
day, month and weekday, with ranges, lists and steps, and jiff finding the next time.

### Files, chosen by the author

```
agent/Cargo.toml
agent/Makefile
agent/AGENTS.md
agent/Dockerfile
agent/src/main.rs       CLI, agent.toml, logging, startup
agent/src/agent.toml    the commented template, include_str!'d
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

- **Two commands, chosen by the author:** `keryx-agent --config <path> run` starts it, and `check` reads
  `agent.toml`, every wake script and the skills, says what is wrong and exits, so a bad config is caught
  before deploying. `--config` is required, as on the server, with no environment variables.
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
- **One device, replaced by itself, chosen by the author.** Starting with no snapshot, keryx-agent signs
  its old device's termination with its words, then registers a new one, so the old device's keys stop
  being members. **The server allows an agent one device and tells its owner of each new one, chosen by
  the author:** see Agent accounts in the root AGENTS.md.
- **It keeps its connection and retries forever**, as the author's first sketch said.
- **Presence only, chosen by the author:** online while connected, offline when it is down, as any
  device. No typing while it works.
- **Yellow, having trouble, while something needs its owner, chosen by the author:** its LLM calls
  failing and backing off, which replaced away; storage full; storage past `storage_warn`, waiting for
  the owner's answer; and a run ended on the tool-call limit or unable to compact, waiting to be told to
  continue. It goes back to green once the cause clears. **The reason is its presence text, sent to its
  owner alone, chosen by the author.** See Presence in the root AGENTS.md.

## Storage

- **One SQLite file, `agent.db`, with the server's `DELETE` journal, chosen by the author:** both
  snapshot slots, the archive of raw frames as the server sent them, both queues, the run logs,
  schedules, memory notes, and the search tables when enabled, so one transaction covers a loop step and
  its queue changes.
- **The plain search database is optional, chosen by the author.** Turning it on means the owner trusts
  the environment to hold plaintext, and gives the LLM rich search. Off, search scans the archive as
  core's search does, which is slow and hard for the LLM.
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
  than passing the cap. Run logs stay until removed this way, as past runs are the agent's memory of
  what it did, and deferred events stay until taken, as they were put back on purpose.

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
- **Each wake starts fresh, chosen by the author:** the system message, the waiting events, and around
  each event a configurable number of nearby messages from its chat. What it did before is reachable
  only through Starlark, in its past runs, its memory and its skills.
- **The system message is `agent.toml`'s prompt as written, then a fixed preamble, with no templating,
  chosen by the author:** who it is (`ME`, `OWNER`), the time, that it acts only through `starlark`, the
  tool's rules, and the skills' names and descriptions. Events go in the user message as grouped JSON.
- **Waiting events go in all at once, grouped by chat, then topic, then thread, chosen by the author**,
  each group with its context.
- **New events are added before each LLM call, chosen by the author**, so the model always sees the
  latest without asking.
- **Context already in the loop is not sent again, chosen by the author**, until after a compaction.
- **What is shown is taken, chosen by the author.** `inbox.defer(ids, after?)` puts events back, per
  event or per group, for the next wake or a later time. The author first asked for an explicit ack;
  taken-by-default was chosen so a model that forgets to ack cannot cause endless redelivery.
- **Each message is one JSON object, chosen by the author:** seq, sender username and display name,
  time, kind, topic, thread, reply-to, text or caption, and files as name, type and size without their
  bytes.
- **The final text is not sent by default, chosen by the author:** the agent acts on the server through
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
- **A rule may carry a Starlark script defining `wake(event)`, returning True or False, chosen by the
  author.** `event` is the JSON the loop would see, with its chat's context. It may read, with keryx's
  reading functions, search and memory, but not write, send or call HTTP. An `err` or a crash means do
  not wake, and is logged.
- **Rules match these events, chosen by the author:** content messages; reactions on its messages;
  answers to its questions and acks of its suggestions; membership, being added, removed or invited,
  or a join request where it may let people in; and edits and deletes. Schedules and the `storage`
  event wake on their own.
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
  frontmatter is skipped and logged.
- **With `writable = true`, the model can create, replace and remove skills, chosen by the author**, and
  the preamble tells it to do so only when a person in the chat asks or agrees.

## Starlark

- **facebook's Starlark as the one tool, chosen by the author.** The LLM sees one tool, `starlark`, with
  one argument, `script`. Its description holds the `value, err` rule, the naming rules, every built-in's
  signature, the globals and the error codes, built from the code so it never drifts. External MCP
  servers come later.
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
  wake. **At most `[limits] memory_notes = 10000` of `memory_note = "16KiB"`, chosen by the author**;
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
  -> {member_list: [{id, username, display_name, role, agent}], more}
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
log.run(id) -> {id, started_at, ended_at, outcome, event_list, step_list: [{n, script, output, at}]}
memory.add(text) -> {id}
memory.note_list(query=None, limit=20)
  -> {note_list: [{id, text, source: {chat, sender, run}, at}], more}      query None: the newest
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
schedule.cron(expr, note, tz="UTC", chat_list=[], join=False) -> {id}
schedule.schedule_list() -> {schedule_list: [{id, kind, when, next_at, note, chat_list, join}], more}
schedule.remove(id)
inbox.defer(id_list, after=None)                         ids of events or groups
llm.attach(path)
time.now() -> "2026-11-15T09:12:00Z"    time.add(time, duration)    time.diff(a, b) -> duration

# globals
CONNECTED    ME {id, username, display_name}    OWNER {id, username, display_name}
```

## `agent.toml`, approved by the author

```toml
url = "http://127.0.0.1:7777"
token = "..."
words = "..."
directory = "/var/lib/keryx-agent"   # agent.db, index/, tmp/, chats/<chat id>/

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
around = 20                          # nearby messages per event

[search]
plain = false

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
# script = "wake/sara.star"          # defines wake(event)
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
- The words of each trouble reason in its presence text
- The Responses API, and calling external MCP servers from scripts, when they come
