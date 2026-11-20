//! Every operation, and the axum router: REST and WebSocket.
//!
//! **No operation exists without its check.** Each `run_*` function calls `check` first, and the
//! check reads the role tables each time, so a role change applies at once. Secrets travel only in
//! headers and bodies, because the access log records the path and the query.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::path::{Path as FilePath, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use argon2::{Argon2, PasswordHasher, PasswordVerifier};
use axum::Router;
use axum::body::Bytes as RawBody;
use axum::extract::rejection::BytesRejection;
use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::extract::ws::{CloseFrame, Message, Utf8Bytes, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, DefaultBodyLimit, MatchedPath, Path, Query, Request, State};
use axum::http::header::{
    AUTHORIZATION, CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE, COOKIE, ORIGIN, SERVER, SET_COOKIE,
};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, delete, get, post, put};
use axum_server::accept::Accept;
use axum_server::tls_rustls::{RustlsAcceptor, RustlsConfig};
use base64ct::{Base64UrlUnpadded, Encoding};
use hmac::{Hmac, KeyInit, Mac};
use keryx_core::mls::{
    verify_account_key, verify_device_login, verify_device_stamp, verify_invite, verify_invite_key,
    verify_logout, verify_password_reset, verify_revocation,
};
use keryx_core::protocol::{
    AccountInfo, AccountKeysBody, AccountListReply, AccountReply, AgentClaims, AgentEndBody,
    AgentListReply, AgentLoginBody, AgentStatement, AgentTokenReply, Audience, Bytes,
    ChallengeBody, ChallengeReply, ChatAction, ChatDeviceListReply, ChatEvent, ChatFolders,
    ChatOptions, ChatProfileBody, ChatProfileItem, ChatReply, ChatRole, ChatType, ClaimBody,
    ClientFrame, ContentKind, CountReply, CreateAccountBody, CreateAgentBody, CreateChatBody,
    CreateFileBody, CreatePackBody, CreatedAccountReply, CreatedAgentReply, DeviceListReply,
    DeviceProof, DeviceReply, DevicesReply, DraftBody, DraftRequest, EmptyReply, FeaturesReply,
    FileReply, FileStatusReply, FindableBody, FindableItem, FolderReply, GENERAL_TOPIC,
    GroupInfoReply, InvitationItem, InviteAnswerBody, InviteBody, KeyPackageListReply,
    KeyPackagesBody, KvChange, KvReply, LoginBody, LoginReply, MemberItem, MemberListReply,
    NamesBody, PackReply, PhotoReply, PresenceBody, PresenceItem, PresenceListReply, ProfileReply,
    PutProfileBody, ReducedDeviceItem, ReducedDeviceListReply, RegisterBody, RegisterDeviceBody,
    RequestBody, RequestItem, RequestListReply, RequestState, ResetBody, RoleBody, SelfChatReply,
    ServerFrame, SessionListReply, SetupBody, SetupReply, SignedBody, StickerItem,
    StickerListReply, StickerReply, TimesReply, TopicItem, TopicOptionsBody, TopicReply,
    TopicsBody, UpdateAgentBody, VersionReply, WrapListReply,
};
use keryx_core::ulid::Ulid;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio::sync::{Notify, mpsc, oneshot, watch};
use tokio::time::Sleep;
use tower_http::services::{ServeDir, ServeFile};
use tracing::Instrument;

use crate::captcha;
use crate::chat::{
    Hub, Job, Out, Work, answer_of, encode, hand_welcomes, push, push_events, push_folders,
    push_lines,
};
use crate::file;
use crate::moq;
use crate::storage::sqlite::Sqlite;
use crate::storage::{
    self, AgentChange, AgentCreated, Answered, Asked, BuiltInRole, ChatAccess, ChatProfileSaved,
    KvChanged, Left, NamesSaved, NewAccount, NewAccountKeys, NewAgent, NewChatOptions,
    NewChatProfile, NewDevice, NewFile, NewGroup, NewInvitation, NewNames, NewPhoto, NewProfile,
    NewSession, NewSticker, NewTopicOptions, NewTopicProfile, ProfileSaved, Registered, Removed,
    RoleSet, Session, Storage, StoredFile, SyncFrom, TimesShown, TopicAccess, TopicSaved,
    TopicsPut,
};

pub const DIRECT_CREATE: &str = "direct.create";
pub const GROUP_CREATE: &str = "group.create";
pub const CHANNEL_CREATE: &str = "channel.create";
pub const DIRECTORY_SEARCH: &str = "directory.search";
pub const ACCOUNT_CREATE: &str = "account.create";
pub const STICKER_MANAGE: &str = "sticker.manage";
pub const PROFILE_EDIT: &str = "profile.edit";
pub const ACCOUNT_RENAME: &str = "account.rename";
pub const FOLDER_CREATE: &str = "folder.create";
pub const FOLDER_RENAME: &str = "folder.rename";
pub const FOLDER_REMOVE: &str = "folder.remove";
pub const FOLDER_REORDER: &str = "folder.reorder";
pub const FOLDER_DEFAULT: &str = "folder.default";
pub const FOLDER_CHATS: &str = "folder.chats";
pub const CHAT_PIN: &str = "chat.pin";
pub const PRESENCE_IN_APP: &str = "presence.in_app";
pub const PRESENCE_CONNECTED: &str = "presence.connected";
pub const AGENT_CREATE: &str = "agent.create";

/// The server-wide permission names, which server roles hold.
const SERVER_PERMISSION_LIST: &[&str] = &[
    DIRECT_CREATE,
    GROUP_CREATE,
    CHANNEL_CREATE,
    DIRECTORY_SEARCH,
    ACCOUNT_CREATE,
    STICKER_MANAGE,
    PROFILE_EDIT,
    ACCOUNT_RENAME,
    FOLDER_CREATE,
    FOLDER_RENAME,
    FOLDER_REMOVE,
    FOLDER_REORDER,
    FOLDER_DEFAULT,
    FOLDER_CHATS,
    CHAT_PIN,
    PRESENCE_IN_APP,
    PRESENCE_CONNECTED,
    AGENT_CREATE,
];

/// Server names the built-in admin alone holds.
const SERVER_ADMIN_ONLY: &[&str] = &[ACCOUNT_CREATE, STICKER_MANAGE];

// The names used inside a chat, without their prefix: `direct.`, `group.` or `channel.`, by the
// chat's type, chosen by the author. A check names one of these, and `check` puts the prefix on.
pub const SEND_UNENCRYPTED: &str = "message.send.unencrypted";
pub const MESSAGE_COMMIT: &str = "message.commit";
pub const EDIT_OWN: &str = "message.edit.own";
pub const DELETE_OWN: &str = "message.delete.own";
pub const MEMBER_ADD: &str = "member.add";
pub const MEMBER_REMOVE: &str = "member.remove";
pub const MEMBER_ROLE: &str = "member.role";
pub const DELETE_OTHERS: &str = "message.delete.others";
/// Seeing when members got and saw a message: your own messages, and others', chosen by the author.
pub const VIEW_RECEIVED_OWN: &str = "message.view_received.own";
pub const VIEW_SEEN_OWN: &str = "message.view_seen.own";
pub const VIEW_RECEIVED_OTHERS: &str = "message.view_received.others";
pub const VIEW_SEEN_OTHERS: &str = "message.view_seen.others";
pub const TOPIC_MANAGE: &str = "topic.manage";
pub const CHAT_PROFILE_EDIT: &str = "profile.edit";
pub const OPTIONS_EDIT: &str = "options.edit";
/// A topic's profile, options, closing and hiding: owners and moderators, and a topic's own
/// moderators there.
pub const TOPIC_EDIT: &str = "topic.edit";
/// Channels only: writing in the main list.
pub const MESSAGE_POST: &str = "message.post";

/// Every chat type's names. Members hold them all but unencrypted, which is the admin's alone, and
/// in groups and channels but `VIEWS_OF_OTHERS`, which owners and moderators hold there.
const CHAT_PERMISSION_LIST: &[&str] = &[
    "message.send.text",
    "message.send.voice",
    "message.send.reaction",
    "message.send.image",
    "message.send.music",
    "message.send.video",
    "message.send.doc",
    "message.send.file",
    "message.send.call",
    "message.send.media_bundle",
    "message.send.question",
    "message.send.answer",
    "message.send.sticker",
    "message.send.suggestion",
    "message.send.ack",
    "message.send.activity",
    SEND_UNENCRYPTED,
    MESSAGE_COMMIT,
    EDIT_OWN,
    DELETE_OWN,
    VIEW_RECEIVED_OWN,
    VIEW_SEEN_OWN,
    VIEW_RECEIVED_OTHERS,
    VIEW_SEEN_OTHERS,
];
const VIEWS_OF_OTHERS: &[&str] = &[VIEW_RECEIVED_OTHERS, VIEW_SEEN_OTHERS];

/// Groups' and channels' own names. Owners hold them all, and moderators all but these two.
const GROUP_PERMISSION_LIST: &[&str] = &[
    MEMBER_ADD,
    MEMBER_REMOVE,
    MEMBER_ROLE,
    DELETE_OTHERS,
    TOPIC_MANAGE,
    TOPIC_EDIT,
    CHAT_PROFILE_EDIT,
    OPTIONS_EDIT,
];

/// What a topic's own options may change of the member role, chosen by the author: reactions,
/// stickers, files and the four times permissions.
const TOPIC_OPTION_LIST: &[&str] = &[
    "message.send.reaction",
    "message.send.sticker",
    "message.send.voice",
    "message.send.image",
    "message.send.music",
    "message.send.video",
    "message.send.doc",
    "message.send.file",
    "message.send.media_bundle",
    VIEW_RECEIVED_OWN,
    VIEW_SEEN_OWN,
    VIEW_RECEIVED_OTHERS,
    VIEW_SEEN_OTHERS,
];

/// A topic's own answer to a member's permission there, before the chat's: allowed, refused, or
/// `None` to ask the chat. Moderators follow the chat's alone.
pub fn topic_says(
    rules: &Option<(TopicAccess, bool)>,
    chat_type: ChatType,
    name: &str,
) -> Option<bool> {
    let (rules, moderator) = rules.as_ref()?;
    if *moderator {
        return None;
    }
    let full = format!("{}.{name}", chat_prefix(chat_type));
    if rules.deny.contains(&full) {
        return Some(false);
    }
    if rules.allow.contains(&full) {
        return Some(true);
    }
    None
}
const OWNER_ONLY: &[&str] = &[MEMBER_ROLE, OPTIONS_EDIT];

/// The closed list of permission names, prefixed. A stored role holding any other name stops
/// startup, so a typo in a role cannot silently grant nothing.
pub fn permission_list() -> Vec<String> {
    let mut list = Vec::new();
    for name in SERVER_PERMISSION_LIST {
        list.push(name.to_string());
    }
    for prefix in ["direct", "group", "channel"] {
        for name in CHAT_PERMISSION_LIST {
            list.push(format!("{prefix}.{name}"));
        }
        if prefix != "direct" {
            for name in GROUP_PERMISSION_LIST {
                list.push(format!("{prefix}.{name}"));
            }
        }
    }
    list.push(format!("channel.{MESSAGE_POST}"));
    list
}

/// The prefix a chat type's permission names carry. A self chat is a direct one.
pub fn chat_prefix(chat_type: ChatType) -> &'static str {
    match chat_type {
        ChatType::SelfChat | ChatType::Direct => "direct",
        ChatType::Group => "group",
        ChatType::Channel => "channel",
    }
}

/// The permission to send one content kind, without its prefix.
pub fn send_permission(content_kind: ContentKind) -> &'static str {
    match content_kind {
        ContentKind::Text => "message.send.text",
        ContentKind::Voice => "message.send.voice",
        ContentKind::Reaction => "message.send.reaction",
        ContentKind::Image => "message.send.image",
        ContentKind::Music => "message.send.music",
        ContentKind::Video => "message.send.video",
        ContentKind::Doc => "message.send.doc",
        ContentKind::File => "message.send.file",
        ContentKind::Call => "message.send.call",
        ContentKind::MediaBundle => "message.send.media_bundle",
        ContentKind::Question => "message.send.question",
        ContentKind::Answer => "message.send.answer",
        ContentKind::Sticker => "message.send.sticker",
        ContentKind::Suggestion => "message.send.suggestion",
        ContentKind::Ack => "message.send.ack",
        ContentKind::Activity => "message.send.activity",
    }
}

const SUBPROTOCOL: &str = "keryx.v1";
const COOKIE_NAME: &str = "keryx_session";
const MAX_PASSWORD_CHARS: usize = 1024;

/// A first `sync` page starts this far before the client's `since`, for rows saved just before it
/// whose writes landed after the client read past them.
const SYNC_BACK_MS: i64 = 1000;
/// A challenge or a new device's id works for 5 minutes.
const CHALLENGE_MS: i64 = 5 * 60 * 1000;
const PURPOSE_RESET: i64 = 0;
const PURPOSE_DEVICE_LOGIN: i64 = 1;
const PURPOSE_DEVICE_ID: i64 = 2;
const PURPOSE_FOLDER_ID: i64 = 3;

/// A setup link works for 7 days.
const SETUP_LINK_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// Reconnecting within this extends the same row of the device's log.
const DEVICE_LOG_GRACE_MS: i64 = 60 * 1000;

/// The longest a shutdown takes, under Docker's default 10 before SIGKILL. Most of it is for
/// connections to close, and the rest for chats' tasks to finish.
const SHUTDOWN_CONNECTIONS: Duration = Duration::from_secs(6);
const SHUTDOWN_CHATS: Duration = Duration::from_secs(2);

/// The most a sealed presence text may be: 140 characters of up to 4 bytes each, sealed, with room to
/// spare. It is kept in memory beside the connection, so it is held small.
const PRESENCE_TEXT_BYTES: usize = 1024;

/// An agent's username ends in this, and a person's never does.
const AGENT_SUFFIX: &str = "_agent";

/// Every agent token's header, so a token naming another algorithm is refused.
const JWT_HEADER: &[u8] = br#"{"alg":"HS256","typ":"JWT"}"#;

/// How many frames of one connection may wait to be handled.
const REQUEST_QUEUE: usize = 64;

/// `server.toml`. Durations are whole seconds and sizes are bytes, after tanzim's validators.
#[derive(Deserialize)]
pub struct Settings {
    pub listen: SocketAddr,
    pub public_url: String,
    /// The name a client may show. Blank is the same as unset.
    #[serde(default)]
    pub name: Option<String>,
    pub max_devices: Option<u64>,
    pub database: DatabaseSettings,
    pub operator: Option<OperatorSettings>,
    pub agents: Option<AgentSettings>,
    #[serde(default)]
    pub registration: RegistrationSettings,
    #[serde(default)]
    pub session: SessionSettings,
    #[serde(default)]
    pub chat: ChatSettings,
    #[serde(default)]
    pub calls: CallSettings,
    pub www: WwwSettings,
    pub files: FilesSettings,
    #[serde(default)]
    pub limits: Limits,
    #[serde(default)]
    pub tcp: TcpSettings,
    pub tls: Option<TlsSettings>,
    pub captcha: Option<captcha::Settings>,
    #[serde(default)]
    pub stickers: StickerSettings,
}

/// `[stickers]`.
#[derive(Deserialize)]
#[serde(default)]
pub struct StickerSettings {
    /// Whether the bundled `Default` pack is listed. Off, its bytes are still served, so old messages
    /// keep showing their stickers.
    pub default_pack: bool,
}

impl Default for StickerSettings {
    fn default() -> Self {
        Self { default_pack: true }
    }
}

#[derive(Deserialize)]
pub struct DatabaseSettings {
    #[serde(rename = "type")]
    pub kind: String,
    pub path: PathBuf,
}

#[derive(Deserialize)]
pub struct OperatorSettings {
    pub token: String,
}

/// `[agents]`: the secret agents' tokens are signed with. Unset, agents are off.
#[derive(Deserialize)]
pub struct AgentSettings {
    pub secret: String,
}

#[derive(Deserialize)]
#[serde(default)]
pub struct RegistrationSettings {
    pub allow: bool,
    /// The shortest password. It also applies when a setup link sets one.
    pub password_min_length: usize,
    /// The shortest username. `keryx user add` does not use it.
    pub username_min_length: usize,
}

impl Default for RegistrationSettings {
    fn default() -> Self {
        Self {
            allow: false,
            password_min_length: 12,
            username_min_length: 3,
        }
    }
}

#[derive(Deserialize)]
#[serde(default)]
pub struct SessionSettings {
    pub lifetime: u64,
}

impl Default for SessionSettings {
    fn default() -> Self {
        Self {
            lifetime: 30 * 24 * 60 * 60,
        }
    }
}

#[derive(Deserialize)]
#[serde(default)]
pub struct ChatSettings {
    pub idle_after: u64,
    /// Whether a new group or channel has topics on when its creator does not say.
    pub topics: bool,
}

impl Default for ChatSettings {
    fn default() -> Self {
        Self {
            idle_after: 5 * 60,
            topics: false,
        }
    }
}

/// `[calls]`: for the tests only, left out of the template. The ring and the drop grace are fixed in
/// code, and these shorten them so a test does not wait a minute for a missed call.
#[derive(Deserialize, Default)]
#[serde(default)]
pub struct CallSettings {
    pub test_ring: Option<u64>,
    pub test_grace: Option<u64>,
}

#[derive(Deserialize)]
pub struct WwwSettings {
    pub directory: PathBuf,
}

/// `[files]`: where uploads' bytes live, a folder per chat.
#[derive(Deserialize)]
pub struct FilesSettings {
    pub directory: PathBuf,
}

#[derive(Deserialize)]
#[serde(default)]
pub struct Limits {
    pub websocket_frame: usize,
    pub rest_body: usize,
    pub history_page: u32,
    pub outgoing_frames: usize,
    pub ping_after: u64,
    pub ping_every: u64,
    pub ping_timeout: u64,
    pub key_packages: u64,
    pub call_frame: usize,
    pub call_cache: usize,
    /// An account's folders, not counting `all`.
    pub folders: usize,
    /// How many settings an account may keep.
    pub kv_keys: u64,
    /// A group's member accounts.
    pub group_members: u64,
    /// A channel's member accounts.
    pub channel_members: u64,
    /// A group's or channel's topics, General not counted.
    pub topics: u64,
    /// Seconds an invitation or a join request waits before it goes.
    pub invitation_lifetime: u64,
    /// The agents one owner may have, disabled ones not counted.
    pub agents: u64,
    pub body: BodyLimits,
    pub upload: UploadLimits,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            websocket_frame: 1024 * 1024,
            rest_body: 1024 * 1024,
            history_page: 200,
            outgoing_frames: 1024,
            ping_after: 5,
            ping_every: 15,
            ping_timeout: 5,
            key_packages: 100,
            call_frame: 256 * 1024,
            call_cache: 16 * 1024 * 1024,
            folders: 20,
            kv_keys: 64,
            group_members: 200,
            channel_members: 1000,
            topics: 100,
            invitation_lifetime: 30 * 24 * 60 * 60,
            agents: 5,
            body: BodyLimits::default(),
            upload: UploadLimits::default(),
        }
    }
}

#[derive(Deserialize)]
#[serde(default)]
pub struct BodyLimits {
    pub text: u64,
    pub reaction: u64,
    pub voice: u64,
    pub image: u64,
    pub music: u64,
    pub video: u64,
    pub doc: u64,
    pub file: u64,
    pub call: u64,
    pub media_bundle: u64,
    pub question: u64,
    pub answer: u64,
    pub sticker: u64,
    pub suggestion: u64,
    pub ack: u64,
    /// One step's intention, an agent's activity.
    pub activity: u64,
    /// An account's sealed draft in one chat, which is not a message.
    pub draft: u64,
    /// One sealed folder name.
    pub folder_name: u64,
    /// One setting's value.
    pub kv_value: u64,
}

impl Default for BodyLimits {
    fn default() -> Self {
        Self {
            text: 64 * 1024,
            reaction: 1024,
            voice: 16 * 1024,
            image: 16 * 1024,
            music: 16 * 1024,
            video: 16 * 1024,
            doc: 16 * 1024,
            file: 16 * 1024,
            call: 16 * 1024,
            media_bundle: 64 * 1024,
            question: 64 * 1024,
            answer: 16 * 1024,
            sticker: 1024,
            suggestion: 4 * 1024,
            ack: 1024,
            activity: 1024,
            draft: 64 * 1024,
            folder_name: 256,
            kv_value: 4 * 1024,
        }
    }
}

impl BodyLimits {
    pub fn of(&self, content_kind: ContentKind) -> u64 {
        match content_kind {
            ContentKind::Text => self.text,
            ContentKind::Voice => self.voice,
            ContentKind::Reaction => self.reaction,
            ContentKind::Image => self.image,
            ContentKind::Music => self.music,
            ContentKind::Video => self.video,
            ContentKind::Doc => self.doc,
            ContentKind::File => self.file,
            ContentKind::Call => self.call,
            ContentKind::MediaBundle => self.media_bundle,
            ContentKind::Question => self.question,
            ContentKind::Answer => self.answer,
            ContentKind::Sticker => self.sticker,
            ContentKind::Suggestion => self.suggestion,
            ContentKind::Ack => self.ack,
            ContentKind::Activity => self.activity,
        }
    }
}

/// `[limits.upload]`: one file's encrypted bytes, by the type its uploader declares.
#[derive(Deserialize)]
#[serde(default)]
pub struct UploadLimits {
    pub voice: u64,
    pub image: u64,
    pub music: u64,
    pub video: u64,
    pub doc: u64,
    pub file: u64,
    /// One sticker's WebP, which is not a chat's file.
    pub sticker: u64,
    /// One profile photo, sealed: an account's file, not a chat's.
    pub photo: u64,
}

impl Default for UploadLimits {
    fn default() -> Self {
        const MIB: u64 = 1024 * 1024;
        Self {
            voice: 50 * MIB,
            image: 20 * MIB,
            music: 200 * MIB,
            video: 2048 * MIB,
            doc: 100 * MIB,
            file: 2048 * MIB,
            sticker: 256 * 1024,
            photo: MIB,
        }
    }
}

impl UploadLimits {
    /// `None` for a type that is not uploaded.
    pub fn of(&self, kind: ContentKind) -> Option<u64> {
        match kind {
            ContentKind::Voice => Some(self.voice),
            ContentKind::Image => Some(self.image),
            ContentKind::Music => Some(self.music),
            ContentKind::Video => Some(self.video),
            ContentKind::Doc => Some(self.doc),
            ContentKind::File => Some(self.file),
            _ => None,
        }
    }
}

/// `[tcp]`, for every connection. What is unset stays as the operating system has it.
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct TcpSettings {
    pub send_timeout: u64,
    pub user_timeout: Option<u64>,
    pub nodelay: bool,
    pub keepalive_idle: Option<u64>,
    pub keepalive_interval: Option<u64>,
    pub keepalive_retries: Option<u32>,
    pub send_buffer: Option<usize>,
    pub receive_buffer: Option<usize>,
    pub backlog: i32,
}

impl Default for TcpSettings {
    fn default() -> Self {
        Self {
            send_timeout: 15,
            user_timeout: None,
            nodelay: true,
            keepalive_idle: None,
            keepalive_interval: None,
            keepalive_retries: None,
            send_buffer: None,
            receive_buffer: None,
            backlog: 1024,
        }
    }
}

#[derive(Deserialize)]
pub struct TlsSettings {
    pub certificate: PathBuf,
    pub key: PathBuf,
}

/// Everything an operation can reach. Cloning the `Arc` shares it.
pub struct App {
    pub settings: Settings,
    pub storage: Sqlite,
    pub hub: Hub,
    pub stopping: watch::Sender<bool>,
    /// The relay's one byte budget for cached groups, `[limits] call_cache`, shared by every call.
    pub call_pool: moq_net::cache::Pool,
    /// The bundled sticker pack, checked at startup.
    pub default_pack: file::DefaultPack,
}

#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    #[error("Could not open the database at {path:?}")]
    Database {
        path: PathBuf,
        #[source]
        source: storage::Error,
    },
    #[error("Could not prepare the database")]
    Prepare(#[source] storage::Error),
    #[error("A stored role holds the unknown permission {name:?}")]
    UnknownPermission { name: String },
    #[error("Could not make an id")]
    Id(#[source] keryx_core::ulid::Error),
    #[error("Could not read the TLS certificate or key")]
    Tls(#[source] std::io::Error),
    #[error("A bundled sticker is broken: {0}")]
    DefaultPack(String),
    #[error("Could not listen on {address}")]
    Listen {
        address: SocketAddr,
        #[source]
        source: std::io::Error,
    },
}

/// A refusal. `code` is for programs and `message` for people, and both say what is refused and
/// never how to un-refuse it.
#[derive(Debug)]
pub struct Refused {
    pub status: u16,
    pub code: &'static str,
    pub message: String,
    /// Only for `call_open`: the open call's sequence id, so the client joins that one.
    pub call_seq: Option<u64>,
    /// Only for `kv_stale`: the server's version of the settings, and its whole set.
    pub kv: Option<Box<KvReply>>,
    /// Only for `slow_mode`: the seconds until this member may write there again.
    pub seconds: Option<u64>,
    /// Only for `invite_needed`: the accounts that are added by invitation only.
    pub account_list: Option<Vec<Ulid>>,
}

pub fn refused(status: u16, code: &'static str, message: &str) -> Refused {
    Refused {
        status,
        code,
        message: message.to_string(),
        call_seq: None,
        kv: None,
        seconds: None,
        account_list: None,
    }
}

impl From<storage::Error> for Refused {
    fn from(error: storage::Error) -> Self {
        tracing::error!(msg = "Storage failed", error = ?error);
        refused(500, "internal", "Something failed on the server")
    }
}

pub fn new_id() -> Result<Ulid, Refused> {
    match Ulid::new() {
        Ok(id) => Ok(id),
        Err(error) => {
            tracing::error!(msg = "Could not make an id", error = %error);
            Err(refused(500, "internal", "Something failed on the server"))
        }
    }
}

/// Seconds from the configuration as milliseconds of a time.
pub fn millis(seconds: u64) -> i64 {
    i64::try_from(seconds.saturating_mul(1000)).unwrap_or(i64::MAX)
}

tokio::task_local! {
    /// The request or event being handled, so that every operation's span names it.
    pub static TRACE: Ulid;
}

pub fn current_trace() -> Ulid {
    if let Ok(trace) = TRACE.try_with(|trace| *trace) {
        return trace;
    }
    match Ulid::new() {
        Ok(trace) => trace,
        Err(_) => Ulid::from_bytes([0; 16]),
    }
}

/// One operation: its span, its timing, and the line saying how it went. Every surface shares it.
macro_rules! operation {
    ($name:literal, $op:literal, $body:expr) => {{
        let started = std::time::Instant::now();
        let span = tracing::info_span!(
            $name,
            op = $op,
            trace = %$crate::api::current_trace(),
            total_ms = tracing::field::Empty
        );
        let result: Result<_, $crate::api::Refused> =
            tracing::Instrument::instrument($body, span.clone()).await;
        let total_ms = started.elapsed().as_millis() as u64;
        span.record("total_ms", total_ms);
        span.in_scope(|| match &result {
            Ok(_) => tracing::info!(msg = "Operation finished", op = $op, total_ms = total_ms),
            Err(refusal) if refusal.status >= 500 => {
                tracing::error!(msg = "Operation failed", op = $op, code = refusal.code)
            }
            Err(refusal) => {
                tracing::warn!(msg = "Operation refused", op = $op, code = refusal.code)
            }
        });
        result
    }};
}
pub(crate) use operation;

/// Who is asking.
#[derive(Clone, Copy)]
pub enum Caller {
    Nobody,
    /// Holds `[operator] token`. Counts as admin, and has no account of its own.
    Operator,
    Session(Session),
}

impl Caller {
    fn session(&self) -> Result<Session, Refused> {
        match self {
            Caller::Session(session) => Ok(*session),
            Caller::Operator => Err(refused(403, "needs_account", "This needs an account")),
            Caller::Nobody => Err(refused(401, "not_logged_in", "You are not logged in")),
        }
    }
}

impl Session {
    /// The device this session is on.
    pub fn device(&self) -> Result<Ulid, Refused> {
        match self.device {
            Some(device) => Ok(device),
            None => Err(refused(403, "no_device", "This session has no device")),
        }
    }
}

/// What an operation requires.
pub enum Need {
    Anyone,
    /// Any session, even one with no device.
    Session,
    /// A registered device, of an account under its device limit.
    Device,
    /// A server permission. The operator and the admin hold them all.
    Server(&'static str),
    Member(Ulid),
    /// Membership is not a permission, so the admin needs it too. Then a permission in that chat,
    /// named without its prefix, which comes from the chat's type.
    MemberWith(Ulid, &'static str),
    /// The built-in server admin role, or the operator, for acting on someone else's account.
    Admin,
}

/// The one check every operation passes through.
pub async fn check(app: &App, caller: &Caller, need: Need) -> Result<(), Refused> {
    if matches!(need, Need::Anyone) {
        return Ok(());
    }
    if matches!(
        (caller, &need),
        (Caller::Operator, Need::Server(_) | Need::Admin)
    ) {
        return Ok(());
    }
    let session = caller.session()?;
    match need {
        Need::Anyone | Need::Session => Ok(()),
        Need::Device => {
            session.device()?;
            let usage = app
                .storage
                .device_usage(session.account, app.settings.max_devices)
                .await?;
            if let Some(limit) = usage.limit
                && usage.count > limit
            {
                return Err(Refused {
                    status: 403,
                    code: "device_limit",
                    message: format!("This account may have {limit} devices"),
                    call_seq: None,
                    kv: None,
                    seconds: None,
                    account_list: None,
                });
            }
            Ok(())
        }
        Need::Server(permission) => {
            if app
                .storage
                .server_permitted(session.account, permission)
                .await?
            {
                Ok(())
            } else {
                Err(refused(403, "no_permission", "You may not do this"))
            }
        }
        Need::Admin => {
            if app.storage.is_admin(session.account).await? {
                Ok(())
            } else {
                Err(refused(403, "no_permission", "You may not do this"))
            }
        }
        Need::Member(chat) => match app.storage.chat_access(chat, session.account).await? {
            Some(_) => Ok(()),
            None => Err(refused(404, "not_member", "You are not in that chat")),
        },
        Need::MemberWith(chat, permission) => {
            let Some(access) = app.storage.chat_access(chat, session.account).await? else {
                return Err(refused(404, "not_member", "You are not in that chat"));
            };
            let permission = format!("{}.{permission}", chat_prefix(access.chat_type));
            if app
                .storage
                .chat_permitted(chat, session.account, permission)
                .await?
            {
                Ok(())
            } else {
                Err(refused(403, "no_permission", "You may not do this"))
            }
        }
    }
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// 32 random bytes as text, and the hash that is all the server keeps.
fn new_token() -> Result<(String, [u8; 32]), Refused> {
    let mut bytes = [0u8; 32];
    if let Err(error) = getrandom::fill(&mut bytes) {
        tracing::error!(msg = "Could not read random bytes", error = %error);
        return Err(refused(500, "internal", "Something failed on the server"));
    }
    let token = Base64UrlUnpadded::encode_string(&bytes);
    let hash = sha256(token.as_bytes());
    Ok((token, hash))
}

fn fixed<const N: usize>(bytes: &Bytes, field: &str) -> Result<[u8; N], Refused> {
    match <[u8; N]>::try_from(bytes.0.as_slice()) {
        Ok(array) => Ok(array),
        Err(_) => Err(Refused {
            status: 400,
            code: "bad_body",
            message: format!("{field} must be {N} bytes"),
            call_seq: None,
            kv: None,
            seconds: None,
            account_list: None,
        }),
    }
}

/// A key already in the database, which the schema does not hold to a length.
fn stored_key<const N: usize>(bytes: &[u8]) -> Result<[u8; N], Refused> {
    match <[u8; N]>::try_from(bytes) {
        Ok(array) => Ok(array),
        Err(_) => Err(refused(
            500,
            "internal",
            "A stored key has the wrong length",
        )),
    }
}

/// An agent's username ends in `_agent`, and a person's never does, chosen by the author, so a name
/// always tells whether an account is an agent.
fn check_username(username: &str, min_length: Option<usize>, agent: bool) -> Result<(), Refused> {
    let mut valid = !username.is_empty() && username.ends_with(AGENT_SUFFIX) == agent;
    for (index, character) in username.chars().enumerate() {
        let allowed = character.is_ascii_lowercase()
            || character == '_'
            || (character.is_ascii_digit() && index > 0);
        if !allowed {
            valid = false;
        }
    }
    if let Some(min_length) = min_length
        && !(min_length..=32).contains(&username.len())
    {
        valid = false;
    }
    if valid {
        Ok(())
    } else {
        Err(refused(400, "bad_username", "That username is not allowed"))
    }
}

fn check_password(app: &App, password: &str) -> Result<(), Refused> {
    let length = password.chars().count();
    if length < app.settings.registration.password_min_length || length > MAX_PASSWORD_CHARS {
        return Err(refused(
            400,
            "bad_password",
            "That password is too short or too long",
        ));
    }
    Ok(())
}

/// Argon2id on the blocking pool, because it takes tens of milliseconds of CPU.
async fn hash_password(password: String) -> Result<String, Refused> {
    let hashed = tokio::task::spawn_blocking(move || {
        match Argon2::default().hash_password(password.as_bytes()) {
            Ok(hash) => Some(hash.to_string()),
            Err(_) => None,
        }
    })
    .await;
    match hashed {
        Ok(Some(hash)) => Ok(hash),
        _ => Err(refused(500, "internal", "Something failed on the server")),
    }
}

// -------------------------------------------------------------------------------------------------
// Accounts and logins
// -------------------------------------------------------------------------------------------------

pub async fn run_features(app: &App, caller: &Caller) -> Result<FeaturesReply, Refused> {
    check(app, caller, Need::Anyone).await?;
    Ok(FeaturesReply {
        registration: app.settings.registration.allow,
        captcha: app.settings.captcha.is_some(),
        name: app
            .settings
            .name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string),
    })
}

pub async fn run_register(
    app: &App,
    caller: &Caller,
    input: RegisterBody,
) -> Result<AccountReply, Refused> {
    check(app, caller, Need::Anyone).await?;
    if !app.settings.registration.allow {
        return Err(refused(
            403,
            "registration_closed",
            "Registration is closed",
        ));
    }
    captcha::check(app, input.captcha).await?;
    check_username(
        &input.username,
        Some(app.settings.registration.username_min_length),
        false,
    )?;
    check_password(app, &input.password)?;
    let id = new_id()?;
    let password_hash = hash_password(input.password).await?;
    let created = app
        .storage
        .create_account(NewAccount {
            id,
            username: input.username,
            display_name: input.display_name,
            password_hash: Some(password_hash),
            admin: false,
            all_folder: new_id()?,
            setup: None,
            now: Ulid::now(),
        })
        .await?;
    if !created {
        return Err(refused(409, "username_taken", "That username is taken"));
    }
    Ok(AccountReply { account: id })
}

/// `keryx user add`: an account with no password, and the one-time link where its person sets one.
pub async fn run_create_account(
    app: &App,
    caller: &Caller,
    input: CreateAccountBody,
) -> Result<CreatedAccountReply, Refused> {
    check(app, caller, Need::Server(ACCOUNT_CREATE)).await?;
    check_username(&input.username, None, false)?;
    let id = new_id()?;
    let (token, token_hash) = new_token()?;
    let now = Ulid::now();
    let created = app
        .storage
        .create_account(NewAccount {
            id,
            username: input.username,
            display_name: input.display_name,
            password_hash: None,
            admin: input.admin,
            all_folder: new_id()?,
            setup: Some((token_hash, now + SETUP_LINK_MS)),
            now,
        })
        .await?;
    if !created {
        return Err(refused(409, "username_taken", "That username is taken"));
    }
    let base = app.settings.public_url.trim_end_matches('/');
    tracing::info!(msg = "Created an account", account = %id, admin = input.admin);
    Ok(CreatedAccountReply {
        account: id,
        link: format!("{base}/setup#{token}"),
    })
}

pub async fn run_setup(
    app: &App,
    caller: &Caller,
    input: SetupBody,
) -> Result<SetupReply, Refused> {
    check(app, caller, Need::Anyone).await?;
    check_password(app, &input.password)?;
    let now = Ulid::now();
    let token_hash = sha256(input.token.as_bytes());
    let account = match app.storage.setup_link(token_hash, now).await? {
        storage::SetupLink::Ready(account) => account,
        link => return Err(refuse_link(link)),
    };
    let password_hash = hash_password(input.password).await?;
    if !app
        .storage
        .finish_setup(account.id, token_hash, password_hash, now)
        .await?
    {
        return Err(refuse_link(
            app.storage.setup_link(token_hash, Ulid::now()).await?,
        ));
    }
    Ok(SetupReply {
        account: account.id,
        username: account.username,
    })
}

/// The three ways a setup link is refused. Each says which one it was.
fn refuse_link(link: storage::SetupLink) -> Refused {
    match link {
        storage::SetupLink::Used => refused(404, "link_used", "This setup link was already used"),
        storage::SetupLink::Expired => {
            refused(404, "link_expired", "This setup link is older than 7 days")
        }
        storage::SetupLink::Unknown | storage::SetupLink::Ready(_) => {
            refused(404, "bad_link", "This is not a setup link")
        }
    }
}

/// A session, with what to tell the client, and its token for the route to hand over its own way.
pub async fn run_login(
    app: &App,
    caller: &Caller,
    input: LoginBody,
) -> Result<(LoginReply, String), Refused> {
    check(app, caller, Need::Anyone).await?;
    captcha::check(app, input.captcha).await?;
    let wrong = refused(401, "bad_login", "That username or password is wrong");
    if input.password.chars().count() > MAX_PASSWORD_CHARS {
        return Err(wrong);
    }
    let Some(account) = app.storage.account_by_username(input.username).await? else {
        return Err(wrong);
    };
    let Some(stored) = account.password_hash.clone() else {
        return Err(wrong);
    };
    let password = input.password;
    let verified = tokio::task::spawn_blocking(move || {
        Argon2::default()
            .verify_password(password.as_bytes(), stored.as_str())
            .is_ok()
    })
    .await;
    if !matches!(verified, Ok(true)) {
        return Err(wrong);
    }
    start_session(app, account.id, input.device).await
}

/// A session for an account whose credentials held, on the returning device it proved, if any.
async fn start_session(
    app: &App,
    account: Ulid,
    proof: Option<DeviceProof>,
) -> Result<(LoginReply, String), Refused> {
    let now = Ulid::now();
    let mut device = None;
    if let Some(proof) = proof {
        let refused_proof = refused(403, "bad_device_proof", "That device could not be proven");
        let Some(key) = app.storage.device_key(proof.id).await? else {
            return Err(refused_proof);
        };
        if key.account != account {
            return Err(refused_proof);
        }
        if key.ended {
            return Err(refused(403, "device_ended", "That device has ended"));
        }
        let challenge: [u8; 32] = fixed(&proof.challenge, "challenge")?;
        let signature: [u8; 64] = fixed(&proof.signature, "signature")?;
        if !app
            .storage
            .take_challenge(challenge.to_vec(), account, PURPOSE_DEVICE_LOGIN, now)
            .await?
        {
            return Err(refused_proof);
        }
        let public: [u8; 32] = stored_key(&key.public_key)?;
        if verify_device_login(&public, &account, &proof.id, &challenge, &signature).is_err() {
            return Err(refused_proof);
        }
        device = Some(proof.id);
    }
    let (token, token_hash) = new_token()?;
    app.storage
        .create_session(NewSession {
            id: new_id()?,
            token_hash,
            account,
            device,
            now,
            expires_at: now.saturating_add(millis(app.settings.session.lifetime)),
        })
        .await?;
    let reply = LoginReply {
        account,
        device,
        token: None,
    };
    Ok((reply, token))
}

pub async fn run_challenge(
    app: &App,
    caller: &Caller,
    input: ChallengeBody,
) -> Result<ChallengeReply, Refused> {
    check(app, caller, Need::Anyone).await?;
    let purpose = match input.purpose.as_str() {
        "password_reset" => PURPOSE_RESET,
        "device_login" => PURPOSE_DEVICE_LOGIN,
        _ => {
            return Err(refused(
                400,
                "bad_body",
                "That is not a challenge's purpose",
            ));
        }
    };
    // By username, or by account, which an agent reads from its token, chosen by the author.
    let found = match (input.username, input.account) {
        (Some(username), None) => app.storage.account_by_username(username).await?,
        (None, Some(account)) => app.storage.account_by_id(account).await?,
        _ => {
            return Err(refused(
                400,
                "bad_body",
                "A challenge names a username or an account",
            ));
        }
    };
    let Some(account) = found else {
        return Err(refused(
            404,
            "no_such_account",
            "That account does not exist",
        ));
    };
    let mut challenge = [0u8; 32];
    if let Err(error) = getrandom::fill(&mut challenge) {
        tracing::error!(msg = "Could not read random bytes", error = %error);
        return Err(refused(500, "internal", "Something failed on the server"));
    }
    let now = Ulid::now();
    app.storage
        .create_challenge(
            challenge.to_vec(),
            account.id,
            purpose,
            now + CHALLENGE_MS,
            now,
        )
        .await?;
    Ok(ChallengeReply {
        challenge: Bytes(challenge.to_vec()),
        account: account.id,
    })
}

pub async fn run_password_reset(
    app: &App,
    caller: &Caller,
    input: ResetBody,
) -> Result<EmptyReply, Refused> {
    check(app, caller, Need::Anyone).await?;
    check_password(app, &input.new_password)?;
    let denied = refused(403, "bad_signature", "That reset could not be proven");
    let Some(account) = app.storage.account_by_username(input.username).await? else {
        return Err(denied);
    };
    let Some(master) = &account.master_public else {
        return Err(denied);
    };
    let challenge: [u8; 32] = fixed(&input.challenge, "challenge")?;
    let signature: [u8; 64] = fixed(&input.signature, "signature")?;
    if !app
        .storage
        .take_challenge(challenge.to_vec(), account.id, PURPOSE_RESET, Ulid::now())
        .await?
    {
        return Err(denied);
    }
    let master: [u8; 32] = stored_key(master)?;
    if verify_password_reset(
        &master,
        &account.id,
        &challenge,
        &input.new_password,
        &signature,
    )
    .is_err()
    {
        return Err(denied);
    }
    let password_hash = hash_password(input.new_password).await?;
    app.storage.set_password(account.id, password_hash).await?;
    tracing::info!(msg = "Reset a password", account = %account.id);
    Ok(EmptyReply {})
}

pub async fn run_sessions(app: &App, caller: &Caller) -> Result<SessionListReply, Refused> {
    check(app, caller, Need::Session).await?;
    let session = caller.session()?;
    let session_list = app.storage.list_sessions(session.account).await?;
    Ok(SessionListReply { session_list })
}

pub async fn run_delete_session(
    app: &App,
    caller: &Caller,
    id: Ulid,
) -> Result<EmptyReply, Refused> {
    check(app, caller, Need::Session).await?;
    let session = caller.session()?;
    if !app.storage.delete_session(session.account, id).await? {
        return Err(refused(
            404,
            "no_such_session",
            "That session does not exist",
        ));
    }
    Ok(EmptyReply {})
}

/// The words' public keys, once, and the self chat in the same step, so a new device's notice always
/// has somewhere to go.
pub async fn run_account_keys(
    app: &App,
    caller: &Caller,
    input: AccountKeysBody,
) -> Result<SelfChatReply, Refused> {
    check(app, caller, Need::Session).await?;
    let session = caller.session()?;
    save_account_keys(app, session.account, input).await
}

/// `PUT /api/v1/accounts/{id}/keys`: the owner publishes its agent's keys, once, which its own core
/// made from the agent's words.
pub async fn run_agent_keys(
    app: &App,
    caller: &Caller,
    id: Ulid,
    input: AccountKeysBody,
) -> Result<SelfChatReply, Refused> {
    let (_, agent) = own_agent(app, caller, id).await?;
    save_account_keys(app, agent.id, input).await
}

async fn save_account_keys(
    app: &App,
    account: Ulid,
    input: AccountKeysBody,
) -> Result<SelfChatReply, Refused> {
    let master: [u8; 32] = fixed(&input.master_public, "master_public")?;
    let public: [u8; 32] = fixed(&input.account_public, "account_public")?;
    let stamp: [u8; 64] = fixed(&input.account_stamp, "account_stamp")?;
    let invite_public: [u8; 32] = fixed(&input.invite_public, "invite_public")?;
    let invite_stamp: [u8; 64] = fixed(&input.invite_stamp, "invite_stamp")?;
    if verify_account_key(&master, &account, &public, &stamp).is_err()
        || verify_invite_key(&master, &account, &invite_public, &invite_stamp).is_err()
    {
        return Err(refused(403, "bad_signature", "That stamp does not verify"));
    }
    let chat = new_id()?;
    let keys = NewAccountKeys {
        master_public: master.to_vec(),
        account_public: public.to_vec(),
        account_stamp: stamp.to_vec(),
        invite_public: invite_public.to_vec(),
        invite_stamp: invite_stamp.to_vec(),
    };
    let saved = app
        .storage
        .set_account_keys(account, keys, chat, Ulid::now())
        .await?;
    if !saved {
        return Err(refused(
            409,
            "keys_set",
            "This account already has its keys",
        ));
    }
    Ok(SelfChatReply { self_chat: chat })
}

pub async fn run_account(app: &App, caller: &Caller, id: Ulid) -> Result<AccountInfo, Refused> {
    check(app, caller, Need::Session).await?;
    let Some(account) = app.storage.account_by_id(id).await? else {
        return Err(refused(
            404,
            "no_such_account",
            "That account does not exist",
        ));
    };
    let mut reply = AccountInfo {
        id: account.id,
        username: account.username,
        display_name: account.display_name,
        master_public: None,
        account_public: None,
        account_stamp: None,
        invite_public: None,
        invite_stamp: None,
        agent: account.owner.is_some(),
        owner: account.owner,
    };
    if let (Some(master), Some(public), Some(stamp), Some(invite_public), Some(invite_stamp)) = (
        account.master_public,
        account.account_public,
        account.account_stamp,
        account.invite_public,
        account.invite_stamp,
    ) {
        reply.master_public = Some(Bytes(master));
        reply.account_public = Some(Bytes(public));
        reply.account_stamp = Some(Bytes(stamp));
        reply.invite_public = Some(Bytes(invite_public));
        reply.invite_stamp = Some(Bytes(invite_stamp));
    }
    Ok(reply)
}

pub async fn run_directory(
    app: &App,
    caller: &Caller,
    term: String,
) -> Result<AccountListReply, Refused> {
    check(app, caller, Need::Server(DIRECTORY_SEARCH)).await?;
    let account_list = app
        .storage
        .search_accounts(term.clone(), app.settings.limits.history_page)
        .await?;
    // Findable groups and channels show beside accounts, chosen by the author.
    let chat_list = app
        .storage
        .search_chats(term, app.settings.limits.history_page)
        .await?;
    Ok(AccountListReply {
        account_list,
        chat_list,
    })
}

// -------------------------------------------------------------------------------------------------
// Agents
// -------------------------------------------------------------------------------------------------

/// The `[agents]` secret, or `agents_off` when the server has none.
fn agent_secret(app: &App) -> Result<&str, Refused> {
    match &app.settings.agents {
        Some(agents) => Ok(agents.secret.as_str()),
        None => Err(refused(403, "agents_off", "This server has no agents")),
    }
}

fn hs256(secret: &str, bytes: &[u8]) -> Result<Hmac<Sha256>, Refused> {
    match Hmac::<Sha256>::new_from_slice(secret.as_bytes()) {
        Ok(mut mac) => {
            mac.update(bytes);
            Ok(mac)
        }
        Err(_) => Err(refused(500, "internal", "Something failed on the server")),
    }
}

/// An agent's token: a JWT under HS256 with `[agents] secret`, chosen by the author, with no `exp`.
fn agent_jwt(secret: &str, agent: Ulid, jti: &[u8; 16], now: i64) -> Result<String, Refused> {
    let claims = AgentClaims {
        sub: agent,
        jti: Base64UrlUnpadded::encode_string(jti),
        iat: now / 1000,
    };
    let Ok(payload) = serde_json::to_vec(&claims) else {
        return Err(refused(500, "internal", "Something failed on the server"));
    };
    let signed = format!(
        "{}.{}",
        Base64UrlUnpadded::encode_string(JWT_HEADER),
        Base64UrlUnpadded::encode_string(&payload)
    );
    let tag = hs256(secret, signed.as_bytes())?.finalize().into_bytes();
    Ok(format!(
        "{signed}.{}",
        Base64UrlUnpadded::encode_string(&tag)
    ))
}

/// The agent and token id a JWT names, once its header and signature hold.
fn read_agent_jwt(secret: &str, token: &str) -> Result<Option<(Ulid, [u8; 16])>, Refused> {
    let mut part_list = token.split('.');
    let (Some(header), Some(payload), Some(tag), None) = (
        part_list.next(),
        part_list.next(),
        part_list.next(),
        part_list.next(),
    ) else {
        return Ok(None);
    };
    let Ok(tag) = Base64UrlUnpadded::decode_vec(tag) else {
        return Ok(None);
    };
    let signed_length = header.len() + 1 + payload.len();
    let mac = hs256(secret, &token.as_bytes()[..signed_length])?;
    if mac.verify_slice(&tag).is_err() {
        return Ok(None);
    }
    if Base64UrlUnpadded::decode_vec(header).ok().as_deref() != Some(JWT_HEADER) {
        return Ok(None);
    }
    let Ok(payload) = Base64UrlUnpadded::decode_vec(payload) else {
        return Ok(None);
    };
    let Ok(claims) = serde_json::from_slice::<AgentClaims>(&payload) else {
        return Ok(None);
    };
    let Ok(jti) = Base64UrlUnpadded::decode_vec(&claims.jti) else {
        return Ok(None);
    };
    match <[u8; 16]>::try_from(jti.as_slice()) {
        Ok(jti) => Ok(Some((claims.sub, jti))),
        Err(_) => Ok(None),
    }
}

/// 16 random bytes for a token's id.
fn new_jti() -> Result<[u8; 16], Refused> {
    let mut jti = [0u8; 16];
    if let Err(error) = getrandom::fill(&mut jti) {
        tracing::error!(msg = "Could not read random bytes", error = %error);
        return Err(refused(500, "internal", "Something failed on the server"));
    }
    Ok(jti)
}

/// The caller's session and one of its agents, from a registered device of the owner.
async fn own_agent(
    app: &App,
    caller: &Caller,
    id: Ulid,
) -> Result<(Session, storage::Account), Refused> {
    check(app, caller, Need::Device).await?;
    let session = caller.session()?;
    let Some(agent) = app.storage.account_by_id(id).await? else {
        return Err(refused(
            404,
            "no_such_account",
            "That account does not exist",
        ));
    };
    if agent.owner != Some(session.account) {
        return Err(refused(403, "not_owner", "That is not your agent"));
    }
    Ok((session, agent))
}

/// The same, refusing an agent its owner disabled.
async fn running_agent(
    app: &App,
    caller: &Caller,
    id: Ulid,
) -> Result<(Session, storage::Account), Refused> {
    let (session, agent) = own_agent(app, caller, id).await?;
    if agent.disabled {
        return Err(refused(403, "agent_disabled", "That agent is disabled"));
    }
    Ok((session, agent))
}

/// Each ticked name is a server permission the owner holds, never `agent.create`, and `admin` needs
/// an admin owner.
async fn check_ticks(
    app: &App,
    owner: Ulid,
    permission_list: Option<&[String]>,
    admin: Option<bool>,
) -> Result<(), Refused> {
    for name in permission_list.unwrap_or_default() {
        if name == AGENT_CREATE || !SERVER_PERMISSION_LIST.contains(&name.as_str()) {
            return Err(refused(
                400,
                "bad_body",
                "An agent may be given only server permissions, never agent.create",
            ));
        }
        let Some(known) = SERVER_PERMISSION_LIST.iter().find(|known| **known == name) else {
            continue;
        };
        if !app.storage.server_permitted(owner, known).await? {
            return Err(refused(403, "not_held", "You do not hold that permission"));
        }
    }
    if admin == Some(true) && !app.storage.is_admin(owner).await? {
        return Err(refused(403, "not_held", "You are not admin"));
    }
    Ok(())
}

/// `POST /api/v1/agents`: an agent the caller owns, with no password, and its token shown once. Its
/// words are made by the owner's own core, never here.
pub async fn run_create_agent(
    app: &App,
    caller: &Caller,
    input: CreateAgentBody,
) -> Result<CreatedAgentReply, Refused> {
    let secret = agent_secret(app)?;
    check(app, caller, Need::Device).await?;
    check(app, caller, Need::Server(AGENT_CREATE)).await?;
    let session = caller.session()?;
    check_username(
        &input.username,
        Some(app.settings.registration.username_min_length),
        true,
    )?;
    let display_name = input.display_name.trim().to_string();
    let length = display_name.chars().count();
    if length == 0 || length > DISPLAY_NAME_CHARS {
        return Err(refused(
            400,
            "bad_display_name",
            "A display name is 1 to 64 characters",
        ));
    }
    check_ticks(
        app,
        session.account,
        Some(&input.permission_list),
        Some(input.admin),
    )
    .await?;
    let id = new_id()?;
    let jti = new_jti()?;
    let now = Ulid::now();
    let created = app
        .storage
        .create_agent(NewAgent {
            id,
            owner: session.account,
            username: input.username,
            display_name,
            admin: input.admin,
            permission_list: input.permission_list,
            jti,
            all_folder: new_id()?,
            limit: app.settings.limits.agents,
            now,
        })
        .await?;
    match created {
        AgentCreated::Saved => {}
        AgentCreated::UsernameTaken => {
            return Err(refused(409, "username_taken", "That username is taken"));
        }
        AgentCreated::TooMany => {
            return Err(Refused {
                status: 409,
                code: "too_many_agents",
                message: format!("One person may have {} agents", app.settings.limits.agents),
                call_seq: None,
                kv: None,
                seconds: None,
                account_list: None,
            });
        }
    }
    tracing::info!(msg = "Created an agent", account = %id, owner = %session.account);
    Ok(CreatedAgentReply {
        account: id,
        token: agent_jwt(secret, id, &jti, now)?,
    })
}

/// `GET /api/v1/agents`: the caller's own agents.
pub async fn run_agents(app: &App, caller: &Caller) -> Result<AgentListReply, Refused> {
    check(app, caller, Need::Device).await?;
    let session = caller.session()?;
    Ok(AgentListReply {
        agent_list: app.storage.agents(session.account).await?,
    })
}

/// `PUT /api/v1/agents/{id}`: its display name, its ticks and whether it is admin.
pub async fn run_update_agent(
    app: &App,
    caller: &Caller,
    id: Ulid,
    input: UpdateAgentBody,
) -> Result<EmptyReply, Refused> {
    let (session, agent) = running_agent(app, caller, id).await?;
    check_ticks(
        app,
        session.account,
        input.permission_list.as_deref(),
        input.admin,
    )
    .await?;
    let display_name = match input.display_name {
        Some(name) => {
            let name = name.trim().to_string();
            let length = name.chars().count();
            if length == 0 || length > DISPLAY_NAME_CHARS {
                return Err(refused(
                    400,
                    "bad_display_name",
                    "A display name is 1 to 64 characters",
                ));
            }
            Some(name)
        }
        None => None,
    };
    app.storage
        .change_agent(
            agent.id,
            AgentChange {
                permission_list: input.permission_list,
                admin: input.admin,
            },
        )
        .await?;
    if display_name.is_some() {
        let saved = app
            .storage
            .set_names(NewNames {
                account: agent.id,
                display_name,
                username: None,
            })
            .await?;
        if let NamesSaved::Saved {
            version,
            username,
            display_name,
        } = saved
        {
            profile_changed(app, agent.id, version, username, display_name).await?;
        }
    }
    tracing::info!(msg = "Changed an agent", account = %agent.id);
    Ok(EmptyReply {})
}

/// Checks the termination of the agent's one device, which its owner's core signed with the agent's
/// words, chosen by the author, answering the device and signature to end it with. An agent that
/// never registered a device needs no statement.
async fn agent_statement(
    app: &App,
    agent: &storage::Account,
    statement: Option<AgentStatement>,
) -> Result<Option<(Ulid, [u8; 64])>, Refused> {
    let live = app
        .storage
        .list_devices(agent.id)
        .await?
        .into_iter()
        .find(|device| device.terminated_at.is_none());
    let Some(device) = live else {
        return Ok(None);
    };
    let unproven = refused(
        403,
        "bad_signature",
        "Ending the agent's device needs its words",
    );
    let (Some(statement), Some(master)) = (statement, &agent.master_public) else {
        return Err(unproven);
    };
    if statement.device != device.id {
        return Err(unproven);
    }
    let signature: [u8; 64] = fixed(&statement.signature, "signature")?;
    let master: [u8; 32] = stored_key(master)?;
    if verify_revocation(&master, &agent.id, &device.id, &signature).is_err() {
        return Err(unproven);
    }
    Ok(Some((device.id, signature)))
}

/// Ends the agent's device, so its members remove it from MLS at once.
async fn end_agent_device(
    app: &App,
    agent: Ulid,
    ended: Option<(Ulid, [u8; 64])>,
) -> Result<(), Refused> {
    match ended {
        Some((device, signature)) => end_device(app, agent, device, signature).await,
        None => Ok(()),
    }
}

/// `POST /api/v1/agents/{id}/token`: a new token, which ends the old one, its sessions and its device
/// at once.
pub async fn run_renew_agent_token(
    app: &App,
    caller: &Caller,
    id: Ulid,
    input: AgentEndBody,
) -> Result<AgentTokenReply, Refused> {
    let secret = agent_secret(app)?;
    let (_, agent) = running_agent(app, caller, id).await?;
    let ended = agent_statement(app, &agent, input.statement).await?;
    end_agent_device(app, agent.id, ended).await?;
    let jti = new_jti()?;
    let now = Ulid::now();
    app.storage
        .end_agent_token(agent.id, Some(jti), false, now)
        .await?;
    tracing::info!(msg = "Renewed an agent's token", account = %agent.id);
    Ok(AgentTokenReply {
        token: agent_jwt(secret, agent.id, &jti, now)?,
    })
}

/// `DEL /api/v1/agents/{id}/token`: no token at all until a renew.
pub async fn run_revoke_agent_token(
    app: &App,
    caller: &Caller,
    id: Ulid,
    input: AgentEndBody,
) -> Result<EmptyReply, Refused> {
    let (_, agent) = running_agent(app, caller, id).await?;
    let ended = agent_statement(app, &agent, input.statement).await?;
    end_agent_device(app, agent.id, ended).await?;
    app.storage
        .end_agent_token(agent.id, None, false, Ulid::now())
        .await?;
    tracing::info!(msg = "Revoked an agent's token", account = %agent.id);
    Ok(EmptyReply {})
}

/// `POST /api/v1/agents/{id}/disable`: the token goes, its sessions and device end, and it leaves
/// every group and channel as a leave does. The account and device stay for old signatures.
pub async fn run_disable_agent(
    app: &App,
    caller: &Caller,
    id: Ulid,
    input: AgentEndBody,
) -> Result<EmptyReply, Refused> {
    let (_, agent) = running_agent(app, caller, id).await?;
    let ended = agent_statement(app, &agent, input.statement).await?;
    let chat_list = app
        .storage
        .end_agent_token(agent.id, None, true, Ulid::now())
        .await?;
    // A line names the device that left. An agent that never had one holds no keys, so it stays a
    // member of what it was added to, reading nothing.
    let newest = app
        .storage
        .list_devices(agent.id)
        .await?
        .into_iter()
        .max_by_key(|device| device.created_at);
    if let Some(device) = newest {
        for chat in chat_list {
            leave_as(app, chat, agent.id, device.id).await?;
        }
    }
    // After the leaves, so a member's commit removing the device no longer wraps for the agent.
    end_agent_device(app, agent.id, ended).await?;
    presence_moved(app, agent.id).await;
    tracing::info!(msg = "Disabled an agent", account = %agent.id);
    Ok(EmptyReply {})
}

/// `POST /api/v1/login/agent`: the agent's JWT in, an ordinary session out, chosen by the author, so
/// sessions are checked one way.
pub async fn run_login_agent(
    app: &App,
    caller: &Caller,
    input: AgentLoginBody,
) -> Result<(LoginReply, String), Refused> {
    check(app, caller, Need::Anyone).await?;
    let secret = agent_secret(app)?;
    let wrong = refused(401, "bad_login", "That token is not valid");
    let Some((agent, jti)) = read_agent_jwt(secret, &input.token)? else {
        return Err(wrong);
    };
    let Some(account) = app.storage.account_by_id(agent).await? else {
        return Err(wrong);
    };
    if account.owner.is_none() {
        return Err(wrong);
    }
    if account.disabled {
        return Err(refused(403, "agent_disabled", "That agent is disabled"));
    }
    let Some(live) = app.storage.agent_jti(agent).await? else {
        return Err(wrong);
    };
    // The id is not secret, so a plain comparison: the signature is what the secret guards.
    if live != jti {
        return Err(wrong);
    }
    start_session(app, agent, input.device).await
}

/// Only its owner ends an agent's device, through a renew, a revoke or disabling, chosen by the
/// author, so a thief holding a copied `agent.toml` cannot.
async fn refuse_agent_device(app: &App, account: Ulid) -> Result<(), Refused> {
    match app.storage.account_by_id(account).await? {
        Some(found) if found.owner.is_some() => Err(refused(
            403,
            "agent_device",
            "Only the agent's owner ends its device",
        )),
        _ => Ok(()),
    }
}

// -------------------------------------------------------------------------------------------------
// Devices
// -------------------------------------------------------------------------------------------------

/// A password-only session sees each device's platform and last-active day, enough to choose what to
/// terminate. IP addresses and logs need a registered device, so a stolen password does not reveal
/// location or activity history.
pub async fn run_devices(app: &App, caller: &Caller) -> Result<DevicesReply, Refused> {
    check(app, caller, Need::Session).await?;
    let session = caller.session()?;
    let device_list = app.storage.list_devices(session.account).await?;
    if session.device.is_some() {
        return Ok(DevicesReply::Full(DeviceListReply { device_list }));
    }
    let mut reduced = Vec::with_capacity(device_list.len());
    for device in &device_list {
        if device.terminated_at.is_none() {
            reduced.push(ReducedDeviceItem {
                id: device.id,
                platform: device.platform,
                last_active_day: device.last_active_at / (24 * 60 * 60 * 1000),
            });
        }
    }
    Ok(DevicesReply::Reduced(ReducedDeviceListReply {
        device_list: reduced,
    }))
}

/// A new device's id, which its stamp signs, so a client can never choose its own.
pub async fn run_device_id(app: &App, caller: &Caller) -> Result<DeviceReply, Refused> {
    check(app, caller, Need::Session).await?;
    let session = caller.session()?;
    let id = new_id()?;
    let now = Ulid::now();
    app.storage
        .create_challenge(
            id.bytes().to_vec(),
            session.account,
            PURPOSE_DEVICE_ID,
            now + CHALLENGE_MS,
            now,
        )
        .await?;
    Ok(DeviceReply { device: id })
}

pub async fn run_register_device(
    app: &App,
    caller: &Caller,
    input: RegisterDeviceBody,
    ip: String,
) -> Result<DeviceReply, Refused> {
    check(app, caller, Need::Session).await?;
    let session = caller.session()?;
    if session.device.is_some() {
        return Err(refused(
            409,
            "has_device",
            "This session already has a device",
        ));
    }
    if !matches!(input.platform, 0 | 1) {
        return Err(refused(400, "bad_body", "That is not a platform"));
    }
    let Some(account) = app.storage.account_by_id(session.account).await? else {
        return Err(refused(
            404,
            "no_such_account",
            "That account does not exist",
        ));
    };
    let Some(master) = &account.master_public else {
        return Err(refused(
            409,
            "words_not_set",
            "This account has no words yet",
        ));
    };
    let master: [u8; 32] = stored_key(master)?;
    let public: [u8; 32] = fixed(&input.public_key, "public_key")?;
    let stamp: [u8; 64] = fixed(&input.stamp, "stamp")?;
    if verify_device_stamp(&master, &session.account, &input.device, &public, &stamp).is_err() {
        return Err(refused(403, "bad_signature", "That stamp does not verify"));
    }
    let registered = app
        .storage
        .register_device(
            NewDevice {
                id: input.device,
                account: session.account,
                public_key: public.to_vec(),
                stamp: stamp.to_vec(),
                platform: input.platform,
                os: input.os,
                os_version: input.os_version,
                web_platform: input.web_platform,
                web_platform_version: input.web_platform_version,
                ip,
                now: app.storage.saved_now(),
            },
            session.id,
            app.settings.max_devices,
        )
        .await?;
    match registered {
        Registered::UnknownId => Err(refused(
            409,
            "unknown_device_id",
            "That device id was not handed out, or has expired",
        )),
        Registered::AtLimit(limit) => Err(Refused {
            status: 403,
            code: "device_limit",
            message: format!("This account may have {limit} devices"),
            call_seq: None,
            kv: None,
            seconds: None,
            account_list: None,
        }),
        Registered::Saved(notice) => {
            app.hub.deliver(
                &[session.account],
                Some(input.device),
                &ServerFrame::Message(notice),
            );
            Ok(DeviceReply {
                device: input.device,
            })
        }
    }
}

/// A device has ended: stop delivering to it at once, and tell everyone sharing a chat, whose clients
/// verify the statement and commit its removal.
async fn end_device(
    app: &App,
    account: Ulid,
    device: Ulid,
    statement: [u8; 64],
) -> Result<(), Refused> {
    let now = Ulid::now();
    if !app
        .storage
        .end_device(device, statement.to_vec(), now)
        .await?
    {
        return Ok(());
    }
    app.hub.close_device(account, device, 4403, "device_ended");
    let mut account_list = app.storage.accounts_sharing(account).await?;
    if !account_list.contains(&account) {
        account_list.push(account);
    }
    app.hub.deliver(
        &account_list,
        Some(device),
        &ServerFrame::Revoked {
            account,
            device,
            statement: Bytes(statement.to_vec()),
        },
    );
    tracing::info!(msg = "Ended a device", account = %account, device = %device);
    Ok(())
}

/// Needs the words, from any device: a stolen password or session cannot terminate anything.
pub async fn run_terminate(
    app: &App,
    caller: &Caller,
    device: Ulid,
    input: SignedBody,
) -> Result<EmptyReply, Refused> {
    check(app, caller, Need::Session).await?;
    let session = caller.session()?;
    refuse_agent_device(app, session.account).await?;
    let signature: [u8; 64] = fixed(&input.signature, "signature")?;
    let mine = app.storage.device_key(device).await?;
    let Some(key) = mine else {
        return Err(refused(404, "no_such_device", "That device does not exist"));
    };
    if key.account != session.account {
        return Err(refused(404, "no_such_device", "That device does not exist"));
    }
    let Some(account) = app.storage.account_by_id(session.account).await? else {
        return Err(refused(
            404,
            "no_such_account",
            "That account does not exist",
        ));
    };
    let Some(master) = &account.master_public else {
        return Err(refused(
            409,
            "words_not_set",
            "This account has no words yet",
        ));
    };
    let master: [u8; 32] = stored_key(master)?;
    if verify_revocation(&master, &session.account, &device, &signature).is_err() {
        return Err(refused(
            403,
            "bad_signature",
            "That statement does not verify",
        ));
    }
    end_device(app, session.account, device, signature).await?;
    Ok(EmptyReply {})
}

/// Signed by the device's own key. The client wipes its keys only after this answers.
pub async fn run_logout(
    app: &App,
    caller: &Caller,
    device: Ulid,
    input: SignedBody,
) -> Result<EmptyReply, Refused> {
    check(app, caller, Need::Session).await?;
    let session = caller.session()?;
    refuse_agent_device(app, session.account).await?;
    let signature: [u8; 64] = fixed(&input.signature, "signature")?;
    let Some(key) = app.storage.device_key(device).await? else {
        return Err(refused(404, "no_such_device", "That device does not exist"));
    };
    if key.account != session.account {
        return Err(refused(404, "no_such_device", "That device does not exist"));
    }
    let public: [u8; 32] = stored_key(&key.public_key)?;
    if verify_logout(&public, &session.account, &device, &signature).is_err() {
        return Err(refused(
            403,
            "bad_signature",
            "That statement does not verify",
        ));
    }
    end_device(app, session.account, device, signature).await?;
    Ok(EmptyReply {})
}

pub async fn run_key_packages(
    app: &App,
    caller: &Caller,
    input: KeyPackagesBody,
) -> Result<CountReply, Refused> {
    check(app, caller, Need::Device).await?;
    let device = caller.session()?.device()?;
    let mut package_list = Vec::with_capacity(input.body_list.len());
    for body in input.body_list {
        package_list.push((new_id()?, body.0));
    }
    let added = app
        .storage
        .add_key_packages(
            device,
            package_list,
            app.settings.limits.key_packages,
            Ulid::now(),
        )
        .await?;
    match added {
        Some(count) => Ok(CountReply { count }),
        None => Err(refused(
            409,
            "too_many_key_packages",
            "That is more key packages than one device may keep",
        )),
    }
}

/// One package per device of that account. A device that has run out is left out, and joins by
/// itself later. If all have run out, the chat cannot start yet.
pub async fn run_claim_key_packages(
    app: &App,
    caller: &Caller,
    account: Ulid,
    input: ClaimBody,
) -> Result<KeyPackageListReply, Refused> {
    check(app, caller, Need::Device).await?;
    // Adding to a group or channel: someone added by invitation only is refused before any of
    // their one-use packages goes, so the client invites them instead.
    if let Some(chat) = input.chat {
        let (_, access) = group_access(app, caller, chat).await?;
        check(app, caller, Need::MemberWith(chat, MEMBER_ADD)).await?;
        let member = app.storage.chat_access(chat, account).await?.is_some();
        if !member && invite_only(app, chat, account, access.chat_type).await? {
            let mut refusal = refused(
                403,
                "invite_needed",
                "That account is added by invitation only",
            );
            refusal.account_list = Some(vec![account]);
            return Err(refusal);
        }
    }
    let key_package_list = app.storage.claim_key_packages(account).await?;
    if key_package_list.is_empty() {
        return Err(refused(
            409,
            "no_key_packages",
            "That person cannot be added yet",
        ));
    }
    Ok(KeyPackageListReply { key_package_list })
}

// -------------------------------------------------------------------------------------------------
// Chats
// -------------------------------------------------------------------------------------------------

/// A display name's longest, in characters after trimming, fixed by the author, and a group's or
/// channel's name too.
const DISPLAY_NAME_CHARS: usize = 64;

/// A findable group's or channel's readable bio, at most an account's, fixed by the author.
const BIO_CHARS: usize = 140;

/// A join request's note, which the server reads, fixed by the author.
const NOTE_CHARS: usize = 140;

/// A findable group's or channel's readable name and bio, under an account's limits.
fn check_findable(name: Option<String>, bio: Option<String>) -> Result<FindableItem, Refused> {
    let Some(name) = name else {
        return Err(refused(400, "bad_body", "A findable chat needs its name"));
    };
    let name = name.trim().to_string();
    let length = name.chars().count();
    if length == 0 || length > DISPLAY_NAME_CHARS {
        return Err(refused(
            400,
            "bad_name",
            "That name is too short or too long",
        ));
    }
    if let Some(bio) = &bio
        && bio.chars().count() > BIO_CHARS
    {
        return Err(refused(400, "bad_bio", "That bio is too long"));
    }
    Ok(FindableItem { name, bio })
}

/// A direct chat with the pair, made if it is new, or a new group or channel with this account its
/// owner, alone in it until it adds people. If two people start a direct chat at once they get the
/// same id, and the first to upload epoch 0's group info wins, while the other waits for its
/// Welcome.
pub async fn run_create_chat(
    app: &App,
    caller: &Caller,
    input: CreateChatBody,
) -> Result<ChatReply, Refused> {
    check(app, caller, Need::Device).await?;
    let session = caller.session()?;
    if let Some(chat_type) = input.chat_type
        && matches!(chat_type, ChatType::Group | ChatType::Channel)
    {
        let permission = if chat_type == ChatType::Group {
            GROUP_CREATE
        } else {
            CHANNEL_CREATE
        };
        check(app, caller, Need::Server(permission)).await?;
        if input.account.is_some() {
            return Err(refused(
                400,
                "bad_body",
                "A group or channel starts with its owner alone",
            ));
        }
        let mut findable = None;
        if let Some(item) = input.findable {
            findable = Some(check_findable(Some(item.name), item.bio)?);
        }
        let topics_on = match input.topics {
            Some(on) => on,
            None => app.settings.chat.topics,
        };
        let chat = new_id()?;
        let (event, line) = app
            .storage
            .create_group(NewGroup {
                id: chat,
                chat_type,
                owner: session.account,
                device: session.device()?,
                topics_on,
                findable,
                now: Ulid::now(),
            })
            .await?;
        app.hub.deliver(
            &[session.account],
            session.device,
            &ServerFrame::ChatEvent(Box::new(event)),
        );
        push_lines(app, &[session.account], line.into_iter().collect());
        return Ok(ChatReply {
            chat,
            created: true,
        });
    }
    check(app, caller, Need::Server(DIRECT_CREATE)).await?;
    if input.topics.is_some() || input.findable.is_some() {
        return Err(refused(
            400,
            "bad_body",
            "Only a group or channel has topics or is findable",
        ));
    }
    let Some(account) = input.account else {
        return Err(refused(
            400,
            "bad_body",
            "A direct chat names its other person",
        ));
    };
    let Some(other) = app.storage.account_by_id(account).await? else {
        return Err(refused(
            404,
            "no_such_account",
            "That account does not exist",
        ));
    };
    if other.master_public.is_none() {
        return Err(refused(
            409,
            "words_not_set",
            "That person has no words yet",
        ));
    }
    let (low, high) = if session.account <= account {
        (session.account, account)
    } else {
        (account, session.account)
    };
    let (chat, created) = app
        .storage
        .direct_chat(new_id()?, low, high, Ulid::now())
        .await?;
    // The two now share a chat: each is told where the other is, on its announced devices, as when
    // a presence changes, chosen by the author. The chat already exists, so a failure is only
    // written down.
    if created && low != high {
        let told = async {
            for item in presence_now(app, vec![low, high]).await? {
                let viewer = if item.account == low { high } else { low };
                if !app.hub.announced(viewer) {
                    continue;
                }
                if let Some(seen) =
                    seen_by(app, &item, sight_of(app, viewer).await?, viewer).await?
                {
                    app.hub
                        .deliver_announced(viewer, &ServerFrame::Presence(Box::new(seen)));
                }
            }
            Ok::<(), Refused>(())
        }
        .await;
        if let Err(refusal) = told {
            tracing::error!(
                msg = "Could not tell a new chat's people where the other is",
                code = refusal.code
            );
        }
    }
    Ok(ChatReply { chat, created })
}

/// How long an invitation or a join request lasts, from `[limits] invitation_lifetime`.
fn invitation_ms(app: &App) -> i64 {
    millis(app.settings.limits.invitation_lifetime)
}

/// A registered device's account in a group or channel, with what its check needs.
async fn group_access(
    app: &App,
    caller: &Caller,
    chat: Ulid,
) -> Result<(Session, ChatAccess), Refused> {
    check(app, caller, Need::Device).await?;
    let session = caller.session()?;
    let Some(access) = app.storage.chat_access(chat, session.account).await? else {
        return Err(refused(404, "not_member", "You are not in that chat"));
    };
    if !matches!(access.chat_type, ChatType::Group | ChatType::Channel) {
        return Err(refused(400, "not_group", "That is not a group or channel"));
    }
    Ok((session, access))
}

/// Who may let a request in: every member holding `member.add` there.
async fn adders(app: &App, chat: Ulid, chat_type: ChatType) -> Result<Vec<Ulid>, Refused> {
    let permission = format!("{}.{MEMBER_ADD}", chat_prefix(chat_type));
    let mut list = Vec::new();
    for account in app.storage.member_accounts(chat).await? {
        if app
            .storage
            .chat_permitted(chat, account, permission.clone())
            .await?
        {
            list.push(account);
        }
    }
    Ok(list)
}

/// A whole chat gone for everyone: its members told, out of their folders, and its files' bytes
/// removed.
async fn chat_gone(app: &App, chat: Ulid, actor: Ulid, removed: Removed) -> Result<(), Refused> {
    app.hub.deliver(
        &removed.member_list,
        None,
        &ServerFrame::ChatEvent(Box::new(ChatEvent {
            chat,
            n: 0,
            actor,
            action: ChatAction::ChatRemoved,
            target: None,
            seq: None,
            topic: None,
            audience: Audience::Everyone,
            server_at: Ulid::now(),
            copy_list: Vec::new(),
        })),
    );
    push_folders(app, &removed.member_list).await?;
    // Their requests and invitations went with the chat, so their rows go too.
    for account in removed.asker_list {
        app.hub.deliver(
            &[account],
            None,
            &ServerFrame::JoinRequest(Box::new(RequestItem {
                chat,
                account,
                name: None,
                note: None,
                state: RequestState::Gone,
                at: Ulid::now(),
            })),
        );
    }
    app.hub.deliver(
        &removed.invitee_list,
        None,
        &ServerFrame::Invitation {
            chat,
            invitation: None,
        },
    );
    for file in removed.file_list {
        file::remove(app, chat, file).await;
    }
    tracing::info!(msg = "Removed a chat", chat = %chat);
    Ok(())
}

/// Any owner removes a whole group or channel for everyone, with no named permission, since
/// ownership is in MLS. Admin can too.
pub async fn run_chat_remove(
    app: &App,
    caller: &Caller,
    chat: Ulid,
) -> Result<EmptyReply, Refused> {
    let (session, access) = group_access(app, caller, chat).await?;
    if access.role != ChatRole::Owner && !app.storage.is_admin(session.account).await? {
        return Err(refused(403, "no_permission", "You may not do this"));
    }
    let Some(removed) = app.storage.remove_chat(chat).await? else {
        return Err(refused(404, "not_member", "You are not in that chat"));
    };
    chat_gone(app, chat, session.account, removed).await?;
    Ok(EmptyReply {})
}

/// Leaving takes effect at once: no more delivery, out of the account's folders, and a "left" line,
/// after which the next member online commits the MLS removal. The last owner leaving removes the
/// whole chat.
pub async fn run_leave(app: &App, caller: &Caller, chat: Ulid) -> Result<EmptyReply, Refused> {
    let (session, _) = group_access(app, caller, chat).await?;
    leave_as(app, chat, session.account, session.device()?).await?;
    Ok(EmptyReply {})
}

/// One account leaves a group or channel, as the device named.
async fn leave_as(app: &App, chat: Ulid, account: Ulid, device: Ulid) -> Result<(), Refused> {
    match app
        .storage
        .leave_chat(chat, account, device, Ulid::now())
        .await?
    {
        Left::NotMember => Err(refused(404, "not_member", "You are not in that chat")),
        Left::Left {
            event,
            line,
            mut member_list,
        } => {
            push_lines(
                app,
                &member_list,
                line.into_iter().map(|line| *line).collect(),
            );
            member_list.push(account);
            push_events(app, chat, &member_list, vec![*event]).await?;
            push_folders(app, &[account]).await?;
            Ok(())
        }
        Left::Removed(removed) => chat_gone(app, chat, account, removed).await,
    }
}

/// Makes a member an owner, a moderator or a plain member. Only owners hold `member.role`, and the
/// last owner cannot step down. The server alone keeps roles, which clients trust.
pub async fn run_chat_role(
    app: &App,
    caller: &Caller,
    chat: Ulid,
    account: Ulid,
    role: ChatRole,
) -> Result<EmptyReply, Refused> {
    let (session, _) = group_access(app, caller, chat).await?;
    check(app, caller, Need::MemberWith(chat, MEMBER_ROLE)).await?;
    match app
        .storage
        .set_role(
            chat,
            session.account,
            session.device()?,
            account,
            role,
            Ulid::now(),
        )
        .await?
    {
        RoleSet::NotMember => Err(refused(
            404,
            "not_member",
            "That account is not in this chat",
        )),
        RoleSet::LastOwner => Err(refused(
            409,
            "last_owner",
            "A group or channel always keeps an owner",
        )),
        RoleSet::Set {
            event,
            line,
            member_list,
        } => {
            push_events(app, chat, &member_list, vec![*event]).await?;
            push_lines(
                app,
                &member_list,
                line.into_iter().map(|line| *line).collect(),
            );
            Ok(EmptyReply {})
        }
    }
}

/// Asks to join a findable group or channel, with a note the server reads. Every device of the
/// asker, the asking one too, and those who may let it in are told, chosen by the author, so the
/// asking device's chat list shows the request with the name the server keeps.
pub async fn run_join_request(
    app: &App,
    caller: &Caller,
    chat: Ulid,
    note: Option<String>,
) -> Result<EmptyReply, Refused> {
    check(app, caller, Need::Device).await?;
    let session = caller.session()?;
    let mut kept = None;
    if let Some(note) = note {
        let note = note.trim().to_string();
        if note.chars().count() > NOTE_CHARS {
            return Err(refused(400, "bad_note", "That note is too long"));
        }
        if !note.is_empty() {
            kept = Some(note);
        }
    }
    let now = Ulid::now();
    let item = match app
        .storage
        .ask_to_join(chat, session.account, kept, now, now - invitation_ms(app))
        .await?
    {
        Asked::NotFindable => {
            return Err(refused(
                404,
                "not_findable",
                "There is no such findable chat",
            ));
        }
        Asked::AlreadyMember => {
            return Err(refused(
                409,
                "already_member",
                "You are already in that chat",
            ));
        }
        Asked::Saved(item) => item,
    };
    let Some(chat_type) = app.storage.chat_type(chat).await? else {
        return Err(refused(
            404,
            "not_findable",
            "There is no such findable chat",
        ));
    };
    let mut told = adders(app, chat, chat_type).await?;
    told.push(session.account);
    app.hub
        .deliver(&told, None, &ServerFrame::JoinRequest(item));
    Ok(EmptyReply {})
}

/// Takes back this account's request, telling every device of the asker, the taking one too, and
/// those who may let it in.
pub async fn run_join_cancel(
    app: &App,
    caller: &Caller,
    chat: Ulid,
) -> Result<EmptyReply, Refused> {
    check(app, caller, Need::Device).await?;
    let session = caller.session()?;
    if !app.storage.cancel_request(chat, session.account).await? {
        return Err(refused(404, "no_request", "There is no such request"));
    }
    let mut told = vec![session.account];
    if let Some(chat_type) = app.storage.chat_type(chat).await? {
        told.extend(adders(app, chat, chat_type).await?);
    }
    app.hub.deliver(
        &told,
        None,
        &ServerFrame::JoinRequest(Box::new(RequestItem {
            chat,
            account: session.account,
            name: None,
            note: None,
            state: RequestState::Gone,
            at: Ulid::now(),
        })),
    );
    Ok(EmptyReply {})
}

/// Declines someone's request, by one who may let it in. The asker may ask again.
pub async fn run_join_decline(
    app: &App,
    caller: &Caller,
    chat: Ulid,
    account: Ulid,
) -> Result<EmptyReply, Refused> {
    let (session, access) = group_access(app, caller, chat).await?;
    check(app, caller, Need::MemberWith(chat, MEMBER_ADD)).await?;
    let now = Ulid::now();
    let Some(item) = app
        .storage
        .decline_request(chat, account, now, now - invitation_ms(app))
        .await?
    else {
        return Err(refused(404, "no_request", "There is no such request"));
    };
    let mut told = adders(app, chat, access.chat_type).await?;
    told.push(account);
    app.hub.deliver(
        &told,
        session.device,
        &ServerFrame::JoinRequest(Box::new(item)),
    );
    Ok(EmptyReply {})
}

/// A chat's waiting requests, for those who may let them in.
pub async fn run_requests(
    app: &App,
    caller: &Caller,
    chat: Ulid,
) -> Result<RequestListReply, Refused> {
    group_access(app, caller, chat).await?;
    check(app, caller, Need::MemberWith(chat, MEMBER_ADD)).await?;
    let request_list = app
        .storage
        .chat_requests(chat, Ulid::now() - invitation_ms(app))
        .await?;
    Ok(RequestListReply { request_list })
}

/// Invites someone whose switch turns every add into an invitation. The server keeps the sealed
/// part and the inviting device's signed statement, and tells the invited account's devices.
pub async fn run_invite(
    app: &App,
    caller: &Caller,
    chat: Ulid,
    input: InviteBody,
) -> Result<EmptyReply, Refused> {
    let (session, access) = group_access(app, caller, chat).await?;
    check(app, caller, Need::MemberWith(chat, MEMBER_ADD)).await?;
    let device = session.device()?;
    let signature: [u8; 64] = fixed(&input.signature, "signature")?;
    if app.storage.account_by_id(input.account).await?.is_none() {
        return Err(refused(
            404,
            "no_such_account",
            "That account does not exist",
        ));
    }
    let Some(key) = app.storage.device_key(device).await? else {
        return Err(refused(404, "no_such_device", "That device does not exist"));
    };
    let public: [u8; 32] = stored_key(&key.public_key)?;
    if verify_invite(&public, &chat, &input.account, &signature).is_err() {
        return Err(refused(
            403,
            "bad_signature",
            "That invitation is not signed",
        ));
    }
    let now = Ulid::now();
    if !app
        .storage
        .invite(NewInvitation {
            chat,
            account: input.account,
            inviter: device,
            sealed: input.sealed.0.clone(),
            signature: signature.to_vec(),
            now,
        })
        .await?
    {
        return Err(refused(
            409,
            "already_member",
            "That account is already in this chat",
        ));
    }
    app.hub.deliver(
        &[input.account],
        None,
        &ServerFrame::Invitation {
            chat,
            invitation: Some(InvitationItem {
                chat,
                chat_type: access.chat_type,
                inviter: session.account,
                device,
                sealed: input.sealed,
                signature: Bytes(signature.to_vec()),
                at: now,
            }),
        },
    );
    Ok(EmptyReply {})
}

/// Accepts or declines this account's invitation. Accepting makes it a member at once, and its
/// device joins by external commit. Declining tells the inviter alone.
pub async fn run_invite_answer(
    app: &App,
    caller: &Caller,
    chat: Ulid,
    accept: bool,
) -> Result<EmptyReply, Refused> {
    check(app, caller, Need::Device).await?;
    let session = caller.session()?;
    let now = Ulid::now();
    let expire_before = now - invitation_ms(app);
    let mut chat_type = None;
    for item in app
        .storage
        .own_invitations(session.account, expire_before)
        .await?
    {
        if item.chat == chat {
            chat_type = Some(item.chat_type);
        }
    }
    let Some(chat_type) = chat_type else {
        return Err(refused(404, "no_invitation", "There is no such invitation"));
    };
    let limit = if chat_type == ChatType::Group {
        app.settings.limits.group_members
    } else {
        app.settings.limits.channel_members
    };
    match app
        .storage
        .answer_invitation(
            chat,
            session.account,
            accept.then_some(session.device()?),
            limit,
            now,
            expire_before,
        )
        .await?
    {
        Answered::NoInvitation => Err(refused(404, "no_invitation", "There is no such invitation")),
        Answered::Full => Err(refused(409, "chat_full", "This chat has no room for you")),
        Answered::Saved {
            invitation,
            event,
            line,
        } => {
            if accept {
                let member_list = app.storage.member_accounts(chat).await?;
                push_events(app, chat, &member_list, vec![*event]).await?;
                push_lines(
                    app,
                    &member_list,
                    line.into_iter().map(|line| *line).collect(),
                );
            } else {
                app.hub
                    .deliver(&[invitation.inviter], None, &ServerFrame::ChatEvent(event));
            }
            app.hub.deliver(
                &[session.account],
                session.device,
                &ServerFrame::Invitation {
                    chat,
                    invitation: None,
                },
            );
            Ok(EmptyReply {})
        }
    }
}

/// Every member of a group or channel with its role, for any member, channel subscribers too: the
/// group's tree names every device anyway.
pub async fn run_members(
    app: &App,
    caller: &Caller,
    chat: Ulid,
) -> Result<MemberListReply, Refused> {
    group_access(app, caller, chat).await?;
    let member_list = app
        .storage
        .member_roles(chat)
        .await?
        .into_iter()
        .map(|(account, role)| MemberItem { account, role })
        .collect();
    Ok(MemberListReply { member_list })
}

/// A new topic's id, kept for this account for 5 minutes, so its name can be sealed with it.
pub async fn run_topic_id(app: &App, caller: &Caller, chat: Ulid) -> Result<TopicReply, Refused> {
    let (session, _) = group_access(app, caller, chat).await?;
    check(app, caller, Need::MemberWith(chat, TOPIC_MANAGE)).await?;
    let topic = new_id()?;
    app.storage
        .issue_topic(chat, session.account, topic, Ulid::now() + CHALLENGE_MS)
        .await?;
    Ok(TopicReply { topic })
}

/// Whether topics show, and the live topics in order. One left out is removed with every message
/// in it, for everyone.
pub async fn run_topics(
    app: &App,
    caller: &Caller,
    chat: Ulid,
    topics: TopicsBody,
) -> Result<EmptyReply, Refused> {
    let (session, _) = group_access(app, caller, chat).await?;
    check(app, caller, Need::MemberWith(chat, TOPIC_MANAGE)).await?;
    for (index, topic) in topics.topic_list.iter().enumerate() {
        if topics.topic_list[..index].contains(topic) {
            return Err(refused(400, "bad_body", "A topic is named twice"));
        }
    }
    let event_list = match app
        .storage
        .put_topics(
            chat,
            session.account,
            topics.on,
            topics.topic_list.clone(),
            app.settings.limits.topics,
            Ulid::now(),
        )
        .await?
    {
        TopicsPut::Saved { event_list, .. } => event_list,
        TopicsPut::UnknownTopic => {
            return Err(refused(
                404,
                "unknown_topic",
                "That topic is not one of this chat's",
            ));
        }
        TopicsPut::TooMany => {
            return Err(refused(
                409,
                "too_many_topics",
                "This chat has as many topics as it may",
            ));
        }
    };
    let member_list = app.storage.member_accounts(chat).await?;
    app.hub.deliver(
        &member_list,
        session.device,
        &ServerFrame::Topics { chat, topics },
    );
    push_events(app, chat, &member_list, event_list).await?;
    Ok(EmptyReply {})
}

/// A group's or channel's sealed profile, with its topics' names, and the chat file it names as its
/// photo, kept from now on. The photo it replaced goes.
pub async fn run_chat_profile(
    app: &App,
    caller: &Caller,
    chat: Ulid,
    profile: ChatProfileBody,
) -> Result<EmptyReply, Refused> {
    let (session, _) = group_access(app, caller, chat).await?;
    check(app, caller, Need::MemberWith(chat, CHAT_PROFILE_EDIT)).await?;
    let profile = new_chat_profile(app, &session, chat, profile)?;
    let (item, removed) = match app.storage.put_chat_profile(profile).await? {
        ChatProfileSaved::Saved { item, removed } => (item, removed),
        ChatProfileSaved::NoSuchPhoto => return Err(no_such_photo()),
        ChatProfileSaved::NotStarted => {
            return Err(refused(
                409,
                "no_group_info",
                "That chat has no profile until its epoch 0 is up",
            ));
        }
    };
    let member_list = app.storage.member_accounts(chat).await?;
    // The device that saved it gets it too, so it shows the server's copy, chosen by the author.
    chat_profile_kept(app, chat, &member_list, None, item, removed).await;
    Ok(EmptyReply {})
}

/// A sealed profile from this session's device, to keep.
pub fn new_chat_profile(
    app: &App,
    session: &Session,
    chat: Ulid,
    profile: ChatProfileBody,
) -> Result<NewChatProfile, Refused> {
    let device = session.device()?;
    let signature: [u8; 64] = fixed(&profile.signature, "signature")?;
    Ok(NewChatProfile {
        chat,
        device,
        account: session.account,
        sealed: profile.sealed.0,
        signature: signature.to_vec(),
        photo: profile.photo,
        photo_limit: app.settings.limits.upload.photo,
        now: Ulid::now(),
    })
}

pub fn no_such_photo() -> Refused {
    refused(404, "no_such_file", "That photo does not exist")
}

/// A kept profile reaches the members' online devices but `from`, every one when `from` is none,
/// and the photos it no longer names go.
pub async fn chat_profile_kept(
    app: &App,
    chat: Ulid,
    member_list: &[Ulid],
    from: Option<Ulid>,
    item: ChatProfileItem,
    removed: Vec<Ulid>,
) {
    for file in removed {
        file::remove(app, chat, file).await;
    }
    app.hub.deliver(
        member_list,
        from,
        &ServerFrame::ChatProfile {
            chat,
            profile: item,
        },
    );
}

/// Slow mode's choices, in seconds, chosen by the author.
const SLOW_MODE_LIST: &[u64] = &[10, 30, 60, 300, 900, 3600];

/// What a group's or channel's members may do beyond or short of the built-in member role, by
/// owners. Names are a chat's own, without their prefix.
pub async fn run_chat_options(
    app: &App,
    caller: &Caller,
    chat: Ulid,
    options: ChatOptions,
) -> Result<EmptyReply, Refused> {
    let (session, access) = group_access(app, caller, chat).await?;
    check(app, caller, Need::MemberWith(chat, OPTIONS_EDIT)).await?;
    let checked = new_chat_options(access.chat_type, &options)?;
    app.storage.put_chat_options(chat, checked).await?;
    let member_list = app.storage.member_accounts(chat).await?;
    app.hub.deliver(
        &member_list,
        session.device,
        &ServerFrame::ChatOptions { chat, options },
    );
    Ok(EmptyReply {})
}

/// A chat's options checked, with their names prefixed by its type.
pub fn new_chat_options(
    chat_type: ChatType,
    options: &ChatOptions,
) -> Result<NewChatOptions, Refused> {
    if let Some(seconds) = options.slow_mode
        && !SLOW_MODE_LIST.contains(&seconds)
    {
        return Err(refused(400, "bad_body", "That is not a slow mode"));
    }
    let prefix = chat_prefix(chat_type);
    let mut allow = Vec::with_capacity(options.allow.len());
    let mut deny = Vec::with_capacity(options.deny.len());
    for (name, allowed) in options
        .allow
        .iter()
        .map(|name| (name, true))
        .chain(options.deny.iter().map(|name| (name, false)))
    {
        let known = CHAT_PERMISSION_LIST.contains(&name.as_str())
            || GROUP_PERMISSION_LIST.contains(&name.as_str())
            || (chat_type == ChatType::Channel && name == MESSAGE_POST);
        if !known {
            return Err(refused(
                400,
                "unknown_permission",
                "That is not a permission",
            ));
        }
        // Options change what members may do, never hand them an owner's or the admin's power.
        if allowed && (OWNER_ONLY.contains(&name.as_str()) || name == SEND_UNENCRYPTED) {
            return Err(refused(403, "no_permission", "Members may not hold that"));
        }
        let full = format!("{prefix}.{name}");
        if allow.contains(&full) || deny.contains(&full) {
            return Err(refused(400, "bad_body", "A permission is named twice"));
        }
        if allowed {
            allow.push(full);
        } else {
            deny.push(full);
        }
    }
    Ok(NewChatOptions {
        allow,
        deny,
        slow_mode: options.slow_mode,
    })
}

/// The topic an editor names, which must be live and, when hidden, theirs to see: its moderators
/// and the chat's owners and moderators, through `topic.edit`, change its settings.
async fn topic_editor(
    app: &App,
    caller: &Caller,
    session: &Session,
    access: &ChatAccess,
    chat: Ulid,
    topic: Ulid,
) -> Result<TopicAccess, Refused> {
    let Some(rules) = app
        .storage
        .topic_access(chat, topic, session.account)
        .await?
    else {
        return Err(no_such_topic());
    };
    let moderator = access.role != ChatRole::Member || rules.moderator;
    if rules.hidden && !moderator {
        return Err(no_such_topic());
    }
    if !rules.moderator {
        check(app, caller, Need::MemberWith(chat, TOPIC_EDIT)).await?;
    }
    Ok(rules)
}

fn no_such_topic() -> Refused {
    refused(404, "no_such_topic", "That topic does not exist")
}

/// A topic's own options, by its moderators and the chat's owners and moderators. General cannot
/// be hidden, as it cannot be removed.
pub async fn run_topic_options(
    app: &App,
    caller: &Caller,
    chat: Ulid,
    topic: Ulid,
    options: TopicOptionsBody,
) -> Result<EmptyReply, Refused> {
    let (session, access) = group_access(app, caller, chat).await?;
    topic_editor(app, caller, &session, &access, chat, topic).await?;
    if topic == GENERAL_TOPIC && options.hidden {
        return Err(refused(400, "bad_body", "General cannot be hidden"));
    }
    // A topic's 0 is off, even while its chat has slow mode on; none follows the chat's.
    if let Some(seconds) = options.slow_mode
        && seconds != 0
        && !SLOW_MODE_LIST.contains(&seconds)
    {
        return Err(refused(400, "bad_body", "That is not a slow mode"));
    }
    let prefix = chat_prefix(access.chat_type);
    let mut allow = Vec::with_capacity(options.allow.len());
    let mut deny = Vec::with_capacity(options.deny.len());
    for (name, allowed) in options
        .allow
        .iter()
        .map(|name| (name, true))
        .chain(options.deny.iter().map(|name| (name, false)))
    {
        if !TOPIC_OPTION_LIST.contains(&name.as_str()) {
            return Err(refused(
                400,
                "unknown_permission",
                "A topic cannot change that",
            ));
        }
        let full = format!("{prefix}.{name}");
        if allow.contains(&full) || deny.contains(&full) {
            return Err(refused(400, "bad_body", "A permission is named twice"));
        }
        if allowed {
            allow.push(full);
        } else {
            deny.push(full);
        }
    }
    let checked = NewTopicOptions {
        allow,
        deny,
        slow_mode: options.slow_mode,
        write: options.write,
        closed: options.closed,
        hidden: options.hidden,
    };
    let saved = app
        .storage
        .put_topic_options(chat, topic, session.account, checked, Ulid::now())
        .await?;
    topic_saved(app, chat, saved).await
}

/// A topic's sealed profile, by its moderators and the chat's owners and moderators, its photo one
/// of the chat's files.
pub async fn run_topic_profile(
    app: &App,
    caller: &Caller,
    chat: Ulid,
    topic: Ulid,
    profile: ChatProfileBody,
) -> Result<EmptyReply, Refused> {
    let (session, access) = group_access(app, caller, chat).await?;
    topic_editor(app, caller, &session, &access, chat, topic).await?;
    let device = session.device()?;
    let signature: [u8; 64] = fixed(&profile.signature, "signature")?;
    let saved = app
        .storage
        .put_topic_profile(NewTopicProfile {
            chat,
            topic,
            device,
            account: session.account,
            sealed: profile.sealed.0,
            signature: signature.to_vec(),
            photo: profile.photo,
            photo_limit: app.settings.limits.upload.photo,
            now: Ulid::now(),
        })
        .await?;
    topic_saved(app, chat, saved).await
}

/// Makes a member a topic's moderator, or a member there again, by the chat's owners.
pub async fn run_topic_role(
    app: &App,
    caller: &Caller,
    chat: Ulid,
    topic: Ulid,
    account: Ulid,
    role: ChatRole,
) -> Result<EmptyReply, Refused> {
    let (session, _) = group_access(app, caller, chat).await?;
    check(app, caller, Need::MemberWith(chat, MEMBER_ROLE)).await?;
    let moderator = match role {
        ChatRole::Moderator => true,
        ChatRole::Member => false,
        ChatRole::Owner => {
            return Err(refused(
                400,
                "bad_body",
                "A topic has moderators, not owners",
            ));
        }
    };
    let saved = app
        .storage
        .put_topic_role(
            chat,
            topic,
            session.account,
            account,
            moderator,
            Ulid::now(),
        )
        .await?;
    topic_saved(app, chat, saved).await
}

/// A kept topic change reaches the chat's online devices, the one that made it too: the whole item for those who
/// may see it, and for the rest of a hidden topic only that it is hidden. Its history row reaches
/// those who read the topic's history, and photos it no longer names go.
async fn topic_saved(app: &App, chat: Ulid, saved: TopicSaved) -> Result<EmptyReply, Refused> {
    let (event, item, removed) = match saved {
        TopicSaved::Saved {
            event,
            item,
            removed,
            ..
        } => (event, item, removed),
        TopicSaved::NoSuchTopic => return Err(no_such_topic()),
        TopicSaved::NoSuchPhoto => return Err(no_such_photo()),
        TopicSaved::NotMember => {
            return Err(refused(404, "not_member", "They are not in that chat"));
        }
    };
    for file in removed {
        file::remove(app, chat, file).await;
    }
    let mut viewer_list = Vec::new();
    let mut other_list = Vec::new();
    for (account, role) in app.storage.member_roles(chat).await? {
        if role != ChatRole::Member || item.mod_list.contains(&account) {
            viewer_list.push(account);
        } else {
            other_list.push(account);
        }
    }
    if let Some(event) = event {
        app.hub
            .deliver(&viewer_list, None, &ServerFrame::ChatEvent(Box::new(event)));
    }
    if item.options.hidden {
        let trimmed = TopicItem {
            topic: item.topic,
            options: TopicOptionsBody {
                hidden: true,
                ..TopicOptionsBody::default()
            },
            mod_list: Vec::new(),
            profile: None,
        };
        app.hub.deliver(
            &other_list,
            None,
            &ServerFrame::Topic {
                chat,
                item: Box::new(trimmed),
            },
        );
    } else {
        viewer_list.extend(other_list);
    }
    app.hub
        .deliver(&viewer_list, None, &ServerFrame::Topic { chat, item });
    Ok(EmptyReply {})
}

/// Lists a group or channel in the directory under a name and bio the server reads, or stops, by
/// owners.
pub async fn run_findable(
    app: &App,
    caller: &Caller,
    chat: Ulid,
    input: FindableBody,
) -> Result<EmptyReply, Refused> {
    let (session, _) = group_access(app, caller, chat).await?;
    check(app, caller, Need::MemberWith(chat, OPTIONS_EDIT)).await?;
    let mut findable = None;
    if input.findable {
        findable = Some(check_findable(input.name, input.bio)?);
    }
    app.storage.put_findable(chat, findable.clone()).await?;
    let member_list = app.storage.member_accounts(chat).await?;
    let findable = match findable {
        Some(item) => FindableBody {
            findable: true,
            name: Some(item.name),
            bio: item.bio,
        },
        None => FindableBody {
            findable: false,
            name: None,
            bio: None,
        },
    };
    app.hub.deliver(
        &member_list,
        session.device,
        &ServerFrame::Findable { chat, findable },
    );
    Ok(EmptyReply {})
}

pub async fn run_group_info(
    app: &App,
    caller: &Caller,
    chat: Ulid,
) -> Result<GroupInfoReply, Refused> {
    check(app, caller, Need::Device).await?;
    check(app, caller, Need::Member(chat)).await?;
    let Some((epoch, body)) = app.storage.group_info(chat).await? else {
        return Err(refused(
            404,
            "no_group_info",
            "This chat has no group info yet",
        ));
    };
    Ok(GroupInfoReply {
        epoch,
        group_info: Bytes(body),
    })
}

pub async fn run_chat_devices(
    app: &App,
    caller: &Caller,
    chat: Ulid,
) -> Result<ChatDeviceListReply, Refused> {
    check(app, caller, Need::Device).await?;
    check(app, caller, Need::Member(chat)).await?;
    let device_list = app.storage.chat_devices(chat).await?;
    Ok(ChatDeviceListReply { device_list })
}

/// One page of a message's times: 50 members, chosen by the author, so a large group or channel
/// never answers with every member at once.
const TIMES_PAGE: usize = 50;

/// When each member but the sender got and saw one message, keeping only the times the asker may
/// see: received and seen are a permission each, for your own messages and for others', chosen by
/// the author. Someone who may see neither on this message is refused. A page at a time, after the
/// account `after`.
pub async fn run_message_times(
    app: &App,
    caller: &Caller,
    chat: Ulid,
    seq: u64,
    after: Option<Ulid>,
) -> Result<TimesReply, Refused> {
    check(app, caller, Need::Device).await?;
    check(app, caller, Need::Member(chat)).await?;
    let session = caller.session()?;
    let Some(brief) = app.storage.message_brief(chat, seq).await? else {
        return Err(refused(404, "not_found", "There is no such message"));
    };
    let name_list = if brief.account == session.account {
        [VIEW_RECEIVED_OWN, VIEW_SEEN_OWN]
    } else {
        [VIEW_RECEIVED_OTHERS, VIEW_SEEN_OTHERS]
    };
    // A topic's own times permissions come before the chat's, for its members. A hidden topic's
    // message is one that does not exist for whoever may not see it.
    let mut topic_rules = None;
    let mut chat_type = ChatType::Direct;
    if let Some(access) = app.storage.chat_access(chat, session.account).await?
        && matches!(access.chat_type, ChatType::Group | ChatType::Channel)
    {
        chat_type = access.chat_type;
        let topic = brief.topic.unwrap_or(GENERAL_TOPIC);
        if let Some(rules) = app
            .storage
            .topic_access(chat, topic, session.account)
            .await?
        {
            let moderator = access.role != ChatRole::Member || rules.moderator;
            if rules.hidden && !moderator {
                return Err(refused(404, "not_found", "There is no such message"));
            }
            topic_rules = Some((rules, moderator));
        }
    }
    let mut allowed = [false; 2];
    // A topic's own moderators see everyone's times there.
    let topic_moderator = topic_rules
        .as_ref()
        .is_some_and(|(rules, _)| rules.moderator);
    for (place, name) in name_list.into_iter().enumerate() {
        if topic_moderator {
            allowed[place] = true;
            continue;
        }
        allowed[place] = match topic_says(&topic_rules, chat_type, name) {
            Some(said) => said,
            None => match check(app, caller, Need::MemberWith(chat, name)).await {
                Ok(()) => true,
                Err(refusal) if refusal.code == "no_permission" => false,
                Err(refusal) => return Err(refusal),
            },
        };
    }
    let [received, seen] = allowed;
    if !received && !seen {
        return Err(refused(
            403,
            "no_permission",
            "You may not see this message's times",
        ));
    }
    let shown = TimesShown { received, seen };
    let Some((member_list, more)) = app
        .storage
        .message_times(chat, seq, after, shown, TIMES_PAGE)
        .await?
    else {
        return Err(refused(404, "not_found", "There is no such message"));
    };
    Ok(TimesReply {
        chat,
        seq,
        member_list,
        more,
    })
}

/// Keeps or clears this account's draft in a chat, and tells its other online devices. A draft is
/// not a message: it takes no sequence id, no chat task and no content permission, only
/// membership.
pub async fn run_put_draft(
    app: &App,
    caller: &Caller,
    chat: Ulid,
    draft: Option<DraftBody>,
) -> Result<EmptyReply, Refused> {
    check(app, caller, Need::Device).await?;
    check(app, caller, Need::Member(chat)).await?;
    let session = caller.session()?;
    if let Some(draft) = &draft
        && draft.body.0.len() as u64 > app.settings.limits.body.draft
    {
        return Err(refused(
            413,
            "body_too_large",
            "That draft is larger than this server keeps",
        ));
    }
    let kept = app
        .storage
        .put_draft(
            chat,
            session.account,
            draft.map(|draft| (draft.epoch, draft.body.0)),
            Ulid::now(),
        )
        .await?;
    app.hub.deliver(
        &[session.account],
        session.device,
        &ServerFrame::Draft { chat, draft: kept },
    );
    Ok(EmptyReply {})
}

/// A new folder's id, which its sealed name's associated data holds, so a client never chooses its
/// own. Kept for this account for 5 minutes, like a device's.
pub async fn run_folder_id(app: &App, caller: &Caller) -> Result<FolderReply, Refused> {
    check(app, caller, Need::Device).await?;
    check(app, caller, Need::Server(FOLDER_CREATE)).await?;
    let session = caller.session()?;
    let id = new_id()?;
    let now = Ulid::now();
    app.storage
        .create_challenge(
            id.bytes().to_vec(),
            session.account,
            PURPOSE_FOLDER_ID,
            now + CHALLENGE_MS,
            now,
        )
        .await?;
    Ok(FolderReply { folder: id })
}

/// Keeps this account's whole arrangement of folders, and tells its other online devices. Chats it
/// is not in are dropped. Each kind of change needs its own permission, found by comparing with what
/// is kept, and refused in any part is refused whole. Like a draft it takes no chat task.
pub async fn run_put_chat_folders(
    app: &App,
    caller: &Caller,
    folders: ChatFolders,
) -> Result<EmptyReply, Refused> {
    check(app, caller, Need::Device).await?;
    let session = caller.session()?;
    let (held, chat_list) = app.storage.chat_folders(session.account).await?;
    let mut member = HashSet::new();
    for chat in chat_list {
        member.insert(chat);
    }
    let mut held_all = None;
    let mut before = HashMap::new();
    for folder in &held.folder_list {
        if folder.name.is_none() {
            held_all = Some(folder.id);
        }
        before.insert(folder.id, folder);
    }

    // The shape first: one `all`, this account's own and first, and every id, chat and pin once.
    let mut folders = folders;
    let mut id_list = HashSet::new();
    let mut all = None;
    let mut named = 0;
    if folders
        .folder_list
        .first()
        .is_some_and(|folder| folder.name.is_some() || folder.chat_list.is_some())
    {
        return Err(refused(400, "bad_body", "All is always the first folder"));
    }
    for folder in &folders.folder_list {
        if !id_list.insert(folder.id) {
            return Err(refused(400, "bad_body", "A folder is named twice"));
        }
        match (&folder.name, &folder.chat_list) {
            (None, None) => {
                if all.replace(folder.id).is_some() {
                    return Err(refused(400, "bad_body", "There is more than one all"));
                }
            }
            (Some(name), Some(chat_list)) => {
                named += 1;
                if name.0.len() as u64 > app.settings.limits.body.folder_name {
                    return Err(refused(
                        413,
                        "body_too_large",
                        "That folder name is larger than this server keeps",
                    ));
                }
                let mut seen = HashSet::new();
                for chat in chat_list {
                    if !seen.insert(*chat) {
                        return Err(refused(400, "bad_body", "A chat is in a folder twice"));
                    }
                }
            }
            _ => {
                return Err(refused(
                    400,
                    "bad_body",
                    "Every folder but all has a name and chats, and all has neither",
                ));
            }
        }
        let mut seen = HashSet::new();
        for chat in &folder.pin_list {
            if !seen.insert(*chat) {
                return Err(refused(400, "bad_body", "A chat is pinned twice"));
            }
            if let Some(chat_list) = &folder.chat_list
                && !chat_list.contains(chat)
            {
                return Err(refused(
                    400,
                    "bad_body",
                    "A pinned chat is not in its folder",
                ));
            }
        }
    }
    if all.is_none() || all != held_all {
        return Err(refused(400, "bad_body", "This account's all is missing"));
    }
    if !id_list.contains(&folders.default) {
        return Err(refused(
            400,
            "bad_body",
            "The default is not one of the folders",
        ));
    }
    if named > app.settings.limits.folders {
        return Err(Refused {
            status: 403,
            code: "too_many_folders",
            message: format!(
                "This account may have {} folders",
                app.settings.limits.folders
            ),
            call_seq: None,
            kv: None,
            seconds: None,
            account_list: None,
        });
    }
    for folder in &mut folders.folder_list {
        if let Some(chat_list) = &mut folder.chat_list {
            chat_list.retain(|chat| member.contains(chat));
        }
        folder.pin_list.retain(|chat| member.contains(chat));
    }

    // Then what changed, each kind needing its own permission.
    let mut need_list = Vec::new();
    let mut kept_order = Vec::new();
    for folder in &folders.folder_list {
        let Some(old) = before.get(&folder.id) else {
            need_list.push(FOLDER_CREATE);
            if matches!(&folder.chat_list, Some(chat_list) if !chat_list.is_empty()) {
                need_list.push(FOLDER_CHATS);
            }
            if !folder.pin_list.is_empty() {
                need_list.push(CHAT_PIN);
            }
            continue;
        };
        kept_order.push(folder.id);
        if folder.name != old.name {
            need_list.push(FOLDER_RENAME);
        }
        if let (Some(chat_list), Some(old_list)) = (&folder.chat_list, &old.chat_list) {
            let mut moved = chat_list.len() != old_list.len();
            for chat in chat_list {
                if !old_list.contains(chat) {
                    moved = true;
                }
            }
            if moved {
                need_list.push(FOLDER_CHATS);
            }
        }
        if folder.pin_list != old.pin_list {
            need_list.push(CHAT_PIN);
        }
    }
    let mut old_order = Vec::new();
    for folder in &held.folder_list {
        if id_list.contains(&folder.id) {
            old_order.push(folder.id);
        } else {
            need_list.push(FOLDER_REMOVE);
        }
    }
    if old_order != kept_order {
        need_list.push(FOLDER_REORDER);
    }
    // Removing the default makes `all` the default, which is part of removing it.
    let default_removed = !id_list.contains(&held.default);
    if folders.default != held.default && !(default_removed && Some(folders.default) == held_all) {
        need_list.push(FOLDER_DEFAULT);
    }
    need_list.sort_unstable();
    need_list.dedup();
    for permission in need_list {
        check(app, caller, Need::Server(permission)).await?;
    }

    if !app
        .storage
        .put_chat_folders(
            session.account,
            folders.clone(),
            PURPOSE_FOLDER_ID,
            Ulid::now(),
        )
        .await?
    {
        return Err(refused(
            404,
            "unknown_folder",
            "That folder is not one of yours",
        ));
    }
    app.hub.deliver(
        &[session.account],
        session.device,
        &ServerFrame::ChatFolders(Box::new(folders)),
    );
    Ok(EmptyReply {})
}

pub async fn run_chat_wraps(
    app: &App,
    caller: &Caller,
    chat: Ulid,
) -> Result<WrapListReply, Refused> {
    check(app, caller, Need::Device).await?;
    check(app, caller, Need::Member(chat)).await?;
    let wrap_list = app.storage.wraps(chat).await?;
    Ok(WrapListReply { wrap_list })
}

/// A new upload into a chat. Its declared type picks the limit and the permission to send it, and
/// its size must be one a sealed file of that many chunks can have.
pub async fn run_create_file(
    app: &App,
    caller: &Caller,
    chat: Ulid,
    input: CreateFileBody,
) -> Result<FileReply, Refused> {
    check(app, caller, Need::Device).await?;
    check(
        app,
        caller,
        Need::MemberWith(chat, send_permission(input.kind)),
    )
    .await?;
    let encrypted = input.encrypted != Some(false);
    if !encrypted {
        check(app, caller, Need::MemberWith(chat, SEND_UNENCRYPTED)).await?;
    }
    let session = caller.session()?;
    let Some(limit) = app.settings.limits.upload.of(input.kind) else {
        return Err(refused(400, "bad_body", "That type is not uploaded"));
    };
    if input.size > limit {
        return Err(refused(413, "file_too_large", "That file is too large"));
    }
    if !file::fits(input.size, input.chunks) {
        return Err(refused(
            400,
            "bad_body",
            "That size does not fit those chunks",
        ));
    }
    let id = new_id()?;
    app.storage
        .create_file(NewFile {
            id,
            chat,
            encrypted,
            kind: input.kind,
            size: input.size,
            chunks: input.chunks,
            device: session.device()?,
            now: Ulid::now(),
        })
        .await?;
    Ok(FileReply { file: id })
}

/// A chat's file the caller may see: they are in its chat, and it is not removed.
async fn visible_file(app: &App, caller: &Caller, id: Ulid) -> Result<StoredFile, Refused> {
    check(app, caller, Need::Device).await?;
    let missing = || refused(404, "no_such_file", "That file does not exist");
    let Some(stored) = app.storage.file(id).await? else {
        return Err(missing());
    };
    check(app, caller, Need::Member(stored.chat)).await?;
    if stored.removed {
        return Err(missing());
    }
    Ok(stored)
}

/// Chunk `n` of an upload, from its uploader's account, in order: the next one, or the last one
/// again.
pub async fn run_put_chunk(
    app: &App,
    caller: &Caller,
    id: Ulid,
    n: u64,
    bytes: Vec<u8>,
) -> Result<FileStatusReply, Refused> {
    let stored = visible_file(app, caller, id).await?;
    let session = caller.session()?;
    if stored.account != session.account {
        return Err(refused(403, "not_yours", "That file is someone else's"));
    }
    if stored.used {
        return Err(refused(409, "file_sent", "That file is already sent"));
    }
    if n >= stored.chunks || n > stored.received || n + 1 < stored.received {
        return Err(refused(
            409,
            "chunk_out_of_order",
            "That chunk is not the next one",
        ));
    }
    if bytes.len() as u64 != file::chunk_length(stored.size, stored.chunks, n) {
        return Err(refused(400, "bad_chunk", "That chunk is not its length"));
    }
    file::write_chunk(app, stored.chat, id, n, &bytes).await?;
    app.storage.file_received(id, n).await?;
    Ok(FileStatusReply {
        size: stored.size,
        chunks: stored.chunks,
        received: stored.received.max(n + 1),
    })
}

/// How far an upload got, so it resumes from there.
pub async fn run_file_status(
    app: &App,
    caller: &Caller,
    id: Ulid,
) -> Result<FileStatusReply, Refused> {
    let stored = visible_file(app, caller, id).await?;
    Ok(FileStatusReply {
        size: stored.size,
        chunks: stored.chunks,
        received: stored.received,
    })
}

/// Chunk `n`, once a sent message keeps the file. Its uploader may read it before.
pub async fn run_get_chunk(
    app: &App,
    caller: &Caller,
    id: Ulid,
    n: u64,
) -> Result<Vec<u8>, Refused> {
    let stored = visible_file(app, caller, id).await?;
    let session = caller.session()?;
    if !stored.used && stored.account != session.account {
        return Err(refused(404, "no_such_file", "That file does not exist"));
    }
    if n >= stored.received {
        return Err(refused(404, "no_such_chunk", "That chunk is not there"));
    }
    let length = file::chunk_length(stored.size, stored.chunks, n);
    file::read_chunk(app, stored.chat, id, n, length).await
}

/// The longest a sticker pack's name may be, in characters, fixed by the author.
const PACK_NAME_CHARS: usize = 64;

/// Every pack and its stickers, for a registered device or whoever may manage them.
pub async fn run_sticker_list(app: &App, caller: &Caller) -> Result<StickerListReply, Refused> {
    if check(app, caller, Need::Server(STICKER_MANAGE))
        .await
        .is_err()
    {
        check(app, caller, Need::Device).await?;
    }
    let mut pack_list = app.storage.sticker_list().await?;
    if app.settings.stickers.default_pack {
        pack_list.push(app.default_pack.listed());
    }
    Ok(StickerListReply { pack_list })
}

/// A sticker's WebP. Its bytes never change, as an id is never reused. A bundled one is served even
/// while its pack is not listed.
pub async fn run_sticker_image(app: &App, caller: &Caller, id: Ulid) -> Result<Vec<u8>, Refused> {
    check(app, caller, Need::Device).await?;
    if let Some(bytes) = app.default_pack.image(id) {
        return Ok(bytes.to_vec());
    }
    if app.storage.sticker(id).await?.is_none() {
        return Err(refused(
            404,
            "no_such_sticker",
            "That sticker does not exist",
        ));
    }
    file::read_sticker(app, id).await
}

pub async fn run_create_pack(
    app: &App,
    caller: &Caller,
    input: CreatePackBody,
) -> Result<PackReply, Refused> {
    check(app, caller, Need::Server(STICKER_MANAGE)).await?;
    let name = input.name.trim().to_string();
    let length = name.chars().count();
    if length == 0 || length > PACK_NAME_CHARS {
        return Err(refused(
            400,
            "bad_name",
            "A pack's name is 1 to 64 characters",
        ));
    }
    let id = new_id()?;
    if name == file::DEFAULT_PACK_NAME || !app.storage.create_pack(id, name, Ulid::now()).await? {
        return Err(refused(409, "name_taken", "A pack already has that name"));
    }
    tracing::info!(msg = "Added a sticker pack", pack = %id);
    Ok(PackReply { pack: id })
}

pub async fn run_remove_pack(app: &App, caller: &Caller, id: Ulid) -> Result<EmptyReply, Refused> {
    check(app, caller, Need::Server(STICKER_MANAGE)).await?;
    let Some(sticker_list) = app.storage.remove_pack(id).await? else {
        return Err(refused(404, "no_such_pack", "That pack does not exist"));
    };
    for sticker in sticker_list {
        file::remove_sticker(app, sticker).await;
    }
    tracing::info!(msg = "Removed a sticker pack", pack = %id);
    Ok(EmptyReply {})
}

/// A WebP of at most 512x512 and `[limits.upload] sticker`, checked by its header.
pub async fn run_add_sticker(
    app: &App,
    caller: &Caller,
    pack: Ulid,
    emoji: Option<String>,
    bytes: Vec<u8>,
) -> Result<StickerReply, Refused> {
    check(app, caller, Need::Server(STICKER_MANAGE)).await?;
    if bytes.len() as u64 > app.settings.limits.upload.sticker {
        return Err(refused(413, "file_too_large", "That sticker is too large"));
    }
    let Some((width, height)) = file::webp_size(&bytes) else {
        return Err(refused(400, "bad_sticker", "A sticker is a WebP"));
    };
    if width == 0 || height == 0 || width > file::STICKER_SIDE || height > file::STICKER_SIDE {
        return Err(refused(400, "bad_sticker", "A sticker is at most 512x512"));
    }
    let emoji = match emoji {
        Some(emoji) if emoji.trim().is_empty() => None,
        Some(emoji) if emoji.chars().count() > 16 => {
            return Err(refused(400, "bad_body", "That is not one emoji"));
        }
        other => other,
    };
    let id = new_id()?;
    file::write_sticker(app, id, &bytes).await?;
    let sticker = StickerItem {
        id,
        emoji: emoji.clone(),
        width,
        height,
        size: bytes.len() as u64,
    };
    let added = app
        .storage
        .add_sticker(NewSticker {
            id,
            pack,
            emoji,
            width,
            height,
            size: sticker.size,
            now: Ulid::now(),
        })
        .await;
    match added {
        Ok(true) => {
            tracing::info!(msg = "Added a sticker", sticker = %id, pack = %pack);
            Ok(StickerReply { sticker })
        }
        Ok(false) => {
            file::remove_sticker(app, id).await;
            Err(refused(404, "no_such_pack", "That pack does not exist"))
        }
        Err(error) => {
            file::remove_sticker(app, id).await;
            Err(error.into())
        }
    }
}

pub async fn run_remove_sticker(
    app: &App,
    caller: &Caller,
    id: Ulid,
) -> Result<EmptyReply, Refused> {
    check(app, caller, Need::Server(STICKER_MANAGE)).await?;
    if !app.storage.remove_sticker(id).await? {
        return Err(refused(
            404,
            "no_such_sticker",
            "That sticker does not exist",
        ));
    }
    file::remove_sticker(app, id).await;
    tracing::info!(msg = "Removed a sticker", sticker = %id);
    Ok(EmptyReply {})
}

// -------------------------------------------------------------------------------------------------
// Profiles
// -------------------------------------------------------------------------------------------------

/// Whether `other` is this account or shares a chat with it, as profiles and photos need.
async fn shares_or_self(app: &App, account: Ulid, other: Ulid) -> Result<bool, Refused> {
    if profile_of_own(app, account, other).await? {
        return Ok(true);
    }
    Ok(app
        .storage
        .accounts_sharing(account)
        .await?
        .contains(&other))
}

/// Whether `account` keeps `other`'s profile: its own, or an agent it owns, whose profile its owner's
/// device signs, chosen by the author.
async fn profile_of_own(app: &App, account: Ulid, other: Ulid) -> Result<bool, Refused> {
    if account == other {
        return Ok(true);
    }
    Ok(match app.storage.account_by_id(other).await? {
        Some(found) => found.owner == Some(account),
        None => false,
    })
}

/// Tells the online devices of everyone sharing a chat with the account, and its own, that its
/// profile moved.
async fn profile_changed(
    app: &App,
    account: Ulid,
    version: u64,
    username: String,
    display_name: Option<String>,
) -> Result<(), Refused> {
    let mut account_list = app.storage.accounts_sharing(account).await?;
    if !account_list.contains(&account) {
        account_list.push(account);
    }
    app.hub.deliver(
        &account_list,
        None,
        &ServerFrame::Profile {
            account,
            version,
            username,
            display_name,
        },
    );
    Ok(())
}

/// Tells everyone after a change that did not touch the names, which are read back for the frame.
async fn profile_moved(app: &App, account: Ulid, version: u64) -> Result<(), Refused> {
    let Some(found) = app.storage.account_by_id(account).await? else {
        return Ok(());
    };
    profile_changed(app, account, version, found.username, found.display_name).await
}

/// An account's names, version and sealed profile, for itself or someone sharing a chat with it.
pub async fn run_profile(app: &App, caller: &Caller, id: Ulid) -> Result<ProfileReply, Refused> {
    check(app, caller, Need::Device).await?;
    let session = caller.session()?;
    if !shares_or_self(app, session.account, id).await? {
        return Err(refused(
            403,
            "not_shared",
            "You share no chat with that account",
        ));
    }
    match app.storage.profile(id).await? {
        Some(profile) => Ok(profile),
        None => Err(refused(
            404,
            "no_such_account",
            "That account does not exist",
        )),
    }
}

/// Saves your own sealed profile and the photo it names. A photo no longer named is deleted.
pub async fn run_put_profile(
    app: &App,
    caller: &Caller,
    id: Ulid,
    input: PutProfileBody,
) -> Result<VersionReply, Refused> {
    check(app, caller, Need::Device).await?;
    check(app, caller, Need::Server(PROFILE_EDIT)).await?;
    let session = caller.session()?;
    let device = session.device()?;
    if !profile_of_own(app, session.account, id).await? {
        return Err(refused(
            403,
            "no_permission",
            "You may change only your own profile",
        ));
    }
    let sealed = match (input.sealed, input.signature) {
        (Some(sealed), Some(signature)) => {
            let signature: [u8; 64] = fixed(&signature, "signature")?;
            Some((sealed.0, signature.to_vec()))
        }
        (None, None) => None,
        _ => {
            return Err(refused(
                400,
                "bad_body",
                "A sealed profile needs its signature",
            ));
        }
    };
    if sealed.is_none() && input.photo.is_some() {
        return Err(refused(
            400,
            "bad_body",
            "A photo is named by a sealed profile",
        ));
    }
    let saved = app
        .storage
        .put_profile(NewProfile {
            account: id,
            device,
            sealed,
            photo: input.photo,
            now: Ulid::now(),
        })
        .await?;
    let ProfileSaved::Saved { version, removed } = saved else {
        return Err(refused(404, "no_such_photo", "That photo does not exist"));
    };
    for photo in removed {
        file::remove_photo(app, id, photo).await;
    }
    tracing::info!(msg = "Saved a profile", account = %id);
    profile_moved(app, id, version).await?;
    Ok(VersionReply { version })
}

/// Your own display name and username, or admin's change to anyone's. Changing your own username
/// needs your password.
pub async fn run_names(
    app: &App,
    caller: &Caller,
    id: Ulid,
    input: NamesBody,
) -> Result<VersionReply, Refused> {
    if input.display_name.is_none() && input.username.is_none() {
        return Err(refused(400, "bad_body", "Nothing to change"));
    }
    let own = matches!(caller, Caller::Session(session) if session.account == id);
    if own {
        check(app, caller, Need::Device).await?;
        if input.display_name.is_some() {
            check(app, caller, Need::Server(PROFILE_EDIT)).await?;
        }
        if input.username.is_some() {
            check(app, caller, Need::Server(ACCOUNT_RENAME)).await?;
        }
    } else {
        check(app, caller, Need::Admin).await?;
    }
    let display_name = match input.display_name {
        Some(name) => {
            let name = name.trim().to_string();
            let length = name.chars().count();
            if length == 0 || length > DISPLAY_NAME_CHARS {
                return Err(refused(
                    400,
                    "bad_display_name",
                    "A display name is 1 to 64 characters",
                ));
            }
            Some(name)
        }
        None => None,
    };
    if let Some(username) = &input.username {
        let min_length = if own {
            Some(app.settings.registration.username_min_length)
        } else {
            None
        };
        let agent = match app.storage.account_by_id(id).await? {
            Some(account) => account.owner.is_some(),
            None => false,
        };
        check_username(username, min_length, agent)?;
        if own {
            let wrong = refused(401, "bad_password", "That password is wrong");
            let Some(password) = input.password else {
                return Err(wrong);
            };
            if password.chars().count() > MAX_PASSWORD_CHARS {
                return Err(wrong);
            }
            let Some(account) = app.storage.account_by_id(id).await? else {
                return Err(wrong);
            };
            let Some(stored) = account.password_hash else {
                return Err(wrong);
            };
            let verified = tokio::task::spawn_blocking(move || {
                Argon2::default()
                    .verify_password(password.as_bytes(), stored.as_str())
                    .is_ok()
            })
            .await;
            if !matches!(verified, Ok(true)) {
                return Err(wrong);
            }
        }
    }
    let saved = app
        .storage
        .set_names(NewNames {
            account: id,
            display_name,
            username: input.username,
        })
        .await?;
    match saved {
        NamesSaved::Saved {
            version,
            username,
            display_name,
        } => {
            tracing::info!(msg = "Changed an account's names", account = %id);
            profile_changed(app, id, version, username, display_name).await?;
            Ok(VersionReply { version })
        }
        NamesSaved::UsernameTaken => Err(refused(409, "username_taken", "That username is taken")),
        NamesSaved::NoSuchAccount => Err(refused(
            404,
            "no_such_account",
            "That account does not exist",
        )),
    }
}

/// A new photo's sealed bytes, one chunk, for your own profile to name next.
pub async fn run_add_photo(
    app: &App,
    caller: &Caller,
    id: Ulid,
    bytes: Vec<u8>,
) -> Result<PhotoReply, Refused> {
    check(app, caller, Need::Device).await?;
    check(app, caller, Need::Server(PROFILE_EDIT)).await?;
    let session = caller.session()?;
    let device = session.device()?;
    if !profile_of_own(app, session.account, id).await? {
        return Err(refused(
            403,
            "no_permission",
            "You may change only your own profile",
        ));
    }
    let size = bytes.len() as u64;
    if size > app.settings.limits.upload.photo {
        return Err(refused(413, "file_too_large", "That photo is too large"));
    }
    if !file::fits(size, 1) {
        return Err(refused(400, "bad_body", "A photo is one sealed chunk"));
    }
    let photo = new_id()?;
    file::write_photo(app, id, photo, &bytes).await?;
    let removed = match app
        .storage
        .add_photo(NewPhoto {
            id: photo,
            account: id,
            device,
            size,
            now: Ulid::now(),
        })
        .await
    {
        Ok(removed) => removed,
        Err(error) => {
            file::remove_photo(app, id, photo).await;
            return Err(error.into());
        }
    };
    for old in removed {
        file::remove_photo(app, id, old).await;
    }
    Ok(PhotoReply { photo })
}

/// A photo's sealed bytes, for its account or someone sharing a chat with it. They never change, as
/// an id is never reused.
pub async fn run_photo(app: &App, caller: &Caller, id: Ulid) -> Result<Vec<u8>, Refused> {
    check(app, caller, Need::Device).await?;
    let session = caller.session()?;
    let missing = refused(404, "no_such_photo", "That photo does not exist");
    let Some(account) = app.storage.photo(id).await? else {
        return Err(missing);
    };
    if !shares_or_self(app, session.account, account).await? {
        return Err(missing);
    }
    file::read_photo(app, account, id).await
}

/// Admin removes someone's photo, whose sealed profile still names it.
pub async fn run_remove_photo(app: &App, caller: &Caller, id: Ulid) -> Result<EmptyReply, Refused> {
    check(app, caller, Need::Admin).await?;
    let Some((removed, version)) = app.storage.remove_photo(id).await? else {
        return Err(refused(404, "no_such_photo", "That account has no photo"));
    };
    for photo in removed {
        file::remove_photo(app, id, photo).await;
    }
    tracing::info!(msg = "Removed an account's photo", account = %id);
    profile_moved(app, id, version).await?;
    Ok(EmptyReply {})
}

// -------------------------------------------------------------------------------------------------
// Presence
// -------------------------------------------------------------------------------------------------

/// What one viewer may see of anyone's presence: the state and last in the app, and last connected.
#[derive(Clone, Copy)]
struct Sight {
    in_app: bool,
    connected: bool,
}

async fn sight_of(app: &App, viewer: Ulid) -> Result<Sight, Refused> {
    Ok(Sight {
        in_app: app
            .storage
            .server_permitted(viewer, PRESENCE_IN_APP)
            .await?,
        connected: app
            .storage
            .server_permitted(viewer, PRESENCE_CONNECTED)
            .await?,
    })
}

/// These accounts' presence as it is now, before any viewer's permissions take parts away.
async fn presence_now(app: &App, account_list: Vec<Ulid>) -> Result<Vec<PresenceItem>, Refused> {
    Ok(app
        .storage
        .presence_times(account_list)
        .await?
        .into_iter()
        .map(|times| PresenceItem {
            account: times.account,
            state: Some(app.hub.presence_state(times.account)),
            in_app_at: times.in_app_at,
            connected_at: times.connected_at,
            text: app.hub.presence_text(times.account).map(Bytes),
        })
        .collect())
}

/// What a viewer sees of one presence, or nothing when it may see none of it. The text goes with the
/// state, and an agent's, its trouble reason, to its owner alone.
async fn seen_by(
    app: &App,
    item: &PresenceItem,
    sight: Sight,
    viewer: Ulid,
) -> Result<Option<PresenceItem>, Refused> {
    if !sight.in_app && !sight.connected {
        return Ok(None);
    }
    let mut text = item.text.clone().filter(|_| sight.in_app);
    if text.is_some()
        && let Some(account) = app.storage.account_by_id(item.account).await?
        && let Some(owner) = account.owner
        && owner != viewer
    {
        text = None;
    }
    Ok(Some(PresenceItem {
        account: item.account,
        state: item.state.filter(|_| sight.in_app),
        in_app_at: item.in_app_at.filter(|_| sight.in_app),
        connected_at: item.connected_at.filter(|_| sight.connected),
        text,
    }))
}

/// Everyone sharing a chat with this account, not the account itself.
async fn sharing(app: &App, account: Ulid) -> Result<Vec<Ulid>, Refused> {
    let mut account_list = app.storage.accounts_sharing(account).await?;
    account_list.retain(|other| *other != account);
    Ok(account_list)
}

/// Tells everyone sharing a chat with the account where it is now, on their devices that have
/// announced themselves, each as its permissions allow.
async fn presence_changed(app: &App, account: Ulid) -> Result<(), Refused> {
    let Some(item) = presence_now(app, vec![account]).await?.pop() else {
        return Ok(());
    };
    for viewer in sharing(app, account).await? {
        if !app.hub.announced(viewer) {
            continue;
        }
        if let Some(seen) = seen_by(app, &item, sight_of(app, viewer).await?, viewer).await? {
            app.hub
                .deliver_announced(viewer, &ServerFrame::Presence(Box::new(seen)));
        }
    }
    Ok(())
}

/// The same, for what already happened, such as a call's join or a dropped connection, so a failure
/// is only written down.
pub async fn presence_moved(app: &App, account: Ulid) {
    if let Err(refusal) = presence_changed(app, account).await {
        tracing::error!(
            msg = "Could not tell where an account is",
            code = refusal.code
        );
    }
}

/// Everyone sharing a chat with the viewer, as it may see them.
async fn presence_for(app: &App, viewer: Ulid) -> Result<Vec<PresenceItem>, Refused> {
    let sight = sight_of(app, viewer).await?;
    let mut seen_list = Vec::new();
    for item in presence_now(app, sharing(app, viewer).await?).await? {
        if let Some(seen) = seen_by(app, &item, sight, viewer).await? {
            seen_list.push(seen);
        }
    }
    Ok(seen_list)
}

/// A device's presence: its first on a connection announces it, XMPP's way, and every later one is
/// its app opening or closing. Writes its times and tells everyone sharing a chat. `true` if this was
/// the connection's first, so the device is owed everyone else's.
async fn announce(app: &App, session: Session, input: PresenceBody) -> Result<bool, Refused> {
    let device = session.device()?;
    let in_app = input.in_app;
    if input
        .text
        .as_ref()
        .is_some_and(|text| text.0.len() > PRESENCE_TEXT_BYTES)
    {
        return Err(refused(
            413,
            "body_too_large",
            "That presence text is too large",
        ));
    }
    let now = Ulid::now();
    let before = app.hub.announce(
        session.account,
        device,
        in_app,
        input.trouble,
        input.text.map(|text| text.0),
    );
    let first = matches!(before, None | Some(None));
    let was_in_app = matches!(before, Some(Some(true)));
    // With no live connection, as over REST, the device is there for this moment only.
    let connected_at = first.then_some(now);
    let in_app_at = (in_app || was_in_app).then_some(now);
    app.storage
        .device_presence(device, connected_at, in_app_at)
        .await?;
    presence_changed(app, session.account).await?;
    Ok(matches!(before, Some(None)))
}

/// The `presence` frame: an event with no answer, and the first on a connection is followed by one
/// `presence` per person sharing a chat.
pub async fn run_presence(app: &App, caller: &Caller, input: PresenceBody) -> Result<(), Refused> {
    check(app, caller, Need::Device).await?;
    let session = caller.session()?;
    let device = session.device()?;
    if announce(app, session, input).await? {
        for item in presence_for(app, session.account).await? {
            app.hub.deliver_to_device(
                session.account,
                device,
                &ServerFrame::Presence(Box::new(item)),
            );
        }
    }
    Ok(())
}

/// `PUT /api/v1/devices/{id}/presence`: the same over REST, for this session's own device, answered
/// with everyone sharing a chat.
pub async fn run_put_presence(
    app: &App,
    caller: &Caller,
    device: Ulid,
    input: PresenceBody,
) -> Result<PresenceListReply, Refused> {
    check(app, caller, Need::Device).await?;
    let session = caller.session()?;
    if session.device()? != device {
        return Err(refused(404, "no_such_device", "That device does not exist"));
    }
    announce(app, session, input).await?;
    Ok(PresenceListReply {
        presence_list: presence_for(app, session.account).await?,
    })
}

/// Anyone's presence, asked once, as the asker's permissions allow.
pub async fn run_presence_of(
    app: &App,
    caller: &Caller,
    account: Ulid,
) -> Result<PresenceItem, Refused> {
    check(app, caller, Need::Device).await?;
    let session = caller.session()?;
    let sight = sight_of(app, session.account).await?;
    if !sight.in_app && !sight.connected {
        return Err(refused(403, "no_permission", "You may not do this"));
    }
    if app.storage.account_by_id(account).await?.is_none() {
        return Err(refused(
            404,
            "no_such_account",
            "That account does not exist",
        ));
    }
    match presence_now(app, vec![account]).await?.pop() {
        Some(item) => seen_by(app, &item, sight, session.account)
            .await?
            .ok_or_else(|| refused(403, "no_permission", "You may not do this")),
        None => Err(refused(
            404,
            "no_such_account",
            "That account does not exist",
        )),
    }
}

/// A connection that had announced itself ended, or a newer one replaced it: the device was connected
/// until now, and in the app until now if it was open.
async fn connection_gone(app: &App, account: Ulid, device: Ulid, in_app: bool) {
    let now = Ulid::now();
    if let Err(error) = app
        .storage
        .device_presence(device, Some(now), in_app.then_some(now))
        .await
    {
        tracing::error!(msg = "Could not write where a device was", error = %error);
    }
    presence_moved(app, account).await;
}

// -------------------------------------------------------------------------------------------------
// Settings kept per account
// -------------------------------------------------------------------------------------------------

/// The longest a setting's name is.
const KV_KEY_CHARS: usize = 64;

fn check_kv_key(key: &str) -> Result<(), Refused> {
    let fits = !key.is_empty()
        && key.len() <= KV_KEY_CHARS
        && key.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_' || byte == b'.'
        });
    if fits {
        Ok(())
    } else {
        Err(refused(
            400,
            "bad_body",
            "A setting's name is 1 to 64 of a-z, 0-9, _ and .",
        ))
    }
}

/// This account's settings: the version alone when the device holds it, the whole set otherwise.
pub async fn run_kv(app: &App, caller: &Caller, version: Option<u64>) -> Result<KvReply, Refused> {
    check(app, caller, Need::Device).await?;
    let session = caller.session()?;
    Ok(app.storage.kv(session.account, version).await?)
}

/// A change based on the version the device holds. The server wins: one based on an older version is
/// refused with the server's whole set. Saved, it goes to the account's other online devices.
pub async fn run_kv_set(
    app: &App,
    caller: &Caller,
    change: KvChange,
) -> Result<VersionReply, Refused> {
    check(app, caller, Need::Device).await?;
    let session = caller.session()?;
    let mut key_list: Vec<&str> = Vec::with_capacity(change.set.len() + change.remove.len());
    for key in change
        .set
        .iter()
        .map(|item| item.key.as_str())
        .chain(change.remove.iter().map(String::as_str))
    {
        check_kv_key(key)?;
        if key_list.contains(&key) {
            return Err(refused(400, "bad_body", "A setting is named twice"));
        }
        key_list.push(key);
    }
    if change
        .set
        .iter()
        .any(|item| item.value.0.len() as u64 > app.settings.limits.body.kv_value)
    {
        return Err(refused(
            413,
            "body_too_large",
            "That setting is larger than this server keeps",
        ));
    }
    let from = change.version;
    let (set, remove) = (change.set.clone(), change.remove.clone());
    match app
        .storage
        .change_kv(session.account, change, app.settings.limits.kv_keys)
        .await?
    {
        KvChanged::Saved(version) => {
            app.hub.deliver(
                &[session.account],
                session.device,
                &ServerFrame::Kv {
                    reference: None,
                    version,
                    kv_list: None,
                    from: Some(from),
                    set: Some(set),
                    remove: Some(remove),
                },
            );
            Ok(VersionReply { version })
        }
        KvChanged::Stale(kv) => Err(Refused {
            status: 409,
            code: "kv_stale",
            message: "These settings changed on another device".to_string(),
            call_seq: None,
            kv: Some(Box::new(kv)),
            seconds: None,
            account_list: None,
        }),
        KvChanged::TooMany => Err(Refused {
            status: 413,
            code: "too_many_keys",
            message: format!(
                "This account may keep {} settings",
                app.settings.limits.kv_keys
            ),
            call_seq: None,
            kv: None,
            seconds: None,
            account_list: None,
        }),
    }
}

/// What a device's frames do: reads answer here, and everything else goes through the chat's task.
pub async fn run_frame(
    app: &Arc<App>,
    session: Session,
    frame: ClientFrame,
) -> Option<ServerFrame> {
    let caller = Caller::Session(session);
    let (chat, reference) = match &frame {
        ClientFrame::Times {
            reference,
            chat,
            seq,
            after,
        } => {
            let reference = *reference;
            let result = operation!("Times", "times", async {
                let reply = run_message_times(app, &caller, *chat, *seq, *after).await?;
                Ok(ServerFrame::Times {
                    reference,
                    chat: reply.chat,
                    seq: reply.seq,
                    member_list: reply.member_list,
                    more: reply.more,
                })
            });
            return Some(answer_of(reference, result));
        }
        ClientFrame::Presence {
            in_app,
            trouble,
            text,
        } => {
            let input = PresenceBody {
                in_app: *in_app,
                trouble: *trouble,
                text: text.clone(),
            };
            let result = operation!("Presence", "presence", run_presence(app, &caller, input));
            if let Err(refusal) = result {
                tracing::debug!(msg = "Dropped a presence event", code = refusal.code);
            }
            return None;
        }
        ClientFrame::PresenceOf { reference, account } => {
            let (reference, account) = (*reference, *account);
            let result = operation!("PresenceOf", "presence_of", async {
                let presence = run_presence_of(app, &caller, account).await?;
                Ok(ServerFrame::PresenceOf {
                    reference,
                    presence,
                })
            });
            return Some(answer_of(reference, result));
        }
        ClientFrame::Kv { reference, version } => {
            let (reference, version) = (*reference, *version);
            let result = operation!("Kv", "kv", async {
                let reply = run_kv(app, &caller, version).await?;
                Ok(ServerFrame::Kv {
                    reference: Some(reference),
                    version: reply.version,
                    kv_list: reply.kv_list,
                    from: None,
                    set: None,
                    remove: None,
                })
            });
            return Some(answer_of(reference, result));
        }
        ClientFrame::KvSet { reference, change } => {
            let (reference, change) = (*reference, change.clone());
            let result = operation!("KvSet", "kv_set", async {
                let reply = run_kv_set(app, &caller, change).await?;
                Ok(ServerFrame::Kv {
                    reference: Some(reference),
                    version: reply.version,
                    kv_list: None,
                    from: None,
                    set: None,
                    remove: None,
                })
            });
            return Some(answer_of(reference, result));
        }
        ClientFrame::Draft {
            reference,
            chat,
            draft,
        } => {
            let (reference, chat, draft) = (*reference, *chat, draft.clone());
            let result = operation!("Draft", "draft", async {
                run_put_draft(app, &caller, chat, draft).await?;
                Ok(ServerFrame::Done { reference })
            });
            return Some(answer_of(reference, result));
        }
        ClientFrame::FolderId { reference } => {
            let reference = *reference;
            let result = operation!("FolderId", "folder_id", async {
                let reply = run_folder_id(app, &caller).await?;
                Ok(ServerFrame::FolderId {
                    reference,
                    folder: reply.folder,
                })
            });
            return Some(answer_of(reference, result));
        }
        ClientFrame::ChatFolders { reference, folders } => {
            let (reference, folders) = (*reference, folders.clone());
            let result = operation!("ChatFolders", "chat_folders", async {
                run_put_chat_folders(app, &caller, folders).await?;
                Ok(ServerFrame::Done { reference })
            });
            return Some(answer_of(reference, result));
        }
        ClientFrame::Sync {
            reference,
            since,
            after,
            chat,
        } => {
            let (reference, since, after, chat) = (*reference, *since, *after, *chat);
            let result = operation!("Sync", "sync", async {
                check(app, &caller, Need::Session).await?;
                if let Some(chat) = chat {
                    check(app, &caller, Need::Member(chat)).await?;
                }
                // A continued page starts exactly after the last row. A first one goes a second back,
                // for rows saved just before `since` whose writes landed after it was read.
                let from = match after {
                    Some(position) => SyncFrom::After(position),
                    None => {
                        SyncFrom::Since(since.map_or(0, |since| since.saturating_sub(SYNC_BACK_MS)))
                    }
                };
                // The chats list, folders, requests and invitations come on the first page only.
                let mut chat_list = Vec::new();
                let mut chat_folders = None;
                let mut asked_list = Vec::new();
                let mut invitation_list = Vec::new();
                if after.is_none() {
                    let expire_before = Ulid::now() - invitation_ms(app);
                    chat_list = app
                        .storage
                        .chat_list(session.account, Ulid::now() - invitation_ms(app))
                        .await?;
                    chat_folders = Some(app.storage.chat_folders(session.account).await?.0);
                    asked_list = app
                        .storage
                        .own_requests(session.account, expire_before)
                        .await?;
                    invitation_list = app
                        .storage
                        .own_invitations(session.account, expire_before)
                        .await?;
                }
                let page = app
                    .storage
                    .sync(
                        session.account,
                        chat,
                        from,
                        app.settings.limits.history_page,
                    )
                    .await?;
                Ok(ServerFrame::Chats {
                    reference,
                    chat_list,
                    message_list: page.message_list,
                    call_list: page.call_list,
                    chat_event_list: page.chat_event_list,
                    more: page.more,
                    chat_folders,
                    asked_list,
                    invitation_list,
                })
            });
            return Some(answer_of(reference, result));
        }
        ClientFrame::ChatRemove { reference, chat } => {
            let (reference, chat) = (*reference, *chat);
            let result = operation!("ChatRemove", "chat_remove", async {
                run_chat_remove(app, &caller, chat).await?;
                Ok(ServerFrame::Done { reference })
            });
            return Some(answer_of(reference, result));
        }
        ClientFrame::Leave { reference, chat } => {
            let (reference, chat) = (*reference, *chat);
            let result = operation!("Leave", "leave", async {
                run_leave(app, &caller, chat).await?;
                Ok(ServerFrame::Done { reference })
            });
            return Some(answer_of(reference, result));
        }
        ClientFrame::ChatRole {
            reference,
            chat,
            account,
            role,
        } => {
            let (reference, chat, account, role) = (*reference, *chat, *account, *role);
            let result = operation!("ChatRole", "chat_role", async {
                run_chat_role(app, &caller, chat, account, role).await?;
                Ok(ServerFrame::Done { reference })
            });
            return Some(answer_of(reference, result));
        }
        ClientFrame::JoinRequest {
            reference,
            chat,
            note,
        } => {
            let (reference, chat, note) = (*reference, *chat, note.clone());
            let result = operation!("JoinRequest", "join_request", async {
                run_join_request(app, &caller, chat, note).await?;
                Ok(ServerFrame::Done { reference })
            });
            return Some(answer_of(reference, result));
        }
        ClientFrame::JoinCancel { reference, chat } => {
            let (reference, chat) = (*reference, *chat);
            let result = operation!("JoinCancel", "join_cancel", async {
                run_join_cancel(app, &caller, chat).await?;
                Ok(ServerFrame::Done { reference })
            });
            return Some(answer_of(reference, result));
        }
        ClientFrame::JoinDecline {
            reference,
            chat,
            account,
        } => {
            let (reference, chat, account) = (*reference, *chat, *account);
            let result = operation!("JoinDecline", "join_decline", async {
                run_join_decline(app, &caller, chat, account).await?;
                Ok(ServerFrame::Done { reference })
            });
            return Some(answer_of(reference, result));
        }
        ClientFrame::Invite {
            reference,
            chat,
            account,
            sealed,
            signature,
        } => {
            let (reference, chat) = (*reference, *chat);
            let input = InviteBody {
                account: *account,
                sealed: sealed.clone(),
                signature: signature.clone(),
            };
            let result = operation!("Invite", "invite", async {
                run_invite(app, &caller, chat, input).await?;
                Ok(ServerFrame::Done { reference })
            });
            return Some(answer_of(reference, result));
        }
        ClientFrame::InviteAnswer {
            reference,
            chat,
            accept,
        } => {
            let (reference, chat, accept) = (*reference, *chat, *accept);
            let result = operation!("InviteAnswer", "invite_answer", async {
                run_invite_answer(app, &caller, chat, accept).await?;
                Ok(ServerFrame::Done { reference })
            });
            return Some(answer_of(reference, result));
        }
        ClientFrame::Members { reference, chat } => {
            let (reference, chat) = (*reference, *chat);
            let result = operation!("Members", "members", async {
                let reply = run_members(app, &caller, chat).await?;
                Ok(ServerFrame::Members {
                    reference,
                    chat,
                    member_list: reply.member_list,
                })
            });
            return Some(answer_of(reference, result));
        }
        ClientFrame::TopicId { reference, chat } => {
            let (reference, chat) = (*reference, *chat);
            let result = operation!("TopicId", "topic_id", async {
                let reply = run_topic_id(app, &caller, chat).await?;
                Ok(ServerFrame::TopicId {
                    reference,
                    chat,
                    topic: reply.topic,
                })
            });
            return Some(answer_of(reference, result));
        }
        ClientFrame::Topics {
            reference,
            chat,
            topics,
        } => {
            let (reference, chat, topics) = (*reference, *chat, topics.clone());
            let result = operation!("Topics", "topics", async {
                run_topics(app, &caller, chat, topics).await?;
                Ok(ServerFrame::Done { reference })
            });
            return Some(answer_of(reference, result));
        }
        ClientFrame::ChatProfile {
            reference,
            chat,
            profile,
        } => {
            let (reference, chat, profile) = (*reference, *chat, profile.clone());
            let result = operation!("ChatProfile", "chat_profile", async {
                run_chat_profile(app, &caller, chat, profile).await?;
                Ok(ServerFrame::Done { reference })
            });
            return Some(answer_of(reference, result));
        }
        ClientFrame::ChatOptions {
            reference,
            chat,
            options,
        } => {
            let (reference, chat, options) = (*reference, *chat, options.clone());
            let result = operation!("ChatOptions", "chat_options", async {
                run_chat_options(app, &caller, chat, options).await?;
                Ok(ServerFrame::Done { reference })
            });
            return Some(answer_of(reference, result));
        }
        ClientFrame::TopicOptions {
            reference,
            chat,
            topic,
            options,
        } => {
            let (reference, chat, topic, options) = (*reference, *chat, *topic, options.clone());
            let result = operation!("TopicOptions", "topic_options", async {
                run_topic_options(app, &caller, chat, topic, *options).await?;
                Ok(ServerFrame::Done { reference })
            });
            return Some(answer_of(reference, result));
        }
        ClientFrame::TopicProfile {
            reference,
            chat,
            topic,
            profile,
        } => {
            let (reference, chat, topic, profile) = (*reference, *chat, *topic, profile.clone());
            let result = operation!("TopicProfile", "topic_profile", async {
                run_topic_profile(app, &caller, chat, topic, *profile).await?;
                Ok(ServerFrame::Done { reference })
            });
            return Some(answer_of(reference, result));
        }
        ClientFrame::TopicRole {
            reference,
            chat,
            topic,
            account,
            role,
        } => {
            let (reference, chat, topic, account, role) =
                (*reference, *chat, *topic, *account, *role);
            let result = operation!("TopicRole", "topic_role", async {
                run_topic_role(app, &caller, chat, topic, account, role).await?;
                Ok(ServerFrame::Done { reference })
            });
            return Some(answer_of(reference, result));
        }
        ClientFrame::Findable {
            reference,
            chat,
            findable,
        } => {
            let (reference, chat, findable) = (*reference, *chat, findable.clone());
            let result = operation!("Findable", "findable", async {
                run_findable(app, &caller, chat, findable).await?;
                Ok(ServerFrame::Done { reference })
            });
            return Some(answer_of(reference, result));
        }
        ClientFrame::Send(frame) => (frame.chat, Some(frame.reference)),
        ClientFrame::Seen {
            reference, chat, ..
        }
        | ClientFrame::Received {
            reference, chat, ..
        }
        | ClientFrame::GroupInfo {
            reference, chat, ..
        }
        | ClientFrame::Wraps {
            reference, chat, ..
        }
        | ClientFrame::ProfileKeys {
            reference, chat, ..
        }
        | ClientFrame::CallEvent {
            reference, chat, ..
        } => (*chat, Some(*reference)),
        ClientFrame::Typing { chat, .. } => (*chat, None),
        // The socket's own loop answers these, so that a slow request cannot delay one.
        ClientFrame::Ping { reference } => {
            return Some(ServerFrame::Done {
                reference: *reference,
            });
        }
    };
    let Some(reference) = reference else {
        let job = Job {
            trace: current_trace(),
            session,
            work: Work::Frame(frame),
            reply: None,
        };
        app.hub.submit(app, chat, job).await;
        return None;
    };
    if *app.stopping.borrow() {
        return Some(answer_of(
            reference,
            Err(refused(503, "shutting_down", "The server is shutting down")),
        ));
    }
    let (reply, answer) = oneshot::channel();
    let job = Job {
        trace: current_trace(),
        session,
        work: Work::Frame(frame),
        reply: Some(reply),
    };
    app.hub.submit(app, chat, job).await;
    match answer.await {
        Ok(frame) => Some(frame),
        Err(_) => Some(answer_of(
            reference,
            Err(refused(500, "internal", "Something failed on the server")),
        )),
    }
}

// -------------------------------------------------------------------------------------------------
// REST
// -------------------------------------------------------------------------------------------------

fn respond<T: Serialize>(result: Result<T, Refused>) -> Response {
    match result {
        Ok(data) => (
            StatusCode::OK,
            [(CONTENT_TYPE, "application/json")],
            json!({"ok": true, "data": data}).to_string(),
        )
            .into_response(),
        Err(refusal) => refusal.into_response(),
    }
}

impl IntoResponse for Refused {
    fn into_response(self) -> Response {
        let status = match StatusCode::from_u16(self.status) {
            Ok(status) => status,
            Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, [(CONTENT_TYPE, "application/json")], {
            let mut body = json!({"ok": false, "code": self.code, "message": self.message});
            if let Some(call_seq) = self.call_seq {
                body["call_seq"] = json!(call_seq);
            }
            if let Some(seconds) = self.seconds {
                body["seconds"] = json!(seconds);
            }
            if let Some(account_list) = self.account_list {
                body["account_list"] = json!(account_list);
            }
            if let Some(kv) = self.kv {
                body["version"] = json!(kv.version);
                body["kv_list"] = json!(kv.kv_list);
            }
            body.to_string()
        })
            .into_response()
    }
}

type Body = Result<RawBody, BytesRejection>;

fn parse<T: DeserializeOwned>(body: Body) -> Result<T, Refused> {
    let bytes = match body {
        Ok(bytes) => bytes,
        Err(rejection) if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE => {
            return Err(refused(413, "body_too_large", "That body is too large"));
        }
        Err(_) => return Err(refused(400, "bad_body", "That body could not be read")),
    };
    match serde_json::from_slice(&bytes) {
        Ok(value) => Ok(value),
        Err(_) => Err(refused(400, "bad_body", "That body is not what this needs")),
    }
}

fn path_id(text: &str) -> Result<Ulid, Refused> {
    match Ulid::parse(text) {
        Ok(id) => Ok(id),
        Err(_) => Err(refused(404, "not_found", "There is nothing there")),
    }
}

/// Who a request's token says it is. The operator's token is compared as hashes, so the comparison
/// takes the same time however much of a guess is right.
async fn caller_of(app: &App, headers: &HeaderMap) -> Result<Caller, Refused> {
    let mut token = None;
    if let Some(value) = headers.get(AUTHORIZATION)
        && let Ok(text) = value.to_str()
        && let Some(bearer) = text.strip_prefix("Bearer ")
    {
        if let Some(operator) = &app.settings.operator
            && sha256(bearer.as_bytes()) == sha256(operator.token.as_bytes())
        {
            return Ok(Caller::Operator);
        }
        token = Some(bearer.to_string());
    }
    if token.is_none() {
        for value in headers.get_all(COOKIE) {
            let Ok(text) = value.to_str() else {
                continue;
            };
            for pair in text.split(';') {
                if let Some((name, value)) = pair.trim().split_once('=')
                    && name == COOKIE_NAME
                {
                    token = Some(value.to_string());
                }
            }
        }
    }
    let Some(token) = token else {
        return Ok(Caller::Nobody);
    };
    let now = Ulid::now();
    let expires_at = now.saturating_add(millis(app.settings.session.lifetime));
    match app
        .storage
        .session_by_token(sha256(token.as_bytes()), now, expires_at)
        .await?
    {
        Some(session) => Ok(Caller::Session(session)),
        None => Ok(Caller::Nobody),
    }
}

/// From what proxies set, falling back to the socket. A hint and not evidence, since a client that
/// reaches the server directly can send these headers itself.
fn client_ip(headers: &HeaderMap, address: SocketAddr) -> String {
    if let Some(value) = headers.get("x-real-ip")
        && let Ok(text) = value.to_str()
        && !text.trim().is_empty()
    {
        return text.trim().to_string();
    }
    if let Some(value) = headers.get("x-forwarded-for")
        && let Ok(text) = value.to_str()
        && let Some(first) = text.split(',').next()
        && !first.trim().is_empty()
    {
        return first.trim().to_string();
    }
    address.ip().to_string()
}

type Shared = State<Arc<App>>;
type Address = ConnectInfo<SocketAddr>;

async fn guide(State(app): Shared) -> Response {
    let document =
        include_str!("guide.md").replace("{origin}", app.settings.public_url.trim_end_matches('/'));
    ([(CONTENT_TYPE, "text/markdown; charset=utf-8")], document).into_response()
}

async fn captcha_route(State(app): Shared) -> Response {
    respond(operation!("Captcha", "captcha", captcha::run_issue(&app)))
}

async fn features(State(app): Shared, headers: HeaderMap) -> Response {
    respond(operation!("Features", "features", async {
        let caller = caller_of(&app, &headers).await?;
        run_features(&app, &caller).await
    }))
}

async fn register(State(app): Shared, headers: HeaderMap, body: Body) -> Response {
    respond(operation!("Register", "register", async {
        let caller = caller_of(&app, &headers).await?;
        run_register(&app, &caller, parse(body)?).await
    }))
}

async fn login(State(app): Shared, headers: HeaderMap, body: Body) -> Response {
    let result = operation!("Login", "login", async {
        let caller = caller_of(&app, &headers).await?;
        run_login(&app, &caller, parse(body)?).await
    });
    match result {
        Ok((data, token)) => {
            let secure = if app.settings.public_url.starts_with("https://") {
                "; Secure"
            } else {
                ""
            };
            let cookie = format!(
                "{COOKIE_NAME}={token}; HttpOnly; SameSite=Strict; Path=/; Max-Age={}{secure}",
                app.settings.session.lifetime
            );
            let mut response = respond(Ok(data));
            match HeaderValue::from_str(&cookie) {
                Ok(value) => {
                    response.headers_mut().insert(SET_COOKIE, value);
                }
                Err(_) => {
                    return refused(500, "internal", "Something failed on the server")
                        .into_response();
                }
            }
            response
        }
        Err(refusal) => refusal.into_response(),
    }
}

async fn login_token(State(app): Shared, headers: HeaderMap, body: Body) -> Response {
    respond(operation!("Login", "login_token", async {
        let caller = caller_of(&app, &headers).await?;
        let (mut data, token) = run_login(&app, &caller, parse(body)?).await?;
        data.token = Some(token);
        Ok(data)
    }))
}

async fn login_agent(State(app): Shared, headers: HeaderMap, body: Body) -> Response {
    respond(operation!("LoginAgent", "login_agent", async {
        let caller = caller_of(&app, &headers).await?;
        let (mut data, token) = run_login_agent(&app, &caller, parse(body)?).await?;
        data.token = Some(token);
        Ok(data)
    }))
}

async fn agents(State(app): Shared, headers: HeaderMap) -> Response {
    respond(operation!("Agents", "agents", async {
        let caller = caller_of(&app, &headers).await?;
        run_agents(&app, &caller).await
    }))
}

async fn create_agent(State(app): Shared, headers: HeaderMap, body: Body) -> Response {
    respond(operation!("CreateAgent", "create_agent", async {
        let caller = caller_of(&app, &headers).await?;
        run_create_agent(&app, &caller, parse(body)?).await
    }))
}

async fn update_agent(
    State(app): Shared,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Body,
) -> Response {
    respond(operation!("UpdateAgent", "update_agent", async {
        let caller = caller_of(&app, &headers).await?;
        run_update_agent(&app, &caller, path_id(&id)?, parse(body)?).await
    }))
}

async fn renew_agent_token(
    State(app): Shared,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Body,
) -> Response {
    respond(operation!("RenewAgentToken", "renew_agent_token", async {
        let caller = caller_of(&app, &headers).await?;
        run_renew_agent_token(&app, &caller, path_id(&id)?, parse(body)?).await
    }))
}

async fn revoke_agent_token(
    State(app): Shared,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Body,
) -> Response {
    respond(operation!(
        "RevokeAgentToken",
        "revoke_agent_token",
        async {
            let caller = caller_of(&app, &headers).await?;
            run_revoke_agent_token(&app, &caller, path_id(&id)?, parse(body)?).await
        }
    ))
}

async fn disable_agent(
    State(app): Shared,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Body,
) -> Response {
    respond(operation!("DisableAgent", "disable_agent", async {
        let caller = caller_of(&app, &headers).await?;
        run_disable_agent(&app, &caller, path_id(&id)?, parse(body)?).await
    }))
}

async fn agent_keys(
    State(app): Shared,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Body,
) -> Response {
    respond(operation!("AgentKeys", "agent_keys", async {
        let caller = caller_of(&app, &headers).await?;
        run_agent_keys(&app, &caller, path_id(&id)?, parse(body)?).await
    }))
}

async fn setup(State(app): Shared, headers: HeaderMap, body: Body) -> Response {
    respond(operation!("Setup", "setup", async {
        let caller = caller_of(&app, &headers).await?;
        run_setup(&app, &caller, parse(body)?).await
    }))
}

async fn challenge(State(app): Shared, headers: HeaderMap, body: Body) -> Response {
    respond(operation!("Challenge", "challenge", async {
        let caller = caller_of(&app, &headers).await?;
        run_challenge(&app, &caller, parse(body)?).await
    }))
}

async fn password_reset(State(app): Shared, headers: HeaderMap, body: Body) -> Response {
    respond(operation!("PasswordReset", "password_reset", async {
        let caller = caller_of(&app, &headers).await?;
        run_password_reset(&app, &caller, parse(body)?).await
    }))
}

async fn sessions(State(app): Shared, headers: HeaderMap) -> Response {
    respond(operation!("Sessions", "sessions", async {
        let caller = caller_of(&app, &headers).await?;
        run_sessions(&app, &caller).await
    }))
}

async fn delete_session(
    State(app): Shared,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    respond(operation!("DeleteSession", "delete_session", async {
        let caller = caller_of(&app, &headers).await?;
        run_delete_session(&app, &caller, path_id(&id)?).await
    }))
}

async fn account_keys(State(app): Shared, headers: HeaderMap, body: Body) -> Response {
    respond(operation!("AccountKeys", "account_keys", async {
        let caller = caller_of(&app, &headers).await?;
        run_account_keys(&app, &caller, parse(body)?).await
    }))
}

async fn create_account(State(app): Shared, headers: HeaderMap, body: Body) -> Response {
    respond(operation!("CreateAccount", "create_account", async {
        let caller = caller_of(&app, &headers).await?;
        run_create_account(&app, &caller, parse(body)?).await
    }))
}

#[derive(Deserialize)]
struct DirectoryQuery {
    #[serde(default)]
    search: String,
}

async fn directory(
    State(app): Shared,
    headers: HeaderMap,
    Query(query): Query<DirectoryQuery>,
) -> Response {
    respond(operation!("Directory", "directory", async {
        let caller = caller_of(&app, &headers).await?;
        run_directory(&app, &caller, query.search).await
    }))
}

async fn account(State(app): Shared, headers: HeaderMap, Path(id): Path<String>) -> Response {
    respond(operation!("Account", "account", async {
        let caller = caller_of(&app, &headers).await?;
        run_account(&app, &caller, path_id(&id)?).await
    }))
}

async fn devices(State(app): Shared, headers: HeaderMap) -> Response {
    respond(operation!("Devices", "devices", async {
        let caller = caller_of(&app, &headers).await?;
        run_devices(&app, &caller).await
    }))
}

async fn device_id(State(app): Shared, headers: HeaderMap) -> Response {
    respond(operation!("DeviceId", "device_id", async {
        let caller = caller_of(&app, &headers).await?;
        run_device_id(&app, &caller).await
    }))
}

async fn register_device(
    State(app): Shared,
    ConnectInfo(address): Address,
    headers: HeaderMap,
    body: Body,
) -> Response {
    respond(operation!("RegisterDevice", "register_device", async {
        let caller = caller_of(&app, &headers).await?;
        let ip = client_ip(&headers, address);
        run_register_device(&app, &caller, parse(body)?, ip).await
    }))
}

async fn terminate(
    State(app): Shared,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Body,
) -> Response {
    respond(operation!("Terminate", "terminate", async {
        let caller = caller_of(&app, &headers).await?;
        run_terminate(&app, &caller, path_id(&id)?, parse(body)?).await
    }))
}

async fn logout(
    State(app): Shared,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Body,
) -> Response {
    respond(operation!("Logout", "logout", async {
        let caller = caller_of(&app, &headers).await?;
        run_logout(&app, &caller, path_id(&id)?, parse(body)?).await
    }))
}

async fn put_presence(
    State(app): Shared,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Body,
) -> Response {
    respond(operation!("PutPresence", "put_presence", async {
        let caller = caller_of(&app, &headers).await?;
        run_put_presence(&app, &caller, path_id(&id)?, parse(body)?).await
    }))
}

async fn presence_of(State(app): Shared, headers: HeaderMap, Path(id): Path<String>) -> Response {
    respond(operation!("PresenceOf", "presence_of", async {
        let caller = caller_of(&app, &headers).await?;
        run_presence_of(&app, &caller, path_id(&id)?).await
    }))
}

#[derive(Deserialize)]
struct KvQuery {
    version: Option<u64>,
}

async fn kv(State(app): Shared, headers: HeaderMap, Query(query): Query<KvQuery>) -> Response {
    respond(operation!("Kv", "kv", async {
        let caller = caller_of(&app, &headers).await?;
        run_kv(&app, &caller, query.version).await
    }))
}

async fn patch_kv(State(app): Shared, headers: HeaderMap, body: Body) -> Response {
    respond(operation!("KvSet", "kv_set", async {
        let caller = caller_of(&app, &headers).await?;
        run_kv_set(&app, &caller, parse(body)?).await
    }))
}

async fn key_packages(State(app): Shared, headers: HeaderMap, body: Body) -> Response {
    respond(operation!("KeyPackages", "key_packages", async {
        let caller = caller_of(&app, &headers).await?;
        run_key_packages(&app, &caller, parse(body)?).await
    }))
}

async fn claim_key_packages(
    State(app): Shared,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Body,
) -> Response {
    respond(operation!(
        "ClaimKeyPackages",
        "claim_key_packages",
        async {
            let caller = caller_of(&app, &headers).await?;
            let input = match &body {
                Ok(bytes) if bytes.is_empty() => ClaimBody::default(),
                _ => parse(body)?,
            };
            run_claim_key_packages(&app, &caller, path_id(&id)?, input).await
        }
    ))
}

/// Whether this account's switch turns every add to this type of chat into an invitation: a `kv`
/// key left unsealed because the server acts on it. Someone who asked to join this chat is let in
/// whatever it says.
pub async fn invite_only(
    app: &App,
    chat: Ulid,
    account: Ulid,
    chat_type: ChatType,
) -> Result<bool, Refused> {
    let expire_before = Ulid::now() - invitation_ms(app);
    for request in app.storage.own_requests(account, expire_before).await? {
        if request.chat == chat && request.state == RequestState::Waiting {
            return Ok(false);
        }
    }
    let key = if chat_type == ChatType::Channel {
        "privacy.add.channel"
    } else {
        "privacy.add.group"
    };
    if let Some(kv_list) = app.storage.kv(account, None).await?.kv_list {
        for item in kv_list {
            if item.key == key && item.value.0 == b"\"nobody\"" {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

async fn create_chat(State(app): Shared, headers: HeaderMap, body: Body) -> Response {
    respond(operation!("CreateChat", "create_chat", async {
        let caller = caller_of(&app, &headers).await?;
        run_create_chat(&app, &caller, parse(body)?).await
    }))
}

async fn chat_remove(State(app): Shared, headers: HeaderMap, Path(id): Path<String>) -> Response {
    respond(operation!("ChatRemove", "chat_remove", async {
        let caller = caller_of(&app, &headers).await?;
        run_chat_remove(&app, &caller, path_id(&id)?).await
    }))
}

async fn leave(State(app): Shared, headers: HeaderMap, Path(id): Path<String>) -> Response {
    respond(operation!("Leave", "leave", async {
        let caller = caller_of(&app, &headers).await?;
        run_leave(&app, &caller, path_id(&id)?).await
    }))
}

async fn chat_role(
    State(app): Shared,
    headers: HeaderMap,
    Path((id, account)): Path<(String, String)>,
    body: Body,
) -> Response {
    respond(operation!("ChatRole", "chat_role", async {
        let caller = caller_of(&app, &headers).await?;
        let input: RoleBody = parse(body)?;
        run_chat_role(&app, &caller, path_id(&id)?, path_id(&account)?, input.role).await
    }))
}

async fn join_request(
    State(app): Shared,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Body,
) -> Response {
    respond(operation!("JoinRequest", "join_request", async {
        let caller = caller_of(&app, &headers).await?;
        let input: RequestBody = parse(body)?;
        run_join_request(&app, &caller, path_id(&id)?, input.note).await
    }))
}

async fn join_cancel(State(app): Shared, headers: HeaderMap, Path(id): Path<String>) -> Response {
    respond(operation!("JoinCancel", "join_cancel", async {
        let caller = caller_of(&app, &headers).await?;
        run_join_cancel(&app, &caller, path_id(&id)?).await
    }))
}

async fn join_decline(
    State(app): Shared,
    headers: HeaderMap,
    Path((id, account)): Path<(String, String)>,
) -> Response {
    respond(operation!("JoinDecline", "join_decline", async {
        let caller = caller_of(&app, &headers).await?;
        run_join_decline(&app, &caller, path_id(&id)?, path_id(&account)?).await
    }))
}

async fn requests(State(app): Shared, headers: HeaderMap, Path(id): Path<String>) -> Response {
    respond(operation!("Requests", "requests", async {
        let caller = caller_of(&app, &headers).await?;
        run_requests(&app, &caller, path_id(&id)?).await
    }))
}

async fn invite(
    State(app): Shared,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Body,
) -> Response {
    respond(operation!("Invite", "invite", async {
        let caller = caller_of(&app, &headers).await?;
        run_invite(&app, &caller, path_id(&id)?, parse(body)?).await
    }))
}

async fn invite_answer(
    State(app): Shared,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Body,
) -> Response {
    respond(operation!("InviteAnswer", "invite_answer", async {
        let caller = caller_of(&app, &headers).await?;
        let input: InviteAnswerBody = parse(body)?;
        run_invite_answer(&app, &caller, path_id(&id)?, input.accept).await
    }))
}

async fn members(State(app): Shared, headers: HeaderMap, Path(id): Path<String>) -> Response {
    respond(operation!("Members", "members", async {
        let caller = caller_of(&app, &headers).await?;
        run_members(&app, &caller, path_id(&id)?).await
    }))
}

async fn topic_id(State(app): Shared, headers: HeaderMap, Path(id): Path<String>) -> Response {
    respond(operation!("TopicId", "topic_id", async {
        let caller = caller_of(&app, &headers).await?;
        run_topic_id(&app, &caller, path_id(&id)?).await
    }))
}

async fn put_topics(
    State(app): Shared,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Body,
) -> Response {
    respond(operation!("Topics", "topics", async {
        let caller = caller_of(&app, &headers).await?;
        run_topics(&app, &caller, path_id(&id)?, parse(body)?).await
    }))
}

async fn put_chat_profile(
    State(app): Shared,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Body,
) -> Response {
    respond(operation!("ChatProfile", "chat_profile", async {
        let caller = caller_of(&app, &headers).await?;
        run_chat_profile(&app, &caller, path_id(&id)?, parse(body)?).await
    }))
}

async fn put_chat_options(
    State(app): Shared,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Body,
) -> Response {
    respond(operation!("ChatOptions", "chat_options", async {
        let caller = caller_of(&app, &headers).await?;
        run_chat_options(&app, &caller, path_id(&id)?, parse(body)?).await
    }))
}

async fn put_topic_options(
    State(app): Shared,
    headers: HeaderMap,
    Path((id, topic)): Path<(String, String)>,
    body: Body,
) -> Response {
    respond(operation!("TopicOptions", "topic_options", async {
        let caller = caller_of(&app, &headers).await?;
        run_topic_options(&app, &caller, path_id(&id)?, path_id(&topic)?, parse(body)?).await
    }))
}

async fn put_topic_profile(
    State(app): Shared,
    headers: HeaderMap,
    Path((id, topic)): Path<(String, String)>,
    body: Body,
) -> Response {
    respond(operation!("TopicProfile", "topic_profile", async {
        let caller = caller_of(&app, &headers).await?;
        run_topic_profile(&app, &caller, path_id(&id)?, path_id(&topic)?, parse(body)?).await
    }))
}

/// The body of `PUT /api/v1/chats/{id}/topics/{topic}/members/{account}/role`.
#[derive(Deserialize)]
struct TopicRoleBody {
    role: ChatRole,
}

async fn put_topic_role(
    State(app): Shared,
    headers: HeaderMap,
    Path((id, topic, account)): Path<(String, String, String)>,
    body: Body,
) -> Response {
    respond(operation!("TopicRole", "topic_role", async {
        let caller = caller_of(&app, &headers).await?;
        let input: TopicRoleBody = parse(body)?;
        run_topic_role(
            &app,
            &caller,
            path_id(&id)?,
            path_id(&topic)?,
            path_id(&account)?,
            input.role,
        )
        .await
    }))
}

async fn put_findable(
    State(app): Shared,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Body,
) -> Response {
    respond(operation!("Findable", "findable", async {
        let caller = caller_of(&app, &headers).await?;
        run_findable(&app, &caller, path_id(&id)?, parse(body)?).await
    }))
}

async fn group_info(State(app): Shared, headers: HeaderMap, Path(id): Path<String>) -> Response {
    respond(operation!("GroupInfo", "group_info", async {
        let caller = caller_of(&app, &headers).await?;
        run_group_info(&app, &caller, path_id(&id)?).await
    }))
}

async fn chat_devices(State(app): Shared, headers: HeaderMap, Path(id): Path<String>) -> Response {
    respond(operation!("ChatDevices", "chat_devices", async {
        let caller = caller_of(&app, &headers).await?;
        run_chat_devices(&app, &caller, path_id(&id)?).await
    }))
}

#[derive(Deserialize)]
struct TimesQuery {
    after: Option<String>,
}

async fn message_times(
    State(app): Shared,
    headers: HeaderMap,
    Path((id, seq)): Path<(String, String)>,
    Query(query): Query<TimesQuery>,
) -> Response {
    respond(operation!("MessageTimes", "message_times", async {
        let caller = caller_of(&app, &headers).await?;
        let after = match query.after.as_deref().map(Ulid::parse) {
            None => None,
            Some(Ok(account)) => Some(account),
            Some(Err(_)) => return Err(refused(400, "bad_body", "after is not an account id")),
        };
        run_message_times(&app, &caller, path_id(&id)?, chunk_number(&seq)?, after).await
    }))
}

async fn folder_id(State(app): Shared, headers: HeaderMap) -> Response {
    respond(operation!("FolderId", "folder_id", async {
        let caller = caller_of(&app, &headers).await?;
        run_folder_id(&app, &caller).await
    }))
}

async fn put_chat_folders(State(app): Shared, headers: HeaderMap, body: Body) -> Response {
    respond(operation!("PutChatFolders", "put_chat_folders", async {
        let caller = caller_of(&app, &headers).await?;
        run_put_chat_folders(&app, &caller, parse(body)?).await
    }))
}

async fn put_draft(
    State(app): Shared,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Body,
) -> Response {
    respond(operation!("PutDraft", "put_draft", async {
        let caller = caller_of(&app, &headers).await?;
        let request: DraftRequest = parse(body)?;
        run_put_draft(&app, &caller, path_id(&id)?, request.draft).await
    }))
}

async fn chat_wraps(State(app): Shared, headers: HeaderMap, Path(id): Path<String>) -> Response {
    respond(operation!("ChatWraps", "chat_wraps", async {
        let caller = caller_of(&app, &headers).await?;
        run_chat_wraps(&app, &caller, path_id(&id)?).await
    }))
}

async fn create_file(
    State(app): Shared,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Body,
) -> Response {
    respond(operation!("CreateFile", "create_file", async {
        let caller = caller_of(&app, &headers).await?;
        run_create_file(&app, &caller, path_id(&id)?, parse(body)?).await
    }))
}

fn chunk_number(text: &str) -> Result<u64, Refused> {
    match text.parse() {
        Ok(n) => Ok(n),
        Err(_) => Err(refused(404, "not_found", "There is nothing there")),
    }
}

async fn put_chunk(
    State(app): Shared,
    headers: HeaderMap,
    Path((id, n)): Path<(String, String)>,
    body: Body,
) -> Response {
    respond(operation!("PutChunk", "put_chunk", async {
        let caller = caller_of(&app, &headers).await?;
        let bytes = match body {
            Ok(bytes) => bytes.to_vec(),
            Err(rejection) if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE => {
                return Err(refused(413, "body_too_large", "That body is too large"));
            }
            Err(_) => return Err(refused(400, "bad_body", "That body could not be read")),
        };
        run_put_chunk(&app, &caller, path_id(&id)?, chunk_number(&n)?, bytes).await
    }))
}

async fn file_status(State(app): Shared, headers: HeaderMap, Path(id): Path<String>) -> Response {
    respond(operation!("FileStatus", "file_status", async {
        let caller = caller_of(&app, &headers).await?;
        run_file_status(&app, &caller, path_id(&id)?).await
    }))
}

async fn get_chunk(
    State(app): Shared,
    headers: HeaderMap,
    Path((id, n)): Path<(String, String)>,
) -> Response {
    let result = operation!("GetChunk", "get_chunk", async {
        let caller = caller_of(&app, &headers).await?;
        run_get_chunk(&app, &caller, path_id(&id)?, chunk_number(&n)?).await
    });
    match result {
        Ok(bytes) => (
            StatusCode::OK,
            [(CONTENT_TYPE, "application/octet-stream")],
            bytes,
        )
            .into_response(),
        Err(refusal) => refusal.into_response(),
    }
}

async fn sticker_list(State(app): Shared, headers: HeaderMap) -> Response {
    respond(operation!("StickerList", "sticker_list", async {
        let caller = caller_of(&app, &headers).await?;
        run_sticker_list(&app, &caller).await
    }))
}

async fn sticker_image(State(app): Shared, headers: HeaderMap, Path(id): Path<String>) -> Response {
    let result = operation!("StickerImage", "sticker_image", async {
        let caller = caller_of(&app, &headers).await?;
        run_sticker_image(&app, &caller, path_id(&id)?).await
    });
    match result {
        Ok(bytes) => (
            StatusCode::OK,
            [
                (CONTENT_TYPE, "image/webp"),
                (CACHE_CONTROL, "private, max-age=31536000, immutable"),
            ],
            bytes,
        )
            .into_response(),
        Err(refusal) => refusal.into_response(),
    }
}

async fn create_pack(State(app): Shared, headers: HeaderMap, body: Body) -> Response {
    respond(operation!("CreatePack", "create_pack", async {
        let caller = caller_of(&app, &headers).await?;
        run_create_pack(&app, &caller, parse(body)?).await
    }))
}

async fn remove_pack(State(app): Shared, headers: HeaderMap, Path(id): Path<String>) -> Response {
    respond(operation!("RemovePack", "remove_pack", async {
        let caller = caller_of(&app, &headers).await?;
        run_remove_pack(&app, &caller, path_id(&id)?).await
    }))
}

#[derive(Deserialize)]
struct StickerQuery {
    #[serde(default)]
    emoji: Option<String>,
}

async fn add_sticker(
    State(app): Shared,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<StickerQuery>,
    body: Body,
) -> Response {
    respond(operation!("AddSticker", "add_sticker", async {
        let caller = caller_of(&app, &headers).await?;
        let bytes = match body {
            Ok(bytes) => bytes.to_vec(),
            Err(rejection) if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE => {
                return Err(refused(413, "file_too_large", "That sticker is too large"));
            }
            Err(_) => return Err(refused(400, "bad_body", "That body could not be read")),
        };
        run_add_sticker(&app, &caller, path_id(&id)?, query.emoji, bytes).await
    }))
}

async fn remove_sticker(
    State(app): Shared,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    respond(operation!("RemoveSticker", "remove_sticker", async {
        let caller = caller_of(&app, &headers).await?;
        run_remove_sticker(&app, &caller, path_id(&id)?).await
    }))
}

async fn profile(State(app): Shared, headers: HeaderMap, Path(id): Path<String>) -> Response {
    respond(operation!("Profile", "profile", async {
        let caller = caller_of(&app, &headers).await?;
        run_profile(&app, &caller, path_id(&id)?).await
    }))
}

async fn put_profile(
    State(app): Shared,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Body,
) -> Response {
    respond(operation!("PutProfile", "put_profile", async {
        let caller = caller_of(&app, &headers).await?;
        run_put_profile(&app, &caller, path_id(&id)?, parse(body)?).await
    }))
}

async fn put_names(
    State(app): Shared,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Body,
) -> Response {
    respond(operation!("Names", "names", async {
        let caller = caller_of(&app, &headers).await?;
        run_names(&app, &caller, path_id(&id)?, parse(body)?).await
    }))
}

async fn add_photo(
    State(app): Shared,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Body,
) -> Response {
    respond(operation!("AddPhoto", "add_photo", async {
        let caller = caller_of(&app, &headers).await?;
        let bytes = match body {
            Ok(bytes) => bytes.to_vec(),
            Err(rejection) if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE => {
                return Err(refused(413, "file_too_large", "That photo is too large"));
            }
            Err(_) => return Err(refused(400, "bad_body", "That body could not be read")),
        };
        run_add_photo(&app, &caller, path_id(&id)?, bytes).await
    }))
}

async fn photo(State(app): Shared, headers: HeaderMap, Path(id): Path<String>) -> Response {
    let result = operation!("Photo", "photo", async {
        let caller = caller_of(&app, &headers).await?;
        run_photo(&app, &caller, path_id(&id)?).await
    });
    match result {
        Ok(bytes) => (
            StatusCode::OK,
            [
                (CONTENT_TYPE, "application/octet-stream"),
                (CACHE_CONTROL, "private, max-age=31536000, immutable"),
            ],
            bytes,
        )
            .into_response(),
        Err(refusal) => refusal.into_response(),
    }
}

async fn remove_photo(State(app): Shared, headers: HeaderMap, Path(id): Path<String>) -> Response {
    respond(operation!("RemovePhoto", "remove_photo", async {
        let caller = caller_of(&app, &headers).await?;
        run_remove_photo(&app, &caller, path_id(&id)?).await
    }))
}

async fn nothing_here() -> Response {
    refused(404, "not_found", "There is nothing there").into_response()
}

async fn wrong_method() -> Response {
    refused(
        405,
        "wrong_method",
        "This address does not take that method",
    )
    .into_response()
}

// -------------------------------------------------------------------------------------------------
// WebSocket
// -------------------------------------------------------------------------------------------------

async fn websocket(
    State(app): Shared,
    ConnectInfo(address): Address,
    headers: HeaderMap,
    upgrade: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    let (upgrade, caller) = match upgrade_of(&app, &headers, upgrade).await {
        Ok(upgraded) => upgraded,
        Err(refusal) => return refusal.into_response(),
    };
    let ip = client_ip(&headers, address);
    let frame_limit = app.settings.limits.websocket_frame;
    let upgrade = upgrade
        .protocols([SUBPROTOCOL])
        .max_message_size(frame_limit)
        .max_frame_size(frame_limit);
    let spoke_version = upgrade.selected_protocol().is_some();
    upgrade.on_upgrade(move |socket| connection(app, socket, caller, spoke_version, ip))
}

/// A WebSocket upgrade from one of our clients, and who asks. A page from another site is not one of
/// our clients, so it gets no upgrade at all. Scripts and agents send no Origin.
async fn upgrade_of(
    app: &App,
    headers: &HeaderMap,
    upgrade: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Result<(WebSocketUpgrade, Caller), Refused> {
    let Ok(upgrade) = upgrade else {
        return Err(refused(400, "not_websocket", "This address is a WebSocket"));
    };
    if let Some(origin) = headers.get(ORIGIN) {
        let public_url = app.settings.public_url.as_str();
        let authority_at = match public_url.find("://") {
            Some(at) => at + 3,
            None => 0,
        };
        let expected = match public_url[authority_at..].find('/') {
            Some(at) => &public_url[..authority_at + at],
            None => public_url,
        };
        if origin.as_bytes() != expected.as_bytes() {
            return Err(refused(403, "bad_origin", "This site may not connect"));
        }
    }
    let caller = caller_of(app, headers).await?;
    Ok((upgrade, caller))
}

/// A device's socket to a call's MoQ. Admission and refusals happen after the upgrade, in the relay.
async fn call_socket(
    State(app): Shared,
    headers: HeaderMap,
    Path((chat, seq)): Path<(String, String)>,
    upgrade: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    let chat = match path_id(&chat) {
        Ok(chat) => chat,
        Err(refusal) => return refusal.into_response(),
    };
    let Ok(call_seq) = seq.parse::<u64>() else {
        return refused(404, "not_found", "There is nothing there").into_response();
    };
    let (upgrade, caller) = match upgrade_of(&app, &headers, upgrade).await {
        Ok(upgraded) => upgraded,
        Err(refusal) => return refusal.into_response(),
    };
    let frame_limit = app.settings.limits.websocket_frame;
    let upgrade = upgrade
        .protocols([moq::PROTOCOL])
        .max_message_size(frame_limit)
        .max_frame_size(frame_limit);
    let spoke_version = upgrade.selected_protocol().is_some();
    upgrade.on_upgrade(move |socket| {
        moq::connection(app, socket, caller, spoke_version, chat, call_seq)
    })
}

fn close_frame(code: u16, reason: &str) -> Message {
    Message::Close(Some(CloseFrame {
        code,
        reason: Utf8Bytes::from(reason),
    }))
}

/// One device's socket. Refusals come after the upgrade, as close code 4403 with the refusal's code
/// as the reason, because a browser cannot read a failed upgrade's reply.
async fn connection(
    app: Arc<App>,
    mut socket: WebSocket,
    caller: Caller,
    spoke_version: bool,
    ip: String,
) {
    let refusal = if spoke_version {
        match check(&app, &caller, Need::Device).await {
            Ok(()) => None,
            Err(refusal) => Some(refusal.code),
        }
    } else {
        Some("version_mismatch")
    };
    if let Some(code) = refusal {
        tracing::warn!(msg = "Refused a WebSocket", code = code);
        if let Err(error) = socket.send(close_frame(4403, code)).await {
            tracing::debug!(msg = "Could not send a close", error = %error);
        }
        return;
    }
    let Caller::Session(session) = caller else {
        return;
    };
    let Some(device) = session.device else {
        return;
    };
    let limits = &app.settings.limits;
    let features = ServerFrame::Features {
        ping_after: limits.ping_after,
        ping_every: limits.ping_every,
        ping_timeout: limits.ping_timeout,
    };
    // Sent before this connection can get anything else, so that it is always the first frame.
    if let Some(text) = encode(&features)
        && let Err(error) = socket.send(Message::Text(text)).await
    {
        tracing::debug!(msg = "Could not send the features", error = %error);
        return;
    }
    let (outgoing, mut buffer) = mpsc::channel(limits.outgoing_frames.max(1));
    let kill = Arc::new(Notify::new());
    let (connection_id, replaced) =
        app.hub
            .connect(session.account, device, outgoing.clone(), Arc::clone(&kill));
    // The old connection's end writes nothing now, so its times are written here, and the device is
    // not there to others until this one announces itself.
    if let Some(in_app) = replaced {
        connection_gone(&app, session.account, device, in_app).await;
    }
    if let Err(error) = app
        .storage
        .device_online(device, ip, Ulid::now(), DEVICE_LOG_GRACE_MS)
        .await
    {
        tracing::error!(msg = "Could not write the device log", error = %error);
    }
    if let Err(refusal) = hand_welcomes(&app, session.account, device).await {
        tracing::error!(msg = "Could not hand over welcomes", code = refusal.code);
    }

    // Requests are handled one at a time on their own task, so that a slow one never stops this
    // connection's outgoing frames.
    let (requests, mut queue) = mpsc::channel::<ClientFrame>(REQUEST_QUEUE);
    let worker = {
        let app = Arc::clone(&app);
        let outgoing = outgoing.clone();
        let kill = Arc::clone(&kill);
        tokio::spawn(async move {
            while let Some(frame) = queue.recv().await {
                if let Some(answer) = run_frame(&app, session, frame).await
                    && let Some(text) = encode(&answer)
                {
                    push(&outgoing, &kill, text);
                }
            }
        })
    };
    drop(outgoing);

    let ping_after = Duration::from_secs(limits.ping_after);
    let ping_every = Duration::from_secs(limits.ping_every);
    let ping_timeout = Duration::from_secs(limits.ping_timeout);
    // One timer does all of it. Frames arriving only move these times, and it catches up when it
    // wakes, so a busy connection costs no timer work per frame and gets no pings.
    let mut last_event = Instant::now();
    let mut last_ping = None;
    let mut answer_due = false;
    let timer = tokio::time::sleep(ping_after);
    tokio::pin!(timer);
    let mut stopping = app.stopping.subscribe();
    loop {
        // In this order, so that an answer waiting to be read always beats its own deadline.
        let to_send = tokio::select! {
            biased;
            _ = stopping.changed() => {
                Some(close_frame(1001, "shutting_down"))
            }
            _ = kill.notified() => break,
            incoming = socket.recv() => {
                answer_due = false;
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<ClientFrame>(text.as_str()) {
                            Ok(ClientFrame::Ping { reference }) => {
                                encode(&ServerFrame::Done { reference }).map(Message::Text)
                            }
                            Ok(frame) => {
                                last_event = Instant::now();
                                if requests.send(frame).await.is_err() {
                                    break;
                                }
                                None
                            }
                            Err(_) => bad_frame(text.as_str()),
                        }
                    }
                    Some(Ok(Message::Binary(_))) => Some(close_frame(4403, "bad_frame")),
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                    Some(Ok(_)) => None,
                }
            }
            out = buffer.recv() => match out {
                Some(Out::Frame(text)) => Some(Message::Text(text)),
                Some(Out::Close(code, reason)) => Some(close_frame(code, reason)),
                None => break,
            },
            _ = &mut timer => {
                if answer_due {
                    tracing::debug!(msg = "A connection did not answer a ping", device = %device);
                    break;
                }
                let due = match last_ping {
                    Some(pinged_at) if pinged_at > last_event => pinged_at + ping_every,
                    _ => last_event + ping_after,
                };
                let now = Instant::now();
                if now >= due {
                    last_ping = Some(now);
                    answer_due = true;
                    timer.as_mut().reset((now + ping_timeout).into());
                    Some(Message::Ping(RawBody::new()))
                } else {
                    timer.as_mut().reset(due.into());
                    None
                }
            }
        };
        let Some(message) = to_send else {
            continue;
        };
        let closes = matches!(message, Message::Close(_));
        // A client that stopped reading blocks this send, and the buffer then fills behind it. So a
        // full buffer has to be noticed here too, or only `send_timeout` would end it.
        let sent = tokio::select! {
            sent = socket.send(message) => sent,
            _ = kill.notified() => break,
            _ = stopping.changed() => break,
        };
        if sent.is_err() || closes {
            break;
        }
    }
    drop(requests);
    if let Some(announced) = app.hub.disconnect(session.account, device, connection_id) {
        if let Err(error) = app.storage.device_offline(device, Ulid::now()).await {
            tracing::error!(msg = "Could not write the device log", error = %error);
        }
        if let Some(in_app) = announced {
            connection_gone(&app, session.account, device, in_app).await;
        }
    }
    if let Err(error) = worker.await {
        tracing::error!(msg = "A connection's task failed", error = %error);
    }
}

/// A frame that cannot be read. If its `ref` can, the client is told which one. Otherwise there is
/// nothing to answer, so the connection closes.
fn bad_frame(text: &str) -> Option<Message> {
    if let Ok(value) = serde_json::from_str::<Value>(text)
        && let Some(reference) = value.get("ref")
        && let Some(reference) = reference.as_str()
        && let Ok(reference) = Ulid::parse(reference)
    {
        let answer = ServerFrame::Refused {
            reference,
            code: "bad_frame".to_string(),
            message: "That frame is not what this needs".to_string(),
            call_seq: None,
            version: None,
            kv_list: None,
            seconds: None,
            account_list: None,
        };
        if let Some(text) = encode(&answer) {
            return Some(Message::Text(text));
        }
    }
    Some(close_frame(4403, "bad_frame"))
}

// -------------------------------------------------------------------------------------------------
// The server
// -------------------------------------------------------------------------------------------------

/// The access log, the trace id and the headers every response carries.
async fn access_log(request: Request, next: Next) -> Response {
    let trace = current_trace();
    let started = Instant::now();
    let method = request.method().to_string();
    let path = request.uri().path().to_string();
    let query = match request.uri().query() {
        Some(query) => query.to_string(),
        None => String::new(),
    };
    let route = match request.extensions().get::<MatchedPath>() {
        Some(route) => route.as_str().to_string(),
        None => String::new(),
    };
    let request_bytes = content_length(request.headers());
    let span = tracing::info_span!("Http", op = "http", trace = %trace);
    let mut response = TRACE
        .scope(trace, next.run(request))
        .instrument(span.clone())
        .await;
    let headers = response.headers_mut();
    if let Ok(value) = HeaderValue::from_str(&format!(
        "keryx/{} ({})",
        env!("CARGO_PKG_VERSION"),
        env!("CARGO_PKG_REPOSITORY")
    )) {
        headers.insert(SERVER, value);
    }
    headers.insert(
        "x-keryx-version",
        HeaderValue::from_static(env!("CARGO_PKG_VERSION")),
    );
    if let Ok(value) = HeaderValue::from_str(&trace.to_string()) {
        headers.insert("x-trace-id", value);
    }
    let status = response.status().as_u16();
    let response_bytes = content_length(response.headers());
    let total_ms = started.elapsed().as_millis() as u64;
    let trace = trace.to_string();
    span.in_scope(|| {
        if status >= 500 {
            tracing::error!(msg = "HTTP request", http_method = %method, http_path = %path, http_route = %route, http_query = %query, http_status = status, http_request_bytes = request_bytes, http_response_bytes = response_bytes, total_ms = total_ms, trace = %trace);
        } else if status >= 400 {
            tracing::warn!(msg = "HTTP request", http_method = %method, http_path = %path, http_route = %route, http_query = %query, http_status = status, http_request_bytes = request_bytes, http_response_bytes = response_bytes, total_ms = total_ms, trace = %trace);
        } else {
            tracing::info!(msg = "HTTP request", http_method = %method, http_path = %path, http_route = %route, http_query = %query, http_status = status, http_request_bytes = request_bytes, http_response_bytes = response_bytes, total_ms = total_ms, trace = %trace);
        }
    });
    response
}

fn content_length(headers: &HeaderMap) -> u64 {
    if let Some(value) = headers.get(CONTENT_LENGTH)
        && let Ok(text) = value.to_str()
        && let Ok(length) = text.parse()
    {
        return length;
    }
    0
}

fn router(app: Arc<App>, www: &FilePath) -> Router {
    let rest_body = app.settings.limits.rest_body;
    let chunk_body = usize::try_from(file::CHUNK + file::TAG).unwrap_or(usize::MAX);
    let sticker_body = usize::try_from(app.settings.limits.upload.sticker).unwrap_or(usize::MAX);
    let photo_body = usize::try_from(app.settings.limits.upload.photo).unwrap_or(usize::MAX);
    let files = ServeDir::new(www).fallback(ServeFile::new(www.join("index.html")));
    Router::new()
        .route("/api/v1", get(guide))
        .route("/api/v1/ws", get(websocket))
        .route("/api/v1/calls/{chat}/{seq}/moq", get(call_socket))
        .route("/api/v1/captcha", get(captcha_route))
        .route("/api/v1/public/features", get(features))
        .route("/api/v1/register", post(register))
        .route("/api/v1/login", post(login))
        .route("/api/v1/login/token", post(login_token))
        .route("/api/v1/login/agent", post(login_agent))
        .route("/api/v1/agents", get(agents).post(create_agent))
        .route("/api/v1/agents/{id}", put(update_agent))
        .route(
            "/api/v1/agents/{id}/token",
            post(renew_agent_token).delete(revoke_agent_token),
        )
        .route("/api/v1/agents/{id}/disable", post(disable_agent))
        .route("/api/v1/accounts/{id}/keys", put(agent_keys))
        .route("/api/v1/setup", post(setup))
        .route("/api/v1/challenge", post(challenge))
        .route("/api/v1/password/reset", post(password_reset))
        .route("/api/v1/sessions", get(sessions))
        .route("/api/v1/sessions/{id}", delete(delete_session))
        .route("/api/v1/account/keys", put(account_keys))
        .route("/api/v1/accounts", get(directory).post(create_account))
        .route("/api/v1/accounts/{id}", get(account))
        .route(
            "/api/v1/accounts/{id}/profile",
            get(profile).put(put_profile),
        )
        .route("/api/v1/accounts/{id}/names", put(put_names))
        .route(
            "/api/v1/accounts/{id}/photos",
            post(add_photo).layer(DefaultBodyLimit::max(photo_body)),
        )
        .route("/api/v1/accounts/{id}/photo", delete(remove_photo))
        .route("/api/v1/photos/{id}", get(photo))
        .route("/api/v1/devices", get(devices).post(register_device))
        .route("/api/v1/devices/id", post(device_id))
        .route("/api/v1/devices/{id}/terminate", post(terminate))
        .route("/api/v1/devices/{id}/logout", post(logout))
        .route("/api/v1/devices/{id}/presence", put(put_presence))
        .route("/api/v1/accounts/{id}/presence", get(presence_of))
        .route("/api/v1/kv", get(kv).patch(patch_kv))
        .route("/api/v1/key_packages", post(key_packages))
        .route(
            "/api/v1/accounts/{id}/key_packages/claim",
            post(claim_key_packages),
        )
        .route("/api/v1/chats", post(create_chat))
        .route("/api/v1/chats/{id}", delete(chat_remove))
        .route("/api/v1/chats/{id}/leave", post(leave))
        .route("/api/v1/chats/{id}/members/{account}/role", put(chat_role))
        .route(
            "/api/v1/chats/{id}/requests",
            get(requests).post(join_request).delete(join_cancel),
        )
        .route(
            "/api/v1/chats/{id}/requests/{account}/decline",
            post(join_decline),
        )
        .route("/api/v1/chats/{id}/invitations", post(invite))
        .route("/api/v1/chats/{id}/invitation", post(invite_answer))
        .route("/api/v1/chats/{id}/topics/id", post(topic_id))
        .route("/api/v1/chats/{id}/members", get(members))
        .route("/api/v1/chats/{id}/topics", put(put_topics))
        .route("/api/v1/chats/{id}/profile", put(put_chat_profile))
        .route("/api/v1/chats/{id}/options", put(put_chat_options))
        .route(
            "/api/v1/chats/{id}/topics/{topic}/options",
            put(put_topic_options),
        )
        .route(
            "/api/v1/chats/{id}/topics/{topic}/profile",
            put(put_topic_profile),
        )
        .route(
            "/api/v1/chats/{id}/topics/{topic}/members/{account}/role",
            put(put_topic_role),
        )
        .route("/api/v1/chats/{id}/findable", put(put_findable))
        .route("/api/v1/chats/{id}/group_info", get(group_info))
        .route("/api/v1/chats/{id}/devices", get(chat_devices))
        .route(
            "/api/v1/chats/{id}/messages/{seq}/times",
            get(message_times),
        )
        .route("/api/v1/chats/{id}/wraps", get(chat_wraps))
        .route("/api/v1/chats/{id}/draft", put(put_draft))
        .route("/api/v1/chat_folders", put(put_chat_folders))
        .route("/api/v1/chat_folders/id", post(folder_id))
        .route("/api/v1/chats/{id}/files", post(create_file))
        .route("/api/v1/files/{id}", get(file_status))
        .route(
            "/api/v1/files/{id}/chunks/{n}",
            put(put_chunk)
                .layer(DefaultBodyLimit::max(chunk_body))
                .get(get_chunk),
        )
        .route("/api/v1/stickers", get(sticker_list))
        .route(
            "/api/v1/stickers/{id}",
            get(sticker_image).delete(remove_sticker),
        )
        .route("/api/v1/sticker_packs", post(create_pack))
        .route("/api/v1/sticker_packs/{id}", delete(remove_pack))
        .route(
            "/api/v1/sticker_packs/{id}/stickers",
            post(add_sticker).layer(DefaultBodyLimit::max(sticker_body)),
        )
        .route("/api/v1/{*rest}", any(nothing_here))
        .method_not_allowed_fallback(wrong_method)
        .fallback_service(files)
        .layer(DefaultBodyLimit::max(rest_body))
        .layer(middleware::from_fn(access_log))
        .with_state(app)
}

/// The listening socket. The buffers are set here too, because a receive buffer's size is agreed
/// with the peer as the connection opens, before `TcpAcceptor` sees it.
fn listener(address: SocketAddr, tcp: &TcpSettings) -> io::Result<std::net::TcpListener> {
    let socket = socket2::Socket::new(
        socket2::Domain::for_address(address),
        socket2::Type::STREAM,
        Some(socket2::Protocol::TCP),
    )?;
    socket.set_reuse_address(true)?;
    if let Some(size) = tcp.send_buffer {
        socket.set_send_buffer_size(size)?;
    }
    if let Some(size) = tcp.receive_buffer {
        socket.set_recv_buffer_size(size)?;
    }
    socket.bind(&address.into())?;
    socket.listen(tcp.backlog)?;
    socket.set_nonblocking(true)?;
    Ok(socket.into())
}

/// `[tcp]`, on each accepted socket.
fn tune_socket(stream: &TcpStream, tcp: &TcpSettings) -> io::Result<()> {
    let socket = socket2::SockRef::from(stream);
    socket.set_tcp_nodelay(tcp.nodelay)?;
    if tcp.keepalive_idle.is_some()
        || tcp.keepalive_interval.is_some()
        || tcp.keepalive_retries.is_some()
    {
        let mut keepalive = socket2::TcpKeepalive::new();
        if let Some(idle) = tcp.keepalive_idle {
            keepalive = keepalive.with_time(Duration::from_secs(idle));
        }
        if let Some(interval) = tcp.keepalive_interval {
            keepalive = keepalive.with_interval(Duration::from_secs(interval));
        }
        if let Some(retries) = tcp.keepalive_retries {
            keepalive = keepalive.with_retries(retries);
        }
        socket.set_tcp_keepalive(&keepalive)?;
    }
    if let Some(size) = tcp.send_buffer {
        socket.set_send_buffer_size(size)?;
    }
    if let Some(size) = tcp.receive_buffer {
        socket.set_recv_buffer_size(size)?;
    }
    #[cfg(target_os = "linux")]
    if let Some(timeout) = tcp.user_timeout {
        socket.set_tcp_user_timeout(Some(Duration::from_secs(timeout)))?;
    }
    Ok(())
}

/// Sets up each accepted connection, underneath TLS when there is TLS.
#[derive(Clone)]
struct TcpAcceptor {
    tcp: Arc<TcpSettings>,
}

impl<S> Accept<TcpStream, S> for TcpAcceptor {
    type Stream = TimedStream;
    type Service = S;
    type Future = std::future::Ready<io::Result<(TimedStream, S)>>;

    fn accept(&self, stream: TcpStream, service: S) -> Self::Future {
        if let Err(error) = tune_socket(&stream, &self.tcp) {
            tracing::warn!(msg = "Could not set a TCP option on a connection", error = %error);
        }
        std::future::ready(Ok((
            TimedStream {
                inner: stream,
                send_timeout: Duration::from_secs(self.tcp.send_timeout),
                stuck: None,
            },
            service,
        )))
    }
}

/// A connection whose writes fail after `send_timeout` with no byte taken. The operating system's own
/// send timeout does nothing on a non-blocking socket. The timer exists only while a write is stuck,
/// so normal traffic pays one check.
struct TimedStream {
    inner: TcpStream,
    send_timeout: Duration,
    stuck: Option<Pin<Box<Sleep>>>,
}

impl TimedStream {
    fn watch_write(
        &mut self,
        context: &mut Context<'_>,
        attempt: Poll<io::Result<usize>>,
    ) -> Poll<io::Result<usize>> {
        if attempt.is_ready() {
            self.stuck = None;
            return attempt;
        }
        let send_timeout = self.send_timeout;
        let stuck = self
            .stuck
            .get_or_insert_with(|| Box::pin(tokio::time::sleep(send_timeout)));
        match stuck.as_mut().poll(context) {
            Poll::Ready(()) => {
                tracing::debug!(msg = "A client took no bytes for send_timeout");
                Poll::Ready(Err(io::ErrorKind::TimedOut.into()))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl AsyncRead for TimedStream {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_read(context, buffer)
    }
}

impl AsyncWrite for TimedStream {
    fn poll_write(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let attempt = Pin::new(&mut this.inner).poll_write(context, buffer);
        this.watch_write(context, attempt)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer_list: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let attempt = Pin::new(&mut this.inner).poll_write_vectored(context, buffer_list);
        this.watch_write(context, attempt)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(context)
    }

    fn poll_shutdown(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(context)
    }
}

/// `keryx serve`: runs until SIGINT or SIGTERM. Then it stops accepting, closes each WebSocket with
/// 1001, lets each chat's task finish what it already received, closes open device log rows and
/// returns, in at most 8 seconds. A client only counts a message as sent once it has its sequence id,
/// so one cut off here is resent.
pub async fn serve(settings: Settings) -> Result<(), ServeError> {
    let storage = match Sqlite::open(&settings.database.path) {
        Ok(storage) => storage,
        Err(source) => {
            return Err(ServeError::Database {
                path: settings.database.path.clone(),
                source,
            });
        }
    };
    // The server's built-ins hold the server-wide names, and a chat's the in-chat ones, chosen by
    // the author: the admin all, and everyone else all but the admin's own.
    let mut role_list = Vec::with_capacity(6);
    for (scope, name) in [
        (0, "admin"),
        (0, "moderator"),
        (0, "member"),
        (1, "owner"),
        (1, "moderator"),
        (1, "member"),
    ] {
        let mut permission_list = Vec::new();
        if scope == 0 {
            for permission in SERVER_PERMISSION_LIST {
                if name == "admin" || !SERVER_ADMIN_ONLY.contains(permission) {
                    permission_list.push(permission.to_string());
                }
            }
        } else {
            for prefix in ["direct", "group", "channel"] {
                for permission in CHAT_PERMISSION_LIST {
                    let members_lack = prefix != "direct"
                        && name == "member"
                        && VIEWS_OF_OTHERS.contains(permission);
                    if *permission != SEND_UNENCRYPTED && !members_lack {
                        permission_list.push(format!("{prefix}.{permission}"));
                    }
                }
                if prefix == "direct" || name == "member" {
                    continue;
                }
                for permission in GROUP_PERMISSION_LIST {
                    if name == "owner" || !OWNER_ONLY.contains(permission) {
                        permission_list.push(format!("{prefix}.{permission}"));
                    }
                }
            }
            if name != "member" {
                permission_list.push(format!("channel.{MESSAGE_POST}"));
            }
        }
        let id = match Ulid::new() {
            Ok(id) => id,
            Err(source) => return Err(ServeError::Id(source)),
        };
        role_list.push(BuiltInRole {
            id,
            scope,
            name,
            permission_list,
        });
    }
    if let Err(source) = storage.seed_roles(role_list).await {
        return Err(ServeError::Prepare(source));
    }
    let known: HashSet<String> = permission_list().into_iter().collect();
    match storage.permission_names().await {
        Ok(name_list) => {
            for name in name_list {
                if !known.contains(&name) {
                    return Err(ServeError::UnknownPermission { name });
                }
            }
        }
        Err(source) => return Err(ServeError::Prepare(source)),
    }
    if let Err(source) = storage.close_device_logs(Ulid::now()).await {
        return Err(ServeError::Prepare(source));
    }
    if settings.calls.test_ring.is_some() || settings.calls.test_grace.is_some() {
        tracing::warn!(msg = "A call's ring or drop grace is shortened, which is for tests only");
    }
    if let Err(source) = storage.end_open_calls(storage.saved_now()).await {
        return Err(ServeError::Prepare(source));
    }
    if let Some(captcha) = &settings.captcha
        && captcha.test_answer.is_some()
    {
        tracing::warn!(msg = "Every captcha has one fixed answer, which is for tests only");
    }
    if let Some(captcha) = &settings.captcha
        && captcha
            .pass_code
            .as_ref()
            .is_some_and(|code| !code.is_empty())
    {
        tracing::warn!(msg = "A captcha pass code is set, so that code is accepted too");
    }

    #[cfg(not(target_os = "linux"))]
    if settings.tcp.user_timeout.is_some() {
        tracing::warn!(msg = "tcp.user_timeout exists only on Linux, so it is off here");
    }

    let address = settings.listen;
    let tls = match &settings.tls {
        Some(tls) => {
            // The ring provider, which axum-server leaves out. Installing it twice changes nothing.
            if rustls::crypto::ring::default_provider()
                .install_default()
                .is_err()
            {
                tracing::debug!(msg = "A TLS provider was already installed");
            }
            match RustlsConfig::from_pem_file(&tls.certificate, &tls.key).await {
                Ok(config) => Some(config),
                Err(source) => return Err(ServeError::Tls(source)),
            }
        }
        None => None,
    };
    let default_pack =
        file::default_pack(settings.limits.upload.sticker).map_err(ServeError::DefaultPack)?;
    let www = settings.www.directory.clone();
    let (stopping, _) = watch::channel(false);
    let call_pool = moq_net::cache::Pool::new(settings.limits.call_cache as u64);
    let app = Arc::new(App {
        settings,
        storage,
        hub: Hub::default(),
        stopping,
        call_pool,
        default_pack,
    });
    captcha::start_sweep(Arc::clone(&app));
    if let Err(refusal) = file::sweep(&app, None).await {
        tracing::error!(msg = "Could not remove unused uploads", code = refusal.code);
    }

    let handle = axum_server::Handle::new();
    let waiter = {
        let app = Arc::clone(&app);
        let handle = handle.clone();
        tokio::spawn(async move {
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(mut terminate) => {
                    tokio::select! {
                        _ = terminate.recv() => {}
                        _ = tokio::signal::ctrl_c() => {}
                    }
                }
                Err(error) => {
                    tracing::warn!(msg = "Listening only for SIGINT", error = %error);
                    if let Err(error) = tokio::signal::ctrl_c().await {
                        tracing::error!(msg = "Could not listen for SIGINT", error = %error);
                    }
                }
            }
            tracing::info!(msg = "Shutting down");
            app.stopping.send_replace(true);
            handle.graceful_shutdown(Some(SHUTDOWN_CONNECTIONS));
        })
    };

    tracing::info!(msg = "Listening", address = %address, tls = tls.is_some());
    let service =
        router(Arc::clone(&app), &www).into_make_service_with_connect_info::<SocketAddr>();
    let acceptor = TcpAcceptor {
        tcp: Arc::new(app.settings.tcp.clone()),
    };
    let served = match listener(address, &app.settings.tcp) {
        Ok(listener) => match axum_server::from_tcp(listener) {
            Ok(server) => {
                let server = server.handle(handle);
                match tls {
                    Some(config) => {
                        server
                            .acceptor(RustlsAcceptor::new(config).acceptor(acceptor))
                            .serve(service)
                            .await
                    }
                    None => server.acceptor(acceptor).serve(service).await,
                }
            }
            Err(source) => Err(source),
        },
        Err(source) => Err(source),
    };
    waiter.abort();
    if let Err(source) = served {
        return Err(ServeError::Listen { address, source });
    }
    if tokio::time::timeout(SHUTDOWN_CHATS, app.hub.finish())
        .await
        .is_err()
    {
        tracing::warn!(msg = "Some chats' tasks did not finish in time");
    }
    if let Err(source) = app.storage.close_device_logs(Ulid::now()).await {
        return Err(ServeError::Prepare(source));
    }
    tracing::info!(msg = "Shut down");
    Ok(())
}
