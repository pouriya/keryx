//! The socket, driving core, both queues, presence, and snapshot and archive saves.
//!
//! One task owns core's [`Client`] and does what its outputs ask: the WebSocket through
//! tokio-tungstenite, REST through reqwest, the snapshot and the archive in `agent.db`. Everything
//! core shows goes into [`View`], which scripts read, and what may wake the loop goes into the
//! incoming queue. Scripts and the loop ask through [`Ask`]: a command is tagged, so its caller
//! learns when the server took it or why it refused. See agent/AGENTS.md.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use keryx_core::client::{
    Auth, Client, Command, Event, Folder, Hit, Input, Output, Platform, Secret, Shown,
    ShownChatProfile, ShownTopicProfile,
};
use keryx_core::protocol::{
    ChatItem, ChatOptions, ChatRole, ChatType, ContentKind, FindableItem, InvitationShown, Kind,
    LineAction, MemberItem, RequestItem, RequestState, StickerPack, TopicName, TopicOptionsBody,
};
use keryx_core::ulid::Ulid;
use serde_json::{Value, json};
use tokio::sync::{Notify, mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;

use crate::index::{Covered, DocKind, Document, Index};
use crate::store::{EventKind, NewEvent, OutgoingState, Rows, Store};

/// How long an idle loop waits for core's timer before looking again.
const IDLE: Duration = Duration::from_secs(3600);

/// The index commits at most this often while messages are being indexed.
const COMMIT_EVERY: Duration = Duration::from_secs(1);

/// Rows one reindexing page opens at a time.
const REINDEX_PAGE: u32 = 200;

/// Locks a mutex, taking it back from a panicked holder: the state behind it is plain data, and
/// a script's panic must not stop the agent.
pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// The time now, as unix milliseconds.
pub fn now() -> i64 {
    Ulid::now()
}

/// A new id.
pub fn new_id() -> Ulid {
    match Ulid::new() {
        Ok(id) => id,
        Err(_) => Ulid::from_bytes([0; 16]),
    }
}

/// TLS on ring with Mozilla's roots compiled in, for the keryx socket, REST and HTTP alike, or with
/// no check at all for a script's `insecure=True`.
pub fn tls_config(insecure: bool) -> Arc<rustls::ClientConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = match rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
    {
        Ok(builder) => builder,
        Err(_) => rustls::ClientConfig::builder_with_provider(provider.clone())
            .with_protocol_versions(&[&rustls::version::TLS13])
            .unwrap_or_else(|_| unreachable_tls()),
    };
    let config = if insecure {
        builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoCheck(provider)))
            .with_no_client_auth()
    } else {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        builder.with_root_certificates(roots).with_no_client_auth()
    };
    Arc::new(config)
}

/// ring always offers TLS 1.3, so building its config cannot fail.
fn unreachable_tls() -> rustls::ConfigBuilder<rustls::ClientConfig, rustls::WantsVerifier> {
    panic!("ring offers no TLS version")
}

/// A reqwest client on [`tls_config`], with no proxy from the environment, as nothing in agent.toml
/// names one.
pub fn http_client(insecure: bool, timeout: Duration) -> reqwest::Client {
    let config = tls_config(insecure);
    let built = reqwest::Client::builder()
        .tls_backend_preconfigured(Arc::unwrap_or_clone(config))
        .no_proxy()
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::limited(5))
        .build();
    match built {
        Ok(client) => client,
        Err(_) => reqwest::Client::new(),
    }
}

/// Accepts any certificate, for `insecure=True`. Signatures are still checked, so the connection
/// is at least to whoever holds the key it shows.
#[derive(Debug)]
struct NoCheck(Arc<rustls::crypto::CryptoProvider>);

impl rustls::client::danger::ServerCertVerifier for NoCheck {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// One of the agent's own accounts, or anyone it has met.
#[derive(Debug, Clone, Default)]
pub struct AccountView {
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub bio: Option<String>,
    pub link_list: Vec<(String, String)>,
    pub agent: bool,
    pub owner: Option<Ulid>,
    pub presence: Option<String>,
}

/// One chat, as far as core has shown it.
#[derive(Debug, Clone)]
pub struct ChatView {
    pub item: ChatItem,
    pub profile: Option<ShownChatProfile>,
    pub options: Option<ChatOptions>,
    pub topics_on: bool,
    pub topic_list: Vec<TopicName>,
    pub topic_options: HashMap<Ulid, (TopicOptionsBody, Vec<Ulid>)>,
    pub findable: Option<FindableItem>,
    pub member_list: Option<Vec<MemberItem>>,
    /// Its newest content, opened.
    pub preview: Option<Value>,
    /// Ready to send in.
    pub ready: bool,
}

/// Everything core has shown, for scripts to read without asking.
#[derive(Debug, Default)]
pub struct View {
    pub connected: bool,
    /// Logged in and holding its keys.
    pub ready: bool,
    pub account: Option<Ulid>,
    pub username: Option<String>,
    pub device: Option<Ulid>,
    pub own_device_list: Vec<Ulid>,
    pub chats: HashMap<Ulid, ChatView>,
    pub folder_list: Vec<Folder>,
    pub accounts: HashMap<Ulid, AccountView>,
    pub invitations: HashMap<Ulid, InvitationShown>,
    pub requests: HashMap<Ulid, Vec<RequestItem>>,
    pub pack_list: Option<Vec<StickerPack>>,
    /// Each topic's opened profile, by chat and topic.
    pub topic_profiles: HashMap<(Ulid, Ulid), ShownTopicProfile>,
}

impl View {
    /// A person by account, as results name one: id, username and display name.
    pub fn person(&self, account: Ulid) -> Value {
        let known = self.accounts.get(&account);
        json!({
            "id": account.to_string(),
            "username": known.and_then(|known| known.username.clone()),
            "display_name": known.and_then(|known| known.display_name.clone()),
        })
    }

    /// An account by its username, among those core has shown.
    pub fn account_named(&self, username: &str) -> Option<Ulid> {
        for (id, account) in &self.accounts {
            if account.username.as_deref() == Some(username) {
                return Some(*id);
            }
        }
        None
    }

    /// A chat's name: a group's or channel's own, a direct chat's other person, or the self chat's
    /// owner.
    pub fn chat_name(&self, chat: Ulid) -> Option<String> {
        let view = self.chats.get(&chat)?;
        if let Some(profile) = &view.profile {
            return Some(profile.name.clone());
        }
        if let Some(findable) = &view.findable {
            return Some(findable.name.clone());
        }
        match view.item.chat_type {
            ChatType::Direct => {
                let other = self.other_of(chat)?;
                let known = self.accounts.get(&other)?;
                known
                    .display_name
                    .clone()
                    .or_else(|| known.username.clone())
            }
            ChatType::SelfChat => self.username.clone(),
            _ => None,
        }
    }

    /// The direct chat with an account, if it is in the chat list.
    pub fn direct_with(&self, account: Ulid) -> Option<Ulid> {
        for (chat, known) in &self.chats {
            if known.item.chat_type == ChatType::Direct && self.other_of(*chat) == Some(account) {
                return Some(*chat);
            }
        }
        None
    }

    /// A direct chat's other person.
    pub fn other_of(&self, chat: Ulid) -> Option<Ulid> {
        let view = self.chats.get(&chat)?;
        for item in &view.item.profile_key_list {
            if Some(item.account) != self.account {
                return Some(item.account);
            }
        }
        for mark in &view.item.mark_list {
            if Some(mark.account) != self.account {
                return Some(mark.account);
            }
        }
        None
    }

    /// The names of the folders holding a chat, `all` left out.
    pub fn folders_of(&self, chat: Ulid) -> Vec<String> {
        let mut list = Vec::new();
        for folder in &self.folder_list {
            if let (Some(name), Some(chat_list)) = (&folder.name, &folder.chat_list)
                && chat_list.contains(&chat)
            {
                list.push(name.0.clone());
            }
        }
        list
    }

    /// Whether a device is one of this account's.
    pub fn own_device(&self, device: Option<Ulid>) -> bool {
        match device {
            Some(device) => Some(device) == self.device || self.own_device_list.contains(&device),
            None => false,
        }
    }
}

/// A chat type as `[[wake]]` and results name it.
/// How long an event waits for its chat or sender to be known before it is kept as it is.
const HOLD_FOR: Duration = Duration::from_secs(5);

/// Where an event's body names a person: under `message` its sender, elsewhere the key itself.
const PEOPLE: [&str; 4] = ["message", "sender", "by", "inviter"];

/// The account an event names at `key`, as [`PEOPLE`] says.
fn person_id(body: &Value, key: &str) -> Option<Ulid> {
    let person = match key {
        "message" => &body["message"]["sender"],
        _ => &body[key],
    };
    Ulid::parse(person["id"].as_str()?).ok()
}

/// The event has no chat, or the view lists it.
fn listed_in(view: &View, event: &NewEvent) -> bool {
    match event.chat {
        Some(chat) => view.chats.contains_key(&chat),
        None => true,
    }
}

/// The event has the usernames it needs, and a chat name when it adds the agent.
fn named_in(view: &View, event: &NewEvent) -> bool {
    if event.kind == EventKind::Added
        && let Some(chat) = event.chat
        && view.chat_name(chat).is_none()
    {
        return false;
    }
    for key in PEOPLE {
        if let Some(id) = person_id(&event.body, key)
            && view
                .accounts
                .get(&id)
                .is_none_or(|known| known.username.is_none())
        {
            return false;
        }
    }
    true
}

pub fn chat_type_name(chat_type: ChatType) -> &'static str {
    match chat_type {
        ChatType::SelfChat => "self",
        ChatType::Direct => "direct",
        ChatType::Group => "group",
        ChatType::Channel => "channel",
    }
}

/// A content kind as `[[wake]]` and results name it.
pub fn content_kind_name(kind: ContentKind) -> &'static str {
    match kind {
        ContentKind::Text => "text",
        ContentKind::Voice => "voice",
        ContentKind::Reaction => "reaction",
        ContentKind::Image => "image",
        ContentKind::Music => "music",
        ContentKind::Video => "video",
        ContentKind::Doc => "doc",
        ContentKind::File => "file",
        ContentKind::Call => "call",
        ContentKind::MediaBundle => "media_bundle",
        ContentKind::Question => "question",
        ContentKind::Answer => "answer",
        ContentKind::Sticker => "sticker",
        ContentKind::Suggestion => "suggestion",
        ContentKind::Ack => "ack",
        ContentKind::Activity => "activity",
    }
}

/// A role as results name it.
pub fn role_name(role: ChatRole) -> &'static str {
    match role {
        ChatRole::Owner => "owner",
        ChatRole::Moderator => "moderator",
        ChatRole::Member => "member",
    }
}

/// A time as results write it: UTC ISO-8601.
pub fn time_text(at: i64) -> String {
    match jiff::Timestamp::from_millisecond(at) {
        Ok(time) => time.strftime("%Y-%m-%dT%H:%M:%SZ").to_string(),
        Err(_) => at.to_string(),
    }
}

/// One message as the loop and scripts see it.
pub fn message_value(view: &View, shown: &Shown) -> Value {
    let mut file_list = Vec::new();
    for file in &shown.files {
        file_list.push(json!({
            "id": file.id.to_string(),
            "name": file.name,
            "type": file.mime,
            "size": file.size,
        }));
    }
    let text = shown.text.as_ref().map(|text| text.0.clone());
    let with_files = !shown.files.is_empty();
    let mut mention_list = Vec::new();
    for account in &shown.mentions {
        mention_list.push(view.person(*account));
    }
    let mut value = json!({
        "chat": shown.chat.to_string(),
        "seq": shown.seq,
        "topic": shown.topic.map(|topic| topic.to_string()),
        "thread": shown.thread_seq,
        "sender": shown.account.map(|account| view.person(account)),
        "at": time_text(shown.saved_at),
        "kind": shown.content_kind.map(content_kind_name),
        "text": if with_files { None } else { text.clone() },
        "caption": if with_files { text } else { None },
        "reply": shown.reply,
        "mention_list": mention_list,
        "file_list": file_list,
        "edited": false,
        "deleted": false,
    });
    if let Some(object) = value.as_object_mut() {
        if let Some(emoji) = &shown.emoji {
            object.insert("emoji".to_string(), json!(emoji));
        }
        if let Some(on) = shown.on {
            object.insert("on".to_string(), json!(on));
        }
        if let Some(sticker) = shown.sticker {
            object.insert("sticker".to_string(), json!(sticker.to_string()));
        }
        if let Some(question) = &shown.question {
            object.insert("question".to_string(), json!(question));
        }
        if let Some(answer) = &shown.answer {
            object.insert("answer".to_string(), json!(answer));
        }
        if let Some(suggestion) = &shown.suggestion {
            object.insert("suggestion".to_string(), json!(suggestion));
        }
        if let Some(ack) = &shown.ack {
            object.insert("ack".to_string(), json!(ack));
        }
        if shown.problem.is_some() {
            object.insert("problem".to_string(), json!("cannot_open"));
        }
    }
    value
}

/// What a tagged command's answer said, with every event core showed while it was open.
#[derive(Debug, Default)]
pub struct Answer {
    pub seq: Option<u64>,
    pub code: Option<String>,
    pub message: Option<String>,
    pub seconds: Option<u64>,
    pub event_list: Vec<Event>,
}

type ScanReply = oneshot::Sender<Result<(Vec<Hit>, bool), crate::store::Error>>;

/// What scripts and the loop ask of the task that drives core.
pub enum Ask {
    /// A command, answered once core says everything it asked was answered. `outgoing` names its
    /// row in the outgoing queue, kept current as it goes. `handles` are the files it reads or
    /// writes, by the handles its command names.
    Command {
        command: Box<Command>,
        reply: Option<oneshot::Sender<Answer>>,
        outgoing: Option<Ulid>,
        handles: Vec<(u64, PathBuf)>,
    },
    /// Opens rows of the archive through core, answering with what it showed of them.
    Open {
        chat: Ulid,
        rows: Rows,
        reply: oneshot::Sender<Vec<Event>>,
    },
    /// Reads one page of core's archive scan. Text starts a search; none continues it.
    Scan {
        text: Option<Secret>,
        reply: ScanReply,
    },
    /// The agent's trouble, its presence text sent to its owner, or none once it cleared.
    Trouble { text: Option<String> },
}

/// What every part shares: the store, what core showed, the index, and the way to ask.
pub struct Shared {
    pub store: Mutex<Store>,
    pub view: Mutex<View>,
    /// Woken when the chat list or an opened chat profile changes.
    pub chats_changed: Notify,
    pub index: Mutex<Index>,
    /// Woken when an event is kept, so the loop looks.
    pub wake: Notify,
    pub asks: mpsc::UnboundedSender<Ask>,
    pub directory: PathBuf,
    /// `[search] plain`: messages' plaintext goes into the index.
    pub plain: bool,
    /// The next file handle, for uploads and downloads.
    pub next_handle: Mutex<u64>,
}

impl Shared {
    /// Sends a command and waits for its answer, or none once the task is gone.
    pub async fn command(&self, command: Command) -> Option<Answer> {
        let (reply, answer) = oneshot::channel();
        let ask = Ask::Command {
            command: Box::new(command),
            reply: Some(reply),
            outgoing: None,
            handles: Vec::new(),
        };
        if self.asks.send(ask).is_err() {
            return None;
        }
        answer.await.ok()
    }

    /// A new handle for a file core reads or writes.
    pub fn handle(&self) -> u64 {
        let mut next = lock(&self.next_handle);
        *next += 1;
        *next
    }
}

/// What the socket, REST and timer tasks hand back.
enum Inner {
    Input(Input),
    /// From the socket of this generation: its writer once open, then its frames and its close.
    Opened(u64, mpsc::UnboundedSender<Message>),
    Socket(u64, Input),
}

/// A tagged command not yet answered.
struct Waiter {
    reply: Option<oneshot::Sender<Answer>>,
    outgoing: Option<Ulid>,
    event_list: Vec<Event>,
}

/// An open of archive rows under way, or waiting its turn.
struct Opening {
    chat: Ulid,
    rows: Option<Rows>,
    reply: Option<oneshot::Sender<Vec<Event>>>,
    event_list: Vec<Event>,
}

/// The task that drives core.
pub struct Driver {
    shared: Arc<Shared>,
    client: Client,
    base: String,
    token: Secret,
    words: Secret,
    http: reqwest::Client,
    tls: Arc<rustls::ClientConfig>,
    inner: mpsc::UnboundedSender<Inner>,
    socket: Option<mpsc::UnboundedSender<Message>>,
    generation: u64,
    wake_at: Option<i64>,
    next_tag: u64,
    waiters: HashMap<u64, Waiter>,
    /// Content's local ids, for its outgoing row once the waiter is gone.
    locals: HashMap<u64, Ulid>,
    opening: Option<Opening>,
    opens: VecDeque<Opening>,
    scanning: Option<(ScanReply, Vec<Hit>)>,
    handles: HashMap<u64, PathBuf>,
    /// Messages saved after this are new and may wake the loop.
    watermark: i64,
    /// The first sync since starting has ended, so unanswered sends can be matched.
    synced: bool,
    sync_tag: Option<u64>,
    covered: Covered,
    index_dirty: bool,
    last_commit: std::time::Instant,
    /// Rows still to open for the index, by chat, once the agent is ready.
    reindex: VecDeque<(Ulid, i64)>,
    trouble: Option<String>,
    later: VecDeque<Input>,
    /// This device's own messages the first sync showed, to match unanswered sends against.
    own_seen: Vec<OwnSeen>,
    /// Verified deletes that core showed before their archive rows were saved.
    pending_deletes: HashMap<(Ulid, u64), u64>,
    /// Profiles core opened before their chat first appeared on the list.
    unlisted_profiles: HashMap<Ulid, ShownChatProfile>,
    /// Events whose chat the view does not list yet, such as a direct chat joined by its Welcome,
    /// or whose sender's name core is still fetching, with when each came. Each is kept once both
    /// are known, so a wake rule sees them, or after [`HOLD_FOR`] as it is.
    held: Vec<(NewEvent, std::time::Instant)>,
    /// A chat list was asked for a held event and not yet answered.
    listing: bool,
}

impl Driver {
    /// Sets up the task: what `agent.db` held is loaded into core when [`Driver::run`] starts.
    pub fn new(shared: Arc<Shared>, url: &str, token: &str, words: &str) -> (Self, Inbox) {
        let (inner, inbox) = mpsc::unbounded_channel();
        let platform = Platform {
            platform: 0,
            os: std::env::consts::OS.to_string(),
            os_version: "keryx-agent".to_string(),
            web_platform: None,
            web_platform_version: Some(env!("CARGO_PKG_VERSION").to_string()),
        };
        let watermark = {
            let store = lock(&shared.store);
            match store.newest_saved_at() {
                Ok(Some(at)) => at,
                _ => now(),
            }
        };
        let covered = lock(&shared.index).covered();
        let driver = Self {
            shared,
            client: Client::new(Auth::Token, platform),
            base: url.trim_end_matches('/').to_string(),
            token: Secret(token.to_string()),
            words: Secret(words.to_string()),
            http: http_client(false, Duration::from_secs(60)),
            tls: tls_config(false),
            inner,
            socket: None,
            generation: 0,
            wake_at: None,
            next_tag: 0,
            waiters: HashMap::new(),
            locals: HashMap::new(),
            opening: None,
            opens: VecDeque::new(),
            scanning: None,
            handles: HashMap::new(),
            watermark,
            synced: false,
            sync_tag: None,
            covered,
            index_dirty: false,
            last_commit: std::time::Instant::now(),
            reindex: VecDeque::new(),
            trouble: None,
            later: VecDeque::new(),
            own_seen: Vec::new(),
            pending_deletes: HashMap::new(),
            unlisted_profiles: HashMap::new(),
            held: Vec::new(),
            listing: false,
        };
        (driver, Inbox(inbox))
    }

    /// Runs until every sender of asks is gone.
    pub async fn run(mut self, mut inbox: Inbox, mut asks: mpsc::UnboundedReceiver<Ask>) {
        let (slots, session) = {
            let store = lock(&self.shared.store);
            (store.slots(), store.session())
        };
        let [first, second] = match slots {
            Ok(slots) => slots,
            Err(error) => {
                tracing::error!(error = %error, "could not read the snapshot");
                [None, None]
            }
        };
        let token = match session {
            Ok(token) => token.map(Secret),
            Err(_) => None,
        };
        self.prepare_index();
        self.pump(Input::Loaded {
            first,
            second,
            resume: true,
            token,
        });
        loop {
            let sleep = match self.wake_at {
                Some(at) => Duration::from_millis(u64::try_from(at - now()).unwrap_or(0)),
                None => IDLE,
            };
            let commit_due = self.index_dirty && self.last_commit.elapsed() >= COMMIT_EVERY;
            if commit_due {
                self.commit_index();
            }
            let mut tick = if self.index_dirty {
                COMMIT_EVERY.min(sleep)
            } else {
                sleep
            };
            if let Some((_, since)) = self.held.first() {
                tick = tick.min(HOLD_FOR.saturating_sub(since.elapsed()));
            }
            tokio::select! {
                ask = asks.recv() => match ask {
                    Some(ask) => self.ask(ask),
                    None => break,
                },
                inner = inbox.0.recv() => match inner {
                    Some(Inner::Input(input)) => self.pump(input),
                    Some(Inner::Opened(generation, writer)) => {
                        if generation == self.generation {
                            self.socket = Some(writer);
                            self.pump(Input::Connected);
                        }
                    }
                    Some(Inner::Socket(generation, input)) => {
                        if generation == self.generation {
                            if matches!(input, Input::Closed { .. }) {
                                self.socket = None;
                            }
                            self.pump(input);
                        }
                    }
                    None => break,
                },
                _ = tokio::time::sleep(tick) => {
                    if !self.held.is_empty() {
                        self.release(false);
                    }
                    if let Some(at) = self.wake_at
                        && now() >= at
                    {
                        self.wake_at = None;
                        self.pump(Input::Timer);
                    }
                }
            }
        }
        if self.index_dirty {
            self.commit_index();
        }
    }

    fn ask(&mut self, ask: Ask) {
        match ask {
            Ask::Command {
                command,
                reply,
                outgoing,
                handles,
            } => {
                for (handle, path) in handles {
                    self.handles.insert(handle, path);
                }
                self.tagged(*command, reply, outgoing);
            }
            Ask::Open { chat, rows, reply } => {
                self.opens.push_back(Opening {
                    chat,
                    rows: Some(rows),
                    reply: Some(reply),
                    event_list: Vec::new(),
                });
                self.next_open();
            }
            Ask::Scan { text, reply } => {
                if reply.is_closed() {
                    return;
                }
                self.scanning = Some((reply, Vec::new()));
                let command = match text {
                    Some(text) => Command::SearchMessages { text },
                    None => Command::SearchOlder,
                };
                self.pump(Input::Command { command });
            }
            Ask::Trouble { text } => {
                if self.trouble == text {
                    return;
                }
                self.trouble = text.clone();
                self.pump(Input::Command {
                    command: Command::InApp {
                        in_app: true,
                        trouble: text.is_some(),
                        text,
                    },
                });
            }
        }
    }

    /// Feeds a command tagged, so its answer comes back.
    fn tagged(
        &mut self,
        command: Command,
        reply: Option<oneshot::Sender<Answer>>,
        outgoing: Option<Ulid>,
    ) -> u64 {
        self.next_tag += 1;
        let tag = self.next_tag;
        self.waiters.insert(
            tag,
            Waiter {
                reply,
                outgoing,
                event_list: Vec::new(),
            },
        );
        self.pump(Input::Tagged { tag, command });
        tag
    }

    /// Starts the next open of archive rows, one at a time.
    fn next_open(&mut self) {
        if self.opening.is_some() {
            return;
        }
        let Some(opening) = self.opens.pop_front() else {
            return;
        };
        let chat = opening.chat;
        self.opening = Some(opening);
        self.pump(Input::Command {
            command: Command::OpenChat { chat },
        });
    }

    /// Steps core with one input, and every input what it asked for gives back at once.
    fn pump(&mut self, input: Input) {
        let mut queue = VecDeque::from([input]);
        loop {
            while let Some(input) = queue.pop_front() {
                let outputs = self.client.step(input, now());
                for output in outputs {
                    if let Some(next) = self.output(output) {
                        queue.push_back(next);
                    }
                }
                while let Some(later) = self.later.pop_front() {
                    queue.push_back(later);
                }
            }
            // An open core never asked rows for, as for a chat it does not know, ends with what it
            // showed, so the opens behind it are not held up.
            let unasked = self
                .opening
                .as_ref()
                .is_some_and(|opening| opening.rows.is_some());
            if !unasked {
                break;
            }
            if let Some(mut opening) = self.opening.take()
                && let Some(reply) = opening.reply.take()
            {
                let _ = reply.send(std::mem::take(&mut opening.event_list));
            }
            self.later_open();
            while let Some(later) = self.later.pop_front() {
                queue.push_back(later);
            }
        }
    }

    /// Does what one output asks, answering at once where the answer is local.
    fn output(&mut self, output: Output) -> Option<Input> {
        match output {
            Output::Send { text } => {
                if let Some(socket) = &self.socket {
                    let _ = socket.send(Message::text(text.0.clone()));
                }
                None
            }
            Output::Request {
                id,
                method,
                path,
                body,
                bearer,
            } => {
                let request = self.request(&method, &path, bearer.as_ref());
                let request = match body {
                    Some(body) => request
                        .header("Content-Type", "application/json")
                        .body(body.0.clone()),
                    None => request,
                };
                let inner = self.inner.clone();
                // A query can hold a search's text, so only the path is logged.
                let route = path.split('?').next().unwrap_or_default().to_string();
                tokio::spawn(async move {
                    let (status, body) = match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            (status, response.text().await.unwrap_or_default())
                        }
                        Err(_) => (0, String::new()),
                    };
                    tracing::debug!(method = %method, path = %route, status, "a request was answered");
                    let _ = inner.send(Inner::Input(Input::Reply { id, status, body }));
                });
                None
            }
            Output::PutChunk {
                id,
                method,
                path,
                bearer,
                bytes,
            } => {
                let request = self
                    .request(&method, &path, bearer.as_ref())
                    .header("Content-Type", "application/octet-stream")
                    .body(bytes);
                let inner = self.inner.clone();
                tokio::spawn(async move {
                    let (status, body) = match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            (status, response.text().await.unwrap_or_default())
                        }
                        Err(_) => (0, String::new()),
                    };
                    let _ = inner.send(Inner::Input(Input::Reply { id, status, body }));
                });
                None
            }
            Output::GetChunk { id, path, bearer } => {
                let request = self.request("GET", &path, bearer.as_ref());
                let inner = self.inner.clone();
                tokio::spawn(async move {
                    let (status, bytes) = match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            match response.bytes().await {
                                Ok(bytes) => (status, bytes.to_vec()),
                                Err(_) => (0, Vec::new()),
                            }
                        }
                        Err(_) => (0, Vec::new()),
                    };
                    let _ = inner.send(Inner::Input(Input::Fetched { id, status, bytes }));
                });
                None
            }
            Output::Connect { bearer } => {
                self.connect(bearer);
                None
            }
            Output::Disconnect => {
                self.generation += 1;
                self.socket = None;
                None
            }
            Output::Save { id, slot, bytes } => {
                let saved = lock(&self.shared.store).save_slot(slot, &bytes, now());
                match saved {
                    Ok(()) => Some(Input::Saved { id }),
                    Err(error) => {
                        tracing::error!(error = %error, "could not save the snapshot");
                        Some(Input::SaveFailed { id })
                    }
                }
            }
            Output::Wipe => {
                tracing::warn!("this device ended: its owner renewed or revoked the token");
                if let Err(error) = lock(&self.shared.store).wipe_slots() {
                    tracing::error!(error = %error, "could not wipe the snapshot");
                }
                None
            }
            Output::Open { .. } => {
                let slots = lock(&self.shared.store).slots();
                let [first, second] = slots.unwrap_or_default();
                Some(Input::Opened { first, second })
            }
            Output::WakeAt { at } => {
                self.wake_at = Some(at);
                None
            }
            Output::Event { event } => {
                self.event(*event);
                None
            }
            Output::ReadChunk {
                handle,
                n,
                offset,
                length,
            } => {
                let bytes = match self.handles.get(&handle) {
                    Some(path) => read_at(path, offset, length),
                    None => Vec::new(),
                };
                Some(Input::ChunkRead { handle, n, bytes })
            }
            Output::WriteChunk {
                handle,
                offset,
                bytes,
                ..
            } => {
                if let Some(path) = self.handles.get(&handle)
                    && let Err(error) = write_at(path, offset, &bytes)
                {
                    tracing::error!(handle, error = %error, "could not write a download");
                }
                None
            }
            Output::JoinCall { chat, .. } => {
                // keryx-agent has no calls: leave at once.
                Some(Input::Command {
                    command: Command::LeaveCall { chat },
                })
            }
            Output::CallKey { .. } | Output::LeaveCall { .. } | Output::CallText { .. } => None,
            Output::SaveRows {
                id,
                message_list,
                call_list,
                chat_event_list,
            } => {
                let saved =
                    lock(&self.shared.store).save_rows(&message_list, &call_list, &chat_event_list);
                match saved {
                    Ok(()) => {
                        for message in &message_list {
                            if message.kind == Some(Kind::Delete)
                                && let Some(target_seq) =
                                    self.pending_deletes.remove(&(message.chat, message.seq))
                                && message.pointer_seq == Some(target_seq)
                            {
                                self.incoming(&Event::Deleted {
                                    chat: message.chat,
                                    seq: message.seq,
                                    target_seq,
                                });
                            }
                        }
                        Some(Input::RowsSaved { id })
                    }
                    Err(error) => {
                        tracing::error!(error = %error, "could not save rows to the archive");
                        Some(Input::RowsSaveFailed { id })
                    }
                }
            }
            Output::ReadRows {
                id,
                chat,
                before_seq,
                limit,
            } => {
                let given = match self.opening.as_mut() {
                    Some(opening) if opening.chat == chat => opening.rows.take(),
                    _ => None,
                };
                let rows = match given {
                    Some(rows) => Ok(rows),
                    None => lock(&self.shared.store).read_rows(chat, before_seq, limit),
                };
                let rows = match rows {
                    Ok(rows) => rows,
                    Err(error) => {
                        tracing::error!(error = %error, "could not read the archive");
                        Rows {
                            message_list: Vec::new(),
                            call_list: Vec::new(),
                            chat_event_list: Vec::new(),
                        }
                    }
                };
                Some(Input::RowsRead {
                    id,
                    message_list: rows.message_list,
                    call_list: rows.call_list,
                    chat_event_list: rows.chat_event_list,
                })
            }
            Output::ScanRows { id, before, limit } => {
                if self
                    .scanning
                    .as_ref()
                    .is_some_and(|(reply, _)| reply.is_closed())
                {
                    return Some(Input::RowsScanned {
                        id,
                        message_list: Vec::new(),
                        total: 0,
                    });
                }
                let scanned = lock(&self.shared.store).scan_rows(before, limit);
                let (message_list, total) = match scanned {
                    Ok(rows) => rows,
                    Err(error) => {
                        if let Some((reply, _)) = self.scanning.take() {
                            let _ = reply.send(Err(error));
                        }
                        (Vec::new(), 0)
                    }
                };
                Some(Input::RowsScanned {
                    id,
                    message_list,
                    total,
                })
            }
        }
    }

    fn request(
        &self,
        method: &str,
        path: &str,
        bearer: Option<&Secret>,
    ) -> reqwest::RequestBuilder {
        let url = format!("{}{path}", self.base);
        let method = reqwest::Method::from_bytes(method.as_bytes()).unwrap_or(reqwest::Method::GET);
        let request = self.http.request(method, url);
        match bearer {
            Some(bearer) => request.bearer_auth(&bearer.0),
            None => request,
        }
    }

    /// Opens the WebSocket on its own task, which hands back its frames and its close.
    fn connect(&mut self, bearer: Option<Secret>) {
        self.generation += 1;
        let generation = self.generation;
        self.socket = None;
        let url = format!(
            "{}/api/v1/ws",
            self.base
                .replacen("https://", "wss://", 1)
                .replacen("http://", "ws://", 1)
        );
        let inner = self.inner.clone();
        let tls = self.tls.clone();
        tokio::spawn(async move {
            let closed = |code: u16, reason: String| {
                Inner::Socket(generation, Input::Closed { code, reason })
            };
            let mut request = match url.into_client_request() {
                Ok(request) => request,
                Err(_) => {
                    let _ = inner.send(closed(1006, String::new()));
                    return;
                }
            };
            request.headers_mut().insert(
                "Sec-WebSocket-Protocol",
                HeaderValue::from_static("keryx.v1"),
            );
            if let Some(bearer) = &bearer
                && let Ok(value) = HeaderValue::from_str(&format!("Bearer {}", bearer.0))
            {
                request.headers_mut().insert("Authorization", value);
            }
            let connector = tokio_tungstenite::Connector::Rustls(tls);
            let opened = tokio::time::timeout(
                Duration::from_secs(20),
                tokio_tungstenite::connect_async_tls_with_config(
                    request,
                    None,
                    true,
                    Some(connector),
                ),
            )
            .await;
            let socket = match opened {
                Ok(Ok((socket, _))) => socket,
                _ => {
                    let _ = inner.send(closed(1006, String::new()));
                    return;
                }
            };
            let (mut sink, mut stream) = socket.split();
            let (writer, mut outgoing) = mpsc::unbounded_channel::<Message>();
            if inner.send(Inner::Opened(generation, writer)).is_err() {
                return;
            }
            let writing = tokio::spawn(async move {
                while let Some(message) = outgoing.recv().await {
                    if sink.send(message).await.is_err() {
                        break;
                    }
                }
                let _ = sink.close().await;
            });
            let mut ended = (1006, String::new());
            while let Some(read) = stream.next().await {
                match read {
                    Ok(Message::Text(text)) => {
                        let input = Input::Frame {
                            text: text.to_string(),
                        };
                        if inner.send(Inner::Socket(generation, input)).is_err() {
                            break;
                        }
                    }
                    Ok(Message::Close(frame)) => {
                        if let Some(frame) = frame {
                            ended = (u16::from(frame.code), frame.reason.to_string());
                        }
                        break;
                    }
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
            writing.abort();
            let _ = inner.send(closed(ended.0, ended.1));
        });
    }

    /// Handles one event core showed.
    fn event(&mut self, event: Event) {
        if !matches!(event, Event::Answered { .. }) {
            for waiter in self.waiters.values_mut() {
                waiter.event_list.push(event.clone());
            }
            if let Some(opening) = self.opening.as_mut() {
                opening.event_list.push(event.clone());
            }
        }
        match &event {
            Event::Answered {
                tag,
                seq,
                code,
                message,
                seconds,
            } => {
                self.answered(*tag, *seq, code.clone(), message.clone(), *seconds);
                return;
            }
            Event::PageLoaded { chat, .. } => {
                if self.opening.as_ref().is_some_and(|open| open.chat == *chat)
                    && let Some(mut opening) = self.opening.take()
                {
                    if let Some(reply) = opening.reply.take() {
                        let _ = reply.send(std::mem::take(&mut opening.event_list));
                    }
                    self.later_open();
                }
            }
            Event::MessageHits {
                hit_list,
                done,
                more,
                ..
            } => {
                if let Some((_, hits)) = &mut self.scanning {
                    hits.extend_from_slice(hit_list);
                }
                if *done && let Some((reply, hits)) = self.scanning.take() {
                    let _ = reply.send(Ok((hits, *more)));
                }
            }
            Event::NeedLogin => {
                tracing::info!("logging in with the token");
                self.later.push_back(Input::Command {
                    command: Command::LogInAgent {
                        token: self.token.clone(),
                    },
                });
            }
            Event::NeedWords { restore } => {
                if *restore {
                    tracing::info!("restoring this device from the words");
                    self.later.push_back(Input::Command {
                        command: Command::Restore {
                            words: self.words.clone(),
                        },
                    });
                } else {
                    tracing::error!("the account has no words yet: its owner's app sets them");
                }
            }
            Event::Token { token } => {
                let saved =
                    lock(&self.shared.store).set_session(token.as_ref().map(|t| t.0.as_str()));
                if let Err(error) = saved {
                    tracing::error!(error = %error, "could not keep the session");
                }
            }
            Event::SignedIn {
                account,
                username,
                device,
            } => {
                tracing::info!(account = %account, device = ?device.map(|d| d.to_string()), "signed in");
                let mut view = lock(&self.shared.view);
                view.account = Some(*account);
                view.username = Some(username.clone());
                view.device = *device;
                view.ready = device.is_some();
            }
            Event::Online => {
                tracing::info!("connected");
                lock(&self.shared.view).connected = true;
                if let Some(text) = self.trouble.clone() {
                    self.later.push_back(Input::Command {
                        command: Command::InApp {
                            in_app: true,
                            trouble: true,
                            text: Some(text),
                        },
                    });
                }
                self.later_requeue();
            }
            Event::Offline => {
                tracing::info!("disconnected");
                lock(&self.shared.view).connected = false;
            }
            Event::LoggedOut => {
                tracing::error!("this device ended, and waits for its owner to renew the token");
                let mut view = lock(&self.shared.view);
                view.ready = false;
                view.connected = false;
            }
            Event::DeviceLimit { .. } => {
                tracing::error!("the server refused a second device for this agent");
            }
            Event::Error { code, .. } => {
                tracing::warn!(code = %code, "core reported an error");
            }
            Event::Message { message } if !self.synced && message.mine => {
                let device = lock(&self.shared.view).device;
                if message.device.is_some() && message.device == device {
                    self.own_seen.push(OwnSeen {
                        chat: message.chat,
                        seq: message.seq,
                        sent_at: message.sent_at,
                        text: message.text.as_ref().map(|text| text.0.clone()),
                    });
                }
            }
            _ => {}
        }
        self.view_event(&event);
        let chats_came = matches!(event, Event::Chats { .. });
        if chats_came || !self.held.is_empty() {
            self.release(chats_came);
        }
        self.incoming(&event);
        if self.shared.plain {
            self.index_event(&event);
        }
    }

    /// Queues the next open, once the one under way finished.
    fn later_open(&mut self) {
        if let Some(opening) = self.opens.pop_front() {
            let chat = opening.chat;
            self.opening = Some(opening);
            self.later.push_back(Input::Command {
                command: Command::OpenChat { chat },
            });
        } else {
            self.reindex_next();
        }
    }

    /// A tagged command was answered: its caller is told, and its outgoing row moves on.
    fn answered(
        &mut self,
        tag: u64,
        seq: Option<u64>,
        code: Option<String>,
        message: Option<String>,
        seconds: Option<u64>,
    ) {
        if self.sync_tag == Some(tag) {
            self.sync_tag = None;
            self.synced = true;
            self.match_sent();
        }
        let Some(waiter) = self.waiters.remove(&tag) else {
            return;
        };
        let local = waiter.event_list.iter().find_map(|event| match event {
            Event::Pending { local, .. } => Some(*local),
            _ => None,
        });
        if let Some(outgoing) = waiter.outgoing {
            let stamp = now();
            let store = lock(&self.shared.store);
            let result = match (&code, seq) {
                (Some(code), _) if code == "offline" && local.is_none() => {
                    store.set_outgoing(outgoing, OutgoingState::Queued, None, None, stamp)
                }
                (Some(code), _) => {
                    store.set_outgoing(outgoing, OutgoingState::Refused, None, Some(code), stamp)
                }
                (None, seq) => store.set_outgoing(outgoing, OutgoingState::Done, seq, None, stamp),
            };
            if let Err(error) = result {
                tracing::error!(error = %error, "could not update the outgoing queue");
            }
            if let Some(local) = local {
                self.locals.remove(&local);
            }
        }
        let answer = Answer {
            seq,
            code: code.clone(),
            message,
            seconds,
            event_list: waiter.event_list,
        };
        let told = match waiter.reply {
            Some(reply) => reply.send(answer).is_ok(),
            None => false,
        };
        // Nobody waited any more, as a write past its timeout returned queued: its result reaches
        // the loop as a `sent` event.
        if !told && let Some(outgoing) = waiter.outgoing {
            let body = json!({
                "outgoing": outgoing.to_string(),
                "seq": seq,
                "code": code,
                "seconds": seconds,
            });
            self.keep(NewEvent {
                kind: EventKind::Sent,
                chat: None,
                topic: None,
                thread_seq: None,
                seq: None,
                body,
                until: None,
            });
        }
    }

    /// Sends again what waited in the outgoing queue for a connection, once the first sync is
    /// through for what may already be stored.
    fn later_requeue(&mut self) {
        if !self.synced && self.sync_tag.is_none() {
            self.next_tag += 1;
            let tag = self.next_tag;
            self.sync_tag = Some(tag);
            self.later.push_back(Input::Tagged {
                tag,
                command: Command::ChatList,
            });
            return;
        }
        if self.synced {
            self.resend(OutgoingState::Queued);
        }
    }

    /// Feeds again each outgoing row in `state`, tagged with no one waiting.
    fn resend(&mut self, state: OutgoingState) {
        let rows = match lock(&self.shared.store).open_outgoing() {
            Ok(rows) => rows,
            Err(error) => {
                tracing::error!(error = %error, "could not read the outgoing queue");
                return;
            }
        };
        for row in rows {
            if row.state != state {
                continue;
            }
            let Ok(command) = serde_json::from_value::<Command>(row.command.clone()) else {
                continue;
            };
            if let Err(error) = lock(&self.shared.store).set_outgoing(
                row.id,
                OutgoingState::Sent,
                None,
                None,
                now(),
            ) {
                tracing::error!(error = %error, "could not update the outgoing queue");
            }
            self.next_tag += 1;
            let tag = self.next_tag;
            self.waiters.insert(
                tag,
                Waiter {
                    reply: None,
                    outgoing: Some(row.id),
                    event_list: Vec::new(),
                },
            );
            self.later.push_back(Input::Tagged { tag, command });
        }
    }

    /// After the first sync since starting: a send handed to core before a restart and never
    /// answered is looked for among this device's own messages the sync showed, by its chat and
    /// text and a send time at or after it was handed over. Found, it takes that seq; otherwise it
    /// goes again, with what waited for a connection.
    fn match_sent(&mut self) {
        let rows = match lock(&self.shared.store).open_outgoing() {
            Ok(rows) => rows,
            Err(_) => return,
        };
        let own_seen = std::mem::take(&mut self.own_seen);
        let mut used = HashSet::new();
        for row in rows {
            if row.state != OutgoingState::Sent {
                continue;
            }
            let wanted = command_text(&row.command);
            let mut found = None;
            for seen in &own_seen {
                if seen.chat == row.chat
                    && seen.sent_at + 1_000 >= row.client_at
                    && seen.text == wanted
                    && !used.contains(&(seen.chat, seen.seq))
                {
                    found = Some(seen.seq);
                    used.insert((seen.chat, seen.seq));
                    break;
                }
            }
            let store = lock(&self.shared.store);
            let result = match found {
                Some(seq) => {
                    store.set_outgoing(row.id, OutgoingState::Done, Some(seq), None, now())
                }
                None => store.set_outgoing(row.id, OutgoingState::Queued, None, None, now()),
            };
            if let Err(error) = result {
                tracing::error!(error = %error, "could not update the outgoing queue");
            }
        }
        self.resend(OutgoingState::Queued);
    }

    /// Keeps what core showed of chats, people and the rest in the view scripts read.
    fn view_event(&mut self, event: &Event) {
        let mut view = lock(&self.shared.view);
        match event {
            Event::Chats { chat_list } => {
                let mut present = HashSet::new();
                for item in chat_list {
                    present.insert(item.chat);
                    let topics_on = item.topics_on;
                    let options = item.options.clone();
                    let findable = item.findable.clone();
                    match view.chats.get_mut(&item.chat) {
                        Some(chat) => {
                            chat.item = item.clone();
                            chat.topics_on = topics_on;
                            if options.is_some() {
                                chat.options = options;
                            }
                            chat.findable = findable;
                        }
                        None => {
                            let profile = self.unlisted_profiles.remove(&item.chat);
                            let topic_list = match &profile {
                                Some(profile) => profile.topic_list.clone(),
                                None => Vec::new(),
                            };
                            view.chats.insert(
                                item.chat,
                                ChatView {
                                    item: item.clone(),
                                    profile,
                                    options,
                                    topics_on,
                                    topic_list,
                                    topic_options: HashMap::new(),
                                    findable,
                                    member_list: None,
                                    preview: None,
                                    ready: false,
                                },
                            );
                        }
                    }
                }
                view.chats.retain(|chat, _| present.contains(chat));
                self.shared.chats_changed.notify_one();
            }
            Event::ChatReady { chat } => {
                if let Some(view_chat) = view.chats.get_mut(chat) {
                    view_chat.ready = true;
                }
            }
            Event::ChatLeft { chat } => {
                view.chats.remove(chat);
                self.unlisted_profiles.remove(chat);
            }
            Event::Account { account } => {
                let known = view.accounts.entry(account.id).or_default();
                known.username = Some(account.username.clone());
                if account.display_name.is_some() {
                    known.display_name = account.display_name.clone();
                }
            }
            Event::Profile { profile } => {
                let known = view.accounts.entry(profile.account).or_default();
                known.username = Some(profile.username.clone());
                known.display_name = profile.display_name.clone();
                known.bio = profile.bio.clone();
                known.link_list = profile
                    .link_list
                    .iter()
                    .map(|link| (link.name.clone(), link.url.clone()))
                    .collect();
                known.agent = profile.agent;
                known.owner = profile.owner;
            }
            Event::Presence { presence, .. } => {
                let state = presence
                    .state
                    .and_then(|state| serde_json::to_value(state).ok())
                    .and_then(|value| value.as_str().map(str::to_string));
                view.accounts.entry(presence.account).or_default().presence = state;
            }
            Event::OwnDevices { device_list } => {
                view.own_device_list = device_list.clone();
            }
            Event::Folders { folder_list, .. } => {
                view.folder_list = folder_list.clone();
            }
            Event::ChatProfile { chat, profile } => {
                if let Some(view_chat) = view.chats.get_mut(chat) {
                    view_chat.topic_list = profile.topic_list.clone();
                    view_chat.profile = Some(profile.clone());
                } else {
                    self.unlisted_profiles.insert(*chat, profile.clone());
                }
                self.shared.chats_changed.notify_one();
            }
            Event::ChatOptions { chat, options } => {
                if let Some(view_chat) = view.chats.get_mut(chat) {
                    view_chat.options = Some(options.clone());
                }
            }
            Event::Topics {
                chat,
                on,
                topic_list,
            } => {
                if let Some(view_chat) = view.chats.get_mut(chat) {
                    view_chat.topics_on = *on;
                    view_chat.topic_list = topic_list.clone();
                }
            }
            Event::TopicOptions {
                chat,
                topic,
                options,
                mod_list,
            } => {
                if let Some(view_chat) = view.chats.get_mut(chat) {
                    view_chat
                        .topic_options
                        .insert(*topic, (options.clone(), mod_list.clone()));
                }
            }
            Event::Findable { chat, findable } => {
                if let Some(view_chat) = view.chats.get_mut(chat) {
                    view_chat.findable = findable.clone();
                }
            }
            Event::Members { chat, member_list } => {
                if let Some(view_chat) = view.chats.get_mut(chat) {
                    view_chat.member_list = Some(member_list.clone());
                }
            }
            Event::Invitation { chat, invitation } => match invitation {
                Some(invitation) => {
                    view.invitations.insert(*chat, invitation.clone());
                }
                None => {
                    view.invitations.remove(chat);
                }
            },
            Event::Requests { chat, request_list } => {
                view.requests.insert(*chat, request_list.clone());
            }
            Event::JoinRequest { request } => {
                let list = view.requests.entry(request.chat).or_default();
                list.retain(|item| item.account != request.account);
                if request.state == RequestState::Waiting {
                    list.push(request.clone());
                }
            }
            Event::Stickers { pack_list } => {
                view.pack_list = Some(pack_list.clone());
            }
            Event::TopicProfile {
                chat,
                topic,
                profile,
            } => {
                view.topic_profiles.insert((*chat, *topic), profile.clone());
            }
            Event::Message { message } => {
                let is_preview = view
                    .chats
                    .get(&message.chat)
                    .is_some_and(|chat| chat.item.last_content_seq == Some(message.seq));
                if is_preview {
                    let value = message_value(&view, message);
                    if let Some(view_chat) = view.chats.get_mut(&message.chat) {
                        view_chat.preview = Some(value);
                    }
                }
            }
            _ => {}
        }
    }

    /// Keeps what may wake the loop: what others sent and did, saved after the watermark.
    fn incoming(&mut self, event: &Event) {
        let new = match event {
            Event::Message { message } => self.message_event(message),
            Event::Edited {
                message,
                target_seq,
            } => {
                if message.mine || message.saved_at <= self.watermark {
                    return;
                }
                let view = lock(&self.shared.view);
                let mut body = self.chat_fields(&view, message.chat);
                body["event"] = json!("edit");
                body["message"] = message_value(&view, message);
                body["target"] = json!(target_seq);
                Some(NewEvent {
                    kind: EventKind::Edit,
                    chat: Some(message.chat),
                    topic: message.topic,
                    thread_seq: message.thread_seq,
                    seq: Some(message.seq),
                    body,
                    until: None,
                })
            }
            Event::Deleted {
                chat,
                seq,
                target_seq,
            } => {
                let row = lock(&self.shared.store).message_row(*chat, *seq);
                let (device, saved_at) = match row {
                    Ok(Some(row)) => row,
                    Ok(None) => {
                        self.pending_deletes.insert((*chat, *seq), *target_seq);
                        return;
                    }
                    Err(error) => {
                        tracing::error!(error = %error, "could not read a delete from the archive");
                        return;
                    }
                };
                let view = lock(&self.shared.view);
                if view.own_device(device) || saved_at <= self.watermark {
                    return;
                }
                let mut body = self.chat_fields(&view, *chat);
                body["event"] = json!("delete");
                body["seq"] = json!(seq);
                body["target"] = json!(target_seq);
                Some(NewEvent {
                    kind: EventKind::Delete,
                    chat: Some(*chat),
                    topic: None,
                    thread_seq: None,
                    seq: Some(*seq),
                    body,
                    until: None,
                })
            }
            Event::Line {
                chat,
                seq,
                account,
                action,
                account_list,
                saved_at,
                mine,
                ..
            } => {
                let view = lock(&self.shared.view);
                let me = view.account;
                let named = me.is_some_and(|me| account_list.contains(&me));
                let kind = match action {
                    LineAction::Added | LineAction::LetIn if named => EventKind::Added,
                    LineAction::Removed if named => EventKind::Removed,
                    _ => return,
                };
                if *mine || *saved_at <= self.watermark {
                    return;
                }
                let mut body = self.chat_fields(&view, *chat);
                body["event"] = json!(kind.name());
                body["by"] = view.person(*account);
                body["seq"] = json!(seq);
                Some(NewEvent {
                    kind,
                    chat: Some(*chat),
                    topic: None,
                    thread_seq: None,
                    seq: Some(*seq),
                    body,
                    until: None,
                })
            }
            Event::Invitation {
                chat,
                invitation: Some(invitation),
            } => {
                let view = lock(&self.shared.view);
                let body = json!({
                    "event": "invitation",
                    "chat": chat.to_string(),
                    "chat_type": chat_type_name(invitation.chat_type),
                    "name": invitation.name,
                    "inviter": view.person(invitation.inviter),
                    "note": invitation.note,
                    "at": time_text(invitation.at),
                });
                Some(NewEvent {
                    kind: EventKind::Invitation,
                    chat: Some(*chat),
                    topic: None,
                    thread_seq: None,
                    seq: Some(u64::try_from(invitation.at).unwrap_or(0)),
                    body,
                    until: None,
                })
            }
            Event::JoinRequest { request } if request.state == RequestState::Waiting => {
                let view = lock(&self.shared.view);
                let mut body = self.chat_fields(&view, request.chat);
                body["event"] = json!("join_request");
                body["sender"] = view.person(request.account);
                body["note"] = json!(request.note);
                body["at"] = json!(time_text(request.at));
                Some(NewEvent {
                    kind: EventKind::JoinRequest,
                    chat: Some(request.chat),
                    topic: None,
                    thread_seq: None,
                    seq: Some(u64::try_from(request.at).unwrap_or(0)),
                    body,
                    until: None,
                })
            }
            _ => None,
        };
        if let Some(new) = new {
            self.keep(new);
        }
    }

    /// A message from someone else, as the event it is: a reaction on this agent's message, an
    /// answer to its question, an ack of its suggestion, or any other content.
    fn message_event(&self, message: &Shown) -> Option<NewEvent> {
        if message.mine || message.saved_at <= self.watermark || message.content_kind.is_none() {
            return None;
        }
        let pointed = match message.content_kind {
            Some(ContentKind::Reaction) => message.on,
            Some(ContentKind::Answer) => message.answer.as_ref().map(|answer| answer.question),
            Some(ContentKind::Ack) => message.ack.as_ref().map(|ack| ack.suggestion),
            _ => None,
        };
        let kind = match message.content_kind {
            Some(ContentKind::Reaction) => EventKind::Reaction,
            Some(ContentKind::Answer) => EventKind::Answer,
            Some(ContentKind::Ack) => EventKind::Ack,
            _ => EventKind::Message,
        };
        let view = lock(&self.shared.view);
        if kind != EventKind::Message {
            let target = pointed?;
            let row = lock(&self.shared.store).message_row(message.chat, target);
            let Ok(Some((device, _))) = row else {
                return None;
            };
            if !view.own_device(device) {
                return None;
            }
        }
        let replies_to_me = match message.reply {
            Some(reply) => match lock(&self.shared.store).message_row(message.chat, reply) {
                Ok(Some((device, _))) => view.own_device(device),
                _ => false,
            },
            None => false,
        };
        let mentions_me = view
            .account
            .is_some_and(|me| message.mentions.contains(&me));
        let mut body = self.chat_fields(&view, message.chat);
        body["event"] = json!(kind.name());
        body["message"] = message_value(&view, message);
        body["mentions_me"] = json!(mentions_me);
        body["replies_to_me"] = json!(replies_to_me);
        Some(NewEvent {
            kind,
            chat: Some(message.chat),
            topic: message.topic,
            thread_seq: message.thread_seq,
            seq: Some(message.seq),
            body,
            until: None,
        })
    }

    /// The chat's fields every event about it carries.
    fn chat_fields(&self, view: &View, chat: Ulid) -> Value {
        let chat_type = view
            .chats
            .get(&chat)
            .map(|chat| chat_type_name(chat.item.chat_type));
        json!({
            "chat": chat.to_string(),
            "chat_type": chat_type,
            "chat_name": view.chat_name(chat),
        })
    }

    /// Keeps an event and wakes the loop, or holds it while its chat or sender is not known yet,
    /// asking core for the chat list when its chat is missing.
    fn keep(&mut self, event: NewEvent) {
        let (listed, named) = {
            let view = lock(&self.shared.view);
            (listed_in(&view, &event), named_in(&view, &event))
        };
        if listed && named {
            self.store_event(event);
            return;
        }
        if !listed && !self.listing {
            self.listing = true;
            self.later.push_back(Input::Command {
                command: Command::ChatList,
            });
        }
        self.held.push((event, std::time::Instant::now()));
    }

    /// Keeps each held event that is now known, or waited [`HOLD_FOR`], or whose chat a chat list
    /// just left out, as the chat may be gone. Additions wait for the name up to the hold.
    fn release(&mut self, chats_came: bool) {
        if chats_came {
            self.listing = false;
        }
        let held = std::mem::take(&mut self.held);
        for (mut event, since) in held {
            let ready = {
                let view = lock(&self.shared.view);
                let listed = listed_in(&view, &event);
                let due = since.elapsed() >= HOLD_FOR
                    || (event.kind != EventKind::Added && chats_came && !listed);
                if (listed && named_in(&view, &event)) || due {
                    if let Some(chat) = event.chat
                        && let Value::Object(fields) = self.chat_fields(&view, chat)
                    {
                        for (key, value) in fields {
                            event.body[key] = value;
                        }
                    }
                    for key in PEOPLE {
                        let id = person_id(&event.body, key);
                        if let Some(id) = id {
                            let person = view.person(id);
                            match key {
                                "message" => event.body["message"]["sender"] = person,
                                _ => event.body[key] = person,
                            }
                        }
                    }
                    true
                } else {
                    false
                }
            };
            if ready {
                self.store_event(event);
            } else {
                self.held.push((event, since));
            }
        }
    }

    fn store_event(&self, event: NewEvent) {
        let kept = lock(&self.shared.store).add_event(&event, now());
        match kept {
            Ok(Some(id)) => {
                tracing::debug!(event = %id, kind = event.kind.name(), "event kept");
                self.shared.wake.notify_one();
            }
            Ok(None) => {}
            Err(error) => tracing::error!(error = %error, "could not keep an event"),
        }
    }

    // ---------------------------------------------------------------------------------------------
    // The index
    // ---------------------------------------------------------------------------------------------

    /// With `[search] plain` off, drops any message the index still holds. On, notes what is still
    /// to be indexed, opened once the agent is ready.
    fn prepare_index(&mut self) {
        if !self.shared.plain {
            if self.covered.messages > 0 {
                let mut index = lock(&self.shared.index);
                index.delete_messages();
                self.covered.messages = 0;
                if let Err(error) = index.commit(self.covered) {
                    tracing::error!(error = %error, "could not empty the index of messages");
                }
            }
            return;
        }
        let chats = lock(&self.shared.store).chats_after(self.covered.messages);
        match chats {
            Ok(chat_list) => {
                for chat in chat_list {
                    self.reindex.push_back((chat, self.covered.messages));
                }
            }
            Err(error) => tracing::error!(error = %error, "could not read the archive"),
        }
    }

    /// Opens the next chat's rows not yet in the index, a page at a time.
    fn reindex_next(&mut self) {
        if self.opening.is_some() || !lock(&self.shared.view).ready {
            return;
        }
        while let Some((chat, after)) = self.reindex.front().copied() {
            let rows = lock(&self.shared.store).rows_after(chat, after, REINDEX_PAGE);
            match rows {
                Ok(rows) if !rows.message_list.is_empty() => {
                    let mut newest = after;
                    for message in &rows.message_list {
                        newest = newest.max(message.server_saved_at.unwrap_or(after));
                    }
                    if rows.message_list.len() < usize::try_from(REINDEX_PAGE).unwrap_or(0)
                        || newest <= after
                    {
                        self.reindex.pop_front();
                    } else if let Some(front) = self.reindex.front_mut() {
                        front.1 = newest;
                    }
                    self.opening = Some(Opening {
                        chat,
                        rows: Some(rows),
                        reply: None,
                        event_list: Vec::new(),
                    });
                    self.later.push_back(Input::Command {
                        command: Command::OpenChat { chat },
                    });
                    return;
                }
                _ => {
                    self.reindex.pop_front();
                }
            }
        }
    }

    fn index_event(&mut self, event: &Event) {
        if matches!(event, Event::ChatReady { .. } | Event::Online) {
            self.reindex_next();
            return;
        }
        let mut index = lock(&self.shared.index);
        match event {
            Event::Message { message } => {
                let Some(text) = &message.text else {
                    return;
                };
                if message.content_kind == Some(ContentKind::Reaction) {
                    return;
                }
                let added = index.put(&Document {
                    kind: DocKind::Message,
                    chat: Some(message.chat),
                    seq: Some(message.seq),
                    note: None,
                    sender: message.account,
                    topic: message.topic,
                    thread: message.thread_seq,
                    at: message.saved_at,
                    text: text.0.clone(),
                });
                if let Err(error) = added {
                    tracing::error!(error = %error, "could not index a message");
                }
                self.covered.messages = self.covered.messages.max(message.saved_at);
                self.index_dirty = true;
            }
            Event::Edited {
                message,
                target_seq,
            } => {
                let Some(text) = &message.text else {
                    return;
                };
                let added = index.put(&Document {
                    kind: DocKind::Message,
                    chat: Some(message.chat),
                    seq: Some(*target_seq),
                    note: None,
                    sender: message.account,
                    topic: message.topic,
                    thread: message.thread_seq,
                    at: message.saved_at,
                    text: text.0.clone(),
                });
                if let Err(error) = added {
                    tracing::error!(error = %error, "could not index an edit");
                }
                self.index_dirty = true;
            }
            Event::Deleted {
                chat, target_seq, ..
            } => {
                index.delete(Some(*chat), Some(*target_seq), None);
                self.index_dirty = true;
            }
            Event::Removed { chat, seq } => {
                index.delete(Some(*chat), Some(*seq), None);
                self.index_dirty = true;
            }
            _ => {}
        }
    }

    fn commit_index(&mut self) {
        let mut index = lock(&self.shared.index);
        let mut covered = self.covered;
        covered.notes = index.covered().notes;
        if let Err(error) = index.commit(covered) {
            tracing::error!(error = %error, "could not commit the index");
        }
        self.index_dirty = false;
        self.last_commit = std::time::Instant::now();
    }
}

/// One of this device's own messages a sync showed.
struct OwnSeen {
    chat: Ulid,
    seq: u64,
    sent_at: i64,
    text: Option<String>,
}

/// The text a queued command sends, from its JSON: `{"SendText": {"text": ...}}` and the like.
fn command_text(command: &Value) -> Option<String> {
    let inner = command.as_object()?.values().next()?;
    for key in ["text", "caption"] {
        if let Some(text) = inner.get(key).and_then(Value::as_str) {
            return Some(text.to_string());
        }
    }
    None
}

/// The socket, REST and timer tasks' channel into the driver.
pub struct Inbox(mpsc::UnboundedReceiver<Inner>);

/// Reads `length` bytes at `offset` of a file, or what there is.
fn read_at(path: &std::path::Path, offset: u64, length: u64) -> Vec<u8> {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    if file.seek(SeekFrom::Start(offset)).is_err() {
        return Vec::new();
    }
    let mut bytes = Vec::new();
    let _ = file.take(length).read_to_end(&mut bytes);
    bytes
}

/// Writes bytes at `offset` of a file, making it when missing.
fn write_at(path: &std::path::Path, offset: u64, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::{Seek, SeekFrom, Write};
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    file.write_all(bytes)
}
