//! Starlark: the tool, every built-in, secrets, files, skills and the action list. See agent/AGENTS.md,
//! Starlark.
//!
//! Every built-in is one row of [`BUILTIN_LIST`], its signature written as the model reads it. The
//! same rows check each call's arguments and build the `starlark` tool's description, so the two never
//! drift. A call goes through [`call`] as JSON both ways and returns `(value, err)`.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use keryx_core::client::{
    Command, Event, FileSpec, PhotoInput, Pointer, Secret, Shown, Thumbnail, fold_name,
};
use keryx_core::protocol::{
    ChatOptions, ChatRole, ChatType, ContentKind, FolderSuggestion, ProfileLink, Question,
    Suggestion, TopicName, TopicOptionsBody, TopicWrite,
};
use keryx_core::ulid::Ulid;
use serde_json::{Map, Value, json};
use starlark::PrintHandler;
use starlark::any::ProvidesStaticType;
use starlark::collections::SmallMap;
use starlark::environment::{Globals, GlobalsBuilder, LibraryExtension, Module};
use starlark::eval::Evaluator;
use starlark::starlark_module;
use starlark::syntax::{AstModule, Dialect};
use starlark::values::Value as Star;
use starlark::values::tuple::UnpackTuple;
use tokio::sync::oneshot;

use crate::connect::{
    Answer, Ask, Shared, View, chat_type_name, content_kind_name, http_client, lock, message_value,
    new_id, now, role_name, time_text,
};
use crate::index::{DocKind, Filter, directory_size};
use crate::run::{ContextBudget, Settings};
use crate::schedule::Cron;
use crate::store::{NoteRow, OutgoingRow, OutgoingState, OutputRow, Rows, ScheduleRow};

/// How long a read waits for the server, within the script's own limit.
const READ_TIME: Duration = Duration::from_secs(20);

/// The most one page of a list may hold.
const PAGE_MAX: u64 = 500;

/// The draft of the `starlark` tool's description, for the author's approval before it is kept: the
/// rules, then every built-in's signature from [`BUILTIN_LIST`].
const TOOL_RULES: &str = "\
Runs a Starlark script: the one way to act. Nothing you write as text reaches anyone; a script does.

Rules:
- Every built-in returns (value, err), as Go does: (value, None) on success, (None, err) on failure,
  bad arguments included. err is a dict {code, message}, plus keys where they help, such as seconds
  for slow_mode. Always check err.
- Error codes are the server's, such as no_permission, slow_mode or not_member, and keryx-agent's own:
  bad_argument, offline, timeout, not_found, host_not_allowed, secret_not_allowed, too_large,
  memory_full, storage_full and path_not_allowed.
- A function returning a list ends in _list and returns a page {<name>_list: [...], more}. One
  returning a dict keyed by id or username is plural.
- Required arguments are positional; every filter or option is a keyword argument.
- Times are UTC ISO-8601 strings, such as \"2026-11-15T09:12:00Z\"; durations are strings such as
  \"30s\", \"5m\" or \"7d\"; sizes such as \"64KiB\".
- Chats, files, schedules, runs, notes and events are named by id; wherever a person is meant, a
  username or an account id works.
- A topic of None means any topic, and \"general\" the General topic.
- Every keryx write also takes timeout=\"10s\", enqueue=True and returns {seq}: it waits up to timeout
  for the server. Offline or past timeout, with enqueue=True it returns {queued: id} and still goes
  later, its seq arriving as an event; with enqueue=False it returns err offline or timeout.
- print() what you need to see: printed text and the list of actions the script took come back as the
  step's output. Each script starts empty: keep anything you need in memory, files or schedules.
- secret(name) is a handle printed as <secret name>. It works only inside http header values and the
  URL, and only for the hosts it is bound to.
- Files: write and remove only under tmp/; read under tmp/ and chats/<chat id>/.
- json.encode and json.decode are there.
- intention_list says what this step is for, one text per audience: [{\"intention\": \"...\",
  \"chat_list\": [chat id or {chat, topic?, thread?}]}]. At least one entry, each at most 140
  characters, a chat in at most one entry; an empty chat_list is kept in the log only.

Globals: CONNECTED, ME {id, username, display_name}, OWNER {id, username, display_name}.

Built-ins:
";

/// Who may call a built-in: anything, or only a step's script, not a wake rule's, which only reads.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Reach {
    Read,
    /// A keryx write: also takes `timeout` and `enqueue`.
    Write,
    /// A change outside keryx, such as memory, files or HTTP.
    Change,
}

/// One built-in: its signature as the model reads it, and what it returns.
struct Builtin {
    signature: &'static str,
    returns: &'static str,
    reach: Reach,
}

const fn read(signature: &'static str, returns: &'static str) -> Builtin {
    Builtin {
        signature,
        returns,
        reach: Reach::Read,
    }
}

const fn write(signature: &'static str, returns: &'static str) -> Builtin {
    Builtin {
        signature,
        returns,
        reach: Reach::Write,
    }
}

const fn change(signature: &'static str, returns: &'static str) -> Builtin {
    Builtin {
        signature,
        returns,
        reach: Reach::Change,
    }
}

/// Every built-in, as agent/AGENTS.md lists them, approved by the author.
const BUILTIN_LIST: &[Builtin] = &[
    read(
        "keryx.chat_list(type=None, folder=None, name=None, limit=50)",
        "{chat_list: [{id, type, name, role, folder_list, topic_list: [{id, name}], unread, last_at, preview}], more}, newest activity first",
    ),
    read(
        "keryx.chat(chat)",
        "{id, type, name, bio, link_list, rules, role, topics_on, topic_list, options, slow_mode, findable, folder_list}",
    ),
    read(
        "keryx.message_list(chat, before=None, after=None, limit=20, topic=None, thread=None)",
        "{message_list, more}, oldest first; neither before nor after: the newest",
    ),
    read(
        "keryx.message(chat, seq)",
        "message + {reaction_list, edit_list, thread_count, answer_list}",
    ),
    read(
        "keryx.member_list(chat, role=None, limit=200)",
        "Groups and channels only. Check the chat type with keryx.chat(chat). Returns {member_list: [{id, username, display_name, role, agent}], more}",
    ),
    read(
        "keryx.request_list(chat)",
        "{request_list: [{id, username, note, at}], more}",
    ),
    read(
        "keryx.invitation_list()",
        "{invitation_list: [{chat, inviter, name, note, at}], more}",
    ),
    read(
        "keryx.times(chat, seq)",
        "{username: {received_at, seen_at}}",
    ),
    read(
        "keryx.history_list(chat, topic=None, limit=50)",
        "{history_list: [{actor, action, target, seq, at, copy_list}], more}",
    ),
    read(
        "keryx.profile(person)",
        "{id, username, display_name, bio, link_list, agent, owner, presence}",
    ),
    read(
        "keryx.directory_list(text, limit=20)",
        "{directory_list: [{kind: account|chat, id, name, ...}], more}",
    ),
    read(
        "keryx.pack_list()",
        "{pack_list: [{id, name, sticker_list: [{id, emoji}]}], more}",
    ),
    write(
        "keryx.send(chat, text, topic=None, thread=None, reply=None, mention_list=[])",
        "{seq}",
    ),
    write(
        "keryx.send_files(chat, path_list, caption=None, kind=None, description_list=None, topic=None, thread=None, reply=None)",
        "{seq}; kind None: from each file's type; several images and videos become one media_bundle; music is MP3",
    ),
    write(
        "keryx.sticker(chat, sticker, topic=None, thread=None, reply=None)",
        "{seq}",
    ),
    write("keryx.react(chat, seq, emoji)", "{seq}"),
    write(
        "keryx.unreact(chat, seq, emoji)",
        "{seq}; deletes its own reaction",
    ),
    write("keryx.edit(chat, seq, text)", "{seq}"),
    write("keryx.delete(chat, seq)", "{seq}"),
    write("keryx.seen(chat, seq)", "{seq}"),
    change(
        "keryx.download(chat, seq, file=None)",
        "{path, name, type, size}, under chats/<chat id>/; file is required when the message has several",
    ),
    write(
        "keryx.activity(text, chat_list)",
        "{seqs: {chat: seq}}; one activity message to each chat, as a step's intention_list sends",
    ),
    write(
        "keryx.question(chat, first, nodes, text=None, topic=None, thread=None, reply=None)",
        "{seq}",
    ),
    write(
        "keryx.answer(chat, question, answers=None, skipped=False)",
        "{seq}",
    ),
    write("keryx.edit_answer(chat, seq, answers)", "{seq}"),
    write(
        "keryx.suggest(chat, folder, move=False, text=None, topic=None, thread=None)",
        "{seq}",
    ),
    write("keryx.accept_suggestion(chat, seq)", "{seq}"),
    write("keryx.decline_suggestion(chat, seq)", "{seq}"),
    write("keryx.direct(person)", "{chat}"),
    write(
        "keryx.create(type, name, person_list=[], bio=None, link_list=[], rules=None, topics=False, findable=False, options={}, photo=None, note=None)",
        "{chat}",
    ),
    write("keryx.add(chat, person_list, note=None)", "{seq}"),
    write("keryx.remove(chat, person_list)", "{seq}"),
    write(
        "keryx.role(chat, person, role)",
        "{seq}; owner | moderator | member",
    ),
    write("keryx.leave(chat)", "{seq}"),
    write("keryx.remove_chat(chat)", "{seq}"),
    write("keryx.ask(chat, note=None)", "{seq}"),
    write("keryx.cancel_ask(chat)", "{seq}"),
    write("keryx.let_in(chat, person)", "{seq}"),
    write("keryx.decline_ask(chat, person)", "{seq}"),
    write("keryx.answer_invite(chat, accept)", "{seq}"),
    write("keryx.topics(chat, on)", "{seq}"),
    write("keryx.topic_create(chat, name)", "{topic}"),
    write("keryx.topic_rename(chat, topic, name)", "{seq}"),
    write("keryx.topic_remove(chat, topic)", "{seq}"),
    write(
        "keryx.chat_profile(chat, name=None, bio=None, link_list=None, rules=None, photo=None)",
        "{seq}; link_list: [{name, url}]; photo: a JPEG's path",
    ),
    write(
        "keryx.chat_options(chat, reactions=None, stickers=None, files=None, members_add=None, slow_mode=None)",
        "{seq}; slow_mode: \"off\", \"10s\", \"30s\", \"1m\", \"5m\", \"15m\" or \"1h\"",
    ),
    write(
        "keryx.topic_profile(chat, topic, bio=None, link_list=None, rules=None, photo=None)",
        "{seq}",
    ),
    write(
        "keryx.topic_options(chat, topic, closed=None, hidden=None, write=None, slow_mode=None, reactions=None, stickers=None, files=None, view_received_own=None, view_seen_own=None, view_received_others=None, view_seen_others=None)",
        "{seq}; write: \"everyone\" | \"moderators\"; slow_mode: \"group\", \"off\" or a step as chat_options takes",
    ),
    write(
        "keryx.topic_moderator(chat, topic, person, on=True)",
        "{seq}",
    ),
    write("keryx.findable(chat, on, name=None, bio=None)", "{seq}"),
    read(
        "search.message_list(query, chat=None, folder=None, topic=None, thread=None, sender=None, since=None, until=None, limit=20)",
        "{message_list: [message + {snippet}], more, unknown_list, hint_list}; with the plain index: query syntax (a b) +c -d \"phrase\", parentheses holding one idea in several spellings; without: plain text, newest first",
    ),
    read(
        "log.run_list(query=None, since=None, until=None, limit=20)",
        "{run_list: [{id, started_at, ended_at, outcome, snippet}], more}",
    ),
    read(
        "log.run(id)",
        "{id, started_at, ended_at, outcome, event_list, step_list: [{n, intention_list, script, output, at}]}",
    ),
    change(
        "memory.add(text, chat=None)",
        "{id}; chat None: the global memory",
    ),
    read(
        "memory.note_list(query=None, chat=None, limit=20)",
        "{note_list: [{id, chat, text, source: {chat, sender, run}, at}], more}; query None: the newest",
    ),
    read("memory.note(id)", "{id, chat, text, source, at}"),
    change("memory.replace(id, text)", "{id}"),
    change("memory.delete(id)", "{id}"),
    read("skills.read(*name)", "None; prints each skill"),
    read(
        "skills.skill_list()",
        "{skill_list: [{name, description}], more}",
    ),
    change(
        "skills.write(name, description, body)",
        "{name}; only when skills are writable, and only when a person asks or agrees",
    ),
    change("skills.remove(name)", "{name}"),
    read(
        "storage.usage()",
        "{total, limit, runs, memory, files, archive, index}, in bytes",
    ),
    change("storage.remove_runs(before)", "{count}"),
    change("storage.remove_files(path_list)", "{count}"),
    change(
        "http.request(method, url, headers=None, body=None, json=None, form=None, timeout=\"30s\", insecure=False)",
        "{status, headers, body, json}; json set when the answer is JSON; a non-2xx answer is a value",
    ),
    change(
        "http.get(url, headers=None, timeout=\"30s\", insecure=False)",
        "as http.request",
    ),
    change(
        "http.post(url, body=None, json=None, form=None, headers=None, timeout=\"30s\", insecure=False)",
        "as http.request",
    ),
    change("file.write(path, text, append=False)", "{path, size}"),
    read("file.read(path, offset=0, limit=\"64KiB\")", "{text, more}"),
    read(
        "file.path_list(dir=\"\")",
        "{path_list: [{path, size, at}], more}",
    ),
    change("file.remove(path)", "{path}"),
    change("schedule.at(time, note, chat_list=[], join=False)", "{id}"),
    change(
        "schedule.every(duration, note, chat_list=[], join=False, start=None)",
        "{id}",
    ),
    change(
        "schedule.cron(expr, note, tz=None, chat_list=[], join=False)",
        "{id}; tz None: agent.toml's timezone",
    ),
    read(
        "schedule.schedule_list()",
        "{schedule_list: [{id, kind, when, next_at, note, chat_list, join}], more}",
    ),
    change("schedule.remove(id)", "{id}"),
    change(
        "inbox.defer(id_list, after=None)",
        "{count}; ids of events or groups, back for the next wake, or not before `after`",
    ),
    change(
        "llm.attach(path)",
        "{path}; an image for the next LLM input",
    ),
    read("time.now()", "\"2026-11-15T09:12:00Z\""),
    read("time.add(time, duration)", "a time"),
    read("time.diff(a, b)", "a duration, b minus a"),
];

/// The `starlark` tool, as Chat Completions takes it.
pub fn tool() -> Value {
    let mut description = TOOL_RULES.to_string();
    for builtin in BUILTIN_LIST {
        description.push_str(builtin.signature);
        description.push_str(" -> ");
        description.push_str(builtin.returns);
        description.push('\n');
    }
    json!({
        "type": "function",
        "function": {
            "name": "starlark",
            "description": description,
            "parameters": {
                "type": "object",
                "properties": {
                    "script": {"type": "string", "description": "The Starlark script to run."},
                    "intention_list": {
                        "type": "array",
                        "description": "What this step is for, one text per audience.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "intention": {"type": "string"},
                                "chat_list": {"type": "array", "items": {}}
                            },
                            "required": ["intention", "chat_list"]
                        }
                    }
                },
                "required": ["script", "intention_list"]
            }
        }
    })
}

/// A built-in's parameters, read from its signature.
struct Params {
    positional: Vec<String>,
    /// `*name`: the rest of the positional arguments, as a list.
    rest: Option<String>,
    named: Vec<(String, Value)>,
}

fn params_of(builtin: &Builtin) -> Params {
    let open = builtin.signature.find('(').unwrap_or(0);
    let inner = &builtin.signature[open + 1..builtin.signature.len() - 1];
    let mut params = Params {
        positional: Vec::new(),
        rest: None,
        named: Vec::new(),
    };
    for part in split_top(inner) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some(rest) = part.strip_prefix('*') {
            params.rest = Some(rest.to_string());
        } else if let Some((name, default)) = part.split_once('=') {
            params
                .named
                .push((name.to_string(), literal(default.trim())));
        } else {
            params.positional.push(part.to_string());
        }
    }
    if builtin.reach == Reach::Write {
        params.named.push(("timeout".to_string(), json!("10s")));
        params.named.push(("enqueue".to_string(), json!(true)));
    }
    params
}

/// Splits on commas outside brackets and quotes.
fn split_top(text: &str) -> Vec<&str> {
    let mut list = Vec::new();
    let mut depth = 0;
    let mut quoted = false;
    let mut start = 0;
    for (at, character) in text.char_indices() {
        match character {
            '"' => quoted = !quoted,
            '[' | '{' | '(' if !quoted => depth += 1,
            ']' | '}' | ')' if !quoted => depth -= 1,
            ',' if !quoted && depth == 0 => {
                list.push(&text[start..at]);
                start = at + 1;
            }
            _ => {}
        }
    }
    list.push(&text[start..]);
    list
}

/// A default as a signature writes it: None, True, False, a number, a string, [] or {}.
fn literal(text: &str) -> Value {
    match text {
        "None" => Value::Null,
        "True" => json!(true),
        "False" => json!(false),
        "[]" => json!([]),
        "{}" => json!({}),
        _ => {
            if let Some(inner) = text.strip_prefix('"').and_then(|t| t.strip_suffix('"')) {
                return json!(inner);
            }
            match text.parse::<i64>() {
                Ok(number) => json!(number),
                Err(_) => json!(text),
            }
        }
    }
}

/// A refusal, as a script's `err`.
#[derive(Debug)]
pub struct Fail {
    code: String,
    message: String,
    extra: Map<String, Value>,
}

impl Fail {
    pub fn message(&self) -> &str {
        &self.message
    }

    fn json(&self) -> Value {
        let mut object = self.extra.clone();
        object.insert("code".to_string(), json!(self.code));
        object.insert("message".to_string(), json!(self.message));
        Value::Object(object)
    }
}

fn fail(code: &str, message: impl Into<String>) -> Fail {
    Fail {
        code: code.to_string(),
        message: message.into(),
        extra: Map::new(),
    }
}

fn bad(message: impl Into<String>) -> Fail {
    fail("bad_argument", message)
}

/// The server's refusal of an answered command.
fn refusal(answer: &Answer) -> Fail {
    let code = answer.code.clone().unwrap_or_default();
    let mut fail = fail(
        &code,
        answer
            .message
            .clone()
            .unwrap_or_else(|| format!("the server refused: {code}")),
    );
    if let Some(seconds) = answer.seconds {
        fail.extra.insert("seconds".to_string(), json!(seconds));
    }
    fail
}

/// One call's arguments, by name.
struct Args {
    map: Map<String, Value>,
}

impl Args {
    fn get(&self, name: &str) -> &Value {
        self.map.get(name).unwrap_or(&Value::Null)
    }

    fn present(&self, name: &str) -> bool {
        !self.get(name).is_null()
    }

    fn string(&self, name: &str) -> Result<String, Fail> {
        match self.get(name) {
            Value::String(text) => Ok(text.clone()),
            Value::Number(number) => Ok(number.to_string()),
            _ => Err(bad(format!("{name} is a string"))),
        }
    }

    fn opt_string(&self, name: &str) -> Result<Option<String>, Fail> {
        match self.get(name) {
            Value::Null => Ok(None),
            _ => self.string(name).map(Some),
        }
    }

    fn ulid(&self, name: &str) -> Result<Ulid, Fail> {
        let text = self.string(name)?;
        Ulid::parse(&text).map_err(|_| bad(format!("{name} is an id")))
    }

    fn opt_ulid(&self, name: &str) -> Result<Option<Ulid>, Fail> {
        match self.get(name) {
            Value::Null => Ok(None),
            _ => self.ulid(name).map(Some),
        }
    }

    /// A topic: None for any, `"general"` for General, otherwise its id.
    fn topic(&self, name: &str) -> Result<Option<Option<Ulid>>, Fail> {
        match self.get(name) {
            Value::Null => Ok(None),
            Value::String(text) if text == "general" => Ok(Some(None)),
            _ => self.ulid(name).map(|id| Some(Some(id))),
        }
    }

    /// A topic a message goes in: none for General.
    fn send_topic(&self, name: &str) -> Result<Option<Ulid>, Fail> {
        Ok(self.topic(name)?.flatten())
    }

    fn u64(&self, name: &str) -> Result<u64, Fail> {
        match self.get(name) {
            Value::Number(number) => number
                .as_u64()
                .ok_or_else(|| bad(format!("{name} is a whole number"))),
            Value::String(text) => text
                .parse()
                .map_err(|_| bad(format!("{name} is a whole number"))),
            _ => Err(bad(format!("{name} is a whole number"))),
        }
    }

    fn opt_u64(&self, name: &str) -> Result<Option<u64>, Fail> {
        match self.get(name) {
            Value::Null => Ok(None),
            _ => self.u64(name).map(Some),
        }
    }

    fn limit(&self) -> Result<u64, Fail> {
        Ok(self.u64("limit")?.clamp(1, PAGE_MAX))
    }

    fn bool(&self, name: &str) -> Result<bool, Fail> {
        match self.get(name) {
            Value::Bool(flag) => Ok(*flag),
            _ => Err(bad(format!("{name} is True or False"))),
        }
    }

    fn opt_bool(&self, name: &str) -> Result<Option<bool>, Fail> {
        match self.get(name) {
            Value::Null => Ok(None),
            _ => self.bool(name).map(Some),
        }
    }

    fn list(&self, name: &str) -> Result<Vec<Value>, Fail> {
        match self.get(name) {
            Value::Null => Ok(Vec::new()),
            Value::Array(list) => Ok(list.clone()),
            _ => Err(bad(format!("{name} is a list"))),
        }
    }

    fn strings(&self, name: &str) -> Result<Vec<String>, Fail> {
        let mut list = Vec::new();
        for item in self.list(name)? {
            match item {
                Value::String(text) => list.push(text),
                _ => return Err(bad(format!("{name} is a list of strings"))),
            }
        }
        Ok(list)
    }

    fn duration(&self, name: &str) -> Result<Duration, Fail> {
        duration_of(&self.string(name)?).ok_or_else(|| bad(format!("{name} is a duration")))
    }

    fn time(&self, name: &str) -> Result<i64, Fail> {
        time_of(&self.string(name)?).ok_or_else(|| bad(format!("{name} is a UTC ISO-8601 time")))
    }

    fn opt_time(&self, name: &str) -> Result<Option<i64>, Fail> {
        match self.get(name) {
            Value::Null => Ok(None),
            _ => self.time(name).map(Some),
        }
    }

    fn links(&self, name: &str) -> Result<Vec<ProfileLink>, Fail> {
        serde_json::from_value(Value::Array(self.list(name)?))
            .map_err(|_| bad(format!("{name} is a list of {{name, url}}")))
    }
}

/// A duration as `agent.toml` writes them: `"500ms"`, `"30s"`, `"5m"`, `"2h"`, `"7d"`, `"1w"`.
pub fn duration_of(text: &str) -> Option<Duration> {
    let text = text.trim();
    let split = text.find(|c: char| !c.is_ascii_digit())?;
    let (number, unit) = text.split_at(split);
    let number: u64 = number.parse().ok()?;
    let milliseconds = match unit {
        "ms" => 1,
        "s" => 1_000,
        "m" => 60_000,
        "h" => 3_600_000,
        "d" => 86_400_000,
        "w" => 604_800_000,
        _ => return None,
    };
    Some(Duration::from_millis(number.checked_mul(milliseconds)?))
}

/// A duration written back as the largest unit that divides it.
pub fn duration_text(milliseconds: i64) -> String {
    let sign = if milliseconds < 0 { "-" } else { "" };
    let value = milliseconds.unsigned_abs();
    for (unit, size) in [
        ("d", 86_400_000),
        ("h", 3_600_000),
        ("m", 60_000),
        ("s", 1_000),
    ] {
        if value >= size && value.is_multiple_of(size) {
            return format!("{sign}{}{unit}", value / size);
        }
    }
    format!("{sign}{value}ms")
}

/// A size as `"64KiB"` or a plain number of bytes.
fn size_of(value: &Value) -> Option<u64> {
    match value {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => {
            let text = text.trim();
            let split = text
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(text.len());
            let (number, unit) = text.split_at(split);
            let number: u64 = number.parse().ok()?;
            let size = match unit.trim() {
                "" | "B" => 1,
                "KiB" => 1 << 10,
                "MiB" => 1 << 20,
                "GiB" => 1 << 30,
                "KB" => 1_000,
                "MB" => 1_000_000,
                "GB" => 1_000_000_000,
                _ => return None,
            };
            number.checked_mul(size)
        }
        _ => None,
    }
}

/// A UTC ISO-8601 time, as unix milliseconds.
pub fn time_of(text: &str) -> Option<i64> {
    let time: jiff::Timestamp = text.trim().parse().ok()?;
    Some(time.as_millisecond())
}

/// What a step runs in: which run and step, and the groups of events it was shown.
pub enum Mode {
    Step {
        run: Ulid,
        step: u64,
        /// The events each group named in the input holds, for `inbox.defer`.
        groups: HashMap<String, Vec<Ulid>>,
        /// The chat and message a memory note comes from, when the step is about one.
        source: Option<(Ulid, Option<Ulid>, Option<u64>)>,
        /// The first output's number: after a restart, those already saved come first.
        first_n: u64,
    },
    /// A `[[wake]]` rule's script: reads only.
    Wake,
}

/// What a script did.
#[derive(Debug, Default)]
pub struct Outcome {
    /// Everything it printed, cut at `[limits] script_output` with a note.
    pub output: String,
    pub action_list: Vec<Value>,
    /// Starlark's own error, when it did not run to its end.
    pub error: Option<String>,
    /// `llm.attach`'s images for the next input.
    pub attach_list: Vec<PathBuf>,
}

/// What every script shares: the agent's parts and the built-ins.
pub struct Env {
    pub shared: Arc<Shared>,
    pub settings: Arc<Settings>,
    pub runtime: tokio::runtime::Handle,
    pub globals: Globals,
    pub owner: Mutex2<Value>,
}

/// A plain mutex for one small value, the owner as the server names it.
pub type Mutex2<T> = std::sync::Mutex<T>;

impl Env {
    pub fn new(
        shared: Arc<Shared>,
        settings: Arc<Settings>,
        runtime: tokio::runtime::Handle,
    ) -> Self {
        Self {
            shared,
            settings,
            runtime,
            globals: globals(),
            owner: std::sync::Mutex::new(Value::Null),
        }
    }

    pub fn me(&self) -> Value {
        let view = lock(&self.shared.view);
        match view.account {
            Some(account) => {
                let mut person = view.person(account);
                if person["username"].is_null() {
                    person["username"] = json!(view.username);
                }
                person
            }
            None => Value::Null,
        }
    }
}

/// The built-ins, Starlark's own extras the model may want, and `print`.
fn globals() -> Globals {
    let mut builder = GlobalsBuilder::extended_by(&[
        LibraryExtension::StructType,
        LibraryExtension::Map,
        LibraryExtension::Filter,
        LibraryExtension::Partial,
        LibraryExtension::Print,
        LibraryExtension::Json,
    ]);
    builder.namespace("keryx", keryx_module);
    builder.namespace("search", search_module);
    builder.namespace("log", log_module);
    builder.namespace("memory", memory_module);
    builder.namespace("skills", skills_module);
    builder.namespace("storage", storage_module);
    builder.namespace("http", http_module);
    builder.namespace("file", file_module);
    builder.namespace("schedule", schedule_module);
    builder.namespace("inbox", inbox_module);
    builder.namespace("llm", llm_module);
    builder.namespace("time", time_module);
    top_module(&mut builder);
    builder.build()
}

/// One Starlark module of built-ins, each forwarding to [`builtin`] by its dotted name.
macro_rules! module {
    ($module:ident, $prefix:literal, [$($name:ident),* $(,)?]) => {
        #[starlark_module]
        fn $module(builder: &mut GlobalsBuilder) {
            $(
                fn $name<'v>(
                    #[starlark(args)] args: UnpackTuple<Star<'v>>,
                    #[starlark(kwargs)] kwargs: SmallMap<String, Star<'v>>,
                    eval: &mut Evaluator<'v, '_, '_>,
                ) -> starlark::Result<Star<'v>> {
                    builtin(concat!($prefix, ".", stringify!($name)), args.items, kwargs, eval)
                }
            )*
        }
    };
}

module!(
    keryx_module,
    "keryx",
    [
        chat_list,
        chat,
        message_list,
        message,
        member_list,
        request_list,
        invitation_list,
        times,
        history_list,
        profile,
        directory_list,
        pack_list,
        send,
        send_files,
        sticker,
        react,
        unreact,
        edit,
        delete,
        seen,
        download,
        activity,
        question,
        answer,
        edit_answer,
        suggest,
        accept_suggestion,
        decline_suggestion,
        direct,
        create,
        add,
        remove,
        role,
        leave,
        remove_chat,
        ask,
        cancel_ask,
        let_in,
        decline_ask,
        answer_invite,
        topics,
        topic_create,
        topic_rename,
        topic_remove,
        chat_profile,
        chat_options,
        topic_profile,
        topic_options,
        topic_moderator,
        findable,
    ]
);
module!(search_module, "search", [message_list]);
module!(log_module, "log", [run_list, run]);
module!(
    memory_module,
    "memory",
    [add, note_list, note, replace, delete]
);
module!(skills_module, "skills", [read, skill_list, write, remove]);
module!(
    storage_module,
    "storage",
    [usage, remove_runs, remove_files]
);
module!(http_module, "http", [request, get, post]);
module!(file_module, "file", [write, read, path_list, remove]);
module!(
    schedule_module,
    "schedule",
    [at, every, cron, schedule_list, remove]
);
module!(inbox_module, "inbox", [defer]);
module!(llm_module, "llm", [attach]);
module!(time_module, "time", [now, add, diff]);

#[starlark_module]
fn top_module(builder: &mut GlobalsBuilder) {
    /// A secret's handle, `<secret name>`: it works only in HTTP header values and URLs, for the
    /// hosts it is bound to, and its value never reaches the script.
    fn secret<'v>(name: &str, eval: &mut Evaluator<'v, '_, '_>) -> starlark::Result<String> {
        let context = context(eval)?;
        if !context.env.settings.secrets.contains_key(name) {
            return Err(starlark::Error::new_other(anyhow::anyhow!(
                "no secret is named {name:?}"
            )));
        }
        Ok(format!("<secret {name}>"))
    }
}

/// What a script runs with, reached from each built-in through the evaluator.
#[derive(ProvidesStaticType)]
struct Context {
    env: Arc<Env>,
    mode: Mode,
    forbidden: Cell<bool>,
    deadline: Instant,
    limit: Duration,
    output: RefCell<String>,
    cut: Cell<bool>,
    action_list: RefCell<Vec<Value>>,
    next_n: Cell<u64>,
    attach_list: RefCell<Vec<PathBuf>>,
}

fn context<'a>(eval: &'a Evaluator<'_, '_, '_>) -> starlark::Result<&'a Context> {
    let found = eval.extra.and_then(|extra| extra.downcast_ref::<Context>());
    match found {
        Some(context) => Ok(context),
        None => Err(starlark::Error::new_other(anyhow::anyhow!(
            "built-ins run only inside keryx-agent"
        ))),
    }
}

impl PrintHandler for Context {
    fn println(&self, text: &str) -> starlark::Result<()> {
        let max = usize::try_from(self.env.settings.limits.script_output).unwrap_or(usize::MAX);
        let line = {
            let mut output = self.output.borrow_mut();
            if self.cut.get() {
                return Ok(());
            }
            let room = max.saturating_sub(output.len());
            if text.len() < room {
                output.push_str(text);
                output.push('\n');
                text.to_string()
            } else {
                let mut end = room;
                while end > 0 && !text.is_char_boundary(end) {
                    end -= 1;
                }
                let note = format!(
                    "{}\n[output cut at {} bytes: print less, or write to a file]\n",
                    &text[..end],
                    max
                );
                output.push_str(&note);
                self.cut.set(true);
                note
            }
        };
        self.save_output(false, line);
        Ok(())
    }
}

impl Context {
    fn remaining(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }

    /// Waits for a future, at most `limit` and never past the script's own end.
    fn block<F: std::future::Future>(&self, future: F, limit: Duration) -> Option<F::Output> {
        let wait = limit.min(self.remaining());
        self.env
            .runtime
            .block_on(async { tokio::time::timeout(wait, future).await.ok() })
    }

    fn take_n(&self) -> u64 {
        let n = self.next_n.get();
        self.next_n.set(n + 1);
        n
    }

    /// Saves a print or an action as it happens, phase 5 of a step.
    fn save_output(&self, action: bool, body: String) {
        let Mode::Step { run, step, .. } = &self.mode else {
            return;
        };
        let row = OutputRow {
            n: self.take_n(),
            action,
            body,
            at: now(),
        };
        if let Err(error) = lock(&self.env.shared.store).add_output(*run, *step, &row) {
            tracing::error!(run = %run, step, error = %error, "could not save a step's output");
        }
    }

    /// Records a side effect in the action list.
    fn action(&self, action: Value) {
        self.action_list.borrow_mut().push(action.clone());
        self.save_output(true, action.to_string());
    }

    fn view(&self) -> std::sync::MutexGuard<'_, View> {
        lock(&self.env.shared.view)
    }

    /// Sends a command and waits for its answer.
    fn ask(
        &self,
        command: Command,
        handles: Vec<(u64, PathBuf)>,
        limit: Duration,
    ) -> Result<Answer, Fail> {
        let (reply, answer) = oneshot::channel();
        let sent = self.env.shared.asks.send(Ask::Command {
            command: Box::new(command),
            reply: Some(reply),
            outgoing: None,
            handles,
        });
        if sent.is_err() {
            return Err(fail("offline", "the connection is gone"));
        }
        match self.block(answer, limit) {
            Some(Ok(answer)) => Ok(answer),
            Some(Err(_)) => Err(fail("offline", "the connection is gone")),
            None => Err(fail("timeout", "the server did not answer in time")),
        }
    }

    /// A read: the server's answer, or its refusal.
    fn read(&self, command: Command) -> Result<Answer, Fail> {
        let answer = self.ask(command, Vec::new(), READ_TIME)?;
        if answer.code.is_some() {
            return Err(refusal(&answer));
        }
        Ok(answer)
    }

    /// A write: waits up to `timeout`, or queues, as agent/AGENTS.md says. A write in a chat is kept
    /// in the outgoing queue, so a restart sends it again or finds it stored.
    fn write(
        &self,
        name: &str,
        args: &Args,
        chat: Option<Ulid>,
        command: Command,
        handles: Vec<(u64, PathBuf)>,
    ) -> Result<Answer, Fail> {
        let timeout = args.duration("timeout")?;
        let enqueue = args.bool("enqueue")?;
        let mut action = json!({"action": name});
        if let Some(chat) = chat {
            action["chat"] = json!(chat.to_string());
        }
        if !enqueue && !self.view().connected {
            action["err"] = json!("offline");
            self.action(action);
            return Err(fail("offline", "not connected, and enqueue is False"));
        }
        let outgoing = match (enqueue, chat) {
            (true, Some(chat)) => {
                let (run, step) = match &self.mode {
                    Mode::Step { run, step, .. } => (Some(*run), Some(*step)),
                    Mode::Wake => (None, None),
                };
                let id = new_id();
                let row = OutgoingRow {
                    id,
                    run,
                    step,
                    n: Some(self.next_n.get()),
                    chat,
                    command: serde_json::to_value(&command).unwrap_or(Value::Null),
                    client_at: now(),
                    state: OutgoingState::Sent,
                    seq: None,
                    code: None,
                };
                if let Err(error) = lock(&self.env.shared.store).add_outgoing(&row, now()) {
                    tracing::error!(error = %error, "could not keep a write in the outgoing queue");
                }
                Some(id)
            }
            _ => None,
        };
        let (reply, answer) = oneshot::channel();
        let sent = self.env.shared.asks.send(Ask::Command {
            command: Box::new(command),
            reply: Some(reply),
            outgoing,
            handles,
        });
        if sent.is_err() {
            action["err"] = json!("offline");
            self.action(action);
            return Err(fail("offline", "the connection is gone"));
        }
        let result = match self.block(answer, timeout) {
            Some(Ok(answer)) => match &answer.code {
                Some(code) if code == "offline" => match outgoing {
                    Some(id) => Ok(Err(id)),
                    None => Err(refusal(&answer)),
                },
                Some(_) => Err(refusal(&answer)),
                None => Ok(Ok(answer)),
            },
            Some(Err(_)) => Err(fail("offline", "the connection is gone")),
            None => match outgoing {
                Some(id) => Ok(Err(id)),
                None => Err(fail("timeout", "the server did not answer in time")),
            },
        };
        match result {
            Ok(Ok(answer)) => {
                action["seq"] = json!(answer.seq);
                self.action(action);
                Ok(answer)
            }
            Ok(Err(id)) => {
                action["queued"] = json!(id.to_string());
                self.action(action);
                Err(Fail {
                    code: "queued".to_string(),
                    message: id.to_string(),
                    extra: Map::new(),
                })
            }
            Err(failed) => {
                action["err"] = json!(failed.code);
                self.action(action);
                Err(failed)
            }
        }
    }

    /// Opens rows of the archive through core, answering with what it showed.
    fn open(&self, chat: Ulid, rows: Rows) -> Result<Vec<Event>, Fail> {
        let (reply, events) = oneshot::channel();
        let sent = self.env.shared.asks.send(Ask::Open { chat, rows, reply });
        if sent.is_err() {
            return Err(fail("offline", "the connection is gone"));
        }
        match self.block(events, READ_TIME) {
            Some(Ok(events)) => Ok(events),
            _ => Err(fail("timeout", "the archive could not be opened in time")),
        }
    }

    /// One stored message, opened, with its newest text.
    fn shown(&self, chat: Ulid, seq: u64) -> Result<Shown, Fail> {
        let rows = lock(&self.env.shared.store)
            .read_one(chat, seq)
            .map_err(|error| fail("internal", error.to_string()))?;
        if rows.message_list.is_empty() {
            return Err(fail("not_found", format!("no message {seq} in this chat")));
        }
        let events = self.open(chat, rows)?;
        let mut found = None;
        let mut newest = None;
        for event in events {
            match event {
                Event::Message { message } if message.chat == chat && message.seq == seq => {
                    found = Some(message);
                }
                Event::Edited {
                    message,
                    target_seq,
                } if message.chat == chat && target_seq == seq => newest = message.text,
                _ => {}
            }
        }
        let mut shown = found.ok_or_else(|| fail("not_found", "it could not be opened"))?;
        if newest.is_some() {
            shown.text = newest;
        }
        Ok(shown)
    }

    /// An account by id or username: one core has shown, or found in the directory.
    fn person(&self, value: &Value) -> Result<Ulid, Fail> {
        let text = match value {
            Value::String(text) => text.trim().trim_start_matches('@').to_string(),
            Value::Object(object) => match object.get("id").and_then(Value::as_str) {
                Some(id) => id.to_string(),
                None => return Err(bad("a person is a username or an account id")),
            },
            _ => return Err(bad("a person is a username or an account id")),
        };
        if let Ok(id) = Ulid::parse(&text) {
            return Ok(id);
        }
        if let Some(id) = self.view().account_named(&text) {
            return Ok(id);
        }
        let answer = self.read(Command::Search { text: text.clone() })?;
        for event in &answer.event_list {
            if let Event::SearchResults { account_list, .. } = event {
                for account in account_list {
                    if account.username == text {
                        return Ok(account.id);
                    }
                }
            }
        }
        Err(fail("not_found", format!("no account is named {text}")))
    }

    fn people(&self, list: &[Value]) -> Result<Vec<Ulid>, Fail> {
        let mut ids = Vec::new();
        for item in list {
            ids.push(self.person(item)?);
        }
        Ok(ids)
    }

    /// Fetches the profiles of accounts core has not named yet, so results carry their usernames.
    fn learn(&self, account_list: &[Ulid]) {
        let mut waiting = Vec::new();
        for account in account_list {
            let known = self
                .view()
                .accounts
                .get(account)
                .is_some_and(|known| known.username.is_some());
            if known {
                continue;
            }
            let (reply, answer) = oneshot::channel();
            let sent = self.env.shared.asks.send(Ask::Command {
                command: Box::new(Command::Profile { account: *account }),
                reply: Some(reply),
                outgoing: None,
                handles: Vec::new(),
            });
            if sent.is_ok() {
                waiting.push(answer);
            }
        }
        for answer in waiting {
            let _ = self.block(answer, READ_TIME);
        }
    }

    /// A path a script names, under the agent's directory: `tmp/` for writing, and `chats/<chat>/`
    /// too for reading.
    fn path(&self, text: &str, writing: bool) -> Result<PathBuf, Fail> {
        let relative = Path::new(text);
        let mut parts = Vec::new();
        for component in relative.components() {
            match component {
                Component::Normal(part) => parts.push(part.to_string_lossy().to_string()),
                Component::CurDir => {}
                _ => return Err(fail("path_not_allowed", "paths are relative, with no ..")),
            }
        }
        let allowed = match parts.first().map(String::as_str) {
            Some("tmp") => parts.len() >= 2,
            Some("chats") => !writing && parts.len() >= 3,
            _ => false,
        };
        if !allowed {
            let what = if writing {
                "write only under tmp/"
            } else {
                "read only under tmp/ and chats/<chat id>/"
            };
            return Err(fail("path_not_allowed", what));
        }
        let mut path = self.env.settings.directory.clone();
        for part in parts {
            path.push(part);
        }
        Ok(path)
    }

    /// The path a script reads a file back by.
    fn relative(&self, path: &Path) -> String {
        match path.strip_prefix(&self.env.settings.directory) {
            Ok(relative) => relative.to_string_lossy().to_string(),
            Err(_) => path.to_string_lossy().to_string(),
        }
    }

    /// Whether the cap on storage leaves room for more.
    fn room(&self) -> Result<(), Fail> {
        let usage = usage(&self.env);
        if usage.total >= self.env.settings.limits.storage {
            return Err(fail(
                "storage_full",
                "the storage cap is reached: remove runs or files",
            ));
        }
        Ok(())
    }
}

/// Runs one step's script on the calling thread, which may block: the caller runs it on a blocking
/// thread.
pub fn run_step(env: Arc<Env>, mode: Mode, script: &str) -> Outcome {
    let first_n = match &mode {
        Mode::Step { first_n, .. } => *first_n,
        Mode::Wake => 0,
    };
    let limit = env.settings.limits.script_time;
    let context = Context {
        env: env.clone(),
        mode,
        forbidden: Cell::new(false),
        deadline: Instant::now() + limit,
        limit,
        output: RefCell::new(String::new()),
        cut: Cell::new(false),
        action_list: RefCell::new(Vec::new()),
        next_n: Cell::new(first_n),
        attach_list: RefCell::new(Vec::new()),
    };
    let error = evaluate(&context, script, None).err();
    Outcome {
        output: context.output.take(),
        action_list: context.action_list.take(),
        error,
        attach_list: context.attach_list.take(),
    }
}

/// Wake script failures safe to log and report to the owner.
#[derive(Debug, thiserror::Error)]
pub(crate) enum WakeFailure {
    #[error("Its script could not be read.")]
    Unreadable,
    #[error("Its script tried to write or call HTTP.")]
    Forbidden,
    #[error("Its script could not run successfully.")]
    Evaluation,
    #[error("Its script did not return True or False.")]
    NotBoolean,
}

/// Runs a `[[wake]]` rule's script: its `wake(event)` says whether the loop wakes.
pub(crate) fn run_wake(env: Arc<Env>, script: &str, event: &Value) -> Result<bool, WakeFailure> {
    let limit = env.settings.limits.wake_script;
    let context = Context {
        env: env.clone(),
        mode: Mode::Wake,
        forbidden: Cell::new(false),
        deadline: Instant::now() + limit,
        limit,
        output: RefCell::new(String::new()),
        cut: Cell::new(false),
        action_list: RefCell::new(Vec::new()),
        next_n: Cell::new(0),
        attach_list: RefCell::new(Vec::new()),
    };
    match evaluate(&context, script, Some(event)) {
        Ok(Some(woke)) => Ok(woke),
        Ok(None) => Err(WakeFailure::NotBoolean),
        Err(_) if context.forbidden.get() => Err(WakeFailure::Forbidden),
        Err(_) => Err(WakeFailure::Evaluation),
    }
}

/// Checks a wake script for `check`: it parses, runs its top level and defines `wake`.
pub fn check_wake(script: &str) -> Result<(), String> {
    let globals = globals();
    Module::with_temp_heap(|module| {
        let ast = AstModule::parse("wake.star", script.to_string(), &Dialect::Extended)
            .map_err(|error| error.to_string())?;
        let mut eval = Evaluator::new(&module);
        eval.eval_module(ast, &globals)
            .map_err(|error| error.to_string())?;
        match module.get("wake") {
            Some(_) => Ok(()),
            None => Err("it does not define wake(event)".to_string()),
        }
    })
}

/// Evaluates a script, then calls `wake(event)` when an event is given.
fn evaluate(
    context: &Context,
    script: &str,
    event: Option<&Value>,
) -> Result<Option<bool>, String> {
    let env = context.env.clone();
    let connected = lock(&env.shared.view).connected;
    let me = env.me();
    let owner = lock(&env.owner).clone();
    let deadline = context.deadline;
    let limit = context.limit;
    Module::with_temp_heap(|module| {
        let heap = module.heap();
        module.set("CONNECTED", heap.alloc(connected));
        module.set("ME", heap.alloc(me));
        module.set("OWNER", heap.alloc(owner));
        let ast = AstModule::parse("script.star", script.to_string(), &Dialect::Extended)
            .map_err(|error| error.to_string())?;
        let mut eval = Evaluator::new(&module);
        eval.extra = Some(context);
        eval.set_print_handler(context);
        eval.set_check_cancelled(Box::new(move || Instant::now() >= deadline));
        let ran = eval.eval_module(ast, &env.globals);
        if let Err(error) = ran {
            if Instant::now() >= deadline {
                return Err(format!(
                    "the script ran past its limit of {}",
                    duration_text(i64::try_from(limit.as_millis()).unwrap_or(i64::MAX))
                ));
            }
            return Err(error.to_string());
        }
        let Some(event) = event else {
            return Ok(None);
        };
        let Some(wake) = module.get("wake") else {
            return Err("the script does not define wake(event)".to_string());
        };
        let argument = heap.alloc(event.clone());
        let answer = eval
            .eval_function(wake, &[argument], &[])
            .map_err(|error| error.to_string())?;
        Ok(answer.unpack_bool())
    })
}

/// One call of a built-in from a script: its arguments read against its signature, then run.
fn builtin<'v>(
    name: &'static str,
    args: Vec<Star<'v>>,
    kwargs: SmallMap<String, Star<'v>>,
    eval: &mut Evaluator<'v, '_, '_>,
) -> starlark::Result<Star<'v>> {
    let context = context(eval)?;
    let mut found = None;
    for builtin in BUILTIN_LIST {
        if builtin.signature.split('(').next() == Some(name) {
            found = Some(builtin);
            break;
        }
    }
    let result = match found {
        Some(builtin) => {
            if builtin.reach != Reach::Read && matches!(context.mode, Mode::Wake) {
                context.forbidden.set(true);
                return Err(starlark::Error::new_other(WakeFailure::Forbidden));
            }
            match arguments(name, builtin, args, kwargs) {
                Ok(args) => call(context, name, &args),
                Err(failed) => Err(failed),
            }
        }
        None => Err(bad(format!("{name} is not a built-in"))),
    };
    let heap = eval.heap();
    let (value, err) = match result {
        Ok(value) => (value, Value::Null),
        // A write that queued is a value, not an error.
        Err(failed) if failed.code == "queued" => (json!({"queued": failed.message}), Value::Null),
        Err(failed) => (Value::Null, failed.json()),
    };
    Ok(heap.alloc((heap.alloc(value), heap.alloc(err))))
}

/// Reads a call's arguments against its signature, by name.
fn arguments<'v>(
    name: &str,
    builtin: &Builtin,
    args: Vec<Star<'v>>,
    kwargs: SmallMap<String, Star<'v>>,
) -> Result<Args, Fail> {
    let params = params_of(builtin);
    let mut map = Map::new();
    let mut rest = Vec::new();
    for (at, value) in args.into_iter().enumerate() {
        let value = to_json(value)?;
        match params.positional.get(at) {
            Some(param) => {
                map.insert(param.clone(), value);
            }
            None => match &params.rest {
                Some(_) => rest.push(value),
                None => {
                    return Err(bad(format!(
                        "{name} takes {} positional arguments: {}",
                        params.positional.len(),
                        builtin.signature
                    )));
                }
            },
        }
    }
    if let Some(rest_name) = &params.rest {
        map.insert(rest_name.clone(), Value::Array(rest));
    }
    for (key, value) in kwargs {
        let known =
            params.positional.contains(&key) || params.named.iter().any(|(named, _)| *named == key);
        if !known {
            return Err(bad(format!(
                "{name} has no argument {key}: {}",
                builtin.signature
            )));
        }
        map.insert(key, to_json(value)?);
    }
    for param in &params.positional {
        if !map.contains_key(param) {
            return Err(bad(format!("{name} needs {param}: {}", builtin.signature)));
        }
    }
    for (named, default) in &params.named {
        if !map.contains_key(named) {
            map.insert(named.clone(), default.clone());
        }
    }
    Ok(Args { map })
}

fn to_json(value: Star<'_>) -> Result<Value, Fail> {
    value
        .to_json_value()
        .map_err(|error| bad(format!("an argument is not plain data: {error}")))
}

/// Runs a built-in.
fn call(context: &Context, name: &str, args: &Args) -> Result<Value, Fail> {
    match name.split_once('.') {
        Some(("keryx", function)) => keryx(context, function, args),
        Some(("search", _)) => search(context, args),
        Some(("log", function)) => log(context, function, args),
        Some(("memory", function)) => memory(context, function, args),
        Some(("skills", function)) => skills(context, function, args),
        Some(("storage", function)) => storage(context, function, args),
        Some(("http", function)) => http(context, function, args),
        Some(("file", function)) => file(context, function, args),
        Some(("schedule", function)) => schedule(context, function, args),
        Some(("inbox", _)) => inbox(context, args),
        Some(("llm", _)) => attach(context, args),
        Some(("time", function)) => time(function, args),
        _ => Err(bad(format!("{name} is not a built-in"))),
    }
}

// -------------------------------------------------------------------------------------------------
// keryx
// -------------------------------------------------------------------------------------------------

fn keryx(context: &Context, function: &str, args: &Args) -> Result<Value, Fail> {
    match function {
        "chat_list" => chat_list(context, args),
        "chat" => chat(context, args),
        "message_list" => message_list(context, args),
        "message" => message(context, args),
        "member_list" => member_list(context, args),
        "request_list" => {
            let chat = args.ulid("chat")?;
            let answer = context.read(Command::Requests { chat })?;
            let mut list = Vec::new();
            for event in &answer.event_list {
                if let Event::Requests { request_list, .. } = event {
                    let accounts: Vec<Ulid> = request_list.iter().map(|r| r.account).collect();
                    context.learn(&accounts);
                    let view = context.view();
                    for request in request_list {
                        let person = view.person(request.account);
                        list.push(json!({
                            "id": request.account.to_string(),
                            "username": person["username"],
                            "display_name": person["display_name"],
                            "note": request.note,
                            "at": time_text(request.at),
                        }));
                    }
                }
            }
            Ok(json!({"request_list": list, "more": false}))
        }
        "invitation_list" => {
            let view = context.view();
            let mut list = Vec::new();
            for (chat, invitation) in &view.invitations {
                list.push(json!({
                    "chat": chat.to_string(),
                    "type": chat_type_name(invitation.chat_type),
                    "inviter": view.person(invitation.inviter),
                    "name": invitation.name,
                    "note": invitation.note,
                    "at": time_text(invitation.at),
                }));
            }
            Ok(json!({"invitation_list": list, "more": false}))
        }
        "times" => times(context, args),
        "history_list" => history_list(context, args),
        "profile" => profile(context, args),
        "directory_list" => {
            let text = args.string("text")?;
            let limit = usize::try_from(args.limit()?).unwrap_or(usize::MAX);
            let answer = context.read(Command::Search { text })?;
            let mut list = Vec::new();
            for event in &answer.event_list {
                if let Event::SearchResults {
                    account_list,
                    chat_list,
                } = event
                {
                    for account in account_list {
                        list.push(json!({
                            "kind": "account",
                            "id": account.id.to_string(),
                            "username": account.username,
                            "name": account.display_name.clone().unwrap_or_else(|| account.username.clone()),
                        }));
                    }
                    for chat in chat_list {
                        list.push(json!({
                            "kind": "chat",
                            "id": chat.chat.to_string(),
                            "type": chat_type_name(chat.chat_type),
                            "name": chat.name,
                            "bio": chat.bio,
                            "member_count": chat.member_count,
                        }));
                    }
                }
            }
            let more = list.len() > limit;
            list.truncate(limit);
            Ok(json!({"directory_list": list, "more": more}))
        }
        "pack_list" => {
            let answer = context.read(Command::Stickers)?;
            let mut list = Vec::new();
            for event in &answer.event_list {
                if let Event::Stickers { pack_list } = event {
                    for pack in pack_list {
                        let mut stickers = Vec::new();
                        for sticker in &pack.sticker_list {
                            stickers.push(
                                json!({"id": sticker.id.to_string(), "emoji": sticker.emoji}),
                            );
                        }
                        list.push(json!({"id": pack.id.to_string(), "name": pack.name, "sticker_list": stickers}));
                    }
                }
            }
            Ok(json!({"pack_list": list, "more": false}))
        }
        "send" => {
            let chat = args.ulid("chat")?;
            let mentions = context.people(&args.list("mention_list")?)?;
            let command = Command::SendText {
                chat,
                text: Secret(args.string("text")?),
                reply: args.opt_u64("reply")?.map(Pointer::Seq),
                topic: args.send_topic("topic")?,
                thread: args.opt_u64("thread")?,
                mentions,
            };
            seq(context.write("keryx.send", args, Some(chat), command, Vec::new())?)
        }
        "send_files" => send_files(context, args),
        "sticker" => {
            let chat = args.ulid("chat")?;
            let command = Command::SendSticker {
                chat,
                sticker: args.ulid("sticker")?,
                reply: args.opt_u64("reply")?.map(Pointer::Seq),
                topic: args.send_topic("topic")?,
                thread: args.opt_u64("thread")?,
            };
            seq(context.write("keryx.sticker", args, Some(chat), command, Vec::new())?)
        }
        "react" => {
            let chat = args.ulid("chat")?;
            let on = args.u64("seq")?;
            let (topic, thread) = place(context, chat, on);
            let command = Command::SendReaction {
                chat,
                emoji: args.string("emoji")?,
                on: Pointer::Seq(on),
                topic,
                thread,
            };
            seq(context.write("keryx.react", args, Some(chat), command, Vec::new())?)
        }
        "unreact" => {
            let chat = args.ulid("chat")?;
            let on = args.u64("seq")?;
            let emoji = args.string("emoji")?;
            let rows = lock(&context.env.shared.store)
                .read_after(chat, on, 2_000)
                .map_err(|error| fail("internal", error.to_string()))?;
            let events = context.open(chat, rows)?;
            let mut target = None;
            for event in &events {
                if let Event::Message { message } = event
                    && message.on == Some(on)
                    && message.emoji.as_deref() == Some(emoji.as_str())
                    && context.view().own_device(message.device)
                {
                    target = Some(message.seq);
                }
            }
            let Some(target) = target else {
                return Err(fail(
                    "not_found",
                    "no reaction of this agent's with that emoji there",
                ));
            };
            let command = Command::Delete {
                chat,
                target: Pointer::Seq(target),
                remove: Vec::new(),
            };
            seq(context.write("keryx.unreact", args, Some(chat), command, Vec::new())?)
        }
        "edit" => {
            let chat = args.ulid("chat")?;
            let target = args.u64("seq")?;
            let shown = context.shown(chat, target)?;
            let command = Command::Edit {
                chat,
                target: Pointer::Seq(target),
                text: Secret(args.string("text")?),
                files: shown.files,
                remove: Vec::new(),
            };
            seq(context.write("keryx.edit", args, Some(chat), command, Vec::new())?)
        }
        "delete" => {
            let chat = args.ulid("chat")?;
            let target = args.u64("seq")?;
            let remove = match context.shown(chat, target) {
                Ok(shown) => shown.files.iter().map(|file| file.id).collect(),
                Err(_) => Vec::new(),
            };
            let command = Command::Delete {
                chat,
                target: Pointer::Seq(target),
                remove,
            };
            seq(context.write("keryx.delete", args, Some(chat), command, Vec::new())?)
        }
        "seen" => {
            let chat = args.ulid("chat")?;
            let up_to_seq = args.u64("seq")?;
            let (topic, thread) = place(context, chat, up_to_seq);
            let command = Command::Seen {
                chat,
                up_to_seq,
                topic,
                thread,
            };
            context.write("keryx.seen", args, None, command, Vec::new())?;
            Ok(json!({"seq": up_to_seq}))
        }
        "download" => download(context, args),
        "activity" => {
            let text = args.string("text")?;
            let mut seqs = Map::new();
            for item in args.list("chat_list")? {
                let (chat, topic, thread) = audience(&item)?;
                let command = Command::SendActivity {
                    chat,
                    text: text.clone(),
                    topic,
                    thread,
                };
                let answer = context.write("keryx.activity", args, Some(chat), command, Vec::new());
                match answer {
                    Ok(answer) => {
                        seqs.insert(chat.to_string(), json!(answer.seq));
                    }
                    Err(failed) if failed.code == "queued" => {
                        seqs.insert(chat.to_string(), json!({"queued": failed.message}));
                    }
                    Err(failed) => return Err(failed),
                }
            }
            Ok(json!({"seqs": seqs}))
        }
        "question" => {
            let chat = args.ulid("chat")?;
            let question: Question = serde_json::from_value(json!({
                "first": args.string("first")?,
                "nodes": args.list("nodes")?,
            }))
            .map_err(|error| bad(format!("the question does not read: {error}")))?;
            let command = Command::SendQuestion {
                chat,
                text: args.opt_string("text")?.map(Secret),
                question,
                reply: args.opt_u64("reply")?.map(Pointer::Seq),
                topic: args.send_topic("topic")?,
                thread: args.opt_u64("thread")?,
            };
            seq(context.write("keryx.question", args, Some(chat), command, Vec::new())?)
        }
        "answer" => {
            let chat = args.ulid("chat")?;
            let question_seq = args.u64("question")?;
            let shown = context.shown(chat, question_seq)?;
            let question = shown
                .question
                .ok_or_else(|| bad("that message is not a question"))?;
            let answer = serde_json::from_value(json!({
                "question": question_seq,
                "answers": args.list("answers")?,
                "skipped": args.bool("skipped")?,
            }))
            .map_err(|error| bad(format!("the answers do not read: {error}")))?;
            let command = Command::SendAnswer {
                chat,
                question,
                answer,
            };
            seq(context.write("keryx.answer", args, Some(chat), command, Vec::new())?)
        }
        "edit_answer" => {
            let chat = args.ulid("chat")?;
            let target = args.u64("seq")?;
            let mine = context.shown(chat, target)?;
            let question_seq = mine
                .answer
                .map(|answer| answer.question)
                .ok_or_else(|| bad("that message is not an answer"))?;
            let question = context
                .shown(chat, question_seq)?
                .question
                .ok_or_else(|| fail("not_found", "its question could not be opened"))?;
            let answer = serde_json::from_value(json!({
                "question": question_seq,
                "answers": args.list("answers")?,
            }))
            .map_err(|error| bad(format!("the answers do not read: {error}")))?;
            let command = Command::EditAnswer {
                chat,
                target: Pointer::Seq(target),
                question,
                answer,
            };
            seq(context.write("keryx.edit_answer", args, Some(chat), command, Vec::new())?)
        }
        "suggest" => {
            let chat = args.ulid("chat")?;
            let command = Command::SendSuggestion {
                chat,
                text: args.opt_string("text")?.map(Secret),
                suggestion: Suggestion::Folder(FolderSuggestion {
                    name: args.string("folder")?,
                    r#move: args.bool("move")?,
                }),
                reply: None,
                topic: args.send_topic("topic")?,
                thread: args.opt_u64("thread")?,
            };
            seq(context.write("keryx.suggest", args, Some(chat), command, Vec::new())?)
        }
        "accept_suggestion" => {
            let chat = args.ulid("chat")?;
            let at = args.u64("seq")?;
            let suggestion = context
                .shown(chat, at)?
                .suggestion
                .ok_or_else(|| bad("that message is not a suggestion"))?;
            let command = Command::AcceptSuggestion {
                chat,
                seq: at,
                suggestion: *suggestion,
            };
            seq(context.write(
                "keryx.accept_suggestion",
                args,
                Some(chat),
                command,
                Vec::new(),
            )?)
        }
        "decline_suggestion" => {
            let chat = args.ulid("chat")?;
            let command = Command::DeclineSuggestion {
                chat,
                seq: args.u64("seq")?,
            };
            seq(context.write(
                "keryx.decline_suggestion",
                args,
                Some(chat),
                command,
                Vec::new(),
            )?)
        }
        "direct" => {
            let account = context.person(args.get("person"))?;
            let answer = context.write(
                "keryx.direct",
                args,
                None,
                Command::StartChat { account },
                Vec::new(),
            )?;
            for event in &answer.event_list {
                match event {
                    Event::Started { chat, .. } | Event::ChatReady { chat } => {
                        return Ok(json!({"chat": chat.to_string()}));
                    }
                    _ => {}
                }
            }
            if let Some(chat) = context.view().direct_with(account) {
                return Ok(json!({"chat": chat.to_string()}));
            }
            Err(fail("not_found", "the chat did not start"))
        }
        "create" => create(context, args),
        "add" => {
            let chat = args.ulid("chat")?;
            let command = Command::AddMembers {
                chat,
                account_list: context.people(&args.list("person_list")?)?,
                note: args.opt_string("note")?,
            };
            seq(context.write("keryx.add", args, Some(chat), command, Vec::new())?)
        }
        "remove" => {
            let chat = args.ulid("chat")?;
            let command = Command::RemoveMembers {
                chat,
                account_list: context.people(&args.list("person_list")?)?,
            };
            seq(context.write("keryx.remove", args, Some(chat), command, Vec::new())?)
        }
        "role" => {
            let chat = args.ulid("chat")?;
            let command = Command::SetRole {
                chat,
                account: context.person(args.get("person"))?,
                role: role_of(&args.string("role")?)?,
            };
            seq(context.write("keryx.role", args, Some(chat), command, Vec::new())?)
        }
        "leave" => simple(context, args, "keryx.leave", |chat| Command::Leave { chat }),
        "remove_chat" => simple(context, args, "keryx.remove_chat", |chat| {
            Command::RemoveChat { chat }
        }),
        "ask" => {
            let chat = args.ulid("chat")?;
            let command = Command::AskToJoin {
                chat,
                note: args.opt_string("note")?,
            };
            seq(context.write("keryx.ask", args, Some(chat), command, Vec::new())?)
        }
        "cancel_ask" => simple(context, args, "keryx.cancel_ask", |chat| {
            Command::CancelAsk { chat }
        }),
        "let_in" => {
            let chat = args.ulid("chat")?;
            let command = Command::LetIn {
                chat,
                account: context.person(args.get("person"))?,
            };
            seq(context.write("keryx.let_in", args, Some(chat), command, Vec::new())?)
        }
        "decline_ask" => {
            let chat = args.ulid("chat")?;
            let command = Command::DeclineAsk {
                chat,
                account: context.person(args.get("person"))?,
            };
            seq(context.write("keryx.decline_ask", args, Some(chat), command, Vec::new())?)
        }
        "answer_invite" => {
            let chat = args.ulid("chat")?;
            let command = Command::AnswerInvite {
                chat,
                accept: args.bool("accept")?,
            };
            seq(context.write("keryx.answer_invite", args, Some(chat), command, Vec::new())?)
        }
        "topics" => {
            let chat = args.ulid("chat")?;
            let (_, topic_list) = topics_of(context, chat)?;
            let command = Command::Topics {
                chat,
                on: args.bool("on")?,
                topic_list,
            };
            seq(context.write("keryx.topics", args, Some(chat), command, Vec::new())?)
        }
        "topic_create" => {
            let chat = args.ulid("chat")?;
            let name = topic_name(&args.string("name")?)?;
            let answer = context.read(Command::TopicId { chat })?;
            let mut topic = None;
            for event in &answer.event_list {
                if let Event::TopicId { topic: id, .. } = event {
                    topic = Some(*id);
                }
            }
            let topic = topic.ok_or_else(|| fail("internal", "no topic id came back"))?;
            let (on, mut topic_list) = topics_of(context, chat)?;
            let place = topic_list.iter().map(|t| t.place + 1).max().unwrap_or(0);
            topic_list.push(TopicName {
                id: topic,
                name,
                place,
            });
            let command = Command::Topics {
                chat,
                on,
                topic_list,
            };
            context.write("keryx.topic_create", args, Some(chat), command, Vec::new())?;
            Ok(json!({"topic": topic.to_string()}))
        }
        "topic_rename" => {
            let chat = args.ulid("chat")?;
            let topic = args.ulid("topic")?;
            let name = topic_name(&args.string("name")?)?;
            let (on, mut topic_list) = topics_of(context, chat)?;
            let mut found = false;
            for item in &mut topic_list {
                if item.id == topic {
                    item.name = name.clone();
                    found = true;
                }
            }
            if !found {
                return Err(fail("not_found", "no such topic"));
            }
            let command = Command::Topics {
                chat,
                on,
                topic_list,
            };
            seq(context.write("keryx.topic_rename", args, Some(chat), command, Vec::new())?)
        }
        "topic_remove" => {
            let chat = args.ulid("chat")?;
            let topic = args.ulid("topic")?;
            let (on, mut topic_list) = topics_of(context, chat)?;
            let before = topic_list.len();
            topic_list.retain(|item| item.id != topic);
            if topic_list.len() == before {
                return Err(fail("not_found", "no such topic"));
            }
            let command = Command::Topics {
                chat,
                on,
                topic_list,
            };
            seq(context.write("keryx.topic_remove", args, Some(chat), command, Vec::new())?)
        }
        "chat_profile" => chat_profile(context, args),
        "chat_options" => chat_options(context, args),
        "topic_profile" => topic_profile(context, args),
        "topic_options" => topic_options(context, args),
        "topic_moderator" => {
            let chat = args.ulid("chat")?;
            let role = if args.bool("on")? {
                ChatRole::Moderator
            } else {
                ChatRole::Member
            };
            let command = Command::TopicRole {
                chat,
                topic: args.ulid("topic")?,
                account: context.person(args.get("person"))?,
                role,
            };
            seq(context.write(
                "keryx.topic_moderator",
                args,
                Some(chat),
                command,
                Vec::new(),
            )?)
        }
        "findable" => {
            let chat = args.ulid("chat")?;
            let command = Command::Findable {
                chat,
                findable: args.bool("on")?,
                name: args.opt_string("name")?,
                bio: args.opt_string("bio")?,
            };
            seq(context.write("keryx.findable", args, Some(chat), command, Vec::new())?)
        }
        _ => Err(bad(format!("keryx.{function} is not a built-in"))),
    }
}

/// A write's result: its seq, none for one that stores no message.
fn seq(answer: Answer) -> Result<Value, Fail> {
    Ok(json!({"seq": answer.seq}))
}

/// A write that names only its chat.
fn simple(
    context: &Context,
    args: &Args,
    name: &str,
    command: impl FnOnce(Ulid) -> Command,
) -> Result<Value, Fail> {
    let chat = args.ulid("chat")?;
    seq(context.write(name, args, Some(chat), command(chat), Vec::new())?)
}

/// A message's topic and thread, from the archive's readable header.
fn place(context: &Context, chat: Ulid, seq: u64) -> (Option<Ulid>, Option<u64>) {
    match lock(&context.env.shared.store).place_of(chat, seq) {
        Ok(Some(place)) => place,
        _ => (None, None),
    }
}

/// An item of a `chat_list` that names an audience: a chat id or `{chat, topic?, thread?}`.
pub fn audience(item: &Value) -> Result<(Ulid, Option<Ulid>, Option<u64>), Fail> {
    let bad_item = || bad("a chat_list item is a chat id or {chat, topic?, thread?}");
    match item {
        Value::String(text) => Ok((Ulid::parse(text).map_err(|_| bad_item())?, None, None)),
        Value::Object(object) => {
            let chat = object
                .get("chat")
                .and_then(Value::as_str)
                .and_then(|text| Ulid::parse(text).ok())
                .ok_or_else(bad_item)?;
            let topic = match object.get("topic").and_then(Value::as_str) {
                Some("general") | None => None,
                Some(text) => Some(Ulid::parse(text).map_err(|_| bad_item())?),
            };
            let thread = object.get("thread").and_then(Value::as_u64);
            Ok((chat, topic, thread))
        }
        _ => Err(bad_item()),
    }
}

fn role_of(text: &str) -> Result<ChatRole, Fail> {
    match text {
        "owner" => Ok(ChatRole::Owner),
        "moderator" => Ok(ChatRole::Moderator),
        "member" => Ok(ChatRole::Member),
        _ => Err(bad("a role is owner, moderator or member")),
    }
}

fn topic_name(name: &str) -> Result<String, Fail> {
    let name = name.trim();
    let count = name.chars().count();
    if count == 0 || count > 32 {
        return Err(bad("a topic's name is 1 to 32 characters"));
    }
    Ok(name.to_string())
}

fn topics_of(context: &Context, chat: Ulid) -> Result<(bool, Vec<TopicName>), Fail> {
    match context.view().chats.get(&chat) {
        Some(known) => Ok((known.topics_on, known.topic_list.clone())),
        None => Err(fail("not_found", "this agent is not in that chat")),
    }
}

/// The number of a chat's unread main-list messages: content after its seen position from others.
fn unread(context: &Context, chat: Ulid, seen: u64) -> u64 {
    let devices = lock(&context.env.shared.store).unread_devices(chat, seen);
    let Ok(devices) = devices else {
        return 0;
    };
    let view = context.view();
    let mut count = 0;
    for device in devices {
        if !view.own_device(device) {
            count += 1;
        }
    }
    count
}

fn chat_list(context: &Context, args: &Args) -> Result<Value, Fail> {
    let wanted_type = args.opt_string("type")?;
    let folder = args.opt_string("folder")?.map(|name| fold_name(&name));
    let name = args.opt_string("name")?.map(|name| fold_name(&name));
    let limit = usize::try_from(args.limit()?).unwrap_or(usize::MAX);
    let mut rows = Vec::new();
    {
        let view = context.view();
        for (chat, known) in &view.chats {
            let chat_type = chat_type_name(known.item.chat_type);
            if wanted_type
                .as_deref()
                .is_some_and(|wanted| wanted != chat_type)
            {
                continue;
            }
            let folder_list = view.folders_of(*chat);
            if let Some(folder) = &folder
                && !folder_list.iter().any(|item| fold_name(item) == *folder)
            {
                continue;
            }
            let chat_name = view.chat_name(*chat);
            if let Some(name) = &name
                && !chat_name
                    .as_deref()
                    .is_some_and(|chat_name| fold_name(chat_name).contains(name.as_str()))
            {
                continue;
            }
            let mut topic_list = Vec::new();
            for topic in &known.topic_list {
                topic_list.push(json!({"id": topic.id.to_string(), "name": topic.name}));
            }
            rows.push((
                known.item.last_at.unwrap_or(0),
                *chat,
                known.item.seen_seq.unwrap_or(0),
                json!({
                    "id": chat.to_string(),
                    "type": chat_type,
                    "name": chat_name,
                    "role": known.item.role.map(role_name),
                    "folder_list": folder_list,
                    "topic_list": topic_list,
                    "last_at": known.item.last_at.map(time_text),
                    "preview": known.preview,
                }),
            ));
        }
    }
    rows.sort_by_key(|row| std::cmp::Reverse(row.0));
    let more = rows.len() > limit;
    rows.truncate(limit);
    let mut list = Vec::new();
    for (_, chat, seen, mut row) in rows {
        row["unread"] = json!(unread(context, chat, seen));
        list.push(row);
    }
    Ok(json!({"chat_list": list, "more": more}))
}

/// Chat options as switches, from the member role's per-chat override.
fn switches(options: &ChatOptions) -> Value {
    let denied = |name: &str| options.deny.iter().any(|item| item == name);
    let mut files = true;
    for kind in FILE_KINDS {
        if denied(&format!("message.send.{kind}")) {
            files = false;
        }
    }
    json!({
        "reactions": !denied("message.send.reaction"),
        "stickers": !denied("message.send.sticker"),
        "files": files,
        "members_add": options.allow.iter().any(|item| item == "member.add"),
    })
}

/// The kinds a chat's or topic's `files` switch covers, as Android's.
const FILE_KINDS: [&str; 7] = [
    "voice",
    "image",
    "music",
    "video",
    "doc",
    "file",
    "media_bundle",
];

fn chat(context: &Context, args: &Args) -> Result<Value, Fail> {
    let chat = args.ulid("chat")?;
    let view = context.view();
    let known = view
        .chats
        .get(&chat)
        .ok_or_else(|| fail("not_found", "this agent is not in that chat"))?;
    let profile = known.profile.as_ref();
    let mut topic_list = Vec::new();
    for topic in &known.topic_list {
        topic_list.push(json!({"id": topic.id.to_string(), "name": topic.name}));
    }
    let mut links = Vec::new();
    if let Some(profile) = profile {
        for link in &profile.link_list {
            links.push(json!({"name": link.name, "url": link.url}));
        }
    }
    let options = known.options.clone().unwrap_or_default();
    Ok(json!({
        "id": chat.to_string(),
        "type": chat_type_name(known.item.chat_type),
        "name": view.chat_name(chat),
        "bio": profile.and_then(|profile| profile.bio.clone()),
        "link_list": links,
        "rules": profile.and_then(|profile| profile.rules.clone()),
        "role": known.item.role.map(role_name),
        "topics_on": known.topics_on,
        "topic_list": topic_list,
        "options": switches(&options),
        "slow_mode": options.slow_mode.map(|seconds| duration_text(i64::try_from(seconds).unwrap_or(0) * 1000)),
        "findable": known.findable.is_some(),
        "folder_list": view.folders_of(chat),
    }))
}

/// Folds what core showed of a page into one object per message, by seq: edits give their newest
/// text, deletes mark theirs, and membership lines and notices are messages too.
fn fold_messages(context: &Context, events: &[Event]) -> BTreeMap<(Ulid, u64), Value> {
    let mut accounts = Vec::new();
    for event in events {
        match event {
            Event::Message { message } => {
                if let Some(account) = message.account {
                    accounts.push(account);
                }
            }
            Event::Line {
                account,
                account_list,
                ..
            } => {
                accounts.push(*account);
                accounts.extend(account_list.iter().copied());
            }
            _ => {}
        }
    }
    accounts.sort();
    accounts.dedup();
    context.learn(&accounts);
    let view = context.view();
    let mut map = BTreeMap::new();
    for event in events {
        match event {
            Event::Message { message } => {
                map.insert((message.chat, message.seq), message_value(&view, message));
            }
            Event::Line {
                chat,
                seq,
                account,
                action,
                account_list,
                saved_at,
                ..
            } => {
                let mut people = Vec::new();
                for person in account_list {
                    people.push(view.person(*person));
                }
                map.insert(
                    (*chat, *seq),
                    json!({
                        "chat": chat.to_string(),
                        "seq": seq,
                        "kind": "line",
                        "action": action,
                        "sender": view.person(*account),
                        "person_list": people,
                        "at": time_text(*saved_at),
                    }),
                );
            }
            Event::Notice {
                chat,
                seq,
                saved_at,
                ..
            } => {
                map.insert(
                    (*chat, *seq),
                    json!({
                        "chat": chat.to_string(),
                        "seq": seq,
                        "kind": "device_notice",
                        "at": time_text(*saved_at),
                    }),
                );
            }
            _ => {}
        }
    }
    for event in events {
        match event {
            Event::Edited {
                message,
                target_seq,
            } => {
                if let Some(target) = map.get_mut(&(message.chat, *target_seq)) {
                    let text = message.text.as_ref().map(|text| text.0.clone());
                    if target["caption"].is_null()
                        && target["file_list"].as_array().is_none_or(Vec::is_empty)
                    {
                        target["text"] = json!(text);
                    } else {
                        target["caption"] = json!(text);
                    }
                    target["edited"] = json!(true);
                }
            }
            Event::Deleted {
                chat, target_seq, ..
            } => {
                if let Some(target) = map.get_mut(&(*chat, *target_seq)) {
                    target["deleted"] = json!(true);
                    target["text"] = Value::Null;
                    target["caption"] = Value::Null;
                }
            }
            Event::Removed { chat, seq } => {
                map.remove(&(*chat, *seq));
            }
            _ => {}
        }
    }
    map
}

/// Background before a chat event, or the latest messages for a schedule. The thread's starter
/// gets priority, and the remaining budget goes to the nearest earlier messages.
pub fn background(
    context_env: &Arc<Env>,
    (chat, topic, thread): (Ulid, Option<Option<Ulid>>, Option<u64>),
    mut before: Option<u64>,
    budget: ContextBudget,
    shown: &HashSet<(Ulid, u64)>,
) -> Vec<Value> {
    let context = Context {
        env: context_env.clone(),
        mode: Mode::Wake,
        forbidden: Cell::new(false),
        deadline: Instant::now() + READ_TIME,
        limit: READ_TIME,
        output: RefCell::new(String::new()),
        cut: Cell::new(false),
        action_list: RefCell::new(Vec::new()),
        next_n: Cell::new(0),
        attach_list: RefCell::new(Vec::new()),
    };
    let mut root = None;
    let mut left = budget.count;
    // Count the brackets, separators and serialized messages of the background array.
    let mut bytes = 2_u64;
    if let Some(seq) = thread
        && !shown.contains(&(chat, seq))
        && let Ok((list, _, _)) = page(
            &context,
            chat,
            Some(seq.saturating_sub(1)),
            Some(seq.saturating_add(1)),
            1,
            None,
            None,
        )
        && let Some(message) = list.into_iter().next()
        && message["seq"].as_u64() == Some(seq)
    {
        left = left.saturating_sub(1);
        bytes = bytes.saturating_add(message.to_string().len() as u64);
        root = Some(message);
    }
    let mut kept = Vec::new();
    let filter = if thread.is_some() { None } else { topic };
    let main_list = before.is_some() && thread.is_none();
    'pages: while left > 0 && bytes < budget.bytes {
        let Ok((list, more, oldest)) = page(
            &context,
            chat,
            None,
            before,
            left.min(PAGE_MAX),
            filter,
            thread,
        ) else {
            break;
        };
        for message in list.into_iter().rev() {
            let Some(seq) = message["seq"].as_u64() else {
                continue;
            };
            if thread == Some(seq)
                || shown.contains(&(chat, seq))
                || (main_list && !message["thread"].is_null())
            {
                continue;
            }
            let separator = u64::from(root.is_some() || !kept.is_empty());
            bytes = bytes.saturating_add(message.to_string().len() as u64 + separator);
            if bytes > budget.bytes {
                break 'pages;
            }
            kept.push(message);
            left -= 1;
            if left == 0 {
                break 'pages;
            }
        }
        if !more || oldest.is_none() || oldest == before {
            break;
        }
        before = oldest;
    }
    if let Some(root) = root {
        kept.push(root);
    }
    kept.reverse();
    kept
}

fn page(
    context: &Context,
    chat: Ulid,
    after: Option<u64>,
    before: Option<u64>,
    limit: u64,
    topic: Option<Option<Ulid>>,
    thread: Option<u64>,
) -> Result<(Vec<Value>, bool, Option<u64>), Fail> {
    let fetch = u32::try_from(limit.saturating_add(1)).unwrap_or(u32::MAX);
    let rows =
        lock(&context.env.shared.store).read_range(chat, after, before, fetch, topic, thread);
    let rows = match rows {
        Ok(rows) => rows,
        Err(error) => return Err(fail("internal", error.to_string())),
    };
    let mut count = 0;
    let mut oldest_row = None;
    for message in &rows.message_list {
        if !matches!(
            message.kind,
            Some(keryx_core::protocol::Kind::Edit) | Some(keryx_core::protocol::Kind::Delete)
        ) {
            count += 1;
            if oldest_row.is_none() {
                oldest_row = Some(message.seq);
            }
        }
    }
    let more = count > limit;
    if rows.message_list.is_empty() {
        return Ok((Vec::new(), false, None));
    }
    let events = context.open(chat, rows)?;
    let mut list = Vec::new();
    for ((found_chat, seq), message) in fold_messages(context, &events) {
        if found_chat == chat
            && after.is_none_or(|after| seq > after)
            && before.is_none_or(|before| seq < before)
        {
            list.push(message);
        }
    }
    let cut = usize::try_from(limit).unwrap_or(usize::MAX);
    if list.len() > cut {
        let from_oldest = after.is_some() && before.is_none();
        if from_oldest {
            list.truncate(cut);
        } else {
            list.drain(..list.len() - cut);
        }
    }
    let oldest = match list.first() {
        Some(message) => message["seq"].as_u64(),
        None => oldest_row,
    };
    Ok((list, more, oldest))
}

fn message_list(context: &Context, args: &Args) -> Result<Value, Fail> {
    let chat = args.ulid("chat")?;
    let (list, more, _) = page(
        context,
        chat,
        args.opt_u64("after")?,
        args.opt_u64("before")?,
        args.limit()?,
        args.topic("topic")?,
        args.opt_u64("thread")?,
    )?;
    Ok(json!({"message_list": list, "more": more}))
}

fn message(context: &Context, args: &Args) -> Result<Value, Fail> {
    let chat = args.ulid("chat")?;
    let at = args.u64("seq")?;
    let (one, after, thread_count) = {
        let store = lock(&context.env.shared.store);
        let one = store
            .read_one(chat, at)
            .map_err(|error| fail("internal", error.to_string()))?;
        let after = store
            .read_after(chat, at, 2_000)
            .map_err(|error| fail("internal", error.to_string()))?;
        let count = store.thread_count(chat, at).unwrap_or(0);
        (one, after, count)
    };
    if one.message_list.is_empty() {
        return Err(fail("not_found", format!("no message {at} in this chat")));
    }
    let mut events = context.open(chat, one)?;
    let mut edit_list = Vec::new();
    {
        let view = context.view();
        for event in &events {
            if let Event::Edited {
                message,
                target_seq,
            } = event
                && *target_seq == at
            {
                edit_list.push(json!({
                    "seq": message.seq,
                    "text": message.text.as_ref().map(|text| text.0.clone()),
                    "at": time_text(message.saved_at),
                    "sender": message.account.map(|account| view.person(account)),
                }));
            }
        }
    }
    let mut folded = fold_messages(context, &events);
    let mut value = folded
        .remove(&(chat, at))
        .ok_or_else(|| fail("not_found", "it could not be opened"))?;
    events = if after.message_list.is_empty() {
        Vec::new()
    } else {
        context.open(chat, after)?
    };
    let mut reaction_list = Vec::new();
    let mut answer_list = Vec::new();
    let mut gone = Vec::new();
    for event in &events {
        if let Event::Deleted { target_seq, .. } = event {
            gone.push(*target_seq);
        }
    }
    let shown: Vec<&Shown> = events
        .iter()
        .filter_map(|event| match event {
            Event::Message { message } if !gone.contains(&message.seq) => Some(message),
            _ => None,
        })
        .collect();
    let accounts: Vec<Ulid> = shown.iter().filter_map(|message| message.account).collect();
    context.learn(&accounts);
    let view = context.view();
    for message in shown {
        if message.on == Some(at) {
            reaction_list.push(json!({
                "seq": message.seq,
                "emoji": message.emoji,
                "sender": message.account.map(|account| view.person(account)),
            }));
        }
        if let Some(answer) = &message.answer
            && answer.question == at
        {
            answer_list.push(json!({
                "seq": message.seq,
                "sender": message.account.map(|account| view.person(account)),
                "answer": answer,
            }));
        }
    }
    value["reaction_list"] = json!(reaction_list);
    value["edit_list"] = json!(edit_list);
    value["thread_count"] = json!(thread_count);
    value["answer_list"] = json!(answer_list);
    Ok(value)
}

fn member_list(context: &Context, args: &Args) -> Result<Value, Fail> {
    let chat = args.ulid("chat")?;
    let wanted = match args.opt_string("role")? {
        Some(role) => Some(role_of(&role)?),
        None => None,
    };
    let limit = usize::try_from(args.limit()?).unwrap_or(usize::MAX);
    let answer = context.read(Command::Members { chat })?;
    let mut members = Vec::new();
    for event in &answer.event_list {
        if let Event::Members { member_list, .. } = event {
            members = member_list.clone();
        }
    }
    members.retain(|member| wanted.is_none_or(|wanted| member.role == wanted));
    let more = members.len() > limit;
    members.truncate(limit);
    let accounts: Vec<Ulid> = members.iter().map(|member| member.account).collect();
    context.learn(&accounts);
    let view = context.view();
    let mut list = Vec::new();
    for member in members {
        let person = view.person(member.account);
        let agent = view
            .accounts
            .get(&member.account)
            .is_some_and(|known| known.agent);
        list.push(json!({
            "id": member.account.to_string(),
            "username": person["username"],
            "display_name": person["display_name"],
            "role": role_name(member.role),
            "agent": agent,
        }));
    }
    Ok(json!({"member_list": list, "more": more}))
}

fn times(context: &Context, args: &Args) -> Result<Value, Fail> {
    let chat = args.ulid("chat")?;
    let at = args.u64("seq")?;
    let mut after = None;
    let mut items = Vec::new();
    loop {
        let answer = context.read(Command::Times {
            chat,
            seq: at,
            after,
        })?;
        let mut more = false;
        for event in &answer.event_list {
            if let Event::Times {
                member_list,
                more: further,
                ..
            } = event
            {
                items.extend(member_list.iter().cloned());
                more = *further;
            }
        }
        let last = items.last().map(|item| item.account);
        if !more || last.is_none() || last == after {
            break;
        }
        after = last;
    }
    let accounts: Vec<Ulid> = items.iter().map(|item| item.account).collect();
    context.learn(&accounts);
    let view = context.view();
    let mut map = Map::new();
    for item in items {
        let person = view.person(item.account);
        let key = person["username"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| item.account.to_string());
        map.insert(
            key,
            json!({
                "received_at": item.received_at.map(time_text),
                "seen_at": item.seen_at.map(time_text),
            }),
        );
    }
    Ok(Value::Object(map))
}

/// A history row's action, as a word.
fn action_name(action: &impl std::fmt::Debug) -> String {
    let debug = format!("{action:?}");
    let mut word = String::new();
    for (at, character) in debug.chars().enumerate() {
        if character.is_uppercase() {
            if at > 0 {
                word.push('_');
            }
            word.extend(character.to_lowercase());
        } else {
            word.push(character);
        }
    }
    word
}

fn history_list(context: &Context, args: &Args) -> Result<Value, Fail> {
    let chat = args.ulid("chat")?;
    let topic = args.topic("topic")?.flatten();
    let limit = args.limit()?;
    let rows = lock(&context.env.shared.store)
        .history(
            chat,
            topic,
            None,
            u32::try_from(limit + 1).unwrap_or(u32::MAX),
        )
        .map_err(|error| fail("internal", error.to_string()))?;
    let more = u64::try_from(rows.len()).unwrap_or(0) > limit;
    let mut rows = rows;
    rows.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
    if rows.is_empty() {
        return Ok(json!({"history_list": [], "more": false}));
    }
    let answer = context.read(Command::OpenHistory {
        chat,
        event_list: rows,
    })?;
    let mut accounts = Vec::new();
    for event in &answer.event_list {
        if let Event::ChatEvent { event, .. } = event {
            accounts.push(event.actor);
            if let Some(target) = event.target {
                accounts.push(target);
            }
        }
    }
    context.learn(&accounts);
    let view = context.view();
    let mut list = Vec::new();
    for event in &answer.event_list {
        if let Event::ChatEvent { event, copy_list } = event {
            let mut copies = Vec::new();
            for copy in copy_list {
                copies.push(message_value(&view, copy));
            }
            list.push(json!({
                "actor": view.person(event.actor),
                "action": action_name(&event.action),
                "target": event.target.map(|target| view.person(target)),
                "seq": event.seq,
                "at": time_text(event.server_at),
                "copy_list": copies,
            }));
        }
    }
    Ok(json!({"history_list": list, "more": more}))
}

fn profile(context: &Context, args: &Args) -> Result<Value, Fail> {
    let account = context.person(args.get("person"))?;
    let answer = context.read(Command::Profile { account })?;
    let _ = context.ask(Command::PresenceOf { account }, Vec::new(), READ_TIME);
    let mut found = None;
    for event in &answer.event_list {
        if let Event::Profile { profile } = event
            && profile.account == account
        {
            found = Some(profile.clone());
        }
    }
    let view = context.view();
    let presence = view
        .accounts
        .get(&account)
        .and_then(|known| known.presence.clone());
    match found {
        Some(profile) => {
            let mut links = Vec::new();
            for link in &profile.link_list {
                links.push(json!({"name": link.name, "url": link.url}));
            }
            Ok(json!({
                "id": account.to_string(),
                "username": profile.username,
                "display_name": profile.display_name,
                "bio": profile.bio,
                "link_list": links,
                "agent": profile.agent,
                "owner": profile.owner.map(|owner| view.person(owner)),
                "presence": presence,
            }))
        }
        None => {
            let person = view.person(account);
            Ok(json!({
                "id": account.to_string(),
                "username": person["username"],
                "display_name": person["display_name"],
                "bio": null,
                "link_list": [],
                "agent": false,
                "owner": null,
                "presence": presence,
            }))
        }
    }
}

/// A file's type and kind from its name, for `send_files`.
fn file_type(name: &str) -> (&'static str, ContentKind) {
    let extension = name
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "jpg" | "jpeg" => ("image/jpeg", ContentKind::Image),
        "png" => ("image/png", ContentKind::Image),
        "gif" => ("image/gif", ContentKind::Image),
        "webp" => ("image/webp", ContentKind::Image),
        "mp4" => ("video/mp4", ContentKind::Video),
        "webm" => ("video/webm", ContentKind::Video),
        "mov" => ("video/quicktime", ContentKind::Video),
        "mp3" => ("audio/mpeg", ContentKind::Music),
        "ogg" | "opus" => ("audio/ogg", ContentKind::Voice),
        "m4a" => ("audio/mp4", ContentKind::Voice),
        "pdf" => ("application/pdf", ContentKind::Doc),
        "txt" | "md" => ("text/plain", ContentKind::Doc),
        "csv" => ("text/csv", ContentKind::Doc),
        "json" => ("application/json", ContentKind::Doc),
        "doc" => ("application/msword", ContentKind::Doc),
        "docx" => (
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            ContentKind::Doc,
        ),
        "xlsx" => (
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            ContentKind::Doc,
        ),
        _ => ("application/octet-stream", ContentKind::File),
    }
}

fn kind_of(text: &str) -> Result<ContentKind, Fail> {
    match text {
        "voice" => Ok(ContentKind::Voice),
        "image" => Ok(ContentKind::Image),
        "music" => Ok(ContentKind::Music),
        "video" => Ok(ContentKind::Video),
        "doc" => Ok(ContentKind::Doc),
        "file" => Ok(ContentKind::File),
        _ => Err(bad("kind is voice, image, music, video, doc or file")),
    }
}

fn send_files(context: &Context, args: &Args) -> Result<Value, Fail> {
    let chat = args.ulid("chat")?;
    let kind = match args.opt_string("kind")? {
        Some(kind) => Some(kind_of(&kind)?),
        None => None,
    };
    let descriptions = args.list("description_list")?;
    let path_list = args.strings("path_list")?;
    if path_list.is_empty() {
        return Err(bad("path_list names at least one file"));
    }
    let mut files = Vec::new();
    let mut handles = Vec::new();
    for (at, text) in path_list.iter().enumerate() {
        let path = context.path(text, false)?;
        let size = std::fs::metadata(&path)
            .map_err(|_| fail("not_found", format!("no file at {text}")))?
            .len();
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".to_string());
        let (mime, guessed) = file_type(&name);
        let handle = context.env.shared.handle();
        handles.push((handle, path));
        files.push(FileSpec {
            handle,
            kind: kind.unwrap_or(guessed),
            name,
            mime: mime.to_string(),
            size,
            width: None,
            height: None,
            thumbnail: None,
            description: descriptions
                .get(at)
                .and_then(Value::as_str)
                .map(str::to_string),
            title: None,
            artist: None,
            duration: None,
        });
    }
    let command = Command::SendFiles {
        chat,
        files,
        text: args.opt_string("caption")?.map(Secret),
        topic: args.send_topic("topic")?,
        thread: args.opt_u64("thread")?,
        reply: args.opt_u64("reply")?.map(Pointer::Seq),
    };
    seq(context.write("keryx.send_files", args, Some(chat), command, handles)?)
}

/// A file name kept as written, with anything that could leave its folder replaced.
fn safe_name(name: &str) -> String {
    let mut safe = String::new();
    for character in name.chars() {
        if character == '/' || character == '\\' || character.is_control() {
            safe.push('_');
        } else {
            safe.push(character);
        }
    }
    let safe = safe.trim_start_matches('.').to_string();
    if safe.is_empty() {
        "file".to_string()
    } else {
        safe
    }
}

fn download(context: &Context, args: &Args) -> Result<Value, Fail> {
    let chat = args.ulid("chat")?;
    let at = args.u64("seq")?;
    context.room()?;
    let shown = context.shown(chat, at)?;
    let wanted = args.opt_string("file")?;
    let file = match (&wanted, shown.files.len()) {
        (_, 0) => return Err(fail("not_found", "that message has no file")),
        (None, 1) => shown.files[0].clone(),
        (None, _) => {
            return Err(bad(
                "that message has several files: name one with file= (its id or name)",
            ));
        }
        (Some(wanted), _) => shown
            .files
            .iter()
            .find(|file| file.id.to_string() == *wanted || file.name == *wanted)
            .cloned()
            .ok_or_else(|| fail("not_found", "no such file in that message"))?,
    };
    let folder = context
        .env
        .settings
        .directory
        .join("chats")
        .join(chat.to_string());
    std::fs::create_dir_all(&folder).map_err(|error| fail("internal", error.to_string()))?;
    let path = folder.join(format!("{at}-{}", safe_name(&file.name)));
    let _ = std::fs::remove_file(&path);
    let handle = context.env.shared.handle();
    let answer = context.ask(
        Command::Download {
            file: file.clone(),
            handle,
        },
        vec![(handle, path.clone())],
        Duration::from_secs(3600),
    )?;
    for event in &answer.event_list {
        if let Event::TransferFailed {
            handle: failed,
            code,
        } = event
            && *failed == handle
        {
            let _ = std::fs::remove_file(&path);
            return Err(fail(code, "the download failed"));
        }
    }
    if let Some(code) = &answer.code {
        let _ = std::fs::remove_file(&path);
        return Err(fail(code, "the download failed"));
    }
    let relative = context.relative(&path);
    context.action(
        json!({"action": "keryx.download", "chat": chat.to_string(), "seq": at, "path": relative}),
    );
    Ok(json!({
        "path": relative,
        "name": file.name,
        "type": file.mime,
        "size": file.size,
    }))
}

/// A JPEG's path as a photo. keryx-agent decodes no images, so its BlurHash is a flat grey, as
/// `drive.rs`'s is.
fn photo(context: &Context, text: &str) -> Result<PhotoInput, Fail> {
    let path = context.path(text, false)?;
    let jpeg = std::fs::read(&path).map_err(|_| fail("not_found", format!("no file at {text}")))?;
    if !jpeg.starts_with(&[0xFF, 0xD8]) {
        return Err(bad("a photo is a JPEG"));
    }
    Ok(PhotoInput {
        jpeg,
        thumbnail: Thumbnail {
            width: 4,
            height: 4,
            rgba: [128u8, 128, 128, 255].repeat(16),
        },
    })
}

/// Chat options as the member role's override, from switches, starting from what the chat holds.
fn options_with(mut options: ChatOptions, args: &Args) -> Result<ChatOptions, Fail> {
    let mut set = |names: Vec<String>, on: bool, allow_list: bool| {
        for name in names {
            options.allow.retain(|item| *item != name);
            options.deny.retain(|item| *item != name);
            if allow_list && on {
                options.allow.push(name);
            } else if !allow_list && !on {
                options.deny.push(name);
            }
        }
    };
    if let Some(on) = args.opt_bool("reactions")? {
        set(vec!["message.send.reaction".to_string()], on, false);
    }
    if let Some(on) = args.opt_bool("stickers")? {
        set(vec!["message.send.sticker".to_string()], on, false);
    }
    if let Some(on) = args.opt_bool("files")? {
        let names = FILE_KINDS
            .iter()
            .map(|kind| format!("message.send.{kind}"))
            .collect();
        set(names, on, false);
    }
    if let Some(on) = args.opt_bool("members_add")? {
        set(vec!["member.add".to_string()], on, true);
    }
    if args.present("slow_mode") {
        options.slow_mode = slow_mode(&args.string("slow_mode")?)?;
    }
    Ok(options)
}

/// Slow mode's steps, as a chat's options take them: off, or seconds.
fn slow_mode(text: &str) -> Result<Option<u64>, Fail> {
    if text == "off" {
        return Ok(None);
    }
    let seconds = duration_of(text).map(|duration| duration.as_secs());
    match seconds {
        Some(seconds @ (10 | 30 | 60 | 300 | 900 | 3600)) => Ok(Some(seconds)),
        _ => Err(bad("slow_mode is off, 10s, 30s, 1m, 5m, 15m or 1h")),
    }
}

fn create(context: &Context, args: &Args) -> Result<Value, Fail> {
    let chat_type = match args.string("type")?.as_str() {
        "group" => ChatType::Group,
        "channel" => ChatType::Channel,
        _ => return Err(bad("type is group or channel")),
    };
    let options_given = match args.get("options") {
        Value::Object(object) => object.clone(),
        Value::Null => Map::new(),
        _ => return Err(bad("options is a dict")),
    };
    let options = options_with(ChatOptions::default(), &Args { map: options_given })?;
    let photo = match args.opt_string("photo")? {
        Some(path) => Some(photo(context, &path)?),
        None => None,
    };
    let name = args.string("name")?;
    let bio = args.opt_string("bio")?;
    let link_list = args.links("link_list")?;
    let account = context.view().account;
    let command = Command::CreateChat {
        chat_type,
        name: name.clone(),
        bio: bio.clone(),
        link_list: link_list.clone(),
        account_list: context.people(&args.list("person_list")?)?,
        topics: Some(args.bool("topics")?),
        findable: args.bool("findable")?,
        options,
        photo,
        note: args.opt_string("note")?,
    };
    let answer = context.write("keryx.create", args, None, command, Vec::new())?;
    let mut chat = None;
    for event in answer.event_list {
        if let Event::Started {
            chat: started,
            account: by,
        } = event
            && Some(by) == account
        {
            chat = Some(started);
            break;
        }
    }
    let Some(chat) = chat else {
        return Err(fail("internal", "the new chat did not show"));
    };
    if !context.view().chats.contains_key(&chat) {
        context.read(Command::ChatList)?;
    }
    // Core can queue a list refresh behind an active sync. Wait for this chat's view itself.
    let listed = context.block(
        async {
            loop {
                if let Some(known) = context.view().chats.get(&chat)
                    && known.profile.is_some()
                {
                    return;
                }
                context.env.shared.chats_changed.notified().await;
            }
        },
        READ_TIME,
    );
    if listed.is_none() {
        return Err(fail(
            "timeout",
            "the new chat did not show on the chat list in time",
        ));
    }
    if let Some(rules) = args.opt_string("rules")? {
        let command = Command::ChatProfile {
            chat,
            name,
            bio,
            link_list,
            rules: Some(rules),
        };
        context.write("keryx.chat_profile", args, Some(chat), command, Vec::new())?;
    }
    Ok(json!({"chat": chat.to_string()}))
}

fn chat_profile(context: &Context, args: &Args) -> Result<Value, Fail> {
    let chat = args.ulid("chat")?;
    let current = context
        .view()
        .chats
        .get(&chat)
        .and_then(|known| known.profile.clone());
    let current_name = current.as_ref().map(|profile| profile.name.clone());
    let name = match args.opt_string("name")? {
        Some(name) => name,
        None => {
            current_name.ok_or_else(|| fail("not_found", "this chat's profile is not open yet"))?
        }
    };
    let bio = match args.opt_string("bio")? {
        Some(bio) => Some(bio),
        None => current.as_ref().and_then(|profile| profile.bio.clone()),
    };
    let link_list = match args.get("link_list") {
        Value::Null => current
            .as_ref()
            .map(|profile| profile.link_list.clone())
            .unwrap_or_default(),
        _ => args.links("link_list")?,
    };
    let rules = match args.opt_string("rules")? {
        Some(rules) => Some(rules),
        None => current.as_ref().and_then(|profile| profile.rules.clone()),
    };
    let mut answer = None;
    let changing = args.present("name")
        || args.present("bio")
        || args.present("link_list")
        || args.present("rules");
    if changing {
        let command = Command::ChatProfile {
            chat,
            name,
            bio,
            link_list,
            rules,
        };
        answer =
            Some(context.write("keryx.chat_profile", args, Some(chat), command, Vec::new())?);
    }
    if let Some(path) = args.opt_string("photo")? {
        let input = photo(context, &path)?;
        let command = Command::ChatPhoto {
            chat,
            jpeg: input.jpeg,
            thumbnail: input.thumbnail,
        };
        answer =
            Some(context.write("keryx.chat_profile", args, Some(chat), command, Vec::new())?);
    }
    Ok(json!({"seq": answer.and_then(|answer| answer.seq)}))
}

fn chat_options(context: &Context, args: &Args) -> Result<Value, Fail> {
    let chat = args.ulid("chat")?;
    let current = {
        let view = context.view();
        let known = view
            .chats
            .get(&chat)
            .ok_or_else(|| fail("not_found", "this agent is not in that chat"))?;
        known.options.clone().unwrap_or_default()
    };
    let options = options_with(current, args)?;
    let command = Command::ChatOptions { chat, options };
    seq(context.write("keryx.chat_options", args, Some(chat), command, Vec::new())?)
}

fn topic_profile(context: &Context, args: &Args) -> Result<Value, Fail> {
    let chat = args.ulid("chat")?;
    let topic = args.ulid("topic")?;
    let current = context.view().topic_profiles.get(&(chat, topic)).cloned();
    let bio = match args.opt_string("bio")? {
        Some(bio) => Some(bio),
        None => current.as_ref().and_then(|profile| profile.bio.clone()),
    };
    let link_list = match args.get("link_list") {
        Value::Null => current
            .as_ref()
            .map(|profile| profile.link_list.clone())
            .unwrap_or_default(),
        _ => args.links("link_list")?,
    };
    let rules = match args.opt_string("rules")? {
        Some(rules) => Some(rules),
        None => current.as_ref().and_then(|profile| profile.rules.clone()),
    };
    let mut answer = None;
    if args.present("bio") || args.present("link_list") || args.present("rules") {
        let command = Command::TopicProfile {
            chat,
            topic,
            bio,
            link_list,
            rules,
        };
        answer =
            Some(context.write("keryx.topic_profile", args, Some(chat), command, Vec::new())?);
    }
    if let Some(path) = args.opt_string("photo")? {
        let input = photo(context, &path)?;
        let command = Command::TopicPhoto {
            chat,
            topic,
            jpeg: input.jpeg,
            thumbnail: input.thumbnail,
        };
        answer =
            Some(context.write("keryx.topic_profile", args, Some(chat), command, Vec::new())?);
    }
    Ok(json!({"seq": answer.and_then(|answer| answer.seq)}))
}

fn topic_options(context: &Context, args: &Args) -> Result<Value, Fail> {
    let chat = args.ulid("chat")?;
    let topic = args.ulid("topic")?;
    let mut options = context
        .view()
        .chats
        .get(&chat)
        .and_then(|known| {
            known
                .topic_options
                .get(&topic)
                .map(|(options, _)| options.clone())
        })
        .unwrap_or_default();
    if let Some(closed) = args.opt_bool("closed")? {
        options.closed = closed;
    }
    if let Some(hidden) = args.opt_bool("hidden")? {
        options.hidden = hidden;
    }
    if let Some(write) = args.opt_string("write")? {
        options.write = match write.as_str() {
            "everyone" => TopicWrite::Everyone,
            "moderators" => TopicWrite::Moderators,
            _ => return Err(bad("write is everyone or moderators")),
        };
    }
    if let Some(slow) = args.opt_string("slow_mode")? {
        options.slow_mode = match slow.as_str() {
            "group" => None,
            "off" => Some(0),
            _ => slow_mode(&slow)?,
        };
    }
    // A topic's override names are unprefixed, as Android's.
    let mut set = |names: Vec<String>, on: Option<bool>| {
        let Some(on) = on else {
            return;
        };
        for name in names {
            options.allow.retain(|item| *item != name);
            options.deny.retain(|item| *item != name);
            if on {
                options.allow.push(name);
            } else {
                options.deny.push(name);
            }
        }
    };
    set(
        vec!["message.send.reaction".to_string()],
        args.opt_bool("reactions")?,
    );
    set(
        vec!["message.send.sticker".to_string()],
        args.opt_bool("stickers")?,
    );
    set(
        FILE_KINDS
            .iter()
            .map(|kind| format!("message.send.{kind}"))
            .collect(),
        args.opt_bool("files")?,
    );
    for (key, name) in [
        ("view_received_own", "message.view_received.own"),
        ("view_seen_own", "message.view_seen.own"),
        ("view_received_others", "message.view_received.others"),
        ("view_seen_others", "message.view_seen.others"),
    ] {
        set(vec![name.to_string()], args.opt_bool(key)?);
    }
    let command = Command::TopicOptions {
        chat,
        topic,
        options: options_body(options),
    };
    seq(context.write("keryx.topic_options", args, Some(chat), command, Vec::new())?)
}

fn options_body(options: TopicOptionsBody) -> TopicOptionsBody {
    options
}

// -------------------------------------------------------------------------------------------------
// search and log
// -------------------------------------------------------------------------------------------------

fn search(context: &Context, args: &Args) -> Result<Value, Fail> {
    let query = args.string("query")?;
    let limit = args.limit()?;
    let chat = args.opt_ulid("chat")?;
    let folder = args.opt_string("folder")?;
    let topic = args.topic("topic")?;
    let thread = args.opt_u64("thread")?;
    let sender = match args.get("sender") {
        Value::Null => None,
        value => Some(context.person(value)?),
    };
    let since = args.opt_time("since")?;
    let until = args.opt_time("until")?;
    let chat_list = match (chat, &folder) {
        (Some(chat), _) => Some(vec![Some(chat)]),
        (None, Some(folder)) => {
            let folded = fold_name(folder);
            let view = context.view();
            let mut list = Vec::new();
            for known in view.chats.keys() {
                if view
                    .folders_of(*known)
                    .iter()
                    .any(|name| fold_name(name) == folded)
                {
                    list.push(Some(*known));
                }
            }
            Some(list)
        }
        (None, None) => None,
    };
    if context.env.shared.plain {
        let filter = Filter {
            kind: DocKind::Message,
            chat_list,
            sender,
            topic: topic.flatten(),
            thread,
            since,
            until,
        };
        let outcome = lock(&context.env.shared.index)
            .search(&query, &filter, usize::try_from(limit).unwrap_or(20), 0)
            .map_err(|error| {
                if error.is_query() {
                    bad(error.to_string())
                } else {
                    fail("internal", error.to_string())
                }
            })?;
        let mut wanted = Vec::new();
        for hit in &outcome.hit_list {
            if let (Some(chat), Some(seq)) = (hit.chat, hit.seq) {
                wanted.push((chat, seq, hit.snippet.clone()));
            }
        }
        let list = opened(context, &wanted)?;
        return Ok(json!({
            "message_list": list,
            "more": outcome.more,
            "unknown_list": outcome.unknown_list,
            "hint_list": outcome.hint_list,
        }));
    }
    // Without the plain index: core's own scan of the archive, newest first, as the apps search.
    if fold_name(&query).is_empty() || chat_list.as_ref().is_some_and(Vec::is_empty) {
        return Ok(json!({"message_list": [], "more": false, "unknown_list": [], "hint_list": []}));
    }
    let cut = usize::try_from(limit).unwrap_or(usize::MAX);
    let mut text = Some(Secret(query));
    let mut list = Vec::new();
    let topic_id = match topic {
        Some(Some(id)) => Some(id.to_string()),
        _ => None,
    };
    let more = loop {
        let (reply, page) = oneshot::channel();
        let sent = context.env.shared.asks.send(Ask::Scan {
            text: text.take(),
            reply,
        });
        if sent.is_err() {
            return Err(fail("offline", "the connection is gone"));
        }
        let (hits, older) = match context.block(page, READ_TIME) {
            Some(Ok(Ok(page))) => page,
            Some(Ok(Err(error))) => return Err(fail("internal", error.to_string())),
            Some(Err(_)) => return Err(fail("offline", "the connection is gone")),
            None => return Err(fail("timeout", "the archive search took too long")),
        };
        let mut wanted = Vec::new();
        for hit in hits {
            if chat_list
                .as_ref()
                .is_none_or(|list| list.contains(&Some(hit.chat)))
                && sender.is_none_or(|sender| hit.account == Some(sender))
                && since.is_none_or(|since| hit.saved_at >= since)
                && until.is_none_or(|until| hit.saved_at < until)
            {
                let snippet = format!("{}{}{}", hit.before.0, hit.matched.0, hit.after.0);
                wanted.push((hit.chat, hit.seq, snippet));
            }
        }
        for message in opened(context, &wanted)? {
            if message["deleted"].as_bool() != Some(true)
                && (topic.is_none() || message["topic"].as_str() == topic_id.as_deref())
                && thread.is_none_or(|seq| message["thread"].as_u64() == Some(seq))
            {
                list.push(message);
            }
        }
        if list.len() >= cut || !older {
            break list.len() > cut || older;
        }
    };
    list.truncate(cut);
    Ok(json!({"message_list": list, "more": more, "unknown_list": [], "hint_list": []}))
}

/// Messages by chat and seq, opened a chat at a time, each with its snippet, in the order given.
fn opened(context: &Context, wanted: &[(Ulid, u64, String)]) -> Result<Vec<Value>, Fail> {
    let mut by_chat: HashMap<Ulid, Vec<u64>> = HashMap::new();
    for (chat, seq, _) in wanted {
        by_chat.entry(*chat).or_default().push(*seq);
    }
    let mut found = BTreeMap::new();
    for (chat, seqs) in by_chat {
        let mut rows = Rows {
            message_list: Vec::new(),
            call_list: Vec::new(),
            chat_event_list: Vec::new(),
        };
        for seq in seqs {
            if let Ok(one) = lock(&context.env.shared.store).read_one(chat, seq) {
                rows.message_list.extend(one.message_list);
            }
        }
        rows.message_list.sort_by_key(|message| message.seq);
        rows.message_list.dedup_by_key(|message| message.seq);
        if rows.message_list.is_empty() {
            continue;
        }
        let events = context.open(chat, rows)?;
        found.extend(fold_messages(context, &events));
    }
    let mut list = Vec::new();
    for (chat, seq, snippet) in wanted {
        if let Some(value) = found.get(&(*chat, *seq)) {
            let mut value = value.clone();
            value["snippet"] = json!(snippet);
            list.push(value);
        }
    }
    Ok(list)
}

fn log(context: &Context, function: &str, args: &Args) -> Result<Value, Fail> {
    let store = lock(&context.env.shared.store);
    match function {
        "run_list" => {
            let limit = args.limit()?;
            let query = args.opt_string("query")?;
            let runs = store
                .runs(
                    query.as_deref(),
                    args.opt_time("since")?,
                    args.opt_time("until")?,
                    u32::try_from(limit + 1).unwrap_or(u32::MAX),
                    0,
                )
                .map_err(|error| fail("internal", error.to_string()))?;
            let more = u64::try_from(runs.len()).unwrap_or(0) > limit;
            let mut list = Vec::new();
            for run in runs
                .into_iter()
                .take(usize::try_from(limit).unwrap_or(usize::MAX))
            {
                let steps = store.steps(run.id).unwrap_or_default();
                let snippet = steps
                    .first()
                    .and_then(|step| step.intention_list.clone())
                    .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                    .map(|list| intentions_text(&list))
                    .unwrap_or_default();
                list.push(json!({
                    "id": run.id.to_string(),
                    "started_at": time_text(run.started_at),
                    "ended_at": run.ended_at.map(time_text),
                    "outcome": run.state.name(),
                    "snippet": snippet,
                }));
            }
            Ok(json!({"run_list": list, "more": more}))
        }
        "run" => {
            let id = args.ulid("id")?;
            let run = store
                .run(id)
                .map_err(|error| fail("internal", error.to_string()))?
                .ok_or_else(|| fail("not_found", "no such run"))?;
            let mut event_list = Vec::new();
            for event in store.events_of_run(id).unwrap_or_default() {
                event_list.push(event.body);
            }
            let mut step_list = Vec::new();
            for step in store.steps(id).unwrap_or_default() {
                let mut output = String::new();
                let mut action_list = Vec::new();
                for row in store.outputs(id, step.n).unwrap_or_default() {
                    if row.action {
                        action_list.push(
                            serde_json::from_str::<Value>(&row.body).unwrap_or(json!(row.body)),
                        );
                    } else {
                        output.push_str(&row.body);
                        output.push('\n');
                    }
                }
                step_list.push(json!({
                    "n": step.n,
                    "intention_list": step.intention_list.as_deref().and_then(|text| serde_json::from_str::<Value>(text).ok()),
                    "script": step.script,
                    "output": output,
                    "action_list": action_list,
                    "cut": step.cut,
                    "at": step.started_at.or(step.sent_at).map(time_text),
                }));
            }
            Ok(json!({
                "id": id.to_string(),
                "started_at": time_text(run.started_at),
                "ended_at": run.ended_at.map(time_text),
                "outcome": run.state.name(),
                "event_list": event_list,
                "step_list": step_list,
            }))
        }
        _ => Err(bad(format!("log.{function} is not a built-in"))),
    }
}

/// An intention list's texts, joined, for a run's snippet.
fn intentions_text(list: &Value) -> String {
    let mut texts = Vec::new();
    if let Some(items) = list.as_array() {
        for item in items {
            if let Some(text) = item.get("intention").and_then(Value::as_str) {
                texts.push(text.to_string());
            }
        }
    }
    texts.join("; ")
}

// -------------------------------------------------------------------------------------------------
// memory and skills
// -------------------------------------------------------------------------------------------------

fn note_value(view: &View, note: &NoteRow) -> Value {
    json!({
        "id": note.id.to_string(),
        "chat": note.chat.map(|chat| chat.to_string()),
        "text": note.text,
        "source": {
            "chat": note.source_chat.map(|chat| chat.to_string()),
            "sender": null,
            "seq": note.seq,
            "run": note.run.to_string(),
        },
        "at": time_text(note.updated_at),
        "chat_name": note.chat.and_then(|chat| view.chat_name(chat)),
    })
}

/// Puts a note into the index and commits, so a search finds it at once.
fn index_note(context: &Context, note: NoteRow) {
    let mut index = lock(&context.env.shared.index);
    let mut covered = index.covered();
    covered.notes = covered.notes.max(note.updated_at);
    if let Err(error) = index.put_note(note) {
        tracing::error!(error = %error, "could not index a note");
        return;
    }
    if let Err(error) = index.commit(covered) {
        tracing::error!(error = %error, "could not commit the index");
    }
}

fn memory(context: &Context, function: &str, args: &Args) -> Result<Value, Fail> {
    let limits = &context.env.settings.limits;
    match function {
        "add" => {
            let text = args.string("text")?;
            if u64::try_from(text.len()).unwrap_or(u64::MAX) > limits.memory_note {
                return Err(fail(
                    "too_large",
                    format!("a note is at most {} bytes", limits.memory_note),
                ));
            }
            let chat = args.opt_ulid("chat")?;
            let (run, step, source) = match &context.mode {
                Mode::Step {
                    run, step, source, ..
                } => (*run, *step, *source),
                Mode::Wake => return Err(bad("a wake script only reads")),
            };
            let count = lock(&context.env.shared.store)
                .note_count(None)
                .map_err(|error| fail("internal", error.to_string()))?;
            if count >= limits.memory_notes {
                return Err(fail(
                    "memory_full",
                    format!(
                        "memory holds its {} notes: replace or delete some",
                        limits.memory_notes
                    ),
                ));
            }
            context.room()?;
            let stamp = now();
            let note = NoteRow {
                id: new_id(),
                chat,
                text,
                run,
                step,
                source_chat: source.map(|(chat, _, _)| chat),
                seq: source.and_then(|(_, _, seq)| seq),
                made_at: stamp,
                updated_at: stamp,
            };
            lock(&context.env.shared.store)
                .add_note(&note)
                .map_err(|error| fail("internal", error.to_string()))?;
            let id = note.id.to_string();
            index_note(context, note);
            context.action(json!({"action": "memory.add", "id": id, "chat": chat.map(|chat| chat.to_string())}));
            Ok(json!({"id": id}))
        }
        "note_list" => {
            let limit = args.limit()?;
            let chat = args.opt_ulid("chat")?;
            match args.opt_string("query")? {
                None => {
                    let notes = lock(&context.env.shared.store)
                        .notes(chat, u32::try_from(limit + 1).unwrap_or(u32::MAX), 0)
                        .map_err(|error| fail("internal", error.to_string()))?;
                    let more = u64::try_from(notes.len()).unwrap_or(0) > limit;
                    let view = context.view();
                    let mut list = Vec::new();
                    for note in notes
                        .iter()
                        .take(usize::try_from(limit).unwrap_or(usize::MAX))
                    {
                        list.push(note_value(&view, note));
                    }
                    Ok(json!({"note_list": list, "more": more}))
                }
                Some(query) => {
                    let filter = Filter {
                        kind: DocKind::Note,
                        chat_list: Some(vec![chat]),
                        sender: None,
                        topic: None,
                        thread: None,
                        since: None,
                        until: None,
                    };
                    let outcome = lock(&context.env.shared.index)
                        .search(&query, &filter, usize::try_from(limit).unwrap_or(20), 0)
                        .map_err(|error| {
                            if error.is_query() {
                                bad(error.to_string())
                            } else {
                                fail("internal", error.to_string())
                            }
                        })?;
                    let store = lock(&context.env.shared.store);
                    let view = context.view();
                    let mut list = Vec::new();
                    for hit in &outcome.hit_list {
                        if let Some(id) = hit.note
                            && let Ok(Some(note)) = store.note(id)
                        {
                            let mut value = note_value(&view, &note);
                            value["snippet"] = json!(hit.snippet);
                            list.push(value);
                        }
                    }
                    Ok(json!({
                        "note_list": list,
                        "more": outcome.more,
                        "unknown_list": outcome.unknown_list,
                        "hint_list": outcome.hint_list,
                    }))
                }
            }
        }
        "note" => {
            let id = args.ulid("id")?;
            let note = lock(&context.env.shared.store)
                .note(id)
                .map_err(|error| fail("internal", error.to_string()))?
                .ok_or_else(|| fail("not_found", "no such note"))?;
            let view = context.view();
            Ok(note_value(&view, &note))
        }
        "replace" => {
            let id = args.ulid("id")?;
            let text = args.string("text")?;
            if u64::try_from(text.len()).unwrap_or(u64::MAX) > limits.memory_note {
                return Err(fail(
                    "too_large",
                    format!("a note is at most {} bytes", limits.memory_note),
                ));
            }
            let stamp = now();
            let store = lock(&context.env.shared.store);
            let replaced = store
                .replace_note(id, &text, stamp)
                .map_err(|error| fail("internal", error.to_string()))?;
            if !replaced {
                return Err(fail("not_found", "no such note"));
            }
            let note = store.note(id).ok().flatten();
            drop(store);
            if let Some(note) = note {
                index_note(context, note);
            }
            context.action(json!({"action": "memory.replace", "id": id.to_string()}));
            Ok(json!({"id": id.to_string()}))
        }
        "delete" => {
            let id = args.ulid("id")?;
            let deleted = lock(&context.env.shared.store)
                .delete_note(id)
                .map_err(|error| fail("internal", error.to_string()))?;
            if !deleted {
                return Err(fail("not_found", "no such note"));
            }
            let mut index = lock(&context.env.shared.index);
            index.delete(None, None, Some(id));
            let covered = index.covered();
            if let Err(error) = index.commit(covered) {
                tracing::error!(error = %error, "could not commit the index");
            }
            drop(index);
            context.action(json!({"action": "memory.delete", "id": id.to_string()}));
            Ok(json!({"id": id.to_string()}))
        }
        _ => Err(bad(format!("memory.{function} is not a built-in"))),
    }
}

/// One skill: a `.md` file with `name` and `description` in its frontmatter.
pub struct Skill {
    pub name: String,
    pub description: String,
    pub body: String,
}

/// Reads a skill file's frontmatter: `---`, `name:` and `description:` lines, `---`.
pub fn skill_of(text: &str) -> Option<Skill> {
    let rest = text.strip_prefix("---")?;
    let rest = rest
        .strip_prefix('\n')
        .or_else(|| rest.strip_prefix("\r\n"))?;
    let end = rest.find("\n---")?;
    let (head, tail) = rest.split_at(end);
    let body = tail[4..].trim_start_matches(['\r', '\n']).to_string();
    let mut name = None;
    let mut description = None;
    for line in head.lines() {
        if let Some((key, value)) = line.split_once(':') {
            let value = value.trim().trim_matches('"').to_string();
            match key.trim() {
                "name" => name = Some(value),
                "description" => description = Some(value),
                _ => {}
            }
        }
    }
    let name = name.filter(|name| !name.is_empty())?;
    let description = description.filter(|description| !description.is_empty())?;
    Some(Skill {
        name,
        description,
        body,
    })
}

/// Every skill in the folder, by name; a file without valid frontmatter is skipped and logged.
pub fn skills_in(directory: &Path) -> Vec<Skill> {
    let mut list = Vec::new();
    let Ok(entries) = std::fs::read_dir(directory) else {
        return list;
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    paths.sort();
    for path in paths {
        if path.extension().and_then(|extension| extension.to_str()) != Some("md") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        match skill_of(&text) {
            Some(skill) => list.push(skill),
            None => tracing::warn!(
                file = %path.display(),
                "a skill without valid frontmatter, name and description, is skipped"
            ),
        }
    }
    list
}

fn skill_name(name: &str) -> Result<String, Fail> {
    let valid = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
    if !valid {
        return Err(bad("a skill's name is 1 to 64 of a-z, 0-9, _ and -"));
    }
    Ok(name.to_string())
}

fn skills(context: &Context, function: &str, args: &Args) -> Result<Value, Fail> {
    let Some(settings) = &context.env.settings.skills else {
        return Err(fail("not_found", "no skills folder is set"));
    };
    match function {
        "read" => {
            let wanted = args.strings("name")?;
            let all = skills_in(&settings.directory);
            for name in &wanted {
                let skill = all
                    .iter()
                    .find(|skill| skill.name == *name)
                    .ok_or_else(|| fail("not_found", format!("no skill is named {name}")))?;
                let text = format!(
                    "# skill {}: {}\n{}",
                    skill.name, skill.description, skill.body
                );
                let _ = context.println(&text);
            }
            Ok(Value::Null)
        }
        "skill_list" => {
            let mut list = Vec::new();
            for skill in skills_in(&settings.directory) {
                list.push(json!({"name": skill.name, "description": skill.description}));
            }
            Ok(json!({"skill_list": list, "more": false}))
        }
        "write" | "remove" if !settings.writable => Err(fail(
            "path_not_allowed",
            "skills are not writable here: [skills] writable is false",
        )),
        "write" => {
            let name = skill_name(&args.string("name")?)?;
            let description = args.string("description")?.replace('\n', " ");
            let body = args.string("body")?;
            context.room()?;
            let path = settings.directory.join(format!("{name}.md"));
            let text = format!("---\nname: {name}\ndescription: {description}\n---\n{body}\n");
            std::fs::write(&path, text).map_err(|error| fail("internal", error.to_string()))?;
            context.action(json!({"action": "skills.write", "name": name}));
            Ok(json!({"name": name}))
        }
        "remove" => {
            let name = skill_name(&args.string("name")?)?;
            let path = settings.directory.join(format!("{name}.md"));
            std::fs::remove_file(&path)
                .map_err(|_| fail("not_found", format!("no skill is named {name}")))?;
            context.action(json!({"action": "skills.remove", "name": name}));
            Ok(json!({"name": name}))
        }
        _ => Err(bad(format!("skills.{function} is not a built-in"))),
    }
}

// -------------------------------------------------------------------------------------------------
// storage, files and HTTP
// -------------------------------------------------------------------------------------------------

/// What `storage.usage` shows, in bytes.
pub struct UsageNow {
    pub total: u64,
    pub runs: u64,
    pub memory: u64,
    pub files: u64,
    pub archive: u64,
    pub index: u64,
}

/// How much the agent's directory holds: `agent.db`, `index/`, `tmp/` and `chats/` together.
pub fn usage(env: &Env) -> UsageNow {
    let stored = lock(&env.shared.store).usage().unwrap_or_default();
    let index = lock(&env.shared.index).size();
    let files = directory_size(&env.settings.directory.join("tmp"))
        + directory_size(&env.settings.directory.join("chats"));
    UsageNow {
        total: stored.file + index + files,
        runs: stored.runs,
        memory: stored.memory,
        files,
        archive: stored.archive,
        index,
    }
}

fn storage(context: &Context, function: &str, args: &Args) -> Result<Value, Fail> {
    match function {
        "usage" => {
            let usage = usage(&context.env);
            Ok(json!({
                "total": usage.total,
                "limit": context.env.settings.limits.storage,
                "runs": usage.runs,
                "memory": usage.memory,
                "files": usage.files,
                "archive": usage.archive,
                "index": usage.index,
            }))
        }
        "remove_runs" => {
            let before = args.time("before")?;
            let count = lock(&context.env.shared.store)
                .remove_runs(before)
                .map_err(|error| fail("internal", error.to_string()))?;
            context.action(json!({"action": "storage.remove_runs", "before": time_text(before), "count": count}));
            Ok(json!({"count": count}))
        }
        "remove_files" => {
            let mut count = 0;
            for text in args.strings("path_list")? {
                let path = context.path(&text, false)?;
                if std::fs::remove_file(&path).is_ok() {
                    count += 1;
                    context.action(json!({"action": "storage.remove_files", "path": text}));
                }
            }
            Ok(json!({"count": count}))
        }
        _ => Err(bad(format!("storage.{function} is not a built-in"))),
    }
}

fn file(context: &Context, function: &str, args: &Args) -> Result<Value, Fail> {
    match function {
        "write" => {
            let text = args.string("path")?;
            let path = context.path(&text, true)?;
            context.room()?;
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| fail("internal", error.to_string()))?;
            }
            let body = args.string("text")?;
            let written = if args.bool("append")? {
                use std::io::Write;
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
                    .and_then(|mut file| file.write_all(body.as_bytes()))
            } else {
                std::fs::write(&path, body.as_bytes())
            };
            written.map_err(|error| fail("internal", error.to_string()))?;
            let size = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
            context.action(json!({"action": "file.write", "path": text, "size": size}));
            Ok(json!({"path": text, "size": size}))
        }
        "read" => {
            let text = args.string("path")?;
            let path = context.path(&text, false)?;
            let offset = args.u64("offset")?;
            let limit = size_of(args.get("limit")).ok_or_else(|| bad("limit is a size"))?;
            let total = std::fs::metadata(&path)
                .map_err(|_| fail("not_found", format!("no file at {text}")))?
                .len();
            let bytes = read_part(&path, offset, limit);
            let more = offset + u64::try_from(bytes.len()).unwrap_or(0) < total;
            Ok(json!({"text": String::from_utf8_lossy(&bytes), "more": more}))
        }
        "path_list" => {
            let dir = args.string("dir")?;
            let roots = if dir.is_empty() {
                vec![
                    context.env.settings.directory.join("tmp"),
                    context.env.settings.directory.join("chats"),
                ]
            } else {
                let path = context.path(&format!("{}/x", dir.trim_end_matches('/')), false)?;
                vec![path.parent().map(Path::to_path_buf).unwrap_or(path)]
            };
            let mut list = Vec::new();
            for root in roots {
                walk(&root, &mut |path, meta| {
                    let at = meta
                        .modified()
                        .ok()
                        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|since| time_text(i64::try_from(since.as_millis()).unwrap_or(0)));
                    list.push(
                        json!({"path": context.relative(path), "size": meta.len(), "at": at}),
                    );
                });
            }
            let more = list.len() > usize::try_from(PAGE_MAX).unwrap_or(500);
            list.truncate(usize::try_from(PAGE_MAX).unwrap_or(500));
            Ok(json!({"path_list": list, "more": more}))
        }
        "remove" => {
            let text = args.string("path")?;
            let path = context.path(&text, true)?;
            std::fs::remove_file(&path)
                .map_err(|_| fail("not_found", format!("no file at {text}")))?;
            context.action(json!({"action": "file.remove", "path": text}));
            Ok(json!({"path": text}))
        }
        _ => Err(bad(format!("file.{function} is not a built-in"))),
    }
}

fn read_part(path: &Path, offset: u64, limit: u64) -> Vec<u8> {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    if file.seek(SeekFrom::Start(offset)).is_err() {
        return Vec::new();
    }
    let mut bytes = Vec::new();
    let _ = file.take(limit).read_to_end(&mut bytes);
    bytes
}

fn walk(root: &Path, visit: &mut dyn FnMut(&Path, &std::fs::Metadata)) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    paths.sort();
    for path in paths {
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.is_dir() {
            walk(&path, visit);
        } else if meta.is_file() {
            visit(&path, &meta);
        }
    }
}

/// Puts each `<secret name>` a text holds in its place, for a host it is bound to.
fn with_secrets(context: &Context, text: &str, host: &str) -> Result<String, Fail> {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("<secret ") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 8..];
        let Some(end) = after.find('>') else {
            out.push_str(&rest[start..]);
            return Ok(out);
        };
        let name = &after[..end];
        let Some(secret) = context.env.settings.secrets.get(name) else {
            return Err(fail(
                "secret_not_allowed",
                format!("no secret is named {name}"),
            ));
        };
        if !secret
            .hosts
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(host))
        {
            return Err(fail(
                "secret_not_allowed",
                format!("the secret {name} is not bound to {host}"),
            ));
        }
        out.push_str(&secret.value);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

fn http_failure(error: reqwest::Error) -> Fail {
    if error.is_timeout() {
        return fail("timeout", "the HTTP call took too long");
    }
    fail("offline", error.without_url().to_string())
}

fn http(context: &Context, function: &str, args: &Args) -> Result<Value, Fail> {
    let method = match function {
        "request" => args.string("method")?.to_uppercase(),
        "get" => "GET".to_string(),
        "post" => "POST".to_string(),
        _ => return Err(bad(format!("http.{function} is not a built-in"))),
    };
    let written = args.string("url")?;
    let parsed = reqwest::Url::parse(&written).map_err(|_| bad("url is not a URL"))?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(bad("url is http:// or https://"));
    }
    let host = parsed.host_str().unwrap_or_default().to_lowercase();
    if let Some(settings) = &context.env.settings.http
        && let Some(allow) = &settings.allow
        && !allow
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(&host))
    {
        return Err(fail(
            "host_not_allowed",
            format!("{host} is not in [http] allow"),
        ));
    }
    let url = with_secrets(context, &written, &host)?;
    let limits = &context.env.settings.limits;
    let timeout = args.duration("timeout")?.min(limits.http_time);
    let insecure = args.bool("insecure")?;
    let client = http_client(insecure, timeout);
    let method = reqwest::Method::from_bytes(method.as_bytes())
        .map_err(|_| bad("method is not an HTTP method"))?;
    let mut request = client.request(method.clone(), &url);
    if let Value::Object(headers) = args.get("headers") {
        for (name, value) in headers {
            let value = match value {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            };
            request = request.header(name, with_secrets(context, &value, &host)?);
        }
    }
    if args.present("json") {
        request = request.json(args.get("json"));
    } else if let Value::Object(form) = args.get("form") {
        let mut pairs = Vec::new();
        for (name, value) in form {
            let value = match value {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            };
            pairs.push((name.clone(), value));
        }
        request = request.form(&pairs);
    } else if args.present("body") {
        request = request.body(args.string("body")?);
    }
    let max = limits.http_body;
    let path = parsed.path().to_string();
    let result = context.block(
        async move {
            let response = match request.send().await {
                Ok(response) => response,
                Err(error) => return Err(http_failure(error)),
            };
            let status = response.status().as_u16();
            let mut headers = Map::new();
            for (name, value) in response.headers() {
                headers.insert(name.to_string(), json!(value.to_str().unwrap_or_default()));
            }
            let mut response = response;
            let mut body = Vec::new();
            loop {
                match response.chunk().await {
                    Ok(Some(chunk)) => {
                        body.extend_from_slice(&chunk);
                        if u64::try_from(body.len()).unwrap_or(u64::MAX) > max {
                            return Err(fail(
                                "too_large",
                                format!("the answer is over {max} bytes"),
                            ));
                        }
                    }
                    Ok(None) => break,
                    Err(error) => return Err(http_failure(error)),
                }
            }
            Ok((status, headers, body))
        },
        timeout,
    );
    let mut action =
        json!({"action": "http", "method": method.as_str(), "host": host, "path": path});
    let (status, headers, body) = match result {
        Some(Ok(answer)) => answer,
        Some(Err(failed)) => {
            action["err"] = json!(failed.code);
            context.action(action);
            return Err(failed);
        }
        None => {
            action["err"] = json!("timeout");
            context.action(action);
            return Err(fail("timeout", "the HTTP call took too long"));
        }
    };
    action["status"] = json!(status);
    context.action(action);
    let text = String::from_utf8_lossy(&body).to_string();
    let parsed_json = serde_json::from_str::<Value>(&text).ok();
    Ok(json!({
        "status": status,
        "headers": headers,
        "body": text,
        "json": parsed_json,
    }))
}

// -------------------------------------------------------------------------------------------------
// schedule, inbox, llm and time
// -------------------------------------------------------------------------------------------------

fn chat_ids(args: &Args) -> Result<Vec<Ulid>, Fail> {
    let mut list = Vec::new();
    for item in args.list("chat_list")? {
        let (chat, _, _) = audience(&item)?;
        list.push(chat);
    }
    Ok(list)
}

/// A schedule as the model reads it back.
pub fn schedule_value(row: &ScheduleRow) -> Value {
    let kind = match row.kind {
        0 => "at",
        1 => "every",
        _ => "cron",
    };
    let mut chats = Vec::new();
    for chat in &row.chat_list {
        chats.push(chat.to_string());
    }
    json!({
        "id": row.id.to_string(),
        "kind": kind,
        "when": row.spec,
        "tz": row.tz,
        "next_at": time_text(row.next_at),
        "note": row.note,
        "chat_list": chats,
        "join": row.join,
    })
}

/// The next fire of a schedule after `after`, none once an `at` has fired.
pub fn next_fire(row: &ScheduleRow, after: i64) -> Option<i64> {
    match row.kind {
        0 => None,
        1 => {
            let every = i64::try_from(duration_of(&row.spec)?.as_millis())
                .ok()?
                .max(1_000);
            let start = row.start.unwrap_or(row.made_at);
            if after < start {
                return Some(start);
            }
            let steps = (after - start) / every + 1;
            Some(start + steps * every)
        }
        _ => {
            let cron = Cron::parse(&row.spec).ok()?;
            let zone = jiff::tz::TimeZone::get(&row.tz).ok()?;
            let from = jiff::Timestamp::from_millisecond(after).ok()?;
            Some(cron.next_after(from, &zone)?.as_millisecond())
        }
    }
}

fn schedule(context: &Context, function: &str, args: &Args) -> Result<Value, Fail> {
    let (run, made_at) = match &context.mode {
        Mode::Step { run, .. } => (Some(*run), now()),
        Mode::Wake => (None, now()),
    };
    let mut row = ScheduleRow {
        id: new_id(),
        kind: 0,
        spec: String::new(),
        tz: context.env.settings.timezone.clone(),
        start: None,
        note: String::new(),
        chat_list: Vec::new(),
        join: false,
        next_at: 0,
        fired_at: None,
        run,
        made_at,
    };
    match function {
        "at" => {
            row.spec = args.string("time")?;
            row.next_at = args.time("time")?;
        }
        "every" => {
            row.kind = 1;
            row.spec = args.string("duration")?;
            let every = args.duration("duration")?;
            if every < Duration::from_secs(1) {
                return Err(bad("every is at least 1s"));
            }
            row.start = args.opt_time("start")?;
            row.next_at = match row.start {
                Some(start) if start > made_at => start,
                _ => next_fire(&row, made_at).ok_or_else(|| bad("duration is not a duration"))?,
            };
        }
        "cron" => {
            row.kind = 2;
            row.spec = args.string("expr")?;
            Cron::parse(&row.spec).map_err(|error| bad(error.to_string()))?;
            if let Some(tz) = args.opt_string("tz")? {
                jiff::tz::TimeZone::get(&tz)
                    .map_err(|_| bad(format!("{tz} is not an IANA zone")))?;
                row.tz = tz;
            }
            row.next_at = next_fire(&row, made_at).ok_or_else(|| bad("this cron never fires"))?;
        }
        "schedule_list" => {
            let rows = lock(&context.env.shared.store)
                .schedules()
                .map_err(|error| fail("internal", error.to_string()))?;
            let mut list = Vec::new();
            for row in &rows {
                list.push(schedule_value(row));
            }
            return Ok(json!({"schedule_list": list, "more": false}));
        }
        "remove" => {
            let id = args.ulid("id")?;
            let removed = lock(&context.env.shared.store)
                .remove_schedule(id)
                .map_err(|error| fail("internal", error.to_string()))?;
            if !removed {
                return Err(fail("not_found", "no such schedule"));
            }
            context.env.shared.wake.notify_one();
            context.action(json!({"action": "schedule.remove", "id": id.to_string()}));
            return Ok(json!({"id": id.to_string()}));
        }
        _ => return Err(bad(format!("schedule.{function} is not a built-in"))),
    }
    row.note = args.string("note")?;
    row.chat_list = chat_ids(args)?;
    row.join = args.bool("join")?;
    lock(&context.env.shared.store)
        .add_schedule(&row)
        .map_err(|error| fail("internal", error.to_string()))?;
    context.env.shared.wake.notify_one();
    context.action(json!({"action": format!("schedule.{function}"), "id": row.id.to_string(), "next_at": time_text(row.next_at)}));
    Ok(json!({"id": row.id.to_string()}))
}

fn inbox(context: &Context, args: &Args) -> Result<Value, Fail> {
    let Mode::Step { groups, .. } = &context.mode else {
        return Err(bad("a wake script only reads"));
    };
    let until = args.opt_time("after")?;
    let mut ids = Vec::new();
    for item in args.strings("id_list")? {
        match groups.get(&item) {
            Some(events) => ids.extend(events.iter().copied()),
            None => ids.push(
                Ulid::parse(&item)
                    .map_err(|_| bad(format!("{item} is not an event or group id")))?,
            ),
        }
    }
    let count = lock(&context.env.shared.store)
        .defer_events(&ids, until)
        .map_err(|error| fail("internal", error.to_string()))?;
    context.action(json!({"action": "inbox.defer", "count": count, "after": until.map(time_text)}));
    Ok(json!({"count": count}))
}

fn attach(context: &Context, args: &Args) -> Result<Value, Fail> {
    let text = args.string("path")?;
    let path = context.path(&text, false)?;
    if !path.is_file() {
        return Err(fail("not_found", format!("no file at {text}")));
    }
    let (mime, kind) = file_type(&text);
    if kind != ContentKind::Image {
        return Err(bad(format!("llm.attach takes an image, not {mime}")));
    }
    context.attach_list.borrow_mut().push(path);
    context.action(json!({"action": "llm.attach", "path": text}));
    Ok(json!({"path": text}))
}

fn time(function: &str, args: &Args) -> Result<Value, Fail> {
    match function {
        "now" => Ok(json!(time_text(now()))),
        "add" => {
            let at = args.time("time")?;
            let text = args.string("duration")?;
            let (sign, rest) = match text.strip_prefix('-') {
                Some(rest) => (-1, rest),
                None => (1, text.as_str()),
            };
            let by = duration_of(rest).ok_or_else(|| bad("duration is a duration"))?;
            let by = i64::try_from(by.as_millis()).unwrap_or(i64::MAX);
            Ok(json!(time_text(at.saturating_add(sign * by))))
        }
        "diff" => {
            let a = args.time("a")?;
            let b = args.time("b")?;
            Ok(json!(duration_text(b - a)))
        }
        _ => Err(bad(format!("time.{function} is not a built-in"))),
    }
}

/// What the content kind of a message is called, for the loop's context.
pub fn kind_name(kind: ContentKind) -> &'static str {
    content_kind_name(kind)
}
