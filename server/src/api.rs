//! Every operation, and the axum router: REST and WebSocket.
//!
//! **No operation exists without its check.** Each `run_*` function calls `check` first, and the
//! check reads the role tables each time, so a role change applies at once. Secrets travel only in
//! headers and bodies, because the access log records the path and the query.

use std::collections::HashSet;
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
    AUTHORIZATION, CONTENT_LENGTH, CONTENT_TYPE, COOKIE, ORIGIN, SERVER, SET_COOKIE,
};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, delete, get, post, put};
use axum_server::accept::Accept;
use axum_server::tls_rustls::{RustlsAcceptor, RustlsConfig};
use base64ct::{Base64UrlUnpadded, Encoding};
use keryx_core::mls::{
    verify_account_key, verify_device_login, verify_device_stamp, verify_logout,
    verify_password_reset, verify_revocation,
};
use keryx_core::protocol::{
    AccountInfo, AccountKeysBody, AccountListReply, AccountReply, Bytes, ChallengeBody,
    ChallengeReply, ChatDeviceListReply, ChatReply, ClientFrame, ContentKind, CountReply,
    CreateAccountBody, CreateChatBody, CreatedAccountReply, DeviceListReply, DeviceReply,
    DevicesReply, EmptyReply, FeaturesReply, GroupInfoReply, KeyPackageListReply, KeyPackagesBody,
    LoginBody, LoginReply, ReducedDeviceItem, ReducedDeviceListReply, RegisterBody,
    RegisterDeviceBody, ResetBody, SelfChatReply, ServerFrame, SessionListReply, SetupBody,
    SetupReply, SignedBody, WrapListReply,
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
use crate::chat::{Hub, Job, Out, answer_of, encode, hand_welcomes, push};
use crate::storage::sqlite::Sqlite;
use crate::storage::{
    self, BuiltInRole, NewAccount, NewDevice, NewSession, Registered, Session, Storage,
};

pub const CHAT_CREATE_DIRECT: &str = "chat.create_direct";
pub const DIRECTORY_SEARCH: &str = "directory.search";
pub const ACCOUNT_CREATE: &str = "account.create";
pub const SEND_UNENCRYPTED: &str = "message.send.unencrypted";
pub const MESSAGE_COMMIT: &str = "message.commit";
pub const EDIT_OWN: &str = "message.edit.own";
pub const DELETE_OWN: &str = "message.delete.own";

/// The closed list of permission names. A stored role holding any other name stops startup, so a
/// typo in a role cannot silently grant nothing.
pub const PERMISSION_LIST: &[&str] = &[
    CHAT_CREATE_DIRECT,
    DIRECTORY_SEARCH,
    ACCOUNT_CREATE,
    "message.send.text",
    "message.send.voice",
    "message.send.reaction",
    "message.send.image",
    "message.send.video",
    "message.send.doc",
    "message.send.file",
    "message.send.video_call",
    "message.send.voice_call",
    "message.send.text_call",
    SEND_UNENCRYPTED,
    MESSAGE_COMMIT,
    EDIT_OWN,
    DELETE_OWN,
];

/// Everyone's built-in roles hold the whole list, except these, which are the admin's alone.
const ADMIN_ONLY: &[&str] = &[ACCOUNT_CREATE, SEND_UNENCRYPTED];

/// The permission to send one content kind.
pub fn send_permission(content_kind: ContentKind) -> &'static str {
    match content_kind {
        ContentKind::Text => "message.send.text",
        ContentKind::Voice => "message.send.voice",
        ContentKind::Reaction => "message.send.reaction",
        ContentKind::Image => "message.send.image",
        ContentKind::Video => "message.send.video",
        ContentKind::Doc => "message.send.doc",
        ContentKind::File => "message.send.file",
        ContentKind::VideoCall => "message.send.video_call",
        ContentKind::VoiceCall => "message.send.voice_call",
        ContentKind::TextCall => "message.send.text_call",
    }
}

const SUBPROTOCOL: &str = "keryx.v1";
const COOKIE_NAME: &str = "keryx_session";
const MAX_PASSWORD_CHARS: usize = 1024;

/// A challenge or a new device's id works for 5 minutes.
const CHALLENGE_MS: i64 = 5 * 60 * 1000;
const PURPOSE_RESET: i64 = 0;
const PURPOSE_DEVICE_LOGIN: i64 = 1;
const PURPOSE_DEVICE_ID: i64 = 2;

/// A setup link works for 7 days.
const SETUP_LINK_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// Reconnecting within this extends the same row of the device's log.
const DEVICE_LOG_GRACE_MS: i64 = 60 * 1000;

/// The longest a shutdown takes, under Docker's default 10 before SIGKILL. Most of it is for
/// connections to close, and the rest for chats' tasks to finish.
const SHUTDOWN_CONNECTIONS: Duration = Duration::from_secs(6);
const SHUTDOWN_CHATS: Duration = Duration::from_secs(2);

/// How many frames of one connection may wait to be handled.
const REQUEST_QUEUE: usize = 64;

/// `server.toml`. Durations are whole seconds and sizes are bytes, after tanzim's validators.
#[derive(Deserialize)]
pub struct Settings {
    pub listen: SocketAddr,
    pub public_url: String,
    pub max_devices: Option<u64>,
    pub database: DatabaseSettings,
    pub operator: Option<OperatorSettings>,
    #[serde(default)]
    pub registration: RegistrationSettings,
    #[serde(default)]
    pub session: SessionSettings,
    #[serde(default)]
    pub chat: ChatSettings,
    pub www: WwwSettings,
    #[serde(default)]
    pub limits: Limits,
    #[serde(default)]
    pub tcp: TcpSettings,
    pub tls: Option<TlsSettings>,
    pub captcha: Option<captcha::Settings>,
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
}

impl Default for ChatSettings {
    fn default() -> Self {
        Self { idle_after: 5 * 60 }
    }
}

#[derive(Deserialize)]
pub struct WwwSettings {
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
    pub body: BodyLimits,
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
            body: BodyLimits::default(),
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
    pub video: u64,
    pub doc: u64,
    pub file: u64,
    pub video_call: u64,
    pub voice_call: u64,
    pub text_call: u64,
}

impl Default for BodyLimits {
    fn default() -> Self {
        Self {
            text: 64 * 1024,
            reaction: 1024,
            voice: 16 * 1024,
            image: 16 * 1024,
            video: 16 * 1024,
            doc: 16 * 1024,
            file: 16 * 1024,
            video_call: 16 * 1024,
            voice_call: 16 * 1024,
            text_call: 64 * 1024,
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
            ContentKind::Video => self.video,
            ContentKind::Doc => self.doc,
            ContentKind::File => self.file,
            ContentKind::VideoCall => self.video_call,
            ContentKind::VoiceCall => self.voice_call,
            ContentKind::TextCall => self.text_call,
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
}

pub fn refused(status: u16, code: &'static str, message: &str) -> Refused {
    Refused {
        status,
        code,
        message: message.to_string(),
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
    /// Membership is not a permission, so the admin needs it too. Then a permission in that chat.
    MemberWith(Ulid, &'static str),
}

/// The one check every operation passes through.
pub async fn check(app: &App, caller: &Caller, need: Need) -> Result<(), Refused> {
    if matches!(need, Need::Anyone) {
        return Ok(());
    }
    if matches!((caller, &need), (Caller::Operator, Need::Server(_))) {
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
        Need::Member(chat) => match app.storage.chat_access(chat, session.account).await? {
            Some(_) => Ok(()),
            None => Err(refused(404, "not_member", "You are not in that chat")),
        },
        Need::MemberWith(chat, permission) => {
            if app
                .storage
                .chat_access(chat, session.account)
                .await?
                .is_none()
            {
                return Err(refused(404, "not_member", "You are not in that chat"));
            }
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

fn check_username(username: &str, min_length: Option<usize>) -> Result<(), Refused> {
    let mut valid = !username.is_empty();
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
    check_username(&input.username, None)?;
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
    let now = Ulid::now();
    let mut device = None;
    if let Some(proof) = input.device {
        let refused_proof = refused(403, "bad_device_proof", "That device could not be proven");
        let Some(key) = app.storage.device_key(proof.id).await? else {
            return Err(refused_proof);
        };
        if key.account != account.id {
            return Err(refused_proof);
        }
        if key.ended {
            return Err(refused(403, "device_ended", "That device has ended"));
        }
        let challenge: [u8; 32] = fixed(&proof.challenge, "challenge")?;
        let signature: [u8; 64] = fixed(&proof.signature, "signature")?;
        if !app
            .storage
            .take_challenge(challenge.to_vec(), account.id, PURPOSE_DEVICE_LOGIN, now)
            .await?
        {
            return Err(refused_proof);
        }
        let public: [u8; 32] = stored_key(&key.public_key)?;
        if verify_device_login(&public, &account.id, &proof.id, &challenge, &signature).is_err() {
            return Err(refused_proof);
        }
        device = Some(proof.id);
    }
    let (token, token_hash) = new_token()?;
    app.storage
        .create_session(NewSession {
            id: new_id()?,
            token_hash,
            account: account.id,
            device,
            now,
            expires_at: now.saturating_add(millis(app.settings.session.lifetime)),
        })
        .await?;
    let reply = LoginReply {
        account: account.id,
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
    let Some(account) = app.storage.account_by_username(input.username).await? else {
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
    let master: [u8; 32] = fixed(&input.master_public, "master_public")?;
    let public: [u8; 32] = fixed(&input.account_public, "account_public")?;
    let stamp: [u8; 64] = fixed(&input.account_stamp, "account_stamp")?;
    if verify_account_key(&master, &session.account, &public, &stamp).is_err() {
        return Err(refused(403, "bad_signature", "That stamp does not verify"));
    }
    let chat = new_id()?;
    let saved = app
        .storage
        .set_account_keys(
            session.account,
            master.to_vec(),
            public.to_vec(),
            stamp.to_vec(),
            chat,
            Ulid::now(),
        )
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
    };
    if let (Some(master), Some(public), Some(stamp)) = (
        account.master_public,
        account.account_public,
        account.account_stamp,
    ) {
        reply.master_public = Some(Bytes(master));
        reply.account_public = Some(Bytes(public));
        reply.account_stamp = Some(Bytes(stamp));
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
        .search_accounts(term, app.settings.limits.history_page)
        .await?;
    Ok(AccountListReply { account_list })
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
                now: Ulid::now(),
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
) -> Result<KeyPackageListReply, Refused> {
    check(app, caller, Need::Device).await?;
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

/// The pair's chat, made if it is new. If two people start it at once they get the same id, and the
/// first to upload epoch 0's group info wins, while the other waits for its Welcome.
pub async fn run_create_chat(
    app: &App,
    caller: &Caller,
    input: CreateChatBody,
) -> Result<ChatReply, Refused> {
    check(app, caller, Need::Device).await?;
    check(app, caller, Need::Server(CHAT_CREATE_DIRECT)).await?;
    let session = caller.session()?;
    let Some(other) = app.storage.account_by_id(input.account).await? else {
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
    let (low, high) = if session.account <= input.account {
        (session.account, input.account)
    } else {
        (input.account, session.account)
    };
    let (chat, created) = app
        .storage
        .direct_chat(new_id()?, low, high, Ulid::now())
        .await?;
    Ok(ChatReply { chat, created })
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

/// What a device's frames do: reads answer here, and everything else goes through the chat's task.
pub async fn run_frame(
    app: &Arc<App>,
    session: Session,
    frame: ClientFrame,
) -> Option<ServerFrame> {
    let caller = Caller::Session(session);
    let (chat, reference) = match &frame {
        ClientFrame::Sync { reference } => {
            let reference = *reference;
            let result = operation!("Sync", "sync", async {
                check(app, &caller, Need::Session).await?;
                let chat_list = app.storage.chat_list(session.account).await?;
                Ok(ServerFrame::Chats {
                    reference,
                    chat_list,
                })
            });
            return Some(answer_of(reference, result));
        }
        ClientFrame::History {
            reference,
            chat,
            after_seq,
            before_seq,
            limit,
        } => {
            let (reference, chat, after_seq, before_seq) =
                (*reference, *chat, *after_seq, *before_seq);
            let limit = (*limit).min(app.settings.limits.history_page);
            let result = operation!("History", "history", async {
                check(app, &caller, Need::Member(chat)).await?;
                let message_list = app
                    .storage
                    .history(chat, after_seq, before_seq, limit)
                    .await?;
                Ok(ServerFrame::Page {
                    reference,
                    message_list,
                })
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
            frame,
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
        frame,
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
        (
            status,
            [(CONTENT_TYPE, "application/json")],
            json!({"ok": false, "code": self.code, "message": self.message}).to_string(),
        )
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
) -> Response {
    respond(operation!(
        "ClaimKeyPackages",
        "claim_key_packages",
        async {
            let caller = caller_of(&app, &headers).await?;
            run_claim_key_packages(&app, &caller, path_id(&id)?).await
        }
    ))
}

async fn create_chat(State(app): Shared, headers: HeaderMap, body: Body) -> Response {
    respond(operation!("CreateChat", "create_chat", async {
        let caller = caller_of(&app, &headers).await?;
        run_create_chat(&app, &caller, parse(body)?).await
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

async fn chat_wraps(State(app): Shared, headers: HeaderMap, Path(id): Path<String>) -> Response {
    respond(operation!("ChatWraps", "chat_wraps", async {
        let caller = caller_of(&app, &headers).await?;
        run_chat_wraps(&app, &caller, path_id(&id)?).await
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
    let upgrade = match upgrade {
        Ok(upgrade) => upgrade,
        Err(_) => {
            return refused(400, "not_websocket", "This address is a WebSocket").into_response();
        }
    };
    // A page from another site is not one of our clients, so it gets no upgrade at all. Scripts and
    // agents send no Origin.
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
            return refused(403, "bad_origin", "This site may not connect").into_response();
        }
    }
    let caller = match caller_of(&app, &headers).await {
        Ok(caller) => caller,
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
    let connection_id =
        app.hub
            .connect(session.account, device, outgoing.clone(), Arc::clone(&kill));
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
    if app.hub.disconnect(session.account, device, connection_id)
        && let Err(error) = app.storage.device_offline(device, Ulid::now()).await
    {
        tracing::error!(msg = "Could not write the device log", error = %error);
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
    let files = ServeDir::new(www).fallback(ServeFile::new(www.join("index.html")));
    Router::new()
        .route("/api/v1", get(guide))
        .route("/api/v1/ws", get(websocket))
        .route("/api/v1/captcha", get(captcha_route))
        .route("/api/v1/public/features", get(features))
        .route("/api/v1/register", post(register))
        .route("/api/v1/login", post(login))
        .route("/api/v1/login/token", post(login_token))
        .route("/api/v1/setup", post(setup))
        .route("/api/v1/challenge", post(challenge))
        .route("/api/v1/password/reset", post(password_reset))
        .route("/api/v1/sessions", get(sessions))
        .route("/api/v1/sessions/{id}", delete(delete_session))
        .route("/api/v1/account/keys", put(account_keys))
        .route("/api/v1/accounts", get(directory).post(create_account))
        .route("/api/v1/accounts/{id}", get(account))
        .route("/api/v1/devices", get(devices).post(register_device))
        .route("/api/v1/devices/id", post(device_id))
        .route("/api/v1/devices/{id}/terminate", post(terminate))
        .route("/api/v1/devices/{id}/logout", post(logout))
        .route("/api/v1/key_packages", post(key_packages))
        .route(
            "/api/v1/accounts/{id}/key_packages/claim",
            post(claim_key_packages),
        )
        .route("/api/v1/chats", post(create_chat))
        .route("/api/v1/chats/{id}/group_info", get(group_info))
        .route("/api/v1/chats/{id}/devices", get(chat_devices))
        .route("/api/v1/chats/{id}/wraps", get(chat_wraps))
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
    let mut role_list = Vec::with_capacity(6);
    for scope in [0, 1] {
        for name in ["admin", "moderator", "member"] {
            let mut permission_list = Vec::with_capacity(PERMISSION_LIST.len());
            for permission in PERMISSION_LIST {
                if name == "admin" || !ADMIN_ONLY.contains(permission) {
                    permission_list.push(*permission);
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
    }
    if let Err(source) = storage.seed_roles(role_list).await {
        return Err(ServeError::Prepare(source));
    }
    let known: HashSet<&str> = PERMISSION_LIST.iter().copied().collect();
    match storage.permission_names().await {
        Ok(name_list) => {
            for name in name_list {
                if !known.contains(name.as_str()) {
                    return Err(ServeError::UnknownPermission { name });
                }
            }
        }
        Err(source) => return Err(ServeError::Prepare(source)),
    }
    if let Err(source) = storage.close_device_logs(Ulid::now()).await {
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
    let www = settings.www.directory.clone();
    let (stopping, _) = watch::channel(false);
    let app = Arc::new(App {
        settings,
        storage,
        hub: Hub::default(),
        stopping,
    });
    captcha::start_sweep(Arc::clone(&app));

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
