//! The one loop: wakes, rules, context, compaction, limits, checkpoints and the run log. It starts
//! everything else, so `agent.toml`'s settings, which every part reads, live here.

use std::cell::Cell;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64ct::{Base64, Encoding};
use keryx_core::client::{Command, Event, Pointer, Secret as Hidden, ShownProfile, fold_name};
use keryx_core::ulid::Ulid;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio::sync::{mpsc, oneshot};

use crate::connect::{Ask, lock, new_id, now, time_text};
use crate::index::Index;
use crate::llm::{self, Llm, Reply, ToolCall};
use crate::script::{self, Env, Mode, WakeFailure, duration_text};
use crate::store::{
    EventKind, EventRow, NewEvent, OutgoingRow, OutgoingState, OutputRow, RunState, StepRow,
};

/// `agent.toml`, as `main.rs` read and checked it.
#[derive(Deserialize)]
pub struct Settings {
    /// The keryx server, `http://` or `https://`.
    pub url: String,
    /// The agent account's token, from its owner's app.
    pub token: String,
    /// The account's words. Never logged, never sent.
    pub words: String,
    /// Where `agent.db`, `index/`, `tmp/` and `chats/` live.
    pub directory: PathBuf,
    /// An IANA name: schedules' zone when they name none, and the preamble's local time.
    #[serde(default = "utc")]
    pub timezone: String,
    pub llm: LlmSettings,
    pub prompt: PromptSettings,
    #[serde(default)]
    pub context: ContextSettings,
    #[serde(default)]
    pub context_limits: ContextLimits,
    #[serde(default)]
    pub search: SearchSettings,
    #[serde(default)]
    pub memory: MemorySettings,
    pub skills: Option<SkillSettings>,
    #[serde(default)]
    pub limits: Limits,
    pub http: Option<HttpSettings>,
    /// `[secrets]`: each a value and the hosts it may go to.
    #[serde(default)]
    pub secrets: BTreeMap<String, Secret>,
    /// `[[wake]]`, in order: the first that matches decides.
    #[serde(default)]
    pub wake: Vec<WakeRule>,
}

fn utc() -> String {
    "UTC".to_string()
}

/// `[llm]`.
#[derive(Deserialize)]
pub struct LlmSettings {
    /// Only `chat_completions` for now.
    #[serde(default = "chat_completions")]
    pub protocol: String,
    pub url: String,
    pub key: String,
    pub model: String,
    /// The longest wait between failed calls.
    #[serde(default = "five_minutes")]
    pub backoff_max: Duration,
}

fn chat_completions() -> String {
    "chat_completions".to_string()
}

fn five_minutes() -> Duration {
    Duration::from_secs(5 * 60)
}

/// `[prompt]`. `compact` and `limit` unset mean the built-in defaults.
#[derive(Deserialize)]
pub struct PromptSettings {
    pub system: String,
    pub compact: Option<String>,
    pub limit: Option<String>,
    #[serde(default)]
    pub send_final: bool,
}

/// `[context]`.
#[derive(Deserialize)]
#[serde(default)]
pub struct ContextSettings {
    /// Input bytes before compaction.
    pub max: u64,
    /// Earlier messages per chat, topic and thread group.
    pub around: u64,
    /// Total JSON bytes of those messages.
    pub around_bytes: u64,
}

impl Default for ContextSettings {
    fn default() -> Self {
        let background = ContextBudget::default();
        Self {
            max: 256 * 1024,
            around: background.count,
            around_bytes: background.bytes,
        }
    }
}

/// `[context_limits]`. Scheduled runs and wake scripts have independent limits.
#[derive(Default, Deserialize)]
#[serde(default)]
pub struct ContextLimits {
    pub schedule: ContextBudget,
    pub wake_script: ContextBudget,
}

/// Limits on background messages and their JSON bytes.
#[derive(Clone, Copy, Deserialize)]
#[serde(default)]
pub struct ContextBudget {
    pub count: u64,
    pub bytes: u64,
}

impl Default for ContextBudget {
    fn default() -> Self {
        Self {
            count: 10,
            bytes: 16 * 1024,
        }
    }
}

/// `[search]`. `plain` on keeps messages' plaintext in the index.
#[derive(Deserialize, Default)]
#[serde(default)]
pub struct SearchSettings {
    pub plain: bool,
}

/// `[memory]`.
#[derive(Deserialize)]
#[serde(default)]
pub struct MemorySettings {
    /// The most common words per memory shown in the preamble. 0 shows counts only.
    pub hint_words: u64,
}

impl Default for MemorySettings {
    fn default() -> Self {
        Self { hint_words: 10 }
    }
}

/// `[skills]`.
#[derive(Deserialize)]
pub struct SkillSettings {
    pub directory: PathBuf,
    #[serde(default)]
    pub writable: bool,
}

/// `[limits]`.
#[derive(Deserialize)]
#[serde(default)]
pub struct Limits {
    pub script_time: Duration,
    pub script_output: u64,
    /// Per wake.
    pub tool_calls: u64,
    pub wake_script: Duration,
    pub http_time: Duration,
    pub http_body: u64,
    /// Every memory's notes together.
    pub memory_notes: u64,
    pub memory_note: u64,
    /// `agent.db`, `index/`, `tmp/` and `chats/` together.
    pub storage: u64,
    /// The share of `storage` past which a `storage` event wakes the loop.
    pub storage_warn: f64,
    /// Unset keeps run logs until removed.
    pub runs_keep: Option<Duration>,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            script_time: Duration::from_secs(30),
            script_output: 64 * 1024,
            tool_calls: 50,
            wake_script: Duration::from_secs(1),
            http_time: Duration::from_secs(30),
            http_body: 10 * 1024 * 1024,
            memory_notes: 10_000,
            memory_note: 16 * 1024,
            storage: 10 * 1024 * 1024 * 1024,
            storage_warn: 0.8,
            runs_keep: None,
        }
    }
}

/// `[http]`. `allow` unset means any host; a list, even an empty one, means only those.
#[derive(Deserialize)]
pub struct HttpSettings {
    pub allow: Option<Vec<String>>,
}

/// One of `[secrets]`.
#[derive(Deserialize)]
pub struct Secret {
    pub value: String,
    pub hosts: Vec<String>,
}

/// One `[[wake]]` rule. An empty list is a condition not set.
#[derive(Deserialize)]
pub struct WakeRule {
    #[serde(default)]
    pub chat: Vec<String>,
    #[serde(default)]
    pub chat_type: Vec<String>,
    #[serde(default)]
    pub folder: Vec<String>,
    #[serde(default)]
    pub topic: Vec<String>,
    /// True: only in threads. False: only main lists.
    pub thread: Option<bool>,
    #[serde(default)]
    pub sender: Vec<String>,
    #[serde(default)]
    pub content_kind: Vec<String>,
    #[serde(default)]
    pub event: Vec<String>,
    pub mentions_me: Option<bool>,
    pub replies_to_me: Option<bool>,
    /// A Starlark file defining `wake(event)`.
    pub script: Option<PathBuf>,
    /// Send an encrypted message to the owner for each script failure.
    #[serde(default)]
    pub report_failure: bool,
    /// `wake` or `ignore`.
    pub action: String,
    /// Overrides `[prompt]` for this rule.
    pub prompt: Option<PromptOverride>,
    /// Overrides `[context]` for this rule.
    pub context: Option<ContextOverride>,
}

/// `[wake.prompt]`.
#[derive(Deserialize)]
pub struct PromptOverride {
    pub system: Option<String>,
    pub compact: Option<String>,
    pub limit: Option<String>,
    pub send_final: Option<bool>,
}

/// `[wake.context]`.
#[derive(Deserialize)]
pub struct ContextOverride {
    pub max: Option<u64>,
    pub around: Option<u64>,
    pub around_bytes: Option<u64>,
}

// =================================================================================================
// The loop
// =================================================================================================

/// The draft of the built-in compaction prompt, for the author's approval before it is kept.
pub const COMPACT_PROMPT: &str = "\
This wake has grown past what the model can take in. Write a summary that lets you carry on from here \
with nothing else: what woke you and who asked for what, what you have done so far with every id, seq, \
username and time your scripts returned, what failed and why, and what is left to do. Write only the \
summary.";

/// The draft of the built-in limit prompt, for the author's approval before it is kept.
pub const LIMIT_PROMPT: &str = "\
You have used every tool call this wake allows. You have one last starlark call: leave things in a \
clear state. Tell the people waiting on you where you stopped and that they can tell you to continue, \
and keep in memory or a schedule what the next wake needs. The wake ends after that call.";

/// The draft of the fixed preamble after `[prompt] system`, for the author's approval before it is
/// kept. Who it is, the time, its memory and its skills follow it.
const PREAMBLE: &str = "\
You live in keryx, a messenger, as your own account. You act only through the starlark tool: text you \
write outside it reaches nobody. Its description holds its rules and every built-in.

Each wake starts fresh. The events that woke you come in the user message as JSON, grouped by chat, \
then topic, then thread, each group with nearby messages of its chat as context. Events are taken once \
you see them; inbox.defer puts events or whole groups back for later. New events arrive before each of \
your calls. What you did before is reachable only through log.run_list, memory and skills.

Memory is searched, never shown: when the words below touch what you are doing, search it with \
memory.note_list(query=...).";

/// What a compaction's summary is introduced with.
const COMPACTED: &str = "This wake so far, compacted:\n\n";

/// How often storage is measured and the owner looked for.
const EVERY_MINUTE: i64 = 60_000;

/// How often old runs are removed, besides at startup.
const DAY: i64 = 86_400_000;

/// How long the loop waits for a profile from the server.
const ASK_TIME: Duration = Duration::from_secs(20);

/// Trouble's causes, by rank: the first one set is the presence text sent.
const STORAGE_FULL: u8 = 0;
const LLM_FAILING: u8 = 1;
const STUCK: u8 = 2;
const STORAGE_WARN: u8 = 3;

/// The agent's trouble, sent as its presence text to its owner.
struct Trouble {
    asks: mpsc::UnboundedSender<Ask>,
    causes: BTreeMap<u8, String>,
    sent: Option<String>,
}

impl Trouble {
    fn set(&mut self, rank: u8, text: Option<String>) {
        match text {
            Some(text) => {
                self.causes.insert(rank, text);
            }
            None => {
                self.causes.remove(&rank);
            }
        }
        let first = self.causes.values().next().cloned();
        if first != self.sent {
            match &first {
                Some(text) => tracing::warn!(
                    code = text.split(':').next().unwrap_or_default(),
                    "having trouble"
                ),
                None => tracing::info!("trouble cleared"),
            }
            self.sent = first.clone();
            let _ = self.asks.send(Ask::Trouble { text: first });
        }
    }
}

/// Waiting events sorted by the rules.
#[derive(Default)]
struct Sorted {
    /// Those that wake the loop.
    woken: Vec<EventRow>,
    /// The rule the oldest woken event matched, whose overrides the run takes.
    rule: Option<usize>,
    /// Those shown with the next wake but waking nothing: writes' results and events deferred
    /// with no time.
    riding: Vec<EventRow>,
}

/// One run as it goes.
struct Wake {
    run: Ulid,
    rule: Option<usize>,
    system: Value,
    messages: Vec<Value>,
    /// Which message holds which step's tool output, oldest first, so compaction can trim them.
    tool_steps: VecDeque<(usize, u64)>,
    /// Messages already in the input, by chat and seq, never sent twice until a compaction.
    shown: HashSet<(Ulid, u64)>,
    /// The events each group holds, for `inbox.defer`.
    groups: HashMap<String, Vec<Ulid>>,
    next_group: u64,
    /// Tool calls run.
    calls: u64,
    step: u64,
    /// The first message that woke it: `send_final` replies to it, and notes name it as their source.
    trigger: Option<Value>,
    /// `llm.attach`'s images for the next input.
    attach: Vec<PathBuf>,
    /// The tool calls run when the limit prompt went in: one more ends the wake.
    limit_at: Option<u64>,
    /// Compaction failed once already, so the wake is ending through the limit prompt.
    squeezed: bool,
    /// The events step 1 shows.
    first: Vec<EventRow>,
}

impl Wake {
    fn new(run: Ulid, rule: Option<usize>) -> Self {
        Self {
            run,
            rule,
            system: Value::Null,
            messages: Vec::new(),
            tool_steps: VecDeque::new(),
            shown: HashSet::new(),
            groups: HashMap::new(),
            next_group: 0,
            calls: 0,
            step: 0,
            trigger: None,
            attach: Vec::new(),
            limit_at: None,
            squeezed: false,
            first: Vec::new(),
        }
    }

    /// Where a note made in this run comes from: the trigger's chat, sender and seq.
    fn source(&self) -> Option<(Ulid, Option<Ulid>, Option<u64>)> {
        let trigger = self.trigger.as_ref()?;
        let chat = Ulid::parse(trigger["chat"].as_str()?).ok()?;
        let sender = trigger["sender"]
            .as_str()
            .and_then(|text| Ulid::parse(text).ok());
        Some((chat, sender, trigger["seq"].as_u64()))
    }
}

/// The loop's own state.
struct Agent {
    env: Arc<Env>,
    llm: Llm,
    trouble: Mutex<Trouble>,
    /// When this process started: a schedule due before it was late because the agent was down.
    started: i64,
    /// Schedules that came due while a run went on, and waited for it.
    waited: HashSet<Ulid>,
    storage_at: i64,
    trimmed_at: i64,
    warned: bool,
    full: bool,
}

/// Runs the loop on the calling thread, which may block, for as long as the process lives.
pub fn run(env: Arc<Env>) {
    let settings = env.settings.clone();
    let llm = Llm::new(
        &settings.llm.url,
        &settings.llm.key,
        &settings.llm.model,
        settings.llm.backoff_max,
    );
    let trouble = Trouble {
        asks: env.shared.asks.clone(),
        causes: BTreeMap::new(),
        sent: None,
    };
    let mut agent = Agent {
        env,
        llm,
        trouble: Mutex::new(trouble),
        started: now(),
        waited: HashSet::new(),
        storage_at: 0,
        trimmed_at: 0,
        warned: false,
        full: false,
    };
    agent.remove_old_runs(now());
    let running = lock(&agent.env.shared.store).running();
    match running {
        Ok(Some(run)) => agent.resume(run.id),
        Ok(None) => {}
        Err(error) => tracing::error!(error = %error, "could not read the run log"),
    }
    loop {
        agent.housekeeping();
        agent.fire_due(true);
        let sorted = agent.sort_waiting(true);
        if sorted.woken.is_empty() {
            agent.sleep();
        } else {
            agent.wake(sorted);
        }
    }
}

/// The input's size, as the provider receives it.
fn size(messages: &[Value]) -> u64 {
    let text = serde_json::to_string(messages).unwrap_or_default();
    u64::try_from(text.len()).unwrap_or(u64::MAX)
}

fn user(text: String) -> Value {
    json!({"role": "user", "content": text})
}

/// A size as the presence text shows it, such as `8.3 GiB`.
fn size_text(bytes: u64) -> String {
    for (unit, size) in [("GiB", 1u64 << 30), ("MiB", 1 << 20), ("KiB", 1 << 10)] {
        if bytes >= size {
            return format!("{:.1} {unit}", bytes as f64 / size as f64);
        }
    }
    format!("{bytes} B")
}

/// The time in a zone, for the preamble.
fn local_text(stamp: i64, zone: &str) -> String {
    let Ok(tz) = jiff::tz::TimeZone::get(zone) else {
        return String::new();
    };
    match jiff::Timestamp::from_millisecond(stamp) {
        Ok(time) => time.to_zoned(tz).strftime("%Y-%m-%d %H:%M, %A").to_string(),
        Err(_) => String::new(),
    }
}

fn listed(list: &[String], value: Option<&str>) -> bool {
    list.is_empty() || value.is_some_and(|value| list.iter().any(|item| item == value))
}

/// Who an event is from, by username.
fn sender_of(body: &Value) -> Option<&str> {
    for value in [
        &body["message"]["sender"],
        &body["sender"],
        &body["by"],
        &body["inviter"],
    ] {
        if let Some(name) = value["username"].as_str() {
            return Some(name);
        }
    }
    None
}

/// Whether an event is about a stored message of its chat, whose seq it carries.
fn about_message(kind: EventKind) -> bool {
    matches!(
        kind,
        EventKind::Message
            | EventKind::Reaction
            | EventKind::Answer
            | EventKind::Ack
            | EventKind::Edit
            | EventKind::Delete
            | EventKind::Added
            | EventKind::Removed
    )
}

/// An event as the model reads it: its body with its id, without the chat fields its group carries
/// when `bare`.
fn event_json(row: &EventRow, bare: bool) -> Value {
    let mut body = row.body.clone();
    if let Some(object) = body.as_object_mut() {
        if bare {
            object.remove("chat");
            object.remove("chat_type");
            object.remove("chat_name");
        }
        object.insert("id".to_string(), json!(row.id.to_string()));
    }
    body
}

/// The first message that woke a run.
fn trigger_of(events: &[EventRow]) -> Option<Value> {
    for row in events {
        let message = matches!(
            row.kind,
            EventKind::Message | EventKind::Reaction | EventKind::Answer | EventKind::Ack
        );
        if let (true, Some(chat), Some(seq)) = (message, row.chat, row.seq) {
            return Some(json!({
                "chat": chat.to_string(),
                "seq": seq,
                "topic": row.topic.map(|topic| topic.to_string()),
                "thread": row.thread_seq,
                "sender": row.body["message"]["sender"]["id"],
            }));
        }
    }
    None
}

/// A step's tool output, from its saved prints and actions.
fn tool_text(outputs: &[OutputRow], cut: bool, trimmed: bool) -> String {
    let mut prints = String::new();
    let mut actions = String::new();
    for output in outputs {
        if output.action {
            actions.push_str(&output.body);
            actions.push('\n');
        } else {
            prints.push_str(&output.body);
            if !output.body.ends_with('\n') {
                prints.push('\n');
            }
        }
    }
    let mut text = String::new();
    if cut {
        text.push_str(
            "A restart cut this script short. What it printed and did until then follows; it was \
             not run again.\n",
        );
    }
    if trimmed {
        text.push_str("Its printed output was removed to fit the context; its actions are kept.\n");
    } else {
        text.push_str("output:\n");
        if prints.is_empty() {
            text.push_str("(nothing printed)\n");
        } else {
            text.push_str(&prints);
        }
    }
    text.push_str("actions:\n");
    if actions.is_empty() {
        text.push_str("(none)\n");
    } else {
        text.push_str(&actions);
    }
    text
}

/// Events made into a step's input.
struct Input {
    text: String,
    ids: Vec<Ulid>,
    /// Messages newly shown, by chat and seq.
    shown: Vec<(String, u64)>,
    groups: Map<String, Value>,
}

/// A step's intentions: each text with its audience.
type Intentions = Vec<(String, Vec<(Ulid, Option<Ulid>, Option<u64>)>)>;

/// Reads a tool call: its script and its checked intention list.
fn parse_call(call: &ToolCall) -> Result<(String, Intentions), String> {
    if call.name != "starlark" {
        return Err(format!(
            "there is no tool {:?}: the one tool is starlark",
            call.name
        ));
    }
    let arguments: Value = serde_json::from_str(&call.arguments)
        .map_err(|_| "the arguments are not JSON".to_string())?;
    let script = arguments["script"]
        .as_str()
        .ok_or_else(|| "script is missing".to_string())?
        .to_string();
    let Some(entries) = arguments["intention_list"].as_array() else {
        return Err("intention_list is missing: say what this step is for".to_string());
    };
    if entries.is_empty() {
        return Err("intention_list needs at least one entry".to_string());
    }
    let mut list = Vec::new();
    let mut named = HashSet::new();
    for entry in entries {
        let text = entry["intention"]
            .as_str()
            .map(str::trim)
            .unwrap_or_default();
        if text.is_empty() || text.chars().count() > 140 {
            return Err("each intention is 1 to 140 characters".to_string());
        }
        let mut audience = Vec::new();
        for item in entry["chat_list"].as_array().cloned().unwrap_or_default() {
            let place = script::audience(&item).map_err(|fail| fail.message().to_string())?;
            if !named.insert(place.0) {
                return Err(format!(
                    "chat {} is in more than one intention: one text per chat",
                    place.0
                ));
            }
            audience.push(place);
        }
        list.push((text.to_string(), audience));
    }
    Ok((script, list))
}

impl Agent {
    fn rule(&self, wake: &Wake) -> Option<&WakeRule> {
        wake.rule
            .and_then(|index| self.env.settings.wake.get(index))
    }

    fn prompt_of(&self, wake: &Wake) -> Option<&PromptOverride> {
        self.rule(wake).and_then(|rule| rule.prompt.as_ref())
    }

    fn compact_prompt(&self, wake: &Wake) -> String {
        let prompt = &self.env.settings.prompt;
        self.prompt_of(wake)
            .and_then(|over| over.compact.clone())
            .or_else(|| prompt.compact.clone())
            .unwrap_or_else(|| COMPACT_PROMPT.to_string())
    }

    fn limit_prompt(&self, wake: &Wake) -> String {
        let prompt = &self.env.settings.prompt;
        self.prompt_of(wake)
            .and_then(|over| over.limit.clone())
            .or_else(|| prompt.limit.clone())
            .unwrap_or_else(|| LIMIT_PROMPT.to_string())
    }

    fn send_final(&self, wake: &Wake) -> bool {
        self.prompt_of(wake)
            .and_then(|over| over.send_final)
            .unwrap_or(self.env.settings.prompt.send_final)
    }

    fn max(&self, wake: &Wake) -> u64 {
        self.rule(wake)
            .and_then(|rule| rule.context.as_ref())
            .and_then(|context| context.max)
            .unwrap_or(self.env.settings.context.max)
    }

    fn set_trouble(&self, rank: u8, text: Option<String>) {
        lock(&self.trouble).set(rank, text);
    }

    // ---------------------------------------------------------------------------------------------
    // Between wakes
    // ---------------------------------------------------------------------------------------------

    fn remove_old_runs(&mut self, stamp: i64) {
        self.trimmed_at = stamp;
        let Some(keep) = self.env.settings.limits.runs_keep else {
            return;
        };
        let before = stamp.saturating_sub(i64::try_from(keep.as_millis()).unwrap_or(i64::MAX));
        match lock(&self.env.shared.store).remove_runs(before) {
            Ok(0) => {}
            Ok(count) => tracing::info!(count, "old runs removed"),
            Err(error) => tracing::error!(error = %error, "could not remove old runs"),
        }
    }

    fn housekeeping(&mut self) {
        let stamp = now();
        if stamp - self.trimmed_at >= DAY {
            self.remove_old_runs(stamp);
        }
        if stamp - self.storage_at >= EVERY_MINUTE {
            self.storage_at = stamp;
            self.storage();
        }
        self.learn_owner();
    }

    /// Measures storage: past `storage_warn` a `storage` event wakes the loop once, and at the cap
    /// run logs keep only their actions.
    fn storage(&mut self) {
        let usage = script::usage(&self.env);
        let limits = &self.env.settings.limits;
        let of = format!(
            "{} of {}",
            size_text(usage.total),
            size_text(limits.storage)
        );
        if usage.total >= limits.storage {
            if !self.full {
                self.full = true;
                tracing::warn!(
                    total = usage.total,
                    limit = limits.storage,
                    "storage is at its cap: run logs keep only their actions"
                );
                if let Err(error) = lock(&self.env.shared.store).trim_runs() {
                    tracing::error!(error = %error, "could not trim the run logs");
                }
            }
            self.set_trouble(STORAGE_FULL, Some(format!("storage_full: {of}")));
        } else {
            self.full = false;
            self.set_trouble(STORAGE_FULL, None);
        }
        let warn_at = (limits.storage as f64 * limits.storage_warn) as u64;
        if usage.total >= warn_at {
            if !self.warned {
                self.warned = true;
                let event = NewEvent {
                    kind: EventKind::Storage,
                    chat: None,
                    topic: None,
                    thread_seq: None,
                    seq: None,
                    body: json!({
                        "event": "storage",
                        "total": usage.total,
                        "limit": limits.storage,
                        "warn": limits.storage_warn,
                        "runs": usage.runs,
                        "memory": usage.memory,
                        "files": usage.files,
                        "archive": usage.archive,
                        "index": usage.index,
                    }),
                    until: None,
                };
                if let Err(error) = lock(&self.env.shared.store).add_event(&event, now()) {
                    tracing::error!(error = %error, "could not keep the storage event");
                }
            }
            self.set_trouble(STORAGE_WARN, Some(format!("storage_warn: {of}")));
        } else {
            self.warned = false;
            self.set_trouble(STORAGE_WARN, None);
        }
    }

    fn profile(&self, account: Ulid) -> Option<ShownProfile> {
        let shared = &self.env.shared;
        let answer = self.env.runtime.block_on(async {
            tokio::time::timeout(ASK_TIME, shared.command(Command::Profile { account }))
                .await
                .ok()
                .flatten()
        })?;
        for event in answer.event_list {
            if let Event::Profile { profile } = event
                && profile.account == account
            {
                return Some(profile);
            }
        }
        None
    }

    /// Looks up the agent's owner once it is ready, for `OWNER` and the preamble.
    fn learn_owner(&self) {
        if !lock(&self.env.owner).is_null() {
            return;
        }
        let account = {
            let view = lock(&self.env.shared.view);
            if !view.ready {
                return;
            }
            view.account
        };
        let Some(account) = account else {
            return;
        };
        let Some(owner) = self.profile(account).and_then(|profile| profile.owner) else {
            return;
        };
        let _ = self.profile(owner);
        let person = lock(&self.env.shared.view).person(owner);
        *lock(&self.env.owner) = person;
    }

    /// Fires every schedule that is due: all of them while idle, and only those that may join while
    /// a run goes on.
    fn fire_due(&mut self, idle: bool) {
        let stamp = now();
        let rows = match lock(&self.env.shared.store).schedules() {
            Ok(rows) => rows,
            Err(error) => {
                tracing::error!(error = %error, "could not read the schedules");
                return;
            }
        };
        for row in rows {
            if row.next_at > stamp {
                break;
            }
            if !idle && !row.join {
                self.waited.insert(row.id);
                continue;
            }
            let late = stamp - row.next_at;
            let why = if row.next_at < self.started {
                Some("the agent was down")
            } else if self.waited.remove(&row.id) {
                Some("it waited for the loop to end")
            } else {
                None
            };
            let next = script::next_fire(&row, stamp);
            let mut body = script::schedule_value(&row);
            if let Some(object) = body.as_object_mut() {
                object.remove("next_at");
                object.insert("event".to_string(), json!("schedule"));
                object.insert("planned_at".to_string(), json!(time_text(row.next_at)));
                object.insert("fired_at".to_string(), json!(time_text(stamp)));
                if late >= 1_000 {
                    object.insert("late".to_string(), json!(duration_text(late)));
                    object.insert("why".to_string(), json!(why));
                }
            }
            let event = NewEvent {
                kind: EventKind::Schedule,
                chat: None,
                topic: None,
                thread_seq: None,
                seq: None,
                body,
                until: None,
            };
            match lock(&self.env.shared.store).fire(&row, &event, next, stamp) {
                Ok(id) => {
                    tracing::info!(schedule = %row.id, event = %id, late_ms = late, "a schedule fired")
                }
                Err(error) => {
                    tracing::error!(schedule = %row.id, error = %error, "could not fire a schedule")
                }
            }
        }
    }

    /// Sorts what waits by the `[[wake]]` rules, marking what they ignore. While a run goes on,
    /// events deferred with no time stay for the next wake.
    fn sort_waiting(&mut self, idle: bool) -> Sorted {
        let rows = match lock(&self.env.shared.store).waiting_events(now()) {
            Ok(rows) => rows,
            Err(error) => {
                tracing::error!(error = %error, "could not read the incoming queue");
                return Sorted::default();
            }
        };
        let mut sorted = Sorted::default();
        let mut ignored = Vec::new();
        for row in rows {
            let wakes = match row.kind {
                EventKind::Schedule | EventKind::Storage => Some(None),
                EventKind::Sent => {
                    sorted.riding.push(row);
                    continue;
                }
                _ if row.deferred && row.until.is_none() => {
                    if idle {
                        sorted.riding.push(row);
                    }
                    continue;
                }
                _ if row.deferred => Some(None),
                _ => self.decide(&row).map(Some),
            };
            match wakes {
                Some(rule) => {
                    if sorted.woken.is_empty() {
                        sorted.rule = rule;
                    }
                    sorted.woken.push(row);
                }
                None => ignored.push(row.id),
            }
        }
        if !ignored.is_empty() {
            tracing::debug!(count = ignored.len(), "events ignored by the wake rules");
            if let Err(error) = lock(&self.env.shared.store).ignore_events(&ignored) {
                tracing::error!(error = %error, "could not mark ignored events");
            }
        }
        sorted
    }

    /// The rule that wakes the loop for an event, or none when it is ignored. The first rule that
    /// matches decides; no match ignores.
    fn decide(&self, row: &EventRow) -> Option<usize> {
        for (index, rule) in self.env.settings.wake.iter().enumerate() {
            if !self.conditions(rule, row) {
                continue;
            }
            if let Some(path) = &rule.script {
                match self.wake_script(path, row) {
                    Ok(true) => {}
                    Ok(false) => continue,
                    Err(failure) => {
                        let rule_number = index + 1;
                        tracing::warn!(msg = "Skipped a failing wake rule", rule = rule_number, reason = %failure);
                        if !rule.report_failure {
                            continue;
                        }
                        let (owner, mut chat) = {
                            let view = lock(&self.env.shared.view);
                            let owner = if let Some(account) = view.account
                                && let Some(agent) = view.accounts.get(&account)
                            {
                                agent.owner
                            } else {
                                None
                            };
                            let Some(owner) = owner else {
                                tracing::warn!(
                                    msg = "Could not identify the owner for a wake rule report",
                                    rule = rule_number
                                );
                                continue;
                            };
                            (owner, view.direct_with(owner))
                        };
                        if chat.is_none() {
                            let shared = &self.env.shared;
                            let answer = self.env.runtime.block_on(async {
                                tokio::time::timeout(
                                    ASK_TIME,
                                    shared.command(Command::StartChat { account: owner }),
                                )
                                .await
                            });
                            if let Ok(Some(answer)) = answer
                                && answer.code.is_none()
                            {
                                for event in answer.event_list {
                                    if let Event::Started {
                                        chat: started,
                                        account,
                                    } = event
                                        && account == owner
                                    {
                                        chat = Some(started);
                                        break;
                                    }
                                }
                            }
                        }
                        let Some(chat) = chat else {
                            tracing::warn!(
                                msg = "Could not open the owner's chat for a wake rule report",
                                rule = rule_number
                            );
                            continue;
                        };
                        let command = Command::SendText {
                            chat,
                            text: Hidden(format!("Wake rule {rule_number} failed. {failure}")),
                            reply: None,
                            topic: None,
                            thread: None,
                            mentions: Vec::new(),
                        };
                        self.send_kept(None, chat, command);
                        continue;
                    }
                }
            }
            if rule.action == "wake" {
                return Some(index);
            }
            return None;
        }
        None
    }

    /// A rule's script, one more condition. Its failure gives a reason safe to share.
    fn wake_script(&self, path: &std::path::Path, row: &EventRow) -> Result<bool, WakeFailure> {
        let script = match std::fs::read_to_string(path) {
            Ok(script) => script,
            Err(_) => return Err(WakeFailure::Unreadable),
        };
        let mut event = event_json(row, false);
        if let (Some(chat), Some(seq), true) = (row.chat, row.seq, about_message(row.kind)) {
            let topic = match lock(&self.env.shared.view).chats.get(&chat) {
                Some(known) if known.topics_on => Some(row.topic),
                _ => None,
            };
            let context = script::background(
                &self.env,
                (chat, topic, row.thread_seq),
                Some(seq),
                self.env.settings.context_limits.wake_script,
                &HashSet::new(),
            );
            event["context"] = json!(context);
        }
        script::run_wake(self.env.clone(), &script, &event)
    }

    fn conditions(&self, rule: &WakeRule, row: &EventRow) -> bool {
        let body = &row.body;
        let chat = row.chat.map(|chat| chat.to_string());
        if !listed(&rule.chat, chat.as_deref()) {
            return false;
        }
        if !listed(&rule.chat_type, body["chat_type"].as_str()) {
            return false;
        }
        if !rule.folder.is_empty() {
            let Some(chat) = row.chat else {
                return false;
            };
            let folders = lock(&self.env.shared.view).folders_of(chat);
            let mut wanted = Vec::new();
            for folder in &rule.folder {
                wanted.push(fold_name(folder));
            }
            if !folders
                .iter()
                .any(|folder| wanted.contains(&fold_name(folder)))
            {
                return false;
            }
        }
        let topic = row.topic.map(|topic| topic.to_string());
        if !listed(&rule.topic, topic.as_deref()) {
            return false;
        }
        if let Some(thread) = rule.thread
            && row.thread_seq.is_some() != thread
        {
            return false;
        }
        if !listed(&rule.sender, sender_of(body)) {
            return false;
        }
        if !listed(&rule.content_kind, body["message"]["kind"].as_str()) {
            return false;
        }
        if !listed(&rule.event, Some(row.kind.name())) {
            return false;
        }
        for (wanted, key) in [
            (rule.mentions_me, "mentions_me"),
            (rule.replies_to_me, "replies_to_me"),
        ] {
            if let Some(wanted) = wanted
                && body[key].as_bool().unwrap_or(false) != wanted
            {
                return false;
            }
        }
        true
    }

    /// Waits for an event, the next schedule or deferred event, or a minute.
    fn sleep(&self) {
        let stamp = now();
        let mut until = stamp + EVERY_MINUTE;
        {
            let store = lock(&self.env.shared.store);
            if let Ok(rows) = store.schedules()
                && let Some(first) = rows.first()
            {
                until = until.min(first.next_at);
            }
            if let Ok(Some(at)) = store.next_deferred_at() {
                until = until.min(at);
            }
        }
        let wait = Duration::from_millis(u64::try_from(until - stamp).unwrap_or(0));
        let shared = &self.env.shared;
        self.env.runtime.block_on(async {
            tokio::select! {
                _ = shared.wake.notified() => {}
                _ = tokio::time::sleep(wait) => {}
            }
        });
    }

    // ---------------------------------------------------------------------------------------------
    // A wake
    // ---------------------------------------------------------------------------------------------

    fn wake(&mut self, sorted: Sorted) {
        self.set_trouble(STUCK, None);
        let run = match lock(&self.env.shared.store).start_run(now()) {
            Ok(run) => run,
            Err(error) => {
                tracing::error!(error = %error, "could not start a run");
                std::thread::sleep(Duration::from_secs(1));
                return;
            }
        };
        tracing::info!(run = %run, events = sorted.woken.len(), "a wake starts");
        let mut wake = Wake::new(run, sorted.rule);
        wake.trigger = trigger_of(&sorted.woken);
        wake.first = sorted.woken;
        wake.first.extend(sorted.riding);
        self.steps(&mut wake, None);
    }

    /// Continues a run a restart cut short, from the phase its last step reached.
    fn resume(&mut self, run: Ulid) {
        let steps = match lock(&self.env.shared.store).steps(run) {
            Ok(steps) => steps,
            Err(error) => {
                tracing::error!(run = %run, error = %error, "could not read a cut run");
                return;
            }
        };
        let Some(last) = steps.last().cloned() else {
            // Cut before its first step was saved: its events were never taken, so they wake the
            // loop again.
            if let Err(error) = lock(&self.env.shared.store).end_run(run, RunState::Failed, now()) {
                tracing::error!(run = %run, error = %error, "could not end a cut run");
            }
            return;
        };
        tracing::info!(run = %run, step = last.n, phase = last.phase, "a wake continues after a restart");
        let mut wake = Wake::new(run, None);
        for step in &steps {
            let context: Value = serde_json::from_str(&step.context).unwrap_or(Value::Null);
            if context["reset"].as_bool() == Some(true) {
                wake.messages.clear();
                wake.tool_steps.clear();
                wake.shown.clear();
            }
            if let Some(rule) = context.get("rule") {
                wake.rule = rule.as_u64().and_then(|rule| usize::try_from(rule).ok());
            }
            if let Some(trigger) = context.get("trigger")
                && !trigger.is_null()
            {
                wake.trigger = Some(trigger.clone());
            }
            for message in context["messages"].as_array().cloned().unwrap_or_default() {
                if message["role"] == "system" {
                    wake.system = message.clone();
                }
                wake.messages.push(message);
            }
            for pair in context["shown"].as_array().cloned().unwrap_or_default() {
                let chat = pair[0].as_str().and_then(|text| Ulid::parse(text).ok());
                if let (Some(chat), Some(seq)) = (chat, pair[1].as_u64()) {
                    wake.shown.insert((chat, seq));
                }
            }
            if let Some(groups) = context["groups"].as_object() {
                for (name, ids) in groups {
                    let mut list = Vec::new();
                    for id in ids.as_array().cloned().unwrap_or_default() {
                        if let Some(id) = id.as_str().and_then(|text| Ulid::parse(text).ok()) {
                            list.push(id);
                        }
                    }
                    let number = name.trim_start_matches('g').parse().unwrap_or(0);
                    wake.next_group = wake.next_group.max(number);
                    wake.groups.insert(name.clone(), list);
                }
            }
            if context["limit"].as_bool() == Some(true) {
                wake.limit_at = Some(wake.calls);
            }
            wake.step = step.n;
            let answer = step
                .answer
                .as_deref()
                .and_then(|answer| serde_json::from_str::<Value>(answer).ok());
            if step.phase >= 3
                && let Some(message) = answer
            {
                wake.messages.push(message.clone());
                let reply = Reply::of(message, None, None);
                if step.phase == 6 && !reply.tool_call_list.is_empty() {
                    self.answer_tools(&mut wake, step.n, &reply, step.cut);
                    wake.calls += 1;
                }
            }
        }
        let current = if last.phase < 6 { Some(last) } else { None };
        self.steps(&mut wake, current);
    }

    /// Steps until the wake ends: each an LLM call, then its script.
    fn steps(&mut self, wake: &mut Wake, mut current: Option<StepRow>) {
        loop {
            let mut row = match current.take() {
                Some(row) => row,
                None => match self.next_step(wake) {
                    Some(row) => row,
                    None => return,
                },
            };
            if row.phase <= 2 && !self.call(wake, &mut row) {
                self.end(wake, RunState::Limit, Some("could not compact".to_string()));
                return;
            }
            let message = row
                .answer
                .as_deref()
                .and_then(|answer| serde_json::from_str::<Value>(answer).ok())
                .unwrap_or(Value::Null);
            let reply = Reply::of(message, row.tokens_in, row.tokens_out);
            if row.phase == 3 {
                if reply.tool_call_list.is_empty() {
                    self.finish(wake, &row, &reply);
                    return;
                }
                self.run_script(wake, &mut row, &reply);
            } else if row.phase < 6 {
                self.cut(wake, &mut row, &reply);
            }
            wake.calls += 1;
        }
    }

    fn put(&self, wake: &Wake, row: &StepRow) {
        if let Err(error) = lock(&self.env.shared.store).put_step(wake.run, row) {
            tracing::error!(run = %wake.run, step = row.n, error = %error, "could not save a step");
        }
    }

    /// Phase 1: the next step's input, with the events that arrived since, saved.
    fn next_step(&mut self, wake: &mut Wake) -> Option<StepRow> {
        if let Some(at) = wake.limit_at
            && wake.calls > at
        {
            let limit = self.env.settings.limits.tool_calls;
            self.end(
                wake,
                RunState::Limit,
                Some(format!("reached its limit of {limit} tool calls")),
            );
            return None;
        }
        self.housekeeping();
        wake.step += 1;
        let mut new = Vec::new();
        let mut context = json!({});
        if wake.messages.is_empty() {
            wake.system = self.system_message(wake);
            new.push(wake.system.clone());
            context["reset"] = json!(true);
            context["rule"] = json!(wake.rule);
            context["trigger"] = wake.trigger.clone().unwrap_or(Value::Null);
        }
        let events = if wake.step == 1 {
            std::mem::take(&mut wake.first)
        } else {
            self.fire_due(false);
            let sorted = self.sort_waiting(false);
            let mut list = sorted.woken;
            list.extend(sorted.riding);
            list
        };
        if !events.is_empty() {
            let Input {
                text,
                ids,
                shown,
                groups,
            } = self.build_input(wake, &events);
            new.push(user(text));
            if let Err(error) = lock(&self.env.shared.store).take_events(&ids, wake.run, wake.step)
            {
                tracing::error!(run = %wake.run, error = %error, "could not mark events taken");
            }
            context["shown"] = json!(shown);
            context["groups"] = Value::Object(groups);
        }
        if !wake.attach.is_empty() {
            let mut parts = vec![json!({"type": "text", "text": "Images llm.attach added:"})];
            for path in wake.attach.drain(..) {
                parts.push(json!({"type": "keryx_attach", "path": path.to_string_lossy()}));
            }
            new.push(json!({"role": "user", "content": parts}));
        }
        let limit = self.env.settings.limits.tool_calls;
        if wake.limit_at.is_none() && wake.calls >= limit {
            wake.limit_at = Some(wake.calls);
            new.push(user(self.limit_prompt(wake)));
            context["limit"] = json!(true);
        }
        wake.messages.extend(new.iter().cloned());
        context["messages"] = json!(new);
        let mut row = StepRow {
            n: wake.step,
            phase: 1,
            context: context.to_string(),
            ..StepRow::default()
        };
        self.put(wake, &row);
        if size(&wake.messages) > self.max(wake) && !self.compact(wake, &mut row) {
            self.end(wake, RunState::Limit, Some("could not compact".to_string()));
            return None;
        }
        Some(row)
    }

    /// The input as the provider takes it: `llm.attach`'s images read in as data URLs.
    fn expand(&self, messages: &[Value]) -> Vec<Value> {
        let mut list = Vec::new();
        for message in messages {
            let Some(parts) = message["content"].as_array() else {
                list.push(message.clone());
                continue;
            };
            let mut expanded = Vec::new();
            for part in parts {
                if part["type"] != "keryx_attach" {
                    expanded.push(part.clone());
                    continue;
                }
                let path = part["path"].as_str().unwrap_or_default();
                let mime = match path
                    .rsplit_once('.')
                    .map(|(_, extension)| extension.to_lowercase())
                {
                    Some(extension) if extension == "png" => "image/png",
                    Some(extension) if extension == "gif" => "image/gif",
                    Some(extension) if extension == "webp" => "image/webp",
                    _ => "image/jpeg",
                };
                match std::fs::read(path) {
                    Ok(bytes) => expanded.push(json!({
                        "type": "image_url",
                        "image_url": {"url": format!("data:{mime};base64,{}", Base64::encode_string(&bytes))},
                    })),
                    Err(_) => expanded.push(json!({
                        "type": "text",
                        "text": "[an attached image could not be read]",
                    })),
                }
            }
            let mut message = message.clone();
            message["content"] = json!(expanded);
            list.push(message);
        }
        list
    }

    /// One LLM call, backing off forever, with failures shown as trouble.
    fn ask_llm(&self, messages: &[Value], tools: &[Value]) -> Result<Reply, llm::Error> {
        let input = self.expand(messages);
        let first_failure = Cell::new(None);
        let trouble = &self.trouble;
        self.env.runtime.block_on(self.llm.call_until(
            &input,
            tools,
            |error, _, _| {
                let stamp = now();
                let since = first_failure.get().unwrap_or(stamp);
                first_failure.set(Some(since));
                let minutes = (stamp - since) / 60_000;
                lock(trouble).set(
                    LLM_FAILING,
                    Some(format!("llm_failing: {} since {minutes}m", error.detail())),
                );
            },
            || lock(trouble).set(LLM_FAILING, None),
        ))
    }

    /// Phases 2 and 3: the call, then its answer, saved. False when the input can neither be sent
    /// nor compacted.
    fn call(&mut self, wake: &mut Wake, row: &mut StepRow) -> bool {
        loop {
            row.phase = 2;
            row.sent_at = Some(now());
            self.put(wake, row);
            let tools = [script::tool()];
            match self.ask_llm(&wake.messages, &tools) {
                Ok(reply) => {
                    row.phase = 3;
                    row.answer = Some(reply.message.to_string());
                    row.tokens_in = reply.tokens_in;
                    row.tokens_out = reply.tokens_out;
                    if let Some(call) = reply.tool_call_list.first()
                        && let Ok(arguments) = serde_json::from_str::<Value>(&call.arguments)
                    {
                        row.script = arguments["script"].as_str().map(str::to_string);
                        if !arguments["intention_list"].is_null() {
                            row.intention_list = Some(arguments["intention_list"].to_string());
                        }
                    }
                    self.put(wake, row);
                    tracing::info!(
                        run = %wake.run,
                        step = row.n,
                        tokens_in = reply.tokens_in,
                        tokens_out = reply.tokens_out,
                        tool_calls = reply.tool_call_list.len(),
                        "the model answered"
                    );
                    wake.messages.push(reply.message);
                    return true;
                }
                Err(llm::Error::ContextLength) => {
                    if !self.compact(wake, row) {
                        return false;
                    }
                }
                Err(error) => {
                    tracing::error!(run = %wake.run, error = %error.detail(), "the LLM call gave up");
                    return false;
                }
            }
        }
    }

    /// Phases 4 to 6: the step's intentions go out as activity messages, then its script runs.
    fn run_script(&mut self, wake: &mut Wake, row: &mut StepRow, reply: &Reply) {
        let Some(call) = reply.tool_call_list.first() else {
            return;
        };
        match parse_call(call) {
            Err(problem) => {
                self.note_output(wake, row.n, format!("error: {problem}"));
                row.phase = 6;
                row.ended_at = Some(now());
                self.put(wake, row);
            }
            Ok((script, intentions)) => {
                for (text, audience) in &intentions {
                    for (chat, topic, thread) in audience {
                        let command = Command::SendActivity {
                            chat: *chat,
                            text: text.clone(),
                            topic: *topic,
                            thread: *thread,
                        };
                        self.send_kept(Some((wake.run, row.n)), *chat, command);
                    }
                }
                row.phase = 4;
                row.started_at = Some(now());
                self.put(wake, row);
                let mode = Mode::Step {
                    run: wake.run,
                    step: row.n,
                    groups: wake.groups.clone(),
                    source: wake.source(),
                    first_n: 0,
                };
                let outcome = script::run_step(self.env.clone(), mode, &script);
                if let Some(error) = &outcome.error {
                    self.note_output(wake, row.n, format!("error: {error}"));
                }
                wake.attach.extend(outcome.attach_list);
                row.phase = 6;
                row.ended_at = Some(now());
                self.put(wake, row);
                tracing::info!(
                    run = %wake.run,
                    step = row.n,
                    actions = outcome.action_list.len(),
                    failed = outcome.error.is_some(),
                    "a script ran"
                );
            }
        }
        self.answer_tools(wake, row.n, reply, false);
    }

    /// A line of a step's output written by the loop, such as a script's error, after what the
    /// script saved.
    fn note_output(&self, wake: &Wake, step: u64, body: String) {
        let store = lock(&self.env.shared.store);
        let n = match store.outputs(wake.run, step) {
            Ok(outputs) => outputs.last().map(|output| output.n + 1).unwrap_or(0),
            Err(_) => 0,
        };
        let row = OutputRow {
            n,
            action: false,
            body,
            at: now(),
        };
        if let Err(error) = store.add_output(wake.run, step, &row) {
            tracing::error!(run = %wake.run, step, error = %error, "could not save a step's output");
        }
    }

    /// A script a restart cut at phase 4 or 5 is never run again: its saved output stands.
    fn cut(&mut self, wake: &mut Wake, row: &mut StepRow, reply: &Reply) {
        row.cut = true;
        row.phase = 6;
        row.ended_at = Some(now());
        self.put(wake, row);
        self.answer_tools(wake, row.n, reply, true);
    }

    /// The tool messages answering a step's calls: the first ran, and any more were not.
    fn answer_tools(&self, wake: &mut Wake, step: u64, reply: &Reply, cut: bool) {
        let outputs = lock(&self.env.shared.store)
            .outputs(wake.run, step)
            .unwrap_or_default();
        let text = tool_text(&outputs, cut, false);
        for (at, call) in reply.tool_call_list.iter().enumerate() {
            let content = if at == 0 {
                wake.tool_steps.push_back((wake.messages.len(), step));
                text.clone()
            } else {
                "Not run: one tool call per answer. Call it again.".to_string()
            };
            wake.messages.push(json!({
                "role": "tool",
                "tool_call_id": call.id,
                "content": content,
            }));
        }
    }

    /// The model answered in text: the wake ends, sending it to the trigger when `send_final` says.
    fn finish(&mut self, wake: &mut Wake, row: &StepRow, reply: &Reply) {
        let text = reply
            .content
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty());
        if self.send_final(wake)
            && let Some(text) = text
            && let Some((chat, _, seq)) = wake.source()
        {
            let trigger = wake.trigger.clone().unwrap_or(Value::Null);
            let command = Command::SendText {
                chat,
                text: Hidden(text.to_string()),
                reply: seq.map(Pointer::Seq),
                topic: trigger["topic"]
                    .as_str()
                    .and_then(|topic| Ulid::parse(topic).ok()),
                thread: trigger["thread"].as_u64(),
                mentions: Vec::new(),
            };
            self.send_kept(Some((wake.run, row.n)), chat, command);
        }
        if wake.limit_at.is_some() {
            let limit = self.env.settings.limits.tool_calls;
            self.end(
                wake,
                RunState::Limit,
                Some(format!("reached its limit of {limit} tool calls")),
            );
        } else {
            self.end(wake, RunState::Done, None);
        }
    }

    fn end(&mut self, wake: &Wake, state: RunState, stuck: Option<String>) {
        if let Err(error) = lock(&self.env.shared.store).end_run(wake.run, state, now()) {
            tracing::error!(run = %wake.run, error = %error, "could not end a run");
        }
        tracing::info!(
            run = %wake.run,
            state = state.name(),
            steps = wake.step,
            tool_calls = wake.calls,
            "a wake ended"
        );
        if let Some(reason) = stuck {
            self.set_trouble(STUCK, Some(format!("stuck: {reason}")));
        }
    }

    /// A write the loop makes itself, kept in the outgoing queue so a restart can recover it.
    fn send_kept(&self, source: Option<(Ulid, u64)>, chat: Ulid, command: Command) {
        let id = new_id();
        let (run, step) = match source {
            Some((run, step)) => (Some(run), Some(step)),
            None => (None, None),
        };
        let row = OutgoingRow {
            id,
            run,
            step,
            n: None,
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
        let (reply, answer) = oneshot::channel();
        let ask = Ask::Command {
            command: Box::new(command),
            reply: Some(reply),
            outgoing: Some(id),
            handles: Vec::new(),
        };
        if self.env.shared.asks.send(ask).is_ok() {
            self.env.runtime.spawn(async move {
                let _ = answer.await;
            });
        }
    }

    // ---------------------------------------------------------------------------------------------
    // Compaction
    // ---------------------------------------------------------------------------------------------

    /// Compacts the input with the compaction prompt. If the summary still does not fit, the oldest
    /// steps' printed output goes, keeping their actions; if even that does not fit, the wake ends
    /// through the limit prompt. False once that last way has failed too.
    fn compact(&mut self, wake: &mut Wake, row: &mut StepRow) -> bool {
        if wake.squeezed {
            return false;
        }
        let max = self.max(wake);
        tracing::info!(run = %wake.run, step = row.n, bytes = size(&wake.messages), "compacting");
        let mut input = wake.messages.clone();
        let mut pending = wake.tool_steps.clone();
        input.push(user(self.compact_prompt(wake)));
        let summary = loop {
            match self.ask_llm(&input, &[]) {
                Ok(reply) => break reply.content.unwrap_or_default(),
                Err(llm::Error::ContextLength) => {
                    if !self.trim_one(wake.run, &mut input, &mut pending) {
                        break String::new();
                    }
                }
                Err(_) => break String::new(),
            }
        };
        let summary = summary.trim().to_string();
        if !summary.is_empty() {
            let compacted = vec![wake.system.clone(), user(format!("{COMPACTED}{summary}"))];
            if size(&compacted) <= max {
                self.reset(wake, row, compacted, VecDeque::new(), false);
                return true;
            }
        }
        let mut messages = wake.messages.clone();
        let mut pending = wake.tool_steps.clone();
        while size(&messages) > max {
            if !self.trim_one(wake.run, &mut messages, &mut pending) {
                break;
            }
        }
        if size(&messages) <= max {
            tracing::info!(run = %wake.run, "compacted by dropping old steps' printed output");
            self.reset(wake, row, messages, pending, false);
            return true;
        }
        tracing::warn!(run = %wake.run, "could not compact: the wake ends through the limit prompt");
        wake.squeezed = true;
        wake.limit_at = Some(wake.calls);
        let mut note = summary;
        let room = usize::try_from(max / 2).unwrap_or(usize::MAX);
        if note.len() > room {
            let mut end = room;
            while end > 0 && !note.is_char_boundary(end) {
                end -= 1;
            }
            note.truncate(end);
        }
        if note.is_empty() {
            note =
                "This wake's input no longer fits the model's context and could not be compacted."
                    .to_string();
        }
        let squeezed = vec![
            wake.system.clone(),
            user(format!("{COMPACTED}{note}")),
            user(self.limit_prompt(wake)),
        ];
        self.reset(wake, row, squeezed, VecDeque::new(), true);
        true
    }

    /// Replaces the oldest untrimmed step's printed output with a note, keeping its actions.
    fn trim_one(
        &self,
        run: Ulid,
        messages: &mut [Value],
        pending: &mut VecDeque<(usize, u64)>,
    ) -> bool {
        let Some((at, step)) = pending.pop_front() else {
            return false;
        };
        let outputs = lock(&self.env.shared.store)
            .outputs(run, step)
            .unwrap_or_default();
        if let Some(message) = messages.get_mut(at) {
            message["content"] = json!(tool_text(&outputs, false, true));
        }
        true
    }

    /// Replaces the input, saved on the step so a restart starts from it.
    fn reset(
        &self,
        wake: &mut Wake,
        row: &mut StepRow,
        messages: Vec<Value>,
        tool_steps: VecDeque<(usize, u64)>,
        limit: bool,
    ) {
        let kept_context = !tool_steps.is_empty();
        wake.messages = messages;
        wake.tool_steps = tool_steps;
        if !kept_context {
            wake.shown.clear();
        }
        let mut context: Value = serde_json::from_str(&row.context).unwrap_or(json!({}));
        context["reset"] = json!(true);
        context["messages"] = json!(wake.messages);
        if !kept_context {
            context["shown"] = json!([]);
        }
        if limit {
            context["limit"] = json!(true);
        }
        row.context = context.to_string();
        self.put(wake, row);
    }

    // ---------------------------------------------------------------------------------------------
    // The input
    // ---------------------------------------------------------------------------------------------

    /// The system message: `[prompt] system` as written, then the preamble, who the agent is, the
    /// time, its memory and its skills.
    fn system_message(&self, wake: &Wake) -> Value {
        let settings = &self.env.settings;
        let system = self
            .prompt_of(wake)
            .and_then(|over| over.system.clone())
            .unwrap_or_else(|| settings.prompt.system.clone());
        let me = self.env.me();
        let owner = lock(&self.env.owner).clone();
        let stamp = now();
        let mut text = format!(
            "{system}\n\n---\n{PREAMBLE}\n\nYou are ME: {me}\nYour owner is OWNER: {owner}\nNow: {}, \
             which is {} in {}.\n",
            time_text(stamp),
            local_text(stamp, &settings.timezone),
            settings.timezone
        );
        let mut chats = Vec::new();
        for row in &wake.first {
            if let Some(chat) = row.chat
                && !chats.contains(&chat)
                && lock(&self.env.shared.view).chats.contains_key(&chat)
            {
                chats.push(chat);
            }
        }
        text.push_str(&self.memory_hints(&chats));
        if let Some(skills) = &settings.skills {
            text.push_str("\nSkills, each read whole with skills.read(name):\n");
            let list = script::skills_in(&skills.directory);
            if list.is_empty() {
                text.push_str("(none yet)\n");
            }
            for skill in list {
                text.push_str(&format!("- {}: {}\n", skill.name, skill.description));
            }
            if skills.writable {
                text.push_str(
                    "Write or remove a skill with skills.write and skills.remove only when a person \
                     in the chat asks for it or agrees.\n",
                );
            }
        }
        json!({"role": "system", "content": text})
    }

    /// For the global memory and each chat shown, its note count and most common words.
    fn memory_hints(&self, chats: &[Ulid]) -> String {
        let words = usize::try_from(self.env.settings.memory.hint_words).unwrap_or(0);
        let every = u32::try_from(self.env.settings.limits.memory_notes).unwrap_or(u32::MAX);
        let mut text = String::from("\nMemory:\n");
        let mut list = vec![(None, "the global memory".to_string())];
        for chat in chats {
            let name = lock(&self.env.shared.view)
                .chat_name(*chat)
                .unwrap_or_default();
            list.push((Some(*chat), format!("chat {chat} ({name})")));
        }
        for (chat, label) in list {
            let notes = lock(&self.env.shared.store)
                .notes(chat, every, 0)
                .unwrap_or_default();
            let count = notes.len();
            if words == 0 || count == 0 {
                text.push_str(&format!("- {label}: {count} notes\n"));
                continue;
            }
            let texts: Vec<String> = notes.into_iter().map(|note| note.text).collect();
            let common = Index::common_words(&texts, words);
            text.push_str(&format!(
                "- {label}: {count} notes, most often: {}\n",
                common.join(", ")
            ));
        }
        text
    }

    /// Events as the model reads them, grouped by chat, then topic, then thread, each group with
    /// its chat's nearby messages not yet shown. Answers the text, the events' ids, the messages
    /// newly shown and the groups, and sends `seen` up to each group's newest message.
    fn build_input(&self, wake: &mut Wake, events: &[EventRow]) -> Input {
        type Key = (Option<Ulid>, Option<Ulid>, Option<u64>);
        let context = &self.env.settings.context;
        let mut budget = ContextBudget {
            count: context.around,
            bytes: context.around_bytes,
        };
        if let Some(rule) = self.rule(wake)
            && let Some(over) = &rule.context
        {
            budget.count = over.around.unwrap_or(budget.count);
            budget.bytes = over.around_bytes.unwrap_or(budget.bytes);
        }
        let mut keys: Vec<Key> = Vec::new();
        let mut members: HashMap<Key, Vec<&EventRow>> = HashMap::new();
        let mut shown = Vec::new();
        for row in events {
            let key = match row.kind {
                EventKind::Schedule | EventKind::Storage | EventKind::Sent => (None, None, None),
                EventKind::Invitation | EventKind::JoinRequest => (row.chat, None, None),
                _ => (row.chat, row.topic, row.thread_seq),
            };
            if !members.contains_key(&key) {
                keys.push(key);
            }
            members.entry(key).or_default().push(row);
            if about_message(row.kind)
                && let (Some(chat), Some(seq)) = (row.chat, row.seq)
            {
                if wake.shown.insert((chat, seq)) {
                    shown.push((chat.to_string(), seq));
                }
                if row.kind == EventKind::Edit
                    && let Some(target) = row.body["target"].as_u64()
                    && wake.shown.insert((chat, target))
                {
                    shown.push((chat.to_string(), target));
                }
            }
        }
        let mut group_list = Vec::new();
        let mut ids = Vec::new();
        let mut groups = Map::new();
        for key in keys {
            let mut rows = members.remove(&key).unwrap_or_default();
            rows.sort_by_key(|row| (row.received_at, row.id));
            wake.next_group += 1;
            let name = format!("g{}", wake.next_group);
            let mut group = json!({"group": name});
            let mut newest = None;
            for row in &rows {
                if about_message(row.kind)
                    && let Some(seq) = row.seq
                {
                    newest = newest.max(Some(seq));
                }
            }
            if let (Some(chat), topic, thread) = key {
                let first = &rows[0].body;
                group["chat"] = json!(chat.to_string());
                group["chat_type"] = first["chat_type"].clone();
                group["chat_name"] = first["chat_name"].clone();
                let (in_chat, topics_on, topic_name) = {
                    let view = lock(&self.env.shared.view);
                    match view.chats.get(&chat) {
                        Some(known) => (
                            true,
                            known.topics_on,
                            topic.and_then(|topic| {
                                known
                                    .topic_list
                                    .iter()
                                    .find(|item| item.id == topic)
                                    .map(|item| item.name.clone())
                            }),
                        ),
                        None => (false, false, None),
                    }
                };
                if let Some(topic) = topic {
                    group["topic"] = json!(topic.to_string());
                    group["topic_name"] = json!(topic_name);
                }
                if let Some(thread) = thread {
                    group["thread"] = json!(thread);
                }
                if in_chat && let Some(newest) = newest {
                    let filter = if topics_on { Some(topic) } else { None };
                    let kept = script::background(
                        &self.env,
                        (chat, filter, thread),
                        Some(newest),
                        budget,
                        &wake.shown,
                    );
                    for message in &kept {
                        if let Some(seq) = message["seq"].as_u64()
                            && wake.shown.insert((chat, seq))
                        {
                            shown.push((chat.to_string(), seq));
                        }
                    }
                    group["context"] = json!(kept);
                    let seen = Command::Seen {
                        chat,
                        up_to_seq: newest,
                        topic,
                        thread,
                    };
                    let _ = self.env.shared.asks.send(Ask::Command {
                        command: Box::new(seen),
                        reply: None,
                        outgoing: None,
                        handles: Vec::new(),
                    });
                }
            }
            let mut event_list = Vec::new();
            let mut group_ids = Vec::new();
            for row in &rows {
                let mut value = event_json(row, key.0.is_some());
                if row.kind == EventKind::Schedule {
                    let mut chat_context = Vec::new();
                    for item in row.body["chat_list"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default()
                    {
                        let Some(chat) = item.as_str().and_then(|text| Ulid::parse(text).ok())
                        else {
                            continue;
                        };
                        let kept = script::background(
                            &self.env,
                            (chat, None, None),
                            None,
                            self.env.settings.context_limits.schedule,
                            &wake.shown,
                        );
                        for message in &kept {
                            if let Some(seq) = message["seq"].as_u64()
                                && wake.shown.insert((chat, seq))
                            {
                                shown.push((chat.to_string(), seq));
                            }
                        }
                        let name = lock(&self.env.shared.view).chat_name(chat);
                        chat_context.push(json!({
                            "chat": chat.to_string(),
                            "chat_name": name,
                            "message_list": kept,
                        }));
                    }
                    value["chat_context"] = json!(chat_context);
                }
                event_list.push(value);
                ids.push(row.id);
                group_ids.push(row.id);
            }
            group["event_list"] = json!(event_list);
            let mut id_list = Vec::new();
            for id in &group_ids {
                id_list.push(json!(id.to_string()));
            }
            groups.insert(name.clone(), json!(id_list));
            wake.groups.insert(name, group_ids);
            group_list.push(group);
        }
        let body = serde_json::to_string(&json!({"group_list": group_list})).unwrap_or_default();
        Input {
            text: format!("Events:\n{body}"),
            ids,
            shown,
            groups,
        }
    }
}
