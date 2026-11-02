//! What the rest of the server sees of the database. Each method is one whole operation, and no SQL
//! and no transaction crosses this trait, so that Postgres can be added beside `storage/sqlite.rs`.

use std::future::Future;

use keryx_core::protocol::{Bytes, ChatItem, ChatMode, ChatType, ContentKind, Kind, MessageFrame};
use keryx_core::ulid::Ulid;
use serde::Serialize;

pub mod sqlite;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("The database failed")]
    Database(#[from] rusqlite::Error),
    #[error("A database task failed")]
    Task(#[source] tokio::task::JoinError),
}

pub type Result<T> = std::result::Result<T, Error>;

/// One of the built-in roles, which are only ever added when missing.
pub struct BuiltInRole {
    pub id: Ulid,
    /// 0 server, 1 chat.
    pub scope: i64,
    pub name: &'static str,
    pub permission_list: Vec<&'static str>,
}

pub struct Account {
    pub id: Ulid,
    pub username: String,
    pub display_name: Option<String>,
    /// `None` until the one-time link is used.
    pub password_hash: Option<String>,
    /// `None` until the words are set up, like the two after it.
    pub master_public: Option<Vec<u8>>,
    pub account_public: Option<Vec<u8>>,
    pub account_stamp: Option<Vec<u8>>,
}

pub struct NewAccount {
    pub id: Ulid,
    pub username: String,
    pub display_name: Option<String>,
    pub password_hash: Option<String>,
    pub admin: bool,
    /// The one-time link's token hash and when it stops working.
    pub setup: Option<([u8; 32], i64)>,
    pub now: i64,
}

/// What anyone logged in can see of an account.
#[derive(Serialize)]
pub struct AccountBrief {
    pub id: Ulid,
    pub username: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

/// What a request's token proves.
#[derive(Clone, Copy)]
pub struct Session {
    pub id: Ulid,
    pub account: Ulid,
    /// `None` is a password-only session.
    pub device: Option<Ulid>,
}

pub struct NewSession {
    pub id: Ulid,
    pub token_hash: [u8; 32],
    pub account: Ulid,
    pub device: Option<Ulid>,
    pub now: i64,
    pub expires_at: i64,
}

#[derive(Serialize)]
pub struct SessionItem {
    pub id: Ulid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<Ulid>,
    pub created_at: i64,
    pub last_used_at: i64,
    pub expires_at: i64,
}

/// How many devices an account has and how many it may have. `None` is unlimited.
pub struct DeviceUsage {
    pub count: u64,
    pub limit: Option<u64>,
}

pub struct NewDevice {
    pub id: Ulid,
    pub account: Ulid,
    pub public_key: Vec<u8>,
    pub stamp: Vec<u8>,
    /// 0 desktop, 1 mobile.
    pub platform: i64,
    pub os: String,
    pub os_version: String,
    pub web_platform: Option<String>,
    pub web_platform_version: Option<String>,
    pub ip: String,
    pub now: i64,
}

pub enum Registered {
    /// The id was not handed out to this account, was used, or expired.
    UnknownId,
    AtLimit(u64),
    /// Saved, with its notice in the account's self chat.
    Saved(Box<MessageFrame>),
}

/// Enough about a device to check what it signs.
pub struct DeviceKey {
    pub account: Ulid,
    pub public_key: Vec<u8>,
    pub ended: bool,
}

#[derive(Serialize)]
pub struct DeviceLogItem {
    pub online_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offline_at: Option<i64>,
    pub ip: String,
}

/// All of a device's record, for its owner on a registered device. A password-only session is
/// shown only `platform` and the day of `last_active_at`.
#[derive(Serialize)]
pub struct DeviceItem {
    pub id: Ulid,
    pub platform: i64,
    pub os: String,
    pub os_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub web_platform: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub web_platform_version: Option<String>,
    pub first_ip: String,
    pub last_ip: String,
    pub created_at: i64,
    pub last_active_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminated_at: Option<i64>,
    pub log_list: Vec<DeviceLogItem>,
}

/// A device in a chat, as members need it to compare with their MLS members.
#[derive(Serialize)]
pub struct ChatDevice {
    pub account: Ulid,
    pub device: Ulid,
    pub public_key: Bytes,
    pub stamp: Bytes,
    pub master_public: Bytes,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub statement: Option<Bytes>,
}

#[derive(Serialize)]
pub struct KeyPackageItem {
    pub device: Ulid,
    pub body: Bytes,
}

/// What a member's check needs to know about a chat.
pub struct ChatAccess {
    pub chat_type: ChatType,
    pub mode: ChatMode,
    pub last_seq: u64,
}

/// A stored message, when deciding about an edit or delete pointing at it. `None` for one removed.
pub struct MessageBrief {
    pub kind: Kind,
    pub content_kind: Option<ContentKind>,
    pub account: Ulid,
    pub device_created_at: i64,
    pub saved_at: i64,
}

pub struct NewWrap {
    pub account: Ulid,
    pub wrapped: Vec<u8>,
}

pub struct NewWelcome {
    pub id: Ulid,
    pub device: Ulid,
    pub body: Vec<u8>,
}

pub struct NewMessage {
    pub chat: Ulid,
    pub device: Ulid,
    pub kind: Kind,
    pub content_kind: Option<ContentKind>,
    pub encrypted: bool,
    pub epoch: Option<u64>,
    pub pointer_seq: Option<u64>,
    pub call_seq: Option<u64>,
    pub client_sent_at: i64,
    pub now: i64,
    pub signature: Vec<u8>,
    pub body: Vec<u8>,
    /// A commit's. Empty otherwise.
    pub wrap_list: Vec<NewWrap>,
    pub welcome_list: Vec<NewWelcome>,
}

pub enum Appended {
    Saved(u64),
    /// A commit that was not for the next epoch, or for a group with none uploaded yet.
    CommitLost,
}

pub enum GroupInfoPut {
    Saved,
    /// Epoch 0 of this chat was already uploaded by someone else, and this one lost.
    AlreadyStarted,
    /// Not for the chat's current epoch.
    Stale,
}

#[derive(Serialize)]
pub struct WrapRow {
    pub epoch: u64,
    pub account: Ulid,
    pub wrapped: Bytes,
}

pub trait Storage: Send + Sync + 'static {
    /// Adds whichever built-in roles are missing, leaving existing ones as they are.
    fn seed_roles(&self, role_list: Vec<BuiltInRole>) -> impl Future<Output = Result<()>> + Send;
    /// Every permission name any role holds.
    fn permission_names(&self) -> impl Future<Output = Result<Vec<String>>> + Send;

    /// `false` if the username is taken. Also gives the account its server role.
    fn create_account(&self, account: NewAccount) -> impl Future<Output = Result<bool>> + Send;
    fn account_by_username(
        &self,
        username: String,
    ) -> impl Future<Output = Result<Option<Account>>> + Send;
    fn account_by_id(&self, id: Ulid) -> impl Future<Output = Result<Option<Account>>> + Send;
    /// Partial usernames and display names. An empty term lists everyone.
    fn search_accounts(
        &self,
        term: String,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<AccountBrief>>> + Send;
    /// Uses up a one-time link. `None` if it is unknown or expired.
    fn take_setup(
        &self,
        token_hash: [u8; 32],
        now: i64,
    ) -> impl Future<Output = Result<Option<AccountBrief>>> + Send;
    /// Sets the password and ends all of the account's sessions.
    fn set_password(&self, account: Ulid, hash: String) -> impl Future<Output = Result<()>> + Send;
    /// Sets the public keys and creates the self chat under `chat`, in one step. `false` if the
    /// account already has keys.
    fn set_account_keys(
        &self,
        account: Ulid,
        master_public: Vec<u8>,
        account_public: Vec<u8>,
        stamp: Vec<u8>,
        chat: Ulid,
        now: i64,
    ) -> impl Future<Output = Result<bool>> + Send;

    fn create_challenge(
        &self,
        value: Vec<u8>,
        account: Ulid,
        purpose: i64,
        expires_at: i64,
        now: i64,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Uses one up, whatever happens next. `false` if it is unknown, expired or for something else.
    fn take_challenge(
        &self,
        value: Vec<u8>,
        account: Ulid,
        purpose: i64,
        now: i64,
    ) -> impl Future<Output = Result<bool>> + Send;

    fn create_captcha(
        &self,
        id: Ulid,
        answer: String,
        expires_at: i64,
    ) -> impl Future<Output = Result<()>> + Send;
    /// The answer and expiry, deleting the row whatever the answer turns out to be.
    fn take_captcha(&self, id: Ulid) -> impl Future<Output = Result<Option<(String, i64)>>> + Send;
    fn sweep_captchas(&self, expired_before: i64) -> impl Future<Output = Result<()>> + Send;

    fn create_session(&self, session: NewSession) -> impl Future<Output = Result<()>> + Send;
    /// Finds a live session and slides it forward.
    fn session_by_token(
        &self,
        token_hash: [u8; 32],
        now: i64,
        expires_at: i64,
    ) -> impl Future<Output = Result<Option<Session>>> + Send;
    fn list_sessions(&self, account: Ulid)
    -> impl Future<Output = Result<Vec<SessionItem>>> + Send;
    fn delete_session(&self, account: Ulid, id: Ulid) -> impl Future<Output = Result<bool>> + Send;

    /// Whether an account holds a server permission. The admin role holds them all.
    fn server_permitted(
        &self,
        account: Ulid,
        permission: &'static str,
    ) -> impl Future<Output = Result<bool>> + Send;
    /// Whether a chat's member holds a permission there. The server admin role holds them all, and
    /// in a direct or self chat both people hold the built-in member role.
    fn chat_permitted(
        &self,
        chat: Ulid,
        account: Ulid,
        permission: &'static str,
    ) -> impl Future<Output = Result<bool>> + Send;

    fn device_usage(
        &self,
        account: Ulid,
        configured_limit: Option<u64>,
    ) -> impl Future<Output = Result<DeviceUsage>> + Send;
    /// Uses up the id, saves the device, attaches it to the session and writes the self chat's
    /// notice, in one step.
    fn register_device(
        &self,
        device: NewDevice,
        session: Ulid,
        configured_limit: Option<u64>,
    ) -> impl Future<Output = Result<Registered>> + Send;
    fn device_key(&self, id: Ulid) -> impl Future<Output = Result<Option<DeviceKey>>> + Send;
    fn list_devices(&self, account: Ulid) -> impl Future<Output = Result<Vec<DeviceItem>>> + Send;
    /// Stops the device, ends its sessions and drops its key packages. `false` if it had already
    /// ended, which changes nothing.
    fn end_device(
        &self,
        id: Ulid,
        statement: Vec<u8>,
        now: i64,
    ) -> impl Future<Output = Result<bool>> + Send;
    /// Everyone sharing a chat with this account.
    fn accounts_sharing(&self, account: Ulid) -> impl Future<Output = Result<Vec<Ulid>>> + Send;
    /// Opens a row in the device's log, or extends its last one if that ended within `grace_ms`.
    fn device_online(
        &self,
        id: Ulid,
        ip: String,
        now: i64,
        grace_ms: i64,
    ) -> impl Future<Output = Result<()>> + Send;
    fn device_offline(&self, id: Ulid, now: i64) -> impl Future<Output = Result<()>> + Send;
    /// Closes every open row. At startup these are what a crash left, and at shutdown what is left.
    fn close_device_logs(&self, now: i64) -> impl Future<Output = Result<()>> + Send;

    /// The new total, or `None` if the upload would pass `limit` and nothing was saved.
    fn add_key_packages(
        &self,
        device: Ulid,
        package_list: Vec<(Ulid, Vec<u8>)>,
        limit: u64,
        now: i64,
    ) -> impl Future<Output = Result<Option<u64>>> + Send;
    /// One package per live device of the account, each deleted as it is handed out.
    fn claim_key_packages(
        &self,
        account: Ulid,
    ) -> impl Future<Output = Result<Vec<KeyPackageItem>>> + Send;

    /// The pair's chat, and whether it was made now. Both accounts equal is a self chat.
    fn direct_chat(
        &self,
        id: Ulid,
        low: Ulid,
        high: Ulid,
        now: i64,
    ) -> impl Future<Output = Result<(Ulid, bool)>> + Send;
    /// `None` if this account is not a member, or the chat does not exist.
    fn chat_access(
        &self,
        chat: Ulid,
        account: Ulid,
    ) -> impl Future<Output = Result<Option<ChatAccess>>> + Send;
    fn member_accounts(&self, chat: Ulid) -> impl Future<Output = Result<Vec<Ulid>>> + Send;
    fn chat_list(&self, account: Ulid) -> impl Future<Output = Result<Vec<ChatItem>>> + Send;
    fn chat_devices(&self, chat: Ulid) -> impl Future<Output = Result<Vec<ChatDevice>>> + Send;

    fn message_brief(
        &self,
        chat: Ulid,
        seq: u64,
    ) -> impl Future<Output = Result<Option<MessageBrief>>> + Send;
    /// The next sequence id and everything that comes with this kind, in one transaction: a delete
    /// wipes its target and the target's edits and walks the preview back, and a commit stores its
    /// wraps and welcomes and moves the epoch on.
    fn append_message(&self, message: NewMessage) -> impl Future<Output = Result<Appended>> + Send;
    fn history(
        &self,
        chat: Ulid,
        after_seq: u64,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<MessageFrame>>> + Send;
    /// Keeps the highest. A lower or equal one changes nothing.
    fn record_seen(
        &self,
        chat: Ulid,
        account: Ulid,
        device: Ulid,
        up_to_seq: u64,
        client_seen_at: i64,
        now: i64,
    ) -> impl Future<Output = Result<()>> + Send;

    fn group_info(&self, chat: Ulid)
    -> impl Future<Output = Result<Option<(u64, Vec<u8>)>>> + Send;
    fn put_group_info(
        &self,
        chat: Ulid,
        epoch: u64,
        body: Vec<u8>,
        now: i64,
    ) -> impl Future<Output = Result<GroupInfoPut>> + Send;
    fn wraps(&self, chat: Ulid) -> impl Future<Output = Result<Vec<WrapRow>>> + Send;
    /// Replaces what is stored, because this is how a device corrects a bad wrap.
    fn put_wraps(
        &self,
        chat: Ulid,
        epoch: u64,
        wrap_list: Vec<NewWrap>,
        device: Ulid,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Deletes this device's waiting Welcomes and returns each with its chat.
    fn take_welcomes(
        &self,
        device: Ulid,
    ) -> impl Future<Output = Result<Vec<(Ulid, Vec<u8>)>>> + Send;
}
