//! What the rest of the server sees of the database. Each method is one whole operation, and no SQL
//! and no transaction crosses this trait, so that Postgres can be added beside `storage/sqlite.rs`.

use std::future::Future;

use keryx_core::protocol::{
    AccountBrief, ActionKind, CallEvent, ChatDevice, ChatEvent, ChatFolders, ChatItem, ChatOptions,
    ChatProfileItem, ChatRole, ChatType, ContentKind, DeviceItem, DraftItem, FindableItem,
    FoundChat, InvitationItem, KeyPackageItem, Kind, KvChange, KvReply, MarkItem, MessageFrame,
    ProfileKeyItem, ProfileReply, RequestItem, SessionItem, StickerItem, StickerPack, SyncPosition,
    TimesItem, TopicItem, TopicWrite, WrapEntry,
};
use keryx_core::ulid::Ulid;

pub mod sqlite;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("The database failed")]
    Database(#[from] rusqlite::Error),
    #[error("A database task failed")]
    Task(#[source] tokio::task::JoinError),
}

pub type Result<T> = std::result::Result<T, Error>;

/// One account's presence times, the newest over its devices.
pub struct PresenceTimes {
    pub account: Ulid,
    pub connected_at: Option<i64>,
    pub in_app_at: Option<i64>,
}

/// What became of a change to an account's settings.
pub enum KvChanged {
    /// Saved, under this new version.
    Saved(u64),
    /// Based on an older version: the server's, which wins.
    Stale(KvReply),
    /// It would leave the account with more keys than it may have.
    TooMany,
}

/// One of the built-in roles, which are only ever added when missing.
pub struct BuiltInRole {
    pub id: Ulid,
    /// 0 server, 1 chat.
    pub scope: i64,
    pub name: &'static str,
    pub permission_list: Vec<String>,
}

/// Which of a member's two positions a report moves: "received up to N" or "seen up to N".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Position {
    Received,
    Seen,
}

/// Where in a chat a position is: its main list, a topic or a thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Main,
    Topic(Ulid),
    Thread(u64),
}

/// One `received` or `seen` from a device.
pub struct PositionReport {
    pub position: Position,
    pub chat: Ulid,
    pub account: Ulid,
    pub device: Ulid,
    pub place: Place,
    pub up_to_seq: u64,
    pub client_at: i64,
    pub now: i64,
}

pub struct Account {
    pub id: Ulid,
    pub username: String,
    pub display_name: Option<String>,
    /// `None` until the one-time link is used.
    pub password_hash: Option<String>,
    /// `None` until the words are set up, like the four after it.
    pub master_public: Option<Vec<u8>>,
    pub account_public: Option<Vec<u8>>,
    pub account_stamp: Option<Vec<u8>>,
    pub invite_public: Option<Vec<u8>>,
    pub invite_stamp: Option<Vec<u8>>,
    /// Moves on every change of the names, photo, bio or links.
    pub profile_version: u64,
}

/// The words' public keys and their stamps, checked before they reach storage.
pub struct NewAccountKeys {
    pub master_public: Vec<u8>,
    pub account_public: Vec<u8>,
    pub account_stamp: Vec<u8>,
    pub invite_public: Vec<u8>,
    pub invite_stamp: Vec<u8>,
}

pub struct NewAccount {
    pub id: Ulid,
    pub username: String,
    pub display_name: Option<String>,
    pub password_hash: Option<String>,
    pub admin: bool,
    /// The id of its `all` folder, written with it.
    pub all_folder: Ulid,
    /// The one-time link's token hash and when it stops working.
    pub setup: Option<([u8; 32], i64)>,
    pub now: i64,
}

/// A one-time setup link, looked up by the hash of the part after `#`.
pub enum SetupLink {
    /// Nothing was issued for this hash.
    Unknown,
    /// The password was already chosen with it.
    Used,
    /// Older than its 7 days, and never used.
    Expired,
    /// Unused and still inside its 7 days.
    Ready(AccountBrief),
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

/// What a member's check needs to know about a chat.
pub struct ChatAccess {
    pub chat_type: ChatType,
    pub last_seq: u64,
    /// This member's role. In a direct or self chat, always member.
    pub role: ChatRole,
    /// Seconds between one member's messages in the main list or a topic. `None` is off.
    pub slow_mode: Option<u64>,
    pub topics_on: bool,
}

/// Which of a message's times the asker may see, by its permissions.
#[derive(Debug, Clone, Copy)]
pub struct TimesShown {
    pub received: bool,
    pub seen: bool,
}

/// A stored message, when deciding about an edit or delete pointing at it, or a thread under it.
/// `None` for one removed.
pub struct MessageBrief {
    pub kind: Kind,
    pub content_kind: Option<ContentKind>,
    pub account: Ulid,
    pub device_created_at: i64,
    pub saved_at: i64,
    pub topic: Option<Ulid>,
    pub thread_seq: Option<u64>,
}

/// A new group or channel, with its owner alone in it.
pub struct NewGroup {
    pub id: Ulid,
    pub chat_type: ChatType,
    pub owner: Ulid,
    /// The owner's device, whose "created" line it is.
    pub device: Ulid,
    pub topics_on: bool,
    pub findable: Option<FindableItem>,
    pub now: i64,
}

/// What a group's or channel's commit does to its people, as the chat's task checked it.
pub struct NewAction {
    pub kind: ActionKind,
    pub account_list: Vec<Ulid>,
    /// The account whose device committed.
    pub actor: Ulid,
    /// The most member accounts the chat may hold after an add.
    pub limit: u64,
}

/// What became of leaving a group or channel.
pub enum Left {
    /// Not a member.
    NotMember,
    /// Gone from it, with its history row, its line in a group, and the members left.
    Left {
        event: Box<ChatEvent>,
        line: Option<Box<MessageFrame>>,
        member_list: Vec<Ulid>,
    },
    /// The last owner left, so the whole chat went, for these members.
    Removed(Removed),
}

/// What became of changing a member's role.
pub enum RoleSet {
    /// Not a member.
    NotMember,
    /// It would leave the chat with no owner.
    LastOwner,
    /// Changed, with its history row, its line in a group, and the members.
    Set {
        event: Box<ChatEvent>,
        line: Option<Box<MessageFrame>>,
        member_list: Vec<Ulid>,
    },
}

/// A chat removed for everyone: who was in it, who had asked to join or was invited, whose rows
/// went with it, and its files, whose bytes can go.
pub struct Removed {
    pub member_list: Vec<Ulid>,
    pub asker_list: Vec<Ulid>,
    pub invitee_list: Vec<Ulid>,
    pub file_list: Vec<Ulid>,
}

/// What became of asking to join.
pub enum Asked {
    /// Not a findable group or channel.
    NotFindable,
    AlreadyMember,
    Saved(Box<RequestItem>),
}

/// An invitation as its inviter's device sent it.
pub struct NewInvitation {
    pub chat: Ulid,
    pub account: Ulid,
    pub inviter: Ulid,
    pub sealed: Vec<u8>,
    pub signature: Vec<u8>,
    pub now: i64,
}

/// What became of answering an invitation.
pub enum Answered {
    /// There is none, or it expired.
    NoInvitation,
    /// The chat is full.
    Full,
    /// Accepted, now a member, with its history row and its line in a group, or declined, with
    /// the inviter's line.
    Saved {
        invitation: Box<InvitationItem>,
        event: Box<ChatEvent>,
        line: Option<Box<MessageFrame>>,
    },
}

/// What became of a change of topics.
pub enum TopicsPut {
    /// Saved, with the events it wrote and whether any topic's messages were wiped.
    Saved {
        event_list: Vec<ChatEvent>,
        wiped: bool,
    },
    /// An id that is neither a topic of this chat nor one issued for it to this account.
    UnknownTopic,
    TooMany,
}

/// What a chat's members may do beyond or short of the member role, its names already prefixed,
/// and its slow mode.
pub struct NewChatOptions {
    pub allow: Vec<String>,
    pub deny: Vec<String>,
    pub slow_mode: Option<u64>,
}

/// A new group's or channel's first profile and options, saved with epoch 0's group info.
pub struct NewGroupStart {
    pub profile: NewChatProfile,
    /// The chat's profile key, sealed at epoch 0.
    pub key: Vec<u8>,
    pub options: Option<NewChatOptions>,
}

/// A group's or channel's sealed profile as a device saved it.
pub struct NewChatProfile {
    pub chat: Ulid,
    pub device: Ulid,
    pub account: Ulid,
    pub sealed: Vec<u8>,
    pub signature: Vec<u8>,
    pub photo: Option<Ulid>,
    /// `[limits.upload] photo`, which a chat's photo stays under, though it went up as an image.
    pub photo_limit: u64,
    pub now: i64,
}

/// A topic's settings, for one member's check there.
pub struct TopicAccess {
    pub closed: bool,
    pub hidden: bool,
    pub write: TopicWrite,
    /// Its own slow mode. `None` follows the chat's, and 0 is off.
    pub slow_mode: Option<u64>,
    /// When this account last wrote in its main list, for slow mode.
    pub last_sent_at: Option<i64>,
    /// This account is one of the topic's own moderators.
    pub moderator: bool,
    /// Its changes to the member role, names prefixed by the chat's type.
    pub allow: Vec<String>,
    pub deny: Vec<String>,
}

/// One live topic's own moderators, and whether it is hidden: from everyone but them and the
/// chat's owners and moderators.
pub struct TopicMods {
    pub topic: Ulid,
    pub hidden: bool,
    pub mod_list: Vec<Ulid>,
}

/// A topic's own options, checked, with their names prefixed.
pub struct NewTopicOptions {
    pub allow: Vec<String>,
    pub deny: Vec<String>,
    pub slow_mode: Option<u64>,
    pub write: TopicWrite,
    pub closed: bool,
    pub hidden: bool,
}

/// A topic's sealed profile to keep, its photo checked as a chat's is.
pub struct NewTopicProfile {
    pub chat: Ulid,
    pub topic: Ulid,
    pub device: Ulid,
    pub account: Ulid,
    pub sealed: Vec<u8>,
    pub signature: Vec<u8>,
    pub photo: Option<Ulid>,
    pub photo_limit: u64,
    pub now: i64,
}

/// What became of a change to a topic's settings, moderators or profile.
pub enum TopicSaved {
    /// Kept, with its history row, the whole item as it is now, whether it was hidden before, and
    /// photos no longer named, whose bytes can go. No row for a topic's first profile set by
    /// whoever added it, which is part of creating it.
    Saved {
        event: Option<ChatEvent>,
        item: Box<TopicItem>,
        was_hidden: bool,
        removed: Vec<Ulid>,
    },
    /// No live topic with that id in the chat.
    NoSuchTopic,
    /// The photo named is not a complete image this account uploaded into this chat.
    NoSuchPhoto,
    /// The account to make a moderator is not in the chat.
    NotMember,
}

pub enum ChatProfileSaved {
    /// The profile as kept, with the photos no longer named, whose bytes can go.
    Saved {
        item: ChatProfileItem,
        removed: Vec<Ulid>,
    },
    /// The photo named is not a complete image this account uploaded into this chat, under the
    /// photo limit.
    NoSuchPhoto,
    /// The chat has no profile yet, which its epoch 0 brings.
    NotStarted,
}

/// An upload as it starts.
pub struct NewFile {
    pub id: Ulid,
    pub chat: Ulid,
    pub encrypted: bool,
    pub kind: ContentKind,
    pub size: u64,
    pub chunks: u64,
    pub device: Ulid,
    pub now: i64,
}

/// A sticker whose bytes are already on disk, for its pack.
pub struct NewSticker {
    pub id: Ulid,
    pub pack: Ulid,
    pub emoji: Option<String>,
    pub width: u32,
    pub height: u32,
    pub size: u64,
    pub now: i64,
}

/// A photo whose bytes are already on disk, for its account's profile to name.
pub struct NewPhoto {
    pub id: Ulid,
    pub account: Ulid,
    pub device: Ulid,
    pub size: u64,
    pub now: i64,
}

/// A profile as a device saved it: sealed, signed, and naming its photo.
pub struct NewProfile {
    pub account: Ulid,
    pub device: Ulid,
    pub sealed: Option<(Vec<u8>, Vec<u8>)>,
    pub photo: Option<Ulid>,
    pub now: i64,
}

pub enum ProfileSaved {
    /// The new version, with the photos no longer named, whose bytes can go.
    Saved { version: u64, removed: Vec<Ulid> },
    /// The photo named is not one of this account's uploads.
    NoSuchPhoto,
}

/// New names for an account. `None` keeps what it has.
pub struct NewNames {
    pub account: Ulid,
    pub display_name: Option<String>,
    pub username: Option<String>,
}

pub enum NamesSaved {
    Saved {
        version: u64,
        username: String,
        display_name: Option<String>,
    },
    UsernameTaken,
    NoSuchAccount,
}

/// A chat's stored file. `account` is its uploader's.
pub struct StoredFile {
    pub chat: Ulid,
    pub account: Ulid,
    pub size: u64,
    pub chunks: u64,
    pub received: u64,
    pub used: bool,
    pub removed: bool,
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
    /// The device's account.
    pub account: Ulid,
    pub kind: Kind,
    pub content_kind: Option<ContentKind>,
    pub encrypted: bool,
    pub epoch: Option<u64>,
    pub pointer_seq: Option<u64>,
    pub client_sent_at: i64,
    pub now: i64,
    pub signature: Vec<u8>,
    pub body: Vec<u8>,
    /// A commit's. Empty otherwise.
    pub wrap_list: Vec<NewWrap>,
    pub welcome_list: Vec<NewWelcome>,
    /// A commit's profile keys, sealed at its epoch. Each replaces an older one.
    pub profile_key_list: Vec<(Ulid, Vec<u8>)>,
    /// A commit's group info at its epoch, saved in the same step so it is never left behind.
    /// `None` otherwise.
    pub group_info: Option<Vec<u8>>,
    /// A commit's public group after it, which the chat's task read the commit against.
    pub public_group: Option<Vec<u8>>,
    /// Files a content message names, kept from now on. Checked before.
    pub persist: Vec<Ulid>,
    /// Files an edit or delete drops, marked removed. Checked before; their bytes go after.
    pub remove: Vec<Ulid>,
    /// A content message's topic, which must be live. `None` for General.
    pub topic: Option<Ulid>,
    /// A content message's thread.
    pub thread_seq: Option<u64>,
    /// Counted for slow mode: a content message in the main list or a topic.
    pub slow: bool,
    /// A group's or channel's delete or edit, written into its history with a copy of what it
    /// removes or changes: the message's author.
    pub history: Option<Ulid>,
    /// A group's or channel's commit's action, applied in the same step.
    pub action: Option<NewAction>,
    /// A commit's share of the chat's profile key, sealed at its epoch.
    pub chat_key: Option<Vec<u8>>,
}

pub enum Appended {
    Saved {
        seq: u64,
        /// The chat events it wrote.
        event_list: Vec<ChatEvent>,
        /// The membership lines it wrote after it, in a group.
        line_list: Vec<MessageFrame>,
        /// Accounts it took out, whose folders changed.
        gone_list: Vec<Ulid>,
        /// The chat's profile with the profile key this commit shared, when it shared one.
        profile: Option<ChatProfileItem>,
    },
    /// Content or an edit sealed under an older key than the chat's current one.
    StaleEpoch,
    /// A commit that was not for the next epoch, or for a group with none uploaded yet.
    CommitLost,
    /// The topic was removed.
    TopicGone,
    /// An add would pass the chat's member limit.
    Full,
}

/// Where a `sync` starts: from a server time, already stepped back, or exactly after a page's last
/// row.
#[derive(Debug, Clone, Copy)]
pub enum SyncFrom {
    Since(i64),
    After(SyncPosition),
}

/// One `sync` page: messages, call rows and chat events in their order, and whether more follow.
pub struct SyncPage {
    pub message_list: Vec<MessageFrame>,
    pub call_list: Vec<CallEvent>,
    pub chat_event_list: Vec<ChatEvent>,
    pub more: bool,
}

pub enum GroupInfoPut {
    /// With the first profile as kept, when it carried one and was not a resend.
    Saved { profile: Option<ChatProfileItem> },
    /// The first profile names a photo that is not a complete image this account uploaded here.
    NoSuchPhoto,
    /// Epoch 0 of this chat was already uploaded by someone else, and this one lost. The same
    /// bytes again are `Saved`, since that is a resend whose answer was lost.
    AlreadyStarted,
    /// Not for the chat's current epoch.
    Stale,
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
    /// What a one-time setup link is. A used link stays, so a second try is not the same as one
    /// that was never issued.
    fn setup_link(
        &self,
        token_hash: [u8; 32],
        now: i64,
    ) -> impl Future<Output = Result<SetupLink>> + Send;
    /// Sets the password and marks the link used, in one step. `false` if it was used or expired
    /// between the lookup and this call.
    fn finish_setup(
        &self,
        account: Ulid,
        token_hash: [u8; 32],
        password_hash: String,
        now: i64,
    ) -> impl Future<Output = Result<bool>> + Send;
    /// Sets the password and ends all of the account's sessions.
    fn set_password(&self, account: Ulid, hash: String) -> impl Future<Output = Result<()>> + Send;
    /// Sets the public keys and creates the self chat under `chat`, in one step. `false` if the
    /// account already has keys.
    fn set_account_keys(
        &self,
        account: Ulid,
        keys: NewAccountKeys,
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
    /// Whether an account holds the built-in server admin role.
    fn is_admin(&self, account: Ulid) -> impl Future<Output = Result<bool>> + Send;
    /// Whether a chat's member holds a permission there, named with its prefix. The server admin
    /// role holds them all. An owner holds the built-in owner role, a `member_role` row adds
    /// moderator, and every member holds the built-in member role, as the chat's `chat_permission`
    /// rows change it.
    fn chat_permitted(
        &self,
        chat: Ulid,
        account: Ulid,
        permission: String,
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
    /// Writes a device's presence times. A `None` leaves its column as it is.
    fn device_presence(
        &self,
        id: Ulid,
        connected_at: Option<i64>,
        in_app_at: Option<i64>,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Each account's newest last-connected and last-in-app times over its devices. An account with
    /// no device, or none that ever announced itself, has none.
    fn presence_times(
        &self,
        account_list: Vec<Ulid>,
    ) -> impl Future<Output = Result<Vec<PresenceTimes>>> + Send;

    /// This account's settings: their version, and the whole set unless the version is `known`.
    fn kv(&self, account: Ulid, known: Option<u64>)
    -> impl Future<Output = Result<KvReply>> + Send;
    /// Applies a change based on its version, in one step: refused with the whole set when the
    /// account has moved on, and with nothing changed when it would hold more than `max_keys`.
    fn change_kv(
        &self,
        account: Ulid,
        change: KvChange,
        max_keys: u64,
    ) -> impl Future<Output = Result<KvChanged>> + Send;

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
    /// Makes a group or channel with its owner alone, writing its "created" history row and, in a
    /// group, its line.
    fn create_group(
        &self,
        group: NewGroup,
    ) -> impl Future<Output = Result<(ChatEvent, Option<MessageFrame>)>> + Send;
    /// Each member's role.
    fn member_roles(
        &self,
        chat: Ulid,
    ) -> impl Future<Output = Result<Vec<(Ulid, ChatRole)>>> + Send;
    /// Leaves a group or channel, out of the account's folders, writing a "left" line. The last
    /// owner leaving removes the whole chat.
    fn leave_chat(
        &self,
        chat: Ulid,
        account: Ulid,
        device: Ulid,
        now: i64,
    ) -> impl Future<Output = Result<Left>> + Send;
    /// Makes `account` an owner, a moderator or a plain member, by `actor`, writing its line.
    fn set_role(
        &self,
        chat: Ulid,
        actor: Ulid,
        device: Ulid,
        account: Ulid,
        role: ChatRole,
        now: i64,
    ) -> impl Future<Output = Result<RoleSet>> + Send;
    /// Removes a whole chat for everyone, out of every member's folders. `None` when there is none.
    fn remove_chat(&self, chat: Ulid) -> impl Future<Output = Result<Option<Removed>>> + Send;
    /// A join request, replacing a declined one. Requests asked or declined before
    /// `expire_before` are gone.
    fn ask_to_join(
        &self,
        chat: Ulid,
        account: Ulid,
        note: Option<String>,
        now: i64,
        expire_before: i64,
    ) -> impl Future<Output = Result<Asked>> + Send;
    /// Takes back a request. `false` when there was none.
    fn cancel_request(
        &self,
        chat: Ulid,
        account: Ulid,
    ) -> impl Future<Output = Result<bool>> + Send;
    /// Declines a waiting request. `None` when there was none waiting.
    fn decline_request(
        &self,
        chat: Ulid,
        account: Ulid,
        now: i64,
        expire_before: i64,
    ) -> impl Future<Output = Result<Option<RequestItem>>> + Send;
    /// A chat's waiting requests.
    fn chat_requests(
        &self,
        chat: Ulid,
        expire_before: i64,
    ) -> impl Future<Output = Result<Vec<RequestItem>>> + Send;
    /// An account's own requests, waiting or declined.
    fn own_requests(
        &self,
        account: Ulid,
        expire_before: i64,
    ) -> impl Future<Output = Result<Vec<RequestItem>>> + Send;
    /// An invitation, replacing an older one. `false` when the account is already a member.
    fn invite(&self, invitation: NewInvitation) -> impl Future<Output = Result<bool>> + Send;
    /// Accepts or declines an account's invitation, in one step: accepting makes it a member.
    /// `accept` is the accepting device, whose "joined" line it is, and none declines.
    fn answer_invitation(
        &self,
        chat: Ulid,
        account: Ulid,
        accept: Option<Ulid>,
        limit: u64,
        now: i64,
        expire_before: i64,
    ) -> impl Future<Output = Result<Answered>> + Send;
    /// An account's invitations.
    fn own_invitations(
        &self,
        account: Ulid,
        expire_before: i64,
    ) -> impl Future<Output = Result<Vec<InvitationItem>>> + Send;
    /// A new topic's id for this account in this chat, kept until `expires_at`.
    fn issue_topic(
        &self,
        chat: Ulid,
        account: Ulid,
        topic: Ulid,
        expires_at: i64,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Whether topics show, and the live topics: a new id must have been issued to this account for
    /// this chat, and one left out is removed with its messages wiped.
    fn put_topics(
        &self,
        chat: Ulid,
        account: Ulid,
        on: bool,
        topic_list: Vec<Ulid>,
        max: u64,
        now: i64,
    ) -> impl Future<Output = Result<TopicsPut>> + Send;
    /// Saves a group's or channel's sealed profile, keeping the photo it names and removing the
    /// one it replaced.
    fn put_chat_profile(
        &self,
        profile: NewChatProfile,
    ) -> impl Future<Output = Result<ChatProfileSaved>> + Send;
    /// Replaces a chat's changes to its member role, its permission names already prefixed, and
    /// its slow mode.
    fn put_chat_options(
        &self,
        chat: Ulid,
        options: NewChatOptions,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Lists a group or channel in the directory under a readable name and bio, or stops.
    fn put_findable(
        &self,
        chat: Ulid,
        findable: Option<FindableItem>,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Findable groups and channels whose name holds the term.
    fn search_chats(
        &self,
        term: String,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<FoundChat>>> + Send;
    /// A chat's options, as `chat_list` gives them.
    fn chat_options(&self, chat: Ulid) -> impl Future<Output = Result<ChatOptions>> + Send;
    /// A topic's settings for this account's check, General's under the all-zero id. `None` when it
    /// is not a live topic of the chat.
    fn topic_access(
        &self,
        chat: Ulid,
        topic: Ulid,
        account: Ulid,
    ) -> impl Future<Output = Result<Option<TopicAccess>>> + Send;
    /// Replaces a topic's own options, writing the history row.
    fn put_topic_options(
        &self,
        chat: Ulid,
        topic: Ulid,
        actor: Ulid,
        options: NewTopicOptions,
        now: i64,
    ) -> impl Future<Output = Result<TopicSaved>> + Send;
    /// Keeps a topic's sealed profile, its photo and the history row.
    fn put_topic_profile(
        &self,
        profile: NewTopicProfile,
    ) -> impl Future<Output = Result<TopicSaved>> + Send;
    /// Makes a member one of a topic's moderators, or a plain member there again.
    fn put_topic_role(
        &self,
        chat: Ulid,
        topic: Ulid,
        actor: Ulid,
        account: Ulid,
        moderator: bool,
        now: i64,
    ) -> impl Future<Output = Result<TopicSaved>> + Send;
    /// A chat's live topics, General's settings row included, each with its own moderators and
    /// whether it is hidden.
    fn topic_moderators(&self, chat: Ulid) -> impl Future<Output = Result<Vec<TopicMods>>> + Send;
    /// A chat's type, for one who is not in it yet. `None` when there is no such chat.
    fn chat_type(&self, chat: Ulid) -> impl Future<Output = Result<Option<ChatType>>> + Send;
    /// `None` if this account is not a member, or the chat does not exist.
    fn chat_access(
        &self,
        chat: Ulid,
        account: Ulid,
    ) -> impl Future<Output = Result<Option<ChatAccess>>> + Send;
    fn member_accounts(&self, chat: Ulid) -> impl Future<Output = Result<Vec<Ulid>>> + Send;
    /// The sealed profile keys a chat keeps for these accounts, with each account's profile version,
    /// as a `chats` entry carries them.
    fn chat_profile_keys(
        &self,
        chat: Ulid,
        account_list: Vec<Ulid>,
    ) -> impl Future<Output = Result<Vec<ProfileKeyItem>>> + Send;
    /// The account's chats. Join requests asked before `expire_before` are gone, so they are not
    /// counted.
    fn chat_list(
        &self,
        account: Ulid,
        expire_before: i64,
    ) -> impl Future<Output = Result<Vec<ChatItem>>> + Send;
    fn chat_devices(&self, chat: Ulid) -> impl Future<Output = Result<Vec<ChatDevice>>> + Send;
    /// Keeps this account's sealed draft in the chat as it is, or clears it with `None`, and gives
    /// what is now kept. The server never reads it.
    fn put_draft(
        &self,
        chat: Ulid,
        account: Ulid,
        draft: Option<(u64, Vec<u8>)>,
        now: i64,
    ) -> impl Future<Output = Result<Option<DraftItem>>> + Send;
    /// This account's folders in tab order, `all` included, and the chats it is in.
    fn chat_folders(
        &self,
        account: Ulid,
    ) -> impl Future<Output = Result<(ChatFolders, Vec<Ulid>)>> + Send;
    /// Replaces this account's whole arrangement in one step. Each id not already one of its folders
    /// must have been issued to it under `purpose` and not have expired, and is used up. `false`, with
    /// nothing changed, when one was not.
    fn put_chat_folders(
        &self,
        account: Ulid,
        folders: ChatFolders,
        purpose: i64,
        now: i64,
    ) -> impl Future<Output = Result<bool>> + Send;
    /// When each member but the message's sender first received and first saw it, by the server's
    /// clock, or `None` if there is no such message. A page of at most `limit`, seen first, then
    /// received only, then neither, the newest first and then by account, after the account
    /// `after`, and whether more follow. A time `shown` says the asker may not see is left out
    /// before ordering, so the order cannot tell it.
    fn message_times(
        &self,
        chat: Ulid,
        seq: u64,
        after: Option<Ulid>,
        shown: TimesShown,
        limit: usize,
    ) -> impl Future<Output = Result<Option<(Vec<TimesItem>, bool)>>> + Send;

    fn message_brief(
        &self,
        chat: Ulid,
        seq: u64,
    ) -> impl Future<Output = Result<Option<MessageBrief>>> + Send;
    /// The next sequence id and everything that comes with this kind, in one transaction: a delete
    /// wipes its target and the target's edits and walks the preview back, and a commit stores its
    /// wraps and welcomes and moves the epoch on.
    fn append_message(&self, message: NewMessage) -> impl Future<Output = Result<Appended>> + Send;
    /// Up to `limit` messages and call rows from `from` on, in the account's chats or only `chat`,
    /// ordered by server time, chat, sequence id and `n`, a message's `n` being 0.
    fn sync(
        &self,
        account: Ulid,
        chat: Option<Ulid>,
        from: SyncFrom,
        limit: u32,
    ) -> impl Future<Output = Result<SyncPage>> + Send;
    /// The server time for a message or call row: the clock, but never before the last one handed
    /// out, server-wide, so a clock that steps back cannot hide a row from a `sync`.
    fn saved_now(&self) -> i64;
    /// Keeps the highest, with a row of times for each rise. A lower or equal one changes nothing
    /// and gives `None`, and a rise gives the member's positions after it.
    fn record_position(
        &self,
        report: PositionReport,
    ) -> impl Future<Output = Result<Option<MarkItem>>> + Send;

    fn group_info(&self, chat: Ulid)
    -> impl Future<Output = Result<Option<(u64, Vec<u8>)>>> + Send;
    /// The chat's public group, which commits are read against. `None` before epoch 0.
    fn public_group(&self, chat: Ulid) -> impl Future<Output = Result<Option<Vec<u8>>>> + Send;
    /// A new group's or channel's epoch 0 may carry its first profile and options, saved with it.
    /// Epoch 0 starts the chat's public group too, as `public_group`.
    fn put_group_info(
        &self,
        chat: Ulid,
        epoch: u64,
        body: Vec<u8>,
        now: i64,
        start: Option<NewGroupStart>,
        public_group: Option<Vec<u8>>,
    ) -> impl Future<Output = Result<GroupInfoPut>> + Send;
    fn wraps(&self, chat: Ulid) -> impl Future<Output = Result<Vec<WrapEntry>>> + Send;
    /// Replaces what is stored, because this is how a device corrects a bad wrap.
    fn put_wraps(
        &self,
        chat: Ulid,
        epoch: u64,
        wrap_list: Vec<NewWrap>,
        device: Ulid,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Keeps profile keys sealed at `epoch` in a chat, each replacing one sealed at an older epoch.
    fn put_profile_keys(
        &self,
        chat: Ulid,
        epoch: u64,
        key_list: Vec<(Ulid, Vec<u8>)>,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Deletes this device's waiting Welcomes and returns each with its chat.
    fn take_welcomes(
        &self,
        device: Ulid,
    ) -> impl Future<Output = Result<Vec<(Ulid, Vec<u8>)>>> + Send;

    /// Call rows, saved in one step.
    fn save_call(&self, event_list: Vec<CallEvent>) -> impl Future<Output = Result<()>> + Send;
    /// Ends every call still open, one with no `end` row, as missed unless the other account joined,
    /// with an `end` from the caller's device. Nobody can be in one after a restart.
    fn end_open_calls(&self, now: i64) -> impl Future<Output = Result<()>> + Send;

    /// A new sticker pack. False when its name is taken.
    fn create_pack(
        &self,
        id: Ulid,
        name: String,
        now: i64,
    ) -> impl Future<Output = Result<bool>> + Send;
    /// Removes a pack and its stickers, giving the stickers' ids so their bytes can go, or `None` when
    /// there is no such pack.
    fn remove_pack(&self, id: Ulid) -> impl Future<Output = Result<Option<Vec<Ulid>>>> + Send;
    /// False when its pack does not exist.
    fn add_sticker(&self, sticker: NewSticker) -> impl Future<Output = Result<bool>> + Send;
    /// False when there is no such sticker.
    fn remove_sticker(&self, id: Ulid) -> impl Future<Output = Result<bool>> + Send;
    /// Every pack with its stickers, each oldest first.
    fn sticker_list(&self) -> impl Future<Output = Result<Vec<StickerPack>>> + Send;
    fn sticker(&self, id: Ulid) -> impl Future<Output = Result<Option<StickerItem>>> + Send;

    /// An account's names, version, sealed profile with its device's key and stamp, and photo.
    fn profile(&self, account: Ulid) -> impl Future<Output = Result<Option<ProfileReply>>> + Send;
    /// Saves the profile, marks its photo used and removes the account's other photos' rows.
    fn put_profile(&self, profile: NewProfile)
    -> impl Future<Output = Result<ProfileSaved>> + Send;
    fn set_names(&self, names: NewNames) -> impl Future<Output = Result<NamesSaved>> + Send;
    /// A new photo, removing the rows of the account's photos no profile names yet, which it gives
    /// so their bytes can go.
    fn add_photo(&self, photo: NewPhoto) -> impl Future<Output = Result<Vec<Ulid>>> + Send;
    /// A photo's account, once a profile named it.
    fn photo(&self, id: Ulid) -> impl Future<Output = Result<Option<Ulid>>> + Send;
    /// Removes the account's photos, giving them and the new version, or `None` without one.
    fn remove_photo(
        &self,
        account: Ulid,
    ) -> impl Future<Output = Result<Option<(Vec<Ulid>, u64)>>> + Send;

    fn create_file(&self, file: NewFile) -> impl Future<Output = Result<()>> + Send;
    /// A chat's file, removed or not. `None` for an account's file or none at all.
    fn file(&self, id: Ulid) -> impl Future<Output = Result<Option<StoredFile>>> + Send;
    /// Chunk `n` arrived: the count moves to `n + 1` if it stood at `n`.
    fn file_received(&self, id: Ulid, n: u64) -> impl Future<Output = Result<()>> + Send;
    /// Deletes the rows of uploads never used and created before `before`, in one chat or all,
    /// and lists them with the removed files, whose bytes may be left, as (chat, file).
    fn take_unused_files(
        &self,
        chat: Option<Ulid>,
        before: i64,
    ) -> impl Future<Output = Result<Vec<(Ulid, Ulid)>>> + Send;
}
