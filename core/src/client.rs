//! A device's whole flow, the same for every client, so web and Android share one implementation.
//!
//! **It does no I/O.** One [`Client`] value and one [`Client::step`] function: a command from the
//! person, a server frame, a REST reply, a finished save or the timer goes in, with the time. Frames
//! to send, REST requests, events to show, snapshots to save and "wake me at T" come out. The
//! platform does the fetching, the socket, the storage and the timer.
//!
//! **It keeps no timeline.** Each opened message, edit or delete goes out as an event, and the
//! platform keeps what it shows, keyed by chat and sequence id, since a page and a live frame can
//! carry the same message. It holds the chat list's state, the queue and the MLS state.
//!
//! **Nothing is lost to a bad connection.** Each chat keeps the last sequence id handled with nothing
//! missing below it, and what was missed is paged forward from there. What was sent and not answered
//! is matched by its signature in those pages, and resent only if absent. Requests the client makes
//! by itself retry with a growing wait, and what needs the socket waits for it.
//!
//! **A commit waits for its snapshot.** It is built, saved, and only sent once the platform says the
//! save finished. A failed save drops it, so a commit never leaves without its state on disk.

use std::collections::{HashMap, HashSet};
use std::fmt;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::mls::{
    AccountKey, Device, DeviceRecord, MlsError, sign_revocation, stamp_account_key, verify_logout,
    verify_message, verify_revocation,
};
use crate::protocol::{
    AccountBrief, AccountInfo, AccountKeysBody, AccountListReply, AccountReply, Bytes,
    CaptchaAnswer, CaptchaReply, ChallengeBody, ChallengeReply, ChatDevice, ChatDeviceListReply,
    ChatItem, ChatReply, ChatType, ClientFrame, ContentKind, CountReply, CreateChatBody,
    DeviceProof, DeviceReply, DevicesReply, EmptyReply, FeaturesReply, GroupInfoReply, Header,
    KeyPackageItem, KeyPackageListReply, KeyPackagesBody, Kind, LoginBody, LoginReply, MarkItem,
    MessageFrame, RefusedReply, RegisterBody, RegisterDeviceBody, SendFrame, ServerFrame,
    SetupBody, SetupReply, SignedBody, WelcomeItem, WrapEntry, WrapItem, WrapListReply,
};
use crate::queue::{Level, Queue, Target};
use crate::seed::{self, AccountKeys};
use crate::ulid::Ulid;

/// How many key packages a device keeps on the server.
const KEY_PACKAGES: u64 = 10;
/// The waits before each retry, the last repeating: the connection, background requests, a lost
/// join and a failing save.
const RETRY_MS: [i64; 6] = [1_000, 2_000, 4_000, 8_000, 16_000, 30_000];
/// How many messages one page asks for.
const PAGE: u32 = 50;
/// Before this, everything: the newest page.
const NEWEST: u64 = i64::MAX as u64;

/// Text that must never be printed: a password, the words, a token or a message. Wiped when
/// dropped, and its `Debug` shows nothing of it.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(pub String);

impl fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Secret({} bytes)", self.0.len())
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// How this client proves its session, set when it is made. A browser uses the cookie and never
/// sees a token. Anything else gets the token and sends it as `Bearer`.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Auth {
    Cookie,
    Token,
}

/// What a device says about itself when it registers. Claims, since any of it can be faked.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Platform {
    /// 0 desktop, 1 mobile.
    pub platform: i64,
    pub os: String,
    pub os_version: String,
    #[serde(default)]
    pub web_platform: Option<String>,
    #[serde(default)]
    pub web_platform_version: Option<String>,
}

/// What an edit or delete points at: a stored message, or one of this client's still waiting.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pointer {
    Seq(u64),
    Local(u64),
}

/// What the person does.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    /// Registers, then logs in.
    SignUp {
        username: String,
        password: Secret,
        #[serde(default)]
        display_name: Option<String>,
        #[serde(default)]
        captcha: Option<CaptchaAnswer>,
    },
    LogIn {
        username: String,
        password: Secret,
        #[serde(default)]
        captcha: Option<CaptchaAnswer>,
    },
    /// A new captcha image, for the one under the password.
    Captcha,
    /// The one-time link's token, from after the `#`, then logs in.
    Setup {
        token: Secret,
        password: Secret,
        #[serde(default)]
        captcha: Option<CaptchaAnswer>,
    },
    /// New words to show, 12 or 24. Nothing keeps them.
    GenerateWords {
        count: usize,
    },
    /// The words for an account that has none yet.
    SetWords {
        words: Secret,
    },
    /// The words of an account that has them, on a new device.
    Restore {
        words: Secret,
    },
    ChatList,
    /// Its newest page.
    OpenChat {
        chat: Ulid,
    },
    /// The page below the oldest loaded.
    LoadOlder {
        chat: Ulid,
    },
    /// A direct chat, or with your own account, your self chat.
    StartChat {
        account: Ulid,
    },
    Search {
        text: String,
    },
    SendText {
        chat: Ulid,
        text: Secret,
    },
    Edit {
        chat: Ulid,
        target: Pointer,
        text: Secret,
    },
    Delete {
        chat: Ulid,
        target: Pointer,
    },
    Seen {
        chat: Ulid,
        up_to_seq: u64,
    },
    Typing {
        chat: Ulid,
        on: bool,
    },
    Devices,
    /// Needs the words, from any device.
    Terminate {
        device: Ulid,
        words: Secret,
    },
    LogOut,
    /// Retry the connection and anything waiting to retry, now.
    NetworkBack,
}

/// Everything that goes into [`Client::step`].
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Input {
    Command {
        command: Command,
    },
    /// The two snapshot slots, once, when the platform starts. `resume` connects a cookie
    /// client at once. A login for one username loads with `resume` false, so that device
    /// is not used until its own login.
    Loaded {
        #[serde(with = "serde_bytes")]
        #[cfg_attr(feature = "wasm", tsify(type = "Uint8Array | null"))]
        first: Option<Vec<u8>>,
        #[serde(with = "serde_bytes")]
        #[cfg_attr(feature = "wasm", tsify(type = "Uint8Array | null"))]
        second: Option<Vec<u8>>,
        resume: bool,
        /// What the last [`Event::Token`] handed out, for a token client. A cookie client has
        /// none, as the browser keeps its cookie.
        #[serde(default)]
        token: Option<Secret>,
    },
    Saved {
        id: u64,
    },
    SaveFailed {
        id: u64,
    },
    Connected,
    Closed {
        code: u16,
        reason: String,
    },
    Frame {
        text: String,
    },
    /// A REST reply. Status 0 means nothing answered.
    Reply {
        id: u64,
        status: u16,
        body: String,
    },
    Timer,
}

/// Everything [`Client::step`] asks the platform to do, in order.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify), tsify(missing_as_null))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Output {
    /// A frame for the WebSocket.
    Send {
        text: Secret,
    },
    /// A REST request. Its reply comes back as [`Input::Reply`] with the same id. `bearer` is set
    /// only with [`Auth::Token`].
    Request {
        id: u64,
        method: String,
        path: String,
        body: Option<Secret>,
        bearer: Option<Secret>,
    },
    /// Open the WebSocket at `/api/v1/ws` with the `keryx.v1` subprotocol.
    Connect {
        bearer: Option<Secret>,
    },
    Disconnect,
    /// Write these bytes to this slot as one write, then answer with [`Input::Saved`] or
    /// [`Input::SaveFailed`].
    Save {
        id: u64,
        slot: u8,
        #[serde(with = "serde_bytes")]
        #[cfg_attr(feature = "wasm", tsify(type = "Uint8Array"))]
        bytes: Vec<u8>,
    },
    /// Delete both slots: this device has ended.
    Wipe,
    /// Call `step` with [`Input::Timer`] at this time. A later one replaces it.
    WakeAt {
        at: i64,
    },
    Event {
        event: Event,
    },
}

/// Why a message cannot be shown as it is. It still appears, so the timeline has no hole.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Problem {
    /// Its signature does not verify against its device's key.
    BadSignature,
    /// Its device is not in the chat's device list, even freshly fetched.
    UnknownDevice,
    /// A field every stored message has is missing.
    Malformed,
}

/// A message to show.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify), tsify(missing_as_null))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shown {
    pub chat: Ulid,
    pub seq: u64,
    pub account: Option<Ulid>,
    /// Sent by this account, from any of its devices, so it never counts as unread.
    pub mine: bool,
    pub device: Option<Ulid>,
    pub content_kind: Option<ContentKind>,
    pub encrypted: bool,
    pub sent_at: i64,
    pub saved_at: i64,
    /// `None` when this device holds no key for it: its key's wrap is missing, or, once groups
    /// exist, it was sent while this account was out of the chat.
    pub text: Option<Secret>,
    pub problem: Option<Problem>,
}

/// What the platform shows.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify), tsify(missing_as_null))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// Shown once, to be written down. Nothing keeps them.
    Words {
        words: Secret,
    },
    NeedLogin,
    /// What `GET /api/v1/public/features` said. Sign up is offered only when `registration` is set.
    Features {
        registration: bool,
        captcha: bool,
    },
    /// A captcha image. `id` is what the answer is sent back with.
    Captcha {
        id: Ulid,
        image: String,
    },
    /// The account exists. Login is a second captcha, so it waits for another one.
    Registered {
        username: String,
    },
    /// Logged in with no device: new words, or `restore` with the account's own.
    NeedWords {
        restore: bool,
    },
    SignedIn {
        account: Ulid,
        username: String,
        device: Option<Ulid>,
    },
    /// A token client's login, to keep locked and hand back in [`Input::Loaded`], or `None` once
    /// the session is gone and what was kept must go. A cookie client never gets one.
    Token {
        token: Option<Secret>,
    },
    /// This account has as many devices as it may: list them and terminate some.
    DeviceLimit {
        message: String,
    },
    Account {
        account: AccountBrief,
    },
    /// The list, newest first. Previews follow as messages.
    Chats {
        chat_list: Vec<ChatItem>,
    },
    Message {
        message: Shown,
    },
    Edited {
        message: Shown,
        target_seq: u64,
    },
    Deleted {
        chat: Ulid,
        seq: u64,
        target_seq: u64,
    },
    /// Removed, by a delete or retention: only its sequence id is left.
    Removed {
        chat: Ulid,
        seq: u64,
    },
    /// A device joined this account, in the self chat.
    Notice {
        chat: Ulid,
        seq: u64,
        device: Ulid,
        saved_at: i64,
        problem: Option<Problem>,
    },
    /// A page has been shown. `has_older` is false once the first message is reached.
    PageLoaded {
        chat: Ulid,
        has_older: bool,
    },
    /// Waiting, in the queue or for its sequence id.
    Pending {
        chat: Ulid,
        local: u64,
        kind: Kind,
        target: Option<Pointer>,
        text: Option<Secret>,
    },
    Sent {
        chat: Ulid,
        local: u64,
        seq: u64,
        saved_at: i64,
    },
    Failed {
        chat: Ulid,
        local: u64,
        code: String,
    },
    Marks {
        chat: Ulid,
        mark: MarkItem,
    },
    Typing {
        chat: Ulid,
        account: Ulid,
        on: bool,
    },
    /// The chat a `StartChat` asked for, with its id, so the platform can open it. It may not be
    /// ready yet: `ChatReady` follows once this device is in it.
    Started {
        chat: Ulid,
        account: Ulid,
    },
    /// This device is in the chat and can send.
    ChatReady {
        chat: Ulid,
    },
    ChatLeft {
        chat: Ulid,
    },
    SearchResults {
        account_list: Vec<AccountBrief>,
    },
    DeviceList {
        reply: DevicesReply,
    },
    Terminated {
        device: Ulid,
    },
    Online,
    Offline,
    LoggedOut,
    Error {
        code: String,
        message: String,
    },
}

/// What a queued item does once it may go.
#[derive(Clone)]
enum Item {
    /// A text, an edit or a delete.
    Content {
        chat: Ulid,
        kind: Kind,
        text: Option<Secret>,
    },
    Typing {
        chat: Ulid,
        on: bool,
    },
    Commit {
        chat: Ulid,
        plan: Plan,
    },
}

/// A change to a chat's group, built only when it may go, so it is built on the newest state.
#[derive(Clone)]
enum Plan {
    /// A new group, then adding these devices. Not a commit, so it never enters the queue.
    Create {
        package_list: Vec<KeyPackageItem>,
    },
    Add {
        package_list: Vec<KeyPackageItem>,
    },
    Remove {
        device_list: Vec<Ulid>,
    },
    Rotate,
    Join {
        group_info: Vec<u8>,
    },
}

/// What a REST request was for.
enum Asked {
    Register {
        username: String,
        password: Secret,
        captcha: Option<CaptchaAnswer>,
    },
    Setup {
        password: Secret,
        captcha: Option<CaptchaAnswer>,
    },
    Captcha,
    Challenge {
        username: String,
        password: Secret,
        captcha: Option<CaptchaAnswer>,
    },
    Login {
        username: String,
        password: Secret,
        with_device: bool,
    },
    Me,
    Account {
        account: Ulid,
    },
    AccountKeys,
    /// Whether the keys the server already holds are these words' own.
    KeysCheck,
    DeviceId {
        restore: bool,
    },
    RegisterDevice {
        device: Box<Device>,
        restore: bool,
    },
    KeyPackageCount,
    KeyPackageUpload,
    CreateChat {
        account: Ulid,
    },
    StartInfo {
        chat: Ulid,
    },
    Claim {
        chat: Ulid,
        account: Ulid,
        account_list: Vec<Ulid>,
        package_list: Vec<KeyPackageItem>,
    },
    JoinInfo {
        chat: Ulid,
    },
    ChatDevices {
        chat: Ulid,
        /// How many of the chat's messages waiting for a device this list decides.
        judged: usize,
    },
    Wraps {
        chat: Ulid,
    },
    Search,
    Devices,
    Terminate {
        device: Ulid,
    },
    LogOut,
    Features,
}

/// A REST request, kept until it is answered, to go again if nothing answered.
struct Request {
    method: String,
    path: String,
    body: Option<Secret>,
    asked: Asked,
    attempt: usize,
}

#[derive(Clone, Copy)]
enum Which {
    Received,
    Seen,
}

/// What a frame's `ref` waits for.
enum Waiting {
    Content {
        chat: Ulid,
        local: u64,
    },
    Commit {
        chat: Ulid,
        local: u64,
        plan: Plan,
    },
    Sync,
    Page {
        chat: Ulid,
        purpose: Purpose,
    },
    GroupInfo {
        chat: Ulid,
        epoch: u64,
    },
    Wraps {
        chat: Ulid,
        epoch: u64,
    },
    Position {
        chat: Ulid,
        which: Which,
        up_to: u64,
    },
    Done,
}

/// Why a page was asked for.
enum Purpose {
    Open,
    Older,
    /// What was missed, forward from the last sequence id handled.
    Fill,
    /// Collecting commits missed from before the last sequence id was known, newest first.
    CatchUp {
        found: Vec<MessageFrame>,
    },
}

/// What a save is for.
enum Saving {
    State,
    Commit {
        chat: Ulid,
        local: u64,
        frame: Box<SendFrame>,
        plan: Plan,
    },
}

/// A frame sent and not yet answered.
struct Flight {
    chat: Ulid,
    frame: SendFrame,
    /// A commit's, to retry if it loses.
    plan: Option<Plan>,
    /// The chat's last sequence id handled when it went, so a fill from there finds it.
    after: u64,
}

/// A device whose stamp has been checked.
#[derive(Clone, Copy)]
struct KnownDevice {
    account: Ulid,
    public: [u8; 32],
    master: [u8; 32],
}

#[derive(Default, PartialEq, Eq)]
enum Busy {
    #[default]
    Idle,
    Filling,
    CatchingUp,
}

/// How far this device has received or seen a chat, and what the server has confirmed.
#[derive(Default)]
struct Position {
    wanted: u64,
    acked: u64,
    at: i64,
    asked: bool,
}

/// What can happen to a message waiting to be sealed.
enum Seal {
    Now,
    Wait,
    /// A rotation was just planned, so plans run again before sending.
    Planned,
    Fail(&'static str),
}

/// What applying one commit did.
enum Applied {
    Nothing,
    Changed,
    /// Commits are missing below this one: a catch-up has started, and this waits for it.
    CatchUp,
}

#[derive(Default)]
struct ChatState {
    chat_type: Option<ChatType>,
    /// The accounts in it, this one included.
    member_list: Vec<Ulid>,
    /// The last sequence id handled with nothing missing below it, once known.
    known: Option<u64>,
    /// The highest sequence id heard of.
    top: u64,
    oldest_seq: Option<u64>,
    busy: Busy,
    /// Fill, then match what was in flight before resending it.
    verify: bool,
    /// A commit lost while a fill ran: catch up once it ends.
    catch_up_next: bool,
    /// Live frames kept while busy, handled in order after.
    buffer: Vec<MessageFrame>,
    /// Messages waiting for their epoch or for the chat to be joined.
    waiting: Vec<MessageFrame>,
    /// Messages waiting for their device's key, oldest first.
    unknown: Vec<MessageFrame>,
    fetching_devices: bool,
    /// Plans waiting for the members' keys.
    plan_list: Vec<Plan>,
    /// Retried after catching up, having lost to another commit.
    retry: Option<Plan>,
    /// Starting, joining or rejoining is under way.
    joining: bool,
    /// A rotation is planned or in flight.
    rotating: bool,
    join_attempt: usize,
    join_at: Option<i64>,
    received: Position,
    seen: Position,
    want_open: bool,
    want_older: bool,
    group_info_out: Option<(u64, Vec<u8>)>,
    wraps_out: Option<(u64, Vec<WrapItem>)>,
    /// A group info or wraps frame is waiting for its answer.
    uploading: bool,
    /// A started chat's devices to add, once its first group info and wraps are up.
    start_packages: Option<Vec<KeyPackageItem>>,
}

#[derive(PartialEq, Eq)]
enum Socket {
    Down,
    Connecting,
    Up,
}

/// One device's client. See the module's documentation.
pub struct Client {
    auth: Auth,
    platform: Platform,
    token: Option<Secret>,
    account: Option<Ulid>,
    username: Option<String>,
    device: Option<Device>,
    /// The words' keys, only while setting up or restoring, so a running device cannot unwrap
    /// history or add a device.
    keys: Option<AccountKeys>,
    /// Chats still restoring with the words' keys.
    restoring: HashSet<Ulid>,
    next_id: u64,
    asked: HashMap<u64, Request>,
    /// Background requests to send again, with when.
    later: Vec<(i64, Request)>,
    saving: HashMap<u64, Saving>,
    save_failing: bool,
    save_attempt: usize,
    save_at: Option<i64>,
    socket: Socket,
    /// No reconnecting: the session is gone or the device is over its limit.
    stopped: bool,
    /// Asked to log in again, and not since logged in.
    session_lost: bool,
    /// Over the device limit, until a device is terminated.
    limited: bool,
    /// A device registration refused at the limit, to try again once one is terminated.
    register_restore: Option<bool>,
    /// Accounts whose chat is being started.
    starting: HashSet<Ulid>,
    retry: usize,
    retry_at: Option<i64>,
    woken_at: Option<i64>,
    last_heard: i64,
    /// When the unanswered `ping` went, if one did.
    ping_sent_at: Option<i64>,
    /// `ping_after`, `ping_every` and `ping_timeout`, in milliseconds.
    ping: Option<(i64, i64, i64)>,
    waiting: HashMap<Ulid, Waiting>,
    queue: Queue<Item>,
    /// What has been sent and not answered, by queue id, to match or resend after a reconnect.
    in_flight: HashMap<u64, Flight>,
    commit_busy: bool,
    /// Chats still matching what was in flight after a reconnect. Nothing new goes meanwhile.
    verifying: usize,
    chats: HashMap<Ulid, ChatState>,
    accounts: HashMap<Ulid, AccountInfo>,
    asked_accounts: HashSet<Ulid>,
    /// Every device whose stamp has been checked.
    devices: HashMap<Ulid, KnownDevice>,
    /// Ending statements waiting for their account's master key.
    revocations: Vec<(Ulid, Ulid, Vec<u8>)>,
    dirty: bool,
    now: i64,
    out: Vec<Output>,
}

/// A REST reply's wrapper.
#[derive(Deserialize)]
struct Wrapped<T> {
    data: T,
}

fn reference() -> Ulid {
    match Ulid::new() {
        Ok(id) => id,
        Err(_) => Ulid::from_bytes([0; 16]),
    }
}

/// When to try again after `attempt` earlier tries, with a little randomness so clients spread out.
fn backoff(now: i64, attempt: usize) -> i64 {
    let wait = RETRY_MS[attempt.min(RETRY_MS.len() - 1)];
    let mut random = [0u8; 1];
    let jitter = match getrandom::fill(&mut random) {
        Ok(()) => wait * i64::from(random[0]) / 1280,
        Err(_) => 0,
    };
    now + wait + jitter
}

/// Whether the client sent this by itself, so it retries rather than telling the person.
fn background(asked: &Asked) -> bool {
    !matches!(
        asked,
        Asked::Register { .. }
            | Asked::Setup { .. }
            | Asked::Captcha
            | Asked::Challenge { .. }
            | Asked::Login { .. }
            | Asked::Search
            | Asked::Devices
            | Asked::Terminate { .. }
            | Asked::LogOut
    )
}

/// A commit refused for what it is, not for the state of this device. Rejoining past it would
/// accept what it brought in.
fn refused_for_safety(error: &MlsError) -> bool {
    matches!(
        error,
        MlsError::JoinRefused { .. }
            | MlsError::BadStamp { .. }
            | MlsError::MasterKeyChanged { .. }
            | MlsError::Credential
    )
}

fn text_of(plaintext: &[u8]) -> Option<Secret> {
    #[derive(Deserialize)]
    struct Plain {
        text: String,
    }
    match serde_json::from_slice::<Plain>(plaintext) {
        Ok(plain) => Some(Secret(plain.text)),
        Err(_) => None,
    }
}

fn fixed<const N: usize>(bytes: &[u8]) -> Option<[u8; N]> {
    bytes.try_into().ok()
}

fn account_key(info: &AccountInfo) -> Option<AccountKey> {
    let (Some(master), Some(public), Some(stamp)) = (
        &info.master_public,
        &info.account_public,
        &info.account_stamp,
    ) else {
        return None;
    };
    Some(AccountKey {
        account: info.id,
        account_public: fixed(&public.0)?,
        master_public: fixed(&master.0)?,
        stamp: fixed(&stamp.0)?,
    })
}

fn shown(message: &MessageFrame, account: Option<Ulid>, mine: bool, text: Option<Secret>) -> Shown {
    Shown {
        chat: message.chat,
        seq: message.seq,
        account,
        mine,
        device: message.device,
        content_kind: message.content_kind,
        encrypted: message.encrypted == Some(true),
        sent_at: message.client_sent_at.unwrap_or(0),
        saved_at: message.server_saved_at.unwrap_or(0),
        text,
        problem: None,
    }
}

impl Client {
    pub fn new(auth: Auth, platform: Platform) -> Self {
        Self {
            auth,
            platform,
            token: None,
            account: None,
            username: None,
            device: None,
            keys: None,
            restoring: HashSet::new(),
            next_id: 0,
            asked: HashMap::new(),
            later: Vec::new(),
            saving: HashMap::new(),
            save_failing: false,
            save_attempt: 0,
            save_at: None,
            socket: Socket::Down,
            stopped: false,
            session_lost: false,
            limited: false,
            register_restore: None,
            starting: HashSet::new(),
            retry: 0,
            retry_at: None,
            woken_at: None,
            last_heard: 0,
            ping_sent_at: None,
            ping: None,
            waiting: HashMap::new(),
            queue: Queue::default(),
            in_flight: HashMap::new(),
            commit_busy: false,
            verifying: 0,
            chats: HashMap::new(),
            accounts: HashMap::new(),
            asked_accounts: HashSet::new(),
            devices: HashMap::new(),
            revocations: Vec::new(),
            dirty: false,
            now: 0,
            out: Vec::new(),
        }
    }

    /// This device, once it has one. For tests and tools that need to sign by hand.
    pub fn device(&self) -> Option<&Device> {
        self.device.as_ref()
    }

    /// Gives up this client for its device, for tests and tools that carry on by hand.
    pub fn into_device(self) -> Option<Device> {
        self.device
    }

    pub fn account(&self) -> Option<Ulid> {
        self.account
    }

    /// The one door in: what happened, and the time in Unix milliseconds. Returns what to do.
    pub fn step(&mut self, input: Input, now: i64) -> Vec<Output> {
        self.now = now;
        match input {
            Input::Command { command } => self.command(command),
            Input::Loaded {
                first,
                second,
                resume,
                token,
            } => self.loaded(first, second, resume, token),
            Input::Saved { id } => self.saved(id),
            Input::SaveFailed { id } => self.save_failed(id),
            Input::Connected => self.connected(),
            Input::Closed { code, reason } => self.closed(code, &reason),
            Input::Frame { text } => {
                self.last_heard = now;
                self.ping_sent_at = None;
                match serde_json::from_str::<ServerFrame>(&text) {
                    Ok(frame) => self.frame(frame),
                    Err(_) => self.error(
                        "bad_frame",
                        "The server sent a frame this client cannot read",
                    ),
                }
            }
            Input::Reply { id, status, body } => self.replied(id, status, &body),
            Input::Timer => self.timer(),
        }
        loop {
            self.run_plans();
            if !self.flush() {
                break;
            }
        }
        let save_due = match self.save_at {
            Some(at) => now >= at,
            None => true,
        };
        if self.dirty && !self.commit_busy && save_due {
            self.save_state();
        }
        self.schedule();
        std::mem::take(&mut self.out)
    }

    // ---------------------------------------------------------------------------------------------
    // Plumbing
    // ---------------------------------------------------------------------------------------------

    fn event(&mut self, event: Event) {
        self.out.push(Output::Event { event });
    }

    fn error(&mut self, code: &str, message: &str) {
        self.event(Event::Error {
            code: code.to_string(),
            message: message.to_string(),
        });
    }

    fn bearer(&self) -> Option<Secret> {
        match self.auth {
            Auth::Cookie => None,
            Auth::Token => self.token.clone(),
        }
    }

    fn request<B: Serialize>(
        &mut self,
        method: &str,
        path: String,
        body: Option<&B>,
        asked: Asked,
    ) {
        let mut text = None;
        if let Some(body) = body {
            match serde_json::to_string(body) {
                Ok(body) => text = Some(Secret(body)),
                Err(_) => return self.error("internal", "Could not write a request"),
            }
        }
        self.issue(Request {
            method: method.to_string(),
            path,
            body: text,
            asked,
            attempt: 0,
        });
    }

    fn issue(&mut self, request: Request) {
        let id = self.next_id;
        self.next_id += 1;
        let bearer = self.bearer();
        self.out.push(Output::Request {
            id,
            method: request.method.clone(),
            path: request.path.clone(),
            body: request.body.clone(),
            bearer,
        });
        self.asked.insert(id, request);
    }

    fn get(&mut self, path: String, asked: Asked) {
        self.request::<EmptyReply>("GET", path, None, asked);
    }

    fn send(&mut self, frame: &ClientFrame) -> bool {
        if self.socket != Socket::Up {
            return false;
        }
        match serde_json::to_string(frame) {
            Ok(text) => {
                self.out.push(Output::Send { text: Secret(text) });
                true
            }
            Err(_) => {
                self.error("internal", "Could not write a frame");
                false
            }
        }
    }

    /// Sends a frame that is answered, and remembers what for. False if the socket is down.
    fn ask(&mut self, frame: ClientFrame, reference: Ulid, waiting: Waiting) -> bool {
        let sent = self.send(&frame);
        if sent {
            self.waiting.insert(reference, waiting);
        }
        sent
    }

    fn chat(&mut self, chat: Ulid) -> &mut ChatState {
        self.chats.entry(chat).or_default()
    }

    fn joined(&self, chat: Ulid) -> bool {
        match self.device.as_ref() {
            Some(device) => device.epoch(chat).is_ok(),
            None => false,
        }
    }

    fn save_state(&mut self) {
        let Some(device) = self.device.as_mut() else {
            return;
        };
        match device.export() {
            Ok(snapshot) => {
                let id = self.next_id;
                self.next_id += 1;
                self.saving.insert(id, Saving::State);
                self.out.push(Output::Save {
                    id,
                    slot: snapshot.slot,
                    bytes: snapshot.bytes.to_vec(),
                });
                self.dirty = false;
            }
            Err(error) => self.error("save_failed", &error.to_string()),
        }
    }

    /// One `WakeAt` for the earliest of: retrying the connection, the next ping or giving up on
    /// one, a request to send again, a join to retry and a save to try again.
    fn schedule(&mut self) {
        let mut time_list = Vec::new();
        if let Some(at) = self.retry_at {
            time_list.push(at);
        }
        if self.socket == Socket::Up
            && let Some((_, every, timeout)) = self.ping
        {
            time_list.push(match self.ping_sent_at {
                Some(sent_at) => sent_at + timeout,
                None => self.last_heard + every,
            });
        }
        for (at, _) in &self.later {
            time_list.push(*at);
        }
        for state in self.chats.values() {
            if let Some(at) = state.join_at {
                time_list.push(at);
            }
        }
        if self.dirty
            && let Some(at) = self.save_at
        {
            time_list.push(at);
        }
        let Some(at) = time_list.into_iter().min() else {
            return;
        };
        if self.woken_at != Some(at) {
            self.woken_at = Some(at);
            self.out.push(Output::WakeAt { at });
        }
    }

    // ---------------------------------------------------------------------------------------------
    // Commands
    // ---------------------------------------------------------------------------------------------

    fn command(&mut self, command: Command) {
        match command {
            Command::SignUp {
                username,
                password,
                display_name,
                captcha,
            } => {
                let body = RegisterBody {
                    username: username.clone(),
                    password: password.0.clone(),
                    display_name,
                    captcha: captcha.clone(),
                };
                self.request(
                    "POST",
                    "/api/v1/register".to_string(),
                    Some(&body),
                    Asked::Register {
                        username,
                        password,
                        captcha,
                    },
                );
            }
            Command::LogIn {
                username,
                password,
                captcha,
            } => self.log_in(username, password, captcha),
            Command::Captcha => self.get("/api/v1/captcha".to_string(), Asked::Captcha),
            Command::Setup {
                token,
                password,
                captcha,
            } => {
                let body = SetupBody {
                    token: token.0.clone(),
                    password: password.0.clone(),
                };
                self.request(
                    "POST",
                    "/api/v1/setup".to_string(),
                    Some(&body),
                    Asked::Setup { password, captcha },
                );
            }
            Command::GenerateWords { count } => match seed::generate(count) {
                Ok(words) => self.event(Event::Words {
                    words: Secret(words.to_string()),
                }),
                Err(error) => self.error("bad_words", &error.to_string()),
            },
            Command::SetWords { words } => self.set_words(&words, false),
            Command::Restore { words } => self.set_words(&words, true),
            Command::ChatList => {
                let reference = reference();
                self.ask(ClientFrame::Sync { reference }, reference, Waiting::Sync);
            }
            Command::OpenChat { chat } => {
                self.open_chat(chat);
                self.devices_of(chat);
            }
            Command::LoadOlder { chat } => self.load_older(chat),
            Command::StartChat { account } => {
                if !self.starting.insert(account) {
                    return;
                }
                if !self.accounts.contains_key(&account) {
                    self.ask_account(account);
                }
                let body = CreateChatBody { account };
                self.request(
                    "POST",
                    "/api/v1/chats".to_string(),
                    Some(&body),
                    Asked::CreateChat { account },
                );
            }
            Command::Search { text } => {
                let mut encoded = String::new();
                for byte in text.bytes() {
                    if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                        encoded.push(byte as char);
                    } else {
                        encoded.push_str(&format!("%{byte:02X}"));
                    }
                }
                self.get(format!("/api/v1/accounts?search={encoded}"), Asked::Search);
            }
            Command::SendText { chat, text } => {
                self.push_content(chat, Kind::Content, None, Some(text))
            }
            Command::Edit { chat, target, text } => {
                self.push_content(chat, Kind::Edit, Some(target), Some(text));
            }
            Command::Delete { chat, target } => {
                self.push_content(chat, Kind::Delete, Some(target), None)
            }
            Command::Seen { chat, up_to_seq } => {
                let now = self.now;
                let state = self.chat(chat);
                if up_to_seq > state.seen.wanted {
                    state.seen.wanted = up_to_seq;
                    state.seen.at = now;
                }
                self.report(chat);
            }
            Command::Typing { chat, on } => {
                // Typing is stale by the time a connection comes back, so it is not kept.
                if self.socket == Socket::Up {
                    self.queue
                        .push(Level::Typing, Item::Typing { chat, on }, Target::None);
                }
            }
            Command::Devices => self.get("/api/v1/devices".to_string(), Asked::Devices),
            Command::Terminate { device, words } => {
                let Some(account) = self.account else {
                    return self.error("not_logged_in", "Log in first");
                };
                let keys = match seed::derive(&words.0) {
                    Ok(keys) => keys,
                    Err(error) => return self.error("bad_words", &error.to_string()),
                };
                let signature = sign_revocation(&keys, &account, &device);
                let body = SignedBody {
                    signature: Bytes(signature.to_vec()),
                };
                self.request(
                    "POST",
                    format!("/api/v1/devices/{device}/terminate"),
                    Some(&body),
                    Asked::Terminate { device },
                );
            }
            Command::LogOut => {
                let Some(device) = self.device.as_ref() else {
                    return self.error("no_device", "This client has no device");
                };
                let id = device.record().device;
                let signature = match device.sign_logout() {
                    Ok(signature) => signature,
                    Err(error) => return self.error("sign_failed", &error.to_string()),
                };
                let body = SignedBody {
                    signature: Bytes(signature.to_vec()),
                };
                self.request(
                    "POST",
                    format!("/api/v1/devices/{id}/logout"),
                    Some(&body),
                    Asked::LogOut,
                );
            }
            Command::NetworkBack => {
                for (_, request) in std::mem::take(&mut self.later) {
                    self.issue(request);
                }
                if self.socket == Socket::Down && !self.stopped && self.device.is_some() {
                    self.retry = 0;
                    self.connect();
                }
            }
        }
    }

    fn log_in(&mut self, username: String, password: Secret, captcha: Option<CaptchaAnswer>) {
        if self.device.is_some() {
            let body = ChallengeBody {
                username: username.clone(),
                purpose: "device_login".to_string(),
            };
            self.request(
                "POST",
                "/api/v1/challenge".to_string(),
                Some(&body),
                Asked::Challenge {
                    username,
                    password,
                    captcha,
                },
            );
            return;
        }
        self.post_login(username, password, captcha, None);
    }

    fn post_login(
        &mut self,
        username: String,
        password: Secret,
        captcha: Option<CaptchaAnswer>,
        device: Option<DeviceProof>,
    ) {
        let body = LoginBody {
            username: username.clone(),
            password: password.0.clone(),
            captcha,
            device: device.clone(),
        };
        let route = match self.auth {
            Auth::Cookie => "/api/v1/login",
            Auth::Token => "/api/v1/login/token",
        };
        self.request(
            "POST",
            route.to_string(),
            Some(&body),
            Asked::Login {
                username,
                password,
                with_device: device.is_some(),
            },
        );
    }

    fn set_words(&mut self, words: &Secret, restore: bool) {
        let Some(account) = self.account else {
            return self.error("not_logged_in", "Log in first");
        };
        let keys = match seed::derive(&words.0) {
            Ok(keys) => keys,
            Err(error) => return self.error("bad_words", &error.to_string()),
        };
        if restore {
            let stored = match self.accounts.get(&account) {
                Some(info) => info.master_public.clone(),
                None => None,
            };
            if stored != Some(Bytes(keys.master_public().to_vec())) {
                return self.error("wrong_words", "Those are not this account's words");
            }
            self.keys = Some(keys);
            self.request::<EmptyReply>(
                "POST",
                "/api/v1/devices/id".to_string(),
                None,
                Asked::DeviceId { restore: true },
            );
            return;
        }
        let body = self.keys_body(&keys, account);
        self.keys = Some(keys);
        self.request(
            "PUT",
            "/api/v1/account/keys".to_string(),
            Some(&body),
            Asked::AccountKeys,
        );
    }

    fn keys_body(&self, keys: &AccountKeys, account: Ulid) -> AccountKeysBody {
        AccountKeysBody {
            master_public: Bytes(keys.master_public().to_vec()),
            account_public: Bytes(keys.account_public().to_vec()),
            account_stamp: Bytes(stamp_account_key(keys, &account).to_vec()),
        }
    }

    fn ask_account(&mut self, account: Ulid) {
        if self.asked_accounts.insert(account) {
            self.get(
                format!("/api/v1/accounts/{account}"),
                Asked::Account { account },
            );
        }
    }

    fn page(&mut self, chat: Ulid, after: u64, before: Option<u64>, purpose: Purpose) -> bool {
        let reference = reference();
        let frame = ClientFrame::History {
            reference,
            chat,
            after_seq: after,
            before_seq: before,
            limit: PAGE,
        };
        self.ask(frame, reference, Waiting::Page { chat, purpose })
    }

    fn open_chat(&mut self, chat: Ulid) {
        if !self.page(chat, 0, Some(NEWEST), Purpose::Open) {
            self.chat(chat).want_open = true;
        }
    }

    fn load_older(&mut self, chat: Ulid) {
        let before = match self.chat(chat).oldest_seq {
            Some(oldest) => oldest,
            None => NEWEST,
        };
        if before <= 1 {
            return self.event(Event::PageLoaded {
                chat,
                has_older: false,
            });
        }
        if !self.page(chat, 0, Some(before), Purpose::Older) {
            self.chat(chat).want_older = true;
        }
    }

    /// Fetches the chat's device list, unless one is already coming. It decides every message
    /// waiting for a device so far.
    fn devices_of(&mut self, chat: Ulid) {
        let state = self.chat(chat);
        if state.fetching_devices {
            return;
        }
        state.fetching_devices = true;
        let judged = state.unknown.len();
        self.get(
            format!("/api/v1/chats/{chat}/devices"),
            Asked::ChatDevices { chat, judged },
        );
    }

    fn push_content(
        &mut self,
        chat: Ulid,
        kind: Kind,
        target: Option<Pointer>,
        text: Option<Secret>,
    ) {
        if self.device.is_none() {
            return self.error("no_device", "This client has no device");
        }
        let queue_target = match target {
            Some(Pointer::Seq(seq)) => Target::Seq(seq),
            Some(Pointer::Local(local)) => Target::Pending(local),
            None => Target::None,
        };
        let item = Item::Content {
            chat,
            kind,
            text: text.clone(),
        };
        let local = self.queue.push(Level::Content, item, queue_target);
        self.event(Event::Pending {
            chat,
            local,
            kind,
            target,
            text,
        });
    }

    // ---------------------------------------------------------------------------------------------
    // The socket
    // ---------------------------------------------------------------------------------------------

    fn connect(&mut self) {
        self.retry_at = None;
        self.socket = Socket::Connecting;
        let bearer = self.bearer();
        self.out.push(Output::Connect { bearer });
    }

    fn connected(&mut self) {
        self.socket = Socket::Up;
        self.retry = 0;
        self.retry_at = None;
        self.last_heard = self.now;
        self.ping_sent_at = None;
        self.event(Event::Online);
        let reference = reference();
        self.ask(ClientFrame::Sync { reference }, reference, Waiting::Sync);
        let body = KeyPackagesBody {
            body_list: Vec::new(),
        };
        self.request(
            "POST",
            "/api/v1/key_packages".to_string(),
            Some(&body),
            Asked::KeyPackageCount,
        );
        // What went out before the connection dropped is looked for before anything new goes.
        let mut verify_list: Vec<(Ulid, u64)> = Vec::new();
        for flight in self.in_flight.values() {
            let mut seen = false;
            for entry in &mut verify_list {
                if entry.0 == flight.chat {
                    entry.1 = entry.1.min(flight.after);
                    seen = true;
                }
            }
            if !seen {
                verify_list.push((flight.chat, flight.after));
            }
        }
        verify_list.sort();
        for (chat, after) in verify_list {
            let state = self.chat(chat);
            state.verify = true;
            if state.known.is_none() {
                state.known = Some(after);
            }
            self.verifying += 1;
            self.fill(chat);
        }
        let mut chat_list: Vec<Ulid> = self.chats.keys().copied().collect();
        chat_list.sort();
        for chat in chat_list {
            let state = self.chat(chat);
            let (open, older) = (state.want_open, state.want_older);
            state.want_open = false;
            state.want_older = false;
            if open {
                self.open_chat(chat);
            }
            if older {
                self.load_older(chat);
            }
            self.send_outs(chat);
            self.report(chat);
        }
    }

    fn closed(&mut self, code: u16, reason: &str) {
        let was_up = self.socket == Socket::Up;
        self.socket = Socket::Down;
        self.verifying = 0;
        self.ping = None;
        let mut reopen = Vec::new();
        for (_, waiting) in self.waiting.drain() {
            if let Waiting::Page { chat, purpose } = waiting {
                reopen.push((chat, purpose));
            }
        }
        for (chat, purpose) in reopen {
            match purpose {
                Purpose::Open => self.chat(chat).want_open = true,
                Purpose::Older => self.chat(chat).want_older = true,
                Purpose::Fill | Purpose::CatchUp { .. } => {}
            }
        }
        for state in self.chats.values_mut() {
            // A fill or catch-up starts again from what was handled, so what it kept goes.
            state.busy = Busy::Idle;
            state.buffer.clear();
            state.verify = false;
            state.catch_up_next = false;
            state.uploading = false;
            state.received.asked = false;
            state.seen.asked = false;
        }
        if was_up {
            self.event(Event::Offline);
        }
        if code == 4403 {
            match reason {
                "device_ended" => return self.ended(),
                "device_limit" => {
                    self.stopped = true;
                    self.limited = true;
                    return self.event(Event::DeviceLimit {
                        message: "This account has as many devices as it may".to_string(),
                    });
                }
                _ => return self.session_gone(),
            }
        }
        if self.stopped || self.device.is_none() {
            return;
        }
        self.retry_at = Some(backoff(self.now, self.retry));
        self.retry += 1;
    }

    /// The session is gone: nothing reconnects or retries until the person logs in again.
    fn session_gone(&mut self) {
        self.stopped = true;
        self.forget_token();
        self.later.clear();
        if self.socket != Socket::Down {
            self.socket = Socket::Down;
            self.out.push(Output::Disconnect);
        }
        if !self.session_lost {
            self.session_lost = true;
            self.show_login();
        }
    }

    /// This device is gone: its keys are wiped, and it can never be used again.
    fn ended(&mut self) {
        self.stopped = true;
        self.forget_token();
        self.device = None;
        self.keys = None;
        self.dirty = false;
        self.queue = Queue::default();
        self.in_flight.clear();
        self.chats.clear();
        self.later.clear();
        self.out.push(Output::Wipe);
        self.out.push(Output::Disconnect);
        self.socket = Socket::Down;
        self.event(Event::LoggedOut);
        self.public_features();
    }

    /// A token that no longer logs in, so the platform drops what it kept.
    fn forget_token(&mut self) {
        if self.token.take().is_some() {
            self.event(Event::Token { token: None });
        }
    }

    /// The account screen is up, so ask what anyone can read before an account exists.
    fn show_login(&mut self) {
        self.event(Event::NeedLogin);
        self.public_features();
    }

    fn public_features(&mut self) {
        self.get("/api/v1/public/features".to_string(), Asked::Features);
    }

    /// Sends again what is due, retries the connection when due, and otherwise, after
    /// `ping_every` with nothing heard, sends a `ping`, giving the connection up if nothing comes
    /// back within `ping_timeout`.
    fn timer(&mut self) {
        self.woken_at = None;
        let now = self.now;
        let mut due = Vec::new();
        let mut index = 0;
        while index < self.later.len() {
            if self.later[index].0 <= now {
                due.push(self.later.remove(index).1);
            } else {
                index += 1;
            }
        }
        for request in due {
            self.issue(request);
        }
        let mut join_list = Vec::new();
        for (chat, state) in &mut self.chats {
            if let Some(at) = state.join_at
                && at <= now
            {
                state.join_at = None;
                join_list.push(*chat);
            }
        }
        join_list.sort();
        for chat in join_list {
            self.get(
                format!("/api/v1/chats/{chat}/group_info"),
                Asked::JoinInfo { chat },
            );
        }
        if let Some(at) = self.retry_at
            && now >= at
        {
            return self.connect();
        }
        if self.socket != Socket::Up {
            return;
        }
        let Some((_, every, timeout)) = self.ping else {
            return;
        };
        if let Some(sent_at) = self.ping_sent_at {
            if now >= sent_at + timeout {
                self.out.push(Output::Disconnect);
                self.closed(1006, "");
            }
            return;
        }
        if now >= self.last_heard + every {
            let reference = reference();
            self.ask(ClientFrame::Ping { reference }, reference, Waiting::Done);
            self.ping_sent_at = Some(now);
        }
    }

    // ---------------------------------------------------------------------------------------------
    // Saving
    // ---------------------------------------------------------------------------------------------

    fn loaded(
        &mut self,
        first: Option<Vec<u8>>,
        second: Option<Vec<u8>>,
        resume: bool,
        token: Option<Secret>,
    ) {
        if self.auth == Auth::Token {
            self.token = token;
        }
        if first.is_none() && second.is_none() {
            return self.show_login();
        }
        match Device::import(first.as_deref(), second.as_deref()) {
            Ok(mut imported) => {
                // The queue that held any commit still waiting is gone with the last run.
                match imported.device.drop_pending_commits() {
                    Ok(dropped) => self.dirty = dropped,
                    Err(error) => self.error("load_failed", &error.to_string()),
                }
                self.account = Some(imported.device.record().account);
                self.device = Some(imported.device);
                if self.auth == Auth::Token && self.token.is_none() {
                    return self.show_login();
                }
                if resume {
                    self.connect();
                }
            }
            Err(error) => {
                self.error("load_failed", &error.to_string());
                self.show_login();
            }
        }
    }

    fn saved(&mut self, id: u64) {
        match self.saving.remove(&id) {
            Some(Saving::Commit {
                chat,
                local,
                frame,
                plan,
            }) => {
                let reference = frame.reference;
                let after = self.chat(chat).known.unwrap_or(0);
                self.in_flight.insert(
                    local,
                    Flight {
                        chat,
                        frame: (*frame).clone(),
                        plan: Some(plan.clone()),
                        after,
                    },
                );
                self.ask(
                    ClientFrame::Send(frame),
                    reference,
                    Waiting::Commit { chat, local, plan },
                );
            }
            Some(Saving::State) => {
                self.save_failing = false;
                self.save_attempt = 0;
                self.save_at = None;
            }
            None => {}
        }
    }

    fn save_failed(&mut self, id: u64) {
        match self.saving.remove(&id) {
            Some(Saving::Commit { chat, local, .. }) => {
                if let Some(device) = self.device.as_mut() {
                    let _ = device.commit_rejected(chat);
                }
                self.commit_busy = false;
                self.dirty = true;
                self.commit_failed(chat, local, "save_failed");
                self.error(
                    "save_failed",
                    "Could not save this device's state, so a key change was not sent",
                );
            }
            Some(Saving::State) => {
                self.dirty = true;
                self.save_at = Some(backoff(self.now, self.save_attempt));
                self.save_attempt += 1;
                if !self.save_failing {
                    self.save_failing = true;
                    self.error("save_failed", "Could not save this device's state");
                }
            }
            None => {}
        }
    }

    // ---------------------------------------------------------------------------------------------
    // Sending
    // ---------------------------------------------------------------------------------------------

    /// Hands the plans whose members' keys are known to the queue, and asks for those missing.
    fn run_plans(&mut self) {
        let mut ready = Vec::new();
        let mut missing = Vec::new();
        for (chat, state) in &mut self.chats {
            if state.plan_list.is_empty() {
                continue;
            }
            let mut all_known = true;
            for account in &state.member_list {
                let known = match self.accounts.get(account) {
                    Some(info) => info.account_public.is_some(),
                    None => false,
                };
                if !known {
                    all_known = false;
                    missing.push(*account);
                }
            }
            if all_known {
                for plan in state.plan_list.drain(..) {
                    ready.push((*chat, plan));
                }
            }
        }
        for account in missing {
            self.ask_account(account);
        }
        for (chat, plan) in ready {
            match plan {
                Plan::Create { package_list } => self.create(chat, package_list),
                plan => {
                    self.queue
                        .push(Level::Keys, Item::Commit { chat, plan }, Target::None);
                }
            }
        }
    }

    fn account_list(&self, chat: Ulid) -> Vec<AccountKey> {
        let mut list = Vec::new();
        if let Some(state) = self.chats.get(&chat) {
            for account in &state.member_list {
                if let Some(info) = self.accounts.get(account)
                    && let Some(key) = account_key(info)
                {
                    list.push(key);
                }
            }
        }
        list
    }

    /// Sends what may go. True when a rotation was just planned, so plans run again first.
    fn flush(&mut self) -> bool {
        while self.socket == Socket::Up && self.verifying == 0 {
            let Some(outgoing) = self.queue.take_next(self.commit_busy) else {
                return false;
            };
            match outgoing.item {
                Item::Typing { chat, on } => {
                    self.send(&ClientFrame::Typing { chat, on });
                }
                Item::Content { chat, kind, text } => match self.can_seal(chat) {
                    Seal::Now => {
                        self.send_content(outgoing.id, chat, kind, outgoing.pointer_seq, text);
                    }
                    Seal::Wait => {
                        self.queue.put_back(outgoing.id);
                        return false;
                    }
                    Seal::Planned => {
                        self.queue.put_back(outgoing.id);
                        return true;
                    }
                    Seal::Fail(code) => self.fail(chat, outgoing.id, code),
                },
                Item::Commit { chat, plan } => self.send_commit(outgoing.id, chat, plan),
            }
        }
        false
    }

    /// Whether a message for this chat can be sealed now. It waits while the chat is being joined
    /// or its key is about to change, and a key over 7 days old is changed first.
    fn can_seal(&mut self, chat: Ulid) -> Seal {
        let now = self.now;
        let (joined, due) = match self.device.as_ref() {
            Some(device) => (
                device.epoch(chat).is_ok(),
                matches!(device.rotation_due(chat, now), Ok(true)),
            ),
            None => return Seal::Fail("no_device"),
        };
        let state = self.chat(chat);
        if state.joining || state.rotating || !state.plan_list.is_empty() {
            return Seal::Wait;
        }
        if !joined {
            return Seal::Fail("not_joined");
        }
        if due {
            state.rotating = true;
            state.plan_list.push(Plan::Rotate);
            return Seal::Planned;
        }
        Seal::Now
    }

    fn send_content(
        &mut self,
        local: u64,
        chat: Ulid,
        kind: Kind,
        pointer_seq: Option<u64>,
        text: Option<Secret>,
    ) {
        let after = self.chat(chat).known.unwrap_or(0);
        let Some(device) = self.device.as_ref() else {
            return;
        };
        let epoch = match device.epoch(chat) {
            Ok(epoch) => epoch,
            Err(_) => return self.fail(chat, local, "not_joined"),
        };
        let content_kind = if kind == Kind::Content {
            Some(ContentKind::Text)
        } else {
            None
        };
        let header = Header {
            chat,
            device: device.record().device,
            kind,
            content_kind,
            encrypted: true,
            epoch: Some(epoch),
            pointer_seq,
            call_seq: None,
            client_sent_at: self.now,
        };
        let sealed = match text {
            Some(text) => {
                let plaintext = match serde_json::to_vec(&serde_json::json!({ "text": text.0 })) {
                    Ok(plaintext) => zeroize::Zeroizing::new(plaintext),
                    Err(_) => return self.fail(chat, local, "internal"),
                };
                device.seal(&header, &plaintext)
            }
            None => match device.sign(&header, b"") {
                Ok(signature) => Ok((Vec::new(), signature)),
                Err(error) => Err(error),
            },
        };
        let (body, signature) = match sealed {
            Ok(sealed) => sealed,
            Err(_) => return self.fail(chat, local, "seal_failed"),
        };
        let reference = reference();
        let frame = SendFrame {
            reference,
            chat,
            kind,
            content_kind,
            encrypted: true,
            epoch: Some(epoch),
            pointer_seq,
            call_seq: None,
            client_sent_at: self.now,
            body: Bytes(body),
            signature: Bytes(signature.to_vec()),
            wrap_list: Vec::new(),
            welcome_list: Vec::new(),
            group_info: None,
            persist: Vec::new(),
            remove: Vec::new(),
        };
        self.in_flight.insert(
            local,
            Flight {
                chat,
                frame: frame.clone(),
                plan: None,
                after,
            },
        );
        self.ask(
            ClientFrame::Send(Box::new(frame)),
            reference,
            Waiting::Content { chat, local },
        );
    }

    /// Gives up on a message and everything pointing at it.
    fn fail(&mut self, chat: Ulid, local: u64, code: &str) {
        for dropped in self.queue.drop_with_dependents(local) {
            self.in_flight.remove(&dropped);
            self.event(Event::Failed {
                chat,
                local: dropped,
                code: code.to_string(),
            });
        }
    }

    /// The chat's join or key change finally failed, so what waited for it fails too.
    fn fail_chat(&mut self, chat: Ulid, code: &str) {
        let mut id_list = Vec::new();
        for (id, item) in self.queue.queued_list() {
            if let Item::Content { chat: of, .. } = item
                && *of == chat
            {
                id_list.push(id);
            }
        }
        let mut gone = HashSet::new();
        for id in id_list {
            if gone.contains(&id) {
                continue;
            }
            for dropped in self.queue.drop_with_dependents(id) {
                gone.insert(dropped);
                self.event(Event::Failed {
                    chat,
                    local: dropped,
                    code: code.to_string(),
                });
            }
        }
    }

    /// A commit will never be accepted: what it was for stops, and what waited on it fails.
    fn commit_failed(&mut self, chat: Ulid, local: u64, code: &str) {
        self.in_flight.remove(&local);
        self.queue.drop_with_dependents(local);
        let state = self.chat(chat);
        state.rotating = false;
        state.joining = false;
        state.join_at = None;
        self.restore_done(chat);
        self.fail_chat(chat, code);
        if !self.joined(chat) {
            self.release(chat);
        }
    }

    fn send_commit(&mut self, local: u64, chat: Ulid, plan: Plan) {
        let account_list = self.account_list(chat);
        let now = self.now;
        let Some(device) = self.device.as_mut() else {
            return;
        };
        let mut member_set = HashSet::new();
        if let Ok(member_list) = device.member_list(chat) {
            for member in member_list {
                member_set.insert(member.device);
            }
        }
        let plan = match plan {
            // A Welcome brought this device in meanwhile.
            Plan::Join { .. } if device.epoch(chat).is_ok() => {
                self.queue.confirm(local, 0);
                return;
            }
            // A commit that won meanwhile may have added some of these devices already.
            Plan::Add { package_list } => {
                let mut adding = Vec::new();
                for package in package_list {
                    if !member_set.contains(&package.device) {
                        adding.push(package);
                    }
                }
                if adding.is_empty() {
                    self.queue.confirm(local, 0);
                    return self.ready(chat);
                }
                Plan::Add {
                    package_list: adding,
                }
            }
            plan => plan,
        };
        let mut welcome_list = Vec::new();
        let built = match &plan {
            Plan::Add { package_list } => {
                let mut bodies = Vec::new();
                for package in package_list {
                    bodies.push(package.body.0.clone());
                }
                device.add_devices(chat, &bodies, &account_list)
            }
            Plan::Remove { device_list } => {
                let mut still_in = Vec::new();
                for ended in device_list {
                    if member_set.contains(ended) {
                        still_in.push(*ended);
                    }
                }
                if still_in.is_empty() {
                    self.queue.confirm(local, 0);
                    return;
                }
                device.remove_devices(chat, &still_in, &account_list)
            }
            Plan::Rotate => device.rotate(chat, &account_list),
            Plan::Join { group_info } => device.join_external(chat, group_info, now, &account_list),
            Plan::Create { .. } => return,
        };
        let commit = match built {
            Ok(commit) => commit,
            Err(error) => {
                self.commit_failed(chat, local, "commit_failed");
                return self.error("commit_failed", &error.to_string());
            }
        };
        if let (Plan::Add { package_list }, Some(welcome)) = (&plan, &commit.welcome) {
            for package in package_list {
                welcome_list.push(WelcomeItem {
                    device: package.device,
                    body: Bytes(welcome.clone()),
                });
            }
        }
        let header = Header {
            chat,
            device: device.record().device,
            kind: Kind::Commit,
            content_kind: None,
            encrypted: false,
            epoch: Some(commit.epoch),
            pointer_seq: None,
            call_seq: None,
            client_sent_at: now,
        };
        let signature = match device.sign(&header, &commit.commit) {
            Ok(signature) => signature,
            Err(error) => {
                let _ = device.commit_rejected(chat);
                self.commit_failed(chat, local, "sign_failed");
                return self.error("sign_failed", &error.to_string());
            }
        };
        let mut wrap_list = Vec::new();
        for wrap in &commit.wrap_list {
            wrap_list.push(WrapItem {
                account: wrap.account,
                wrapped: Bytes(wrap.wrapped.to_vec()),
            });
        }
        let frame = SendFrame {
            reference: reference(),
            chat,
            kind: Kind::Commit,
            content_kind: None,
            encrypted: false,
            epoch: Some(commit.epoch),
            pointer_seq: None,
            call_seq: None,
            client_sent_at: now,
            body: Bytes(commit.commit),
            signature: Bytes(signature.to_vec()),
            wrap_list,
            welcome_list,
            group_info: Some(Bytes(commit.group_info)),
            persist: Vec::new(),
            remove: Vec::new(),
        };
        let snapshot = match device.export() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                let _ = device.commit_rejected(chat);
                self.commit_failed(chat, local, "save_failed");
                return self.error("save_failed", &error.to_string());
            }
        };
        self.commit_busy = true;
        self.dirty = false;
        let id = self.next_id;
        self.next_id += 1;
        self.saving.insert(
            id,
            Saving::Commit {
                chat,
                local,
                frame: Box::new(frame),
                plan,
            },
        );
        self.out.push(Output::Save {
            id,
            slot: snapshot.slot,
            bytes: snapshot.bytes.to_vec(),
        });
    }

    /// Starts a chat's group: epoch 0's group info and wraps go up, then the claimed devices are
    /// added.
    fn create(&mut self, chat: Ulid, package_list: Vec<KeyPackageItem>) {
        let account_list = self.account_list(chat);
        let now = self.now;
        let Some(device) = self.device.as_mut() else {
            return;
        };
        let created = match device.create_chat(chat, now, &account_list) {
            Ok(created) => created,
            Err(error) => {
                let state = self.chat(chat);
                state.joining = false;
                self.fail_chat(chat, "start_failed");
                return self.error("start_failed", &error.to_string());
            }
        };
        self.dirty = true;
        let mut wrap_list = Vec::new();
        for wrap in &created.wrap_list {
            wrap_list.push(WrapItem {
                account: wrap.account,
                wrapped: Bytes(wrap.wrapped.to_vec()),
            });
        }
        let state = self.chat(chat);
        state.known = Some(state.known.unwrap_or(0));
        state.group_info_out = Some((0, created.group_info));
        state.wraps_out = Some((0, wrap_list));
        state.start_packages = Some(package_list);
        self.send_outs(chat);
    }

    /// Sends the chat's group info, then its wraps, each kept until the server answers. A started
    /// chat adds its devices once both are up.
    fn send_outs(&mut self, chat: Ulid) {
        if self.socket != Socket::Up {
            return;
        }
        let state = self.chat(chat);
        if state.uploading {
            return;
        }
        if let Some((epoch, body)) = state.group_info_out.clone() {
            state.uploading = true;
            let reference = reference();
            let frame = ClientFrame::GroupInfo {
                reference,
                chat,
                epoch,
                body: Bytes(body),
            };
            self.ask(frame, reference, Waiting::GroupInfo { chat, epoch });
            return;
        }
        if let Some((epoch, wrap_list)) = state.wraps_out.clone() {
            state.uploading = true;
            let reference = reference();
            let frame = ClientFrame::Wraps {
                reference,
                chat,
                epoch,
                wrap_list,
            };
            self.ask(frame, reference, Waiting::Wraps { chat, epoch });
            return;
        }
        if let Some(package_list) = state.start_packages.take() {
            if package_list.is_empty() {
                self.ready(chat);
            } else {
                state.plan_list.push(Plan::Add { package_list });
            }
        }
    }

    fn ready(&mut self, chat: Ulid) {
        let state = self.chat(chat);
        state.joining = false;
        state.join_attempt = 0;
        state.join_at = None;
        self.event(Event::ChatReady { chat });
    }

    /// Drops this chat's group, keeping its keys, and joins it again by external commit.
    fn rejoin(&mut self, chat: Ulid) {
        if let Some(device) = self.device.as_mut()
            && device.epoch(chat).is_ok()
            && let Err(error) = device.forget_chat(chat)
        {
            return self.error("rejoin_failed", &error.to_string());
        }
        self.dirty = true;
        let state = self.chat(chat);
        state.start_packages = None;
        if state.joining {
            return;
        }
        state.joining = true;
        self.get(
            format!("/api/v1/chats/{chat}/group_info"),
            Asked::JoinInfo { chat },
        );
    }

    // ---------------------------------------------------------------------------------------------
    // Frames
    // ---------------------------------------------------------------------------------------------

    fn frame(&mut self, frame: ServerFrame) {
        match frame {
            ServerFrame::Features {
                ping_after,
                ping_every,
                ping_timeout,
            } => {
                let seconds =
                    |value: u64| i64::try_from(value.saturating_mul(1000)).unwrap_or(i64::MAX);
                self.ping = Some((
                    seconds(ping_after),
                    seconds(ping_every),
                    seconds(ping_timeout),
                ));
            }
            ServerFrame::Sent {
                reference,
                seq,
                server_saved_at,
            } => {
                if let Some(waiting) = self.waiting.remove(&reference) {
                    self.stored(waiting, seq, server_saved_at);
                }
            }
            ServerFrame::Done { reference } => {
                if let Some(waiting) = self.waiting.remove(&reference) {
                    self.done(waiting);
                }
            }
            ServerFrame::Refused {
                reference,
                code,
                message,
            } => {
                if let Some(waiting) = self.waiting.remove(&reference) {
                    self.refused(waiting, &code, &message);
                }
            }
            ServerFrame::Message(message) => self.live(*message),
            ServerFrame::Chats {
                reference,
                chat_list,
            } => {
                self.waiting.remove(&reference);
                self.chats_listed(chat_list);
            }
            ServerFrame::Page {
                reference,
                message_list,
            } => {
                if let Some(Waiting::Page { chat, purpose }) = self.waiting.remove(&reference) {
                    self.paged(chat, purpose, message_list);
                }
            }
            ServerFrame::Welcome { chat, body } => self.welcome(chat, &body.0),
            ServerFrame::Typing { chat, account, on } => {
                self.event(Event::Typing { chat, account, on })
            }
            ServerFrame::Marks { chat, mark } => self.event(Event::Marks { chat, mark }),
            ServerFrame::Revoked {
                account,
                device,
                statement,
            } => self.revoked(account, device, &statement.0),
        }
    }

    fn welcome(&mut self, chat: Ulid, body: &[u8]) {
        let now = self.now;
        let Some(device) = self.device.as_mut() else {
            return;
        };
        if device.epoch(chat).is_ok() {
            return;
        }
        match device.join_welcome(chat, body, now) {
            Ok(()) => {
                self.dirty = true;
                self.ready(chat);
                self.devices_of(chat);
                self.restore_chat(chat);
                if !self.restoring.contains(&chat) {
                    let waiting = std::mem::take(&mut self.chat(chat).waiting);
                    self.open_list(chat, waiting, true, true);
                }
            }
            Err(error) if refused_for_safety(&error) => {
                self.error("bad_welcome", &error.to_string())
            }
            Err(_) => self.rejoin(chat),
        }
    }

    /// What was sent got its sequence id, from the server's answer or found in a page.
    fn stored(&mut self, waiting: Waiting, seq: u64, saved_at: i64) {
        match waiting {
            Waiting::Content { chat, local } => {
                self.in_flight.remove(&local);
                self.queue.confirm(local, seq);
                self.advance(chat, seq);
                self.event(Event::Sent {
                    chat,
                    local,
                    seq,
                    saved_at,
                });
            }
            Waiting::Commit { chat, local, plan } => self.commit_stored(chat, local, plan, seq),
            _ => {}
        }
    }

    /// This client's own message took `seq`: it is handled, or shows that something was missed.
    fn advance(&mut self, chat: Ulid, seq: u64) {
        let state = self.chat(chat);
        state.top = state.top.max(seq);
        match state.known {
            None => state.known = Some(seq),
            Some(known) if seq == known + 1 => state.known = Some(seq),
            Some(known) if seq > known + 1 => self.fill(chat),
            Some(_) => {}
        }
    }

    fn commit_stored(&mut self, chat: Ulid, local: u64, plan: Plan, seq: u64) {
        self.in_flight.remove(&local);
        self.queue.confirm(local, seq);
        self.commit_busy = false;
        self.advance(chat, seq);
        let now = self.now;
        let Some(device) = self.device.as_mut() else {
            return;
        };
        // The server stored the group info with the commit, so nothing more goes up for it.
        let accepted = device.commit_accepted(chat, now);
        self.dirty = true;
        if accepted.is_err() {
            return self.rejoin(chat);
        }
        match plan {
            Plan::Join { .. } => {
                self.ready(chat);
                self.devices_of(chat);
                self.restore_chat(chat);
                if !self.restoring.contains(&chat) {
                    let waiting = std::mem::take(&mut self.chat(chat).waiting);
                    self.open_list(chat, waiting, true, true);
                }
            }
            Plan::Add { .. } => self.ready(chat),
            Plan::Rotate => self.chat(chat).rotating = false,
            Plan::Remove { .. } | Plan::Create { .. } => {}
        }
    }

    fn done(&mut self, waiting: Waiting) {
        match waiting {
            Waiting::GroupInfo { chat, epoch } => {
                let state = self.chat(chat);
                state.uploading = false;
                if matches!(state.group_info_out, Some((out, _)) if out == epoch) {
                    state.group_info_out = None;
                }
                self.send_outs(chat);
            }
            Waiting::Wraps { chat, epoch } => {
                let state = self.chat(chat);
                state.uploading = false;
                if matches!(state.wraps_out, Some((out, _)) if out == epoch) {
                    state.wraps_out = None;
                }
                self.send_outs(chat);
            }
            Waiting::Position { chat, which, up_to } => self.position_done(chat, which, up_to),
            _ => {}
        }
    }

    fn position_done(&mut self, chat: Ulid, which: Which, up_to: u64) {
        let state = self.chat(chat);
        let position = match which {
            Which::Received => &mut state.received,
            Which::Seen => &mut state.seen,
        };
        position.asked = false;
        position.acked = position.acked.max(up_to);
        self.report(chat);
    }

    /// Sends this chat's received and seen positions, if higher than the server has confirmed.
    fn report(&mut self, chat: Ulid) {
        if self.socket != Socket::Up {
            return;
        }
        for which in [Which::Received, Which::Seen] {
            let state = self.chat(chat);
            let position = match which {
                Which::Received => &mut state.received,
                Which::Seen => &mut state.seen,
            };
            if position.asked || position.wanted <= position.acked {
                continue;
            }
            position.asked = true;
            let (up_to, at) = (position.wanted, position.at);
            let reference = reference();
            let frame = match which {
                Which::Received => ClientFrame::Received {
                    reference,
                    chat,
                    up_to_seq: up_to,
                    client_received_at: at,
                },
                Which::Seen => ClientFrame::Seen {
                    reference,
                    chat,
                    up_to_seq: up_to,
                    client_seen_at: at,
                },
            };
            self.ask(frame, reference, Waiting::Position { chat, which, up_to });
        }
    }

    fn refused(&mut self, waiting: Waiting, code: &str, message: &str) {
        match waiting {
            Waiting::Content { chat, local } => self.fail(chat, local, code),
            Waiting::Commit { chat, local, plan } => {
                self.commit_busy = false;
                if let Some(device) = self.device.as_mut() {
                    let _ = device.commit_rejected(chat);
                }
                self.dirty = true;
                if code != "commit_lost" {
                    self.commit_failed(chat, local, code);
                    return self.error(code, message);
                }
                self.in_flight.remove(&local);
                self.queue.drop_with_dependents(local);
                match plan {
                    Plan::Join { .. } => {
                        let now = self.now;
                        let state = self.chat(chat);
                        state.join_at = Some(backoff(now, state.join_attempt));
                        state.join_attempt += 1;
                    }
                    plan => {
                        self.chat(chat).retry = Some(plan);
                        self.catch_up(chat);
                    }
                }
            }
            Waiting::Page {
                chat,
                purpose: Purpose::Fill,
            } => {
                let state = self.chat(chat);
                state.busy = Busy::Idle;
                state.top = state.known.unwrap_or(0);
                self.error(code, message);
                self.settle(chat);
            }
            Waiting::Page {
                chat,
                purpose: Purpose::CatchUp { .. },
            } => {
                let state = self.chat(chat);
                state.busy = Busy::Idle;
                state.retry = None;
                state.rotating = false;
                self.fail_chat(chat, code);
                self.error(code, message);
                self.settle(chat);
            }
            Waiting::GroupInfo { chat, epoch } => {
                let state = self.chat(chat);
                state.uploading = false;
                if matches!(state.group_info_out, Some((out, _)) if out == epoch) {
                    state.group_info_out = None;
                }
                match code {
                    // Someone else started this chat first: join theirs instead.
                    "chat_started" => {
                        let state = self.chat(chat);
                        state.wraps_out = None;
                        state.start_packages = None;
                        state.joining = false;
                        self.rejoin(chat);
                    }
                    _ => {
                        self.upload_failed(chat);
                        self.error(code, message);
                    }
                }
            }
            Waiting::Wraps { chat, epoch } => {
                let state = self.chat(chat);
                state.uploading = false;
                if matches!(state.wraps_out, Some((out, _)) if out == epoch) {
                    state.wraps_out = None;
                }
                self.error(code, message);
                // A start's first key opens nothing yet: adding its devices commits a new one,
                // wrapped again. Failing here would leave the server a group with nobody to add.
                if matches!(&self.chat(chat).start_packages, Some(list) if !list.is_empty()) {
                    return self.send_outs(chat);
                }
                self.upload_failed(chat);
            }
            Waiting::Position { chat, which, up_to } => {
                self.error(code, message);
                self.position_done(chat, which, up_to);
            }
            Waiting::Page { .. } | Waiting::Sync | Waiting::Done => self.error(code, message),
        }
    }

    /// A start's group info or wraps were refused, so the start has failed.
    fn upload_failed(&mut self, chat: Ulid) {
        let state = self.chat(chat);
        if state.start_packages.take().is_some() {
            state.joining = false;
            state.wraps_out = None;
            state.group_info_out = None;
            // The group never got going, so starting again begins from the server's state.
            if let Some(device) = self.device.as_mut()
                && device.forget_chat(chat).is_ok()
            {
                self.dirty = true;
            }
            self.fail_chat(chat, "start_failed");
        } else {
            self.send_outs(chat);
        }
    }

    /// Resends this chat's unanswered frames, in the order they were first sent.
    fn resend(&mut self, chat: Ulid) {
        let mut list = Vec::new();
        for (local, flight) in &self.in_flight {
            if flight.chat == chat {
                list.push((*local, flight.frame.clone(), flight.plan.clone()));
            }
        }
        list.sort_by_key(|(local, _, _)| *local);
        for (local, frame, plan) in list {
            let reference = frame.reference;
            let waiting = match plan {
                Some(plan) => Waiting::Commit { chat, local, plan },
                None => Waiting::Content { chat, local },
            };
            self.ask(ClientFrame::Send(Box::new(frame)), reference, waiting);
        }
    }

    fn chats_listed(&mut self, chat_list: Vec<ChatItem>) {
        let Some(me) = self.account else {
            return;
        };
        let mut shown = Vec::with_capacity(chat_list.len());
        let mut preview_list = Vec::new();
        let mut join_list = Vec::new();
        let mut todo = Vec::new();
        for mut item in chat_list {
            let own_epoch = match self.device.as_ref() {
                Some(device) => device.epoch(item.chat).ok(),
                None => None,
            };
            let state = self.chats.entry(item.chat).or_default();
            state.chat_type = Some(item.chat_type);
            let last_seq = item.last_seq.unwrap_or(0);
            state.top = state.top.max(last_seq);
            if state.known.is_none() {
                // A reload has no handled id. Updating starts at this member's seen position when
                // the list carries one, so what they have not seen is fetched and opened. Without
                // it, handling starts at the latest.
                state.known = Some(item.seen_seq.unwrap_or(last_seq));
            }
            let mut member_list = vec![me];
            for mark in &item.mark_list {
                if !member_list.contains(&mark.account) {
                    member_list.push(mark.account);
                }
            }
            if state.member_list.is_empty() || member_list.len() > 1 {
                state.member_list = member_list.clone();
            }
            if let Some(message) = item.last_content.take() {
                preview_list.push(*message);
            }
            match own_epoch {
                Some(own) => {
                    let behind = matches!(item.epoch, Some(epoch) if epoch > own);
                    todo.push((item.chat, behind));
                }
                None if !state.joining => join_list.push(item.chat),
                None => {}
            }
            for account in member_list {
                if !self.accounts.contains_key(&account) {
                    self.asked_accounts.remove(&account);
                    self.ask_account(account);
                }
            }
            shown.push(item);
        }
        self.event(Event::Chats { chat_list: shown });
        if self.keys.is_some() {
            for chat in &join_list {
                self.restoring.insert(*chat);
            }
            if self.restoring.is_empty() {
                self.keys = None;
            }
        }
        for chat in join_list {
            self.chat(chat).joining = true;
            self.get(
                format!("/api/v1/chats/{chat}/group_info"),
                Asked::JoinInfo { chat },
            );
        }
        for message in preview_list {
            let chat = message.chat;
            self.open_list(chat, vec![message], true, true);
        }
        for (chat, behind) in todo {
            let state = self.chat(chat);
            if state.busy != Busy::Idle {
                continue;
            }
            if state.known.unwrap_or(0) < state.top {
                self.fill(chat);
            } else if behind {
                self.catch_up(chat);
            } else {
                self.settle(chat);
            }
        }
    }

    /// Pages forward from the last sequence id handled.
    fn fill(&mut self, chat: Ulid) {
        let state = self.chat(chat);
        if state.busy != Busy::Idle {
            return;
        }
        let Some(after) = state.known else {
            return;
        };
        if self.page(chat, after, None, Purpose::Fill) {
            self.chat(chat).busy = Busy::Filling;
        }
    }

    /// Scans back from the newest message for the commits this device missed.
    fn catch_up(&mut self, chat: Ulid) {
        let state = self.chat(chat);
        match state.busy {
            Busy::CatchingUp => return,
            Busy::Filling => {
                state.catch_up_next = true;
                return;
            }
            Busy::Idle => {}
        }
        if self.page(
            chat,
            0,
            Some(NEWEST),
            Purpose::CatchUp { found: Vec::new() },
        ) {
            self.chat(chat).busy = Busy::CatchingUp;
        }
    }

    /// The chat has caught up with what it knew of: live frames kept meanwhile are handled, a
    /// commit that lost goes again, and what was in flight is resent if it was not found.
    fn settle(&mut self, chat: Ulid) {
        let mut buffer = std::mem::take(&mut self.chat(chat).buffer);
        buffer.sort_by_key(|message| message.seq);
        for message in buffer {
            self.live(message);
        }
        if self.chat(chat).busy != Busy::Idle {
            return;
        }
        if let Some(plan) = self.chat(chat).retry.take() {
            let due = match self.device.as_ref() {
                Some(device) => matches!(device.rotation_due(chat, self.now), Ok(true)),
                None => false,
            };
            match plan {
                // Someone else rotated meanwhile.
                Plan::Rotate if !due => self.chat(chat).rotating = false,
                plan => self.chat(chat).plan_list.push(plan),
            }
        }
        let state = self.chat(chat);
        if state.verify {
            state.verify = false;
            self.verifying = self.verifying.saturating_sub(1);
            self.resend(chat);
        }
        let state = self.chat(chat);
        if state.known.unwrap_or(0) < state.top {
            self.fill(chat);
        }
    }

    /// A message pushed live. Handled at once if it is the next one, and otherwise kept while what
    /// is missing below it is fetched.
    fn live(&mut self, message: MessageFrame) {
        let chat = message.chat;
        let state = self.chat(chat);
        state.top = state.top.max(message.seq);
        if state.busy != Busy::Idle {
            state.buffer.push(message);
            return;
        }
        let known = match state.known {
            Some(known) => known,
            None => message.seq.saturating_sub(1),
        };
        state.known = Some(known);
        if message.seq <= known {
            return;
        }
        if message.seq > known + 1 {
            state.buffer.push(message);
            return self.fill(chat);
        }
        self.handle(chat, vec![message]);
    }

    /// Handles messages in order from the next one: commits applied, what was in flight matched,
    /// and the rest opened.
    fn handle(&mut self, chat: Ulid, message_list: Vec<MessageFrame>) {
        let mut opened = Vec::new();
        let mut changed = false;
        let mut rest = message_list.into_iter();
        while let Some(message) = rest.next() {
            if let Some(known) = self.chat(chat).known
                && message.seq <= known
            {
                continue;
            }
            if self.match_flight(chat, &message) {
                self.chat(chat).known = Some(message.seq);
                continue;
            }
            if message.kind == Some(Kind::Commit) {
                match self.apply_commit(chat, &message, false) {
                    Applied::Nothing => {}
                    Applied::Changed => changed = true,
                    Applied::CatchUp => {
                        let state = self.chat(chat);
                        state.buffer.push(message);
                        state.buffer.extend(rest);
                        break;
                    }
                }
            } else {
                opened.push(message.clone());
            }
            self.chat(chat).known = Some(message.seq);
        }
        self.open_list(chat, opened, true, true);
        if changed {
            self.after_commits(chat);
        }
    }

    /// Whether this is one of this device's own unanswered messages, now found stored.
    fn match_flight(&mut self, chat: Ulid, message: &MessageFrame) -> bool {
        let mine = match self.device.as_ref() {
            Some(device) => device.record().device,
            None => return false,
        };
        if message.device != Some(mine) {
            return false;
        }
        let mut found = None;
        for (local, flight) in &self.in_flight {
            if flight.chat == chat && message.signature.as_ref() == Some(&flight.frame.signature) {
                found = Some((*local, flight.frame.reference, flight.plan.clone()));
            }
        }
        let Some((local, reference, plan)) = found else {
            return false;
        };
        self.waiting.remove(&reference);
        let waiting = match plan {
            Some(plan) => Waiting::Commit { chat, local, plan },
            None => Waiting::Content { chat, local },
        };
        self.stored(waiting, message.seq, message.server_saved_at.unwrap_or(0));
        true
    }

    /// Applies one commit in order. One this device cannot process makes it rejoin, unless the
    /// commit is refused for what it brings in.
    fn apply_commit(&mut self, chat: Ulid, message: &MessageFrame, catching_up: bool) -> Applied {
        let now = self.now;
        let Some(device) = self.device.as_mut() else {
            return Applied::Nothing;
        };
        // Not in the group: joining takes the newest state anyway.
        let Ok(current) = device.epoch(chat) else {
            return Applied::Nothing;
        };
        let Some(epoch) = message.epoch else {
            return Applied::Nothing;
        };
        if epoch <= current {
            return Applied::Nothing;
        }
        if epoch > current + 1 {
            if catching_up {
                self.rejoin(chat);
                return Applied::Nothing;
            }
            self.catch_up(chat);
            return Applied::CatchUp;
        }
        let mine = message.device == Some(device.record().device);
        let body = match &message.body {
            Some(body) if !mine => body.0.clone(),
            // This device's own commit, from a state it no longer has.
            _ => {
                self.rejoin(chat);
                return Applied::Nothing;
            }
        };
        match device.process_commit(chat, &body, now) {
            Ok(processed) => {
                self.dirty = true;
                if processed.removed_self {
                    self.event(Event::ChatLeft { chat });
                    return Applied::Nothing;
                }
                Applied::Changed
            }
            Err(error) if refused_for_safety(&error) => {
                self.error("bad_commit", &error.to_string());
                Applied::Nothing
            }
            Err(_) => {
                self.rejoin(chat);
                Applied::Nothing
            }
        }
    }

    /// After applying commits: check the new key's wraps, refresh the devices, and open what
    /// waited for the key.
    fn after_commits(&mut self, chat: Ulid) {
        self.get(format!("/api/v1/chats/{chat}/wraps"), Asked::Wraps { chat });
        self.devices_of(chat);
        self.release(chat);
    }

    /// Shows what waited for this chat's keys as it is, since nothing more is coming for it.
    fn release(&mut self, chat: Ulid) {
        let waiting = std::mem::take(&mut self.chat(chat).waiting);
        self.open_list(chat, waiting, false, false);
    }

    fn paged(&mut self, chat: Ulid, purpose: Purpose, message_list: Vec<MessageFrame>) {
        match purpose {
            Purpose::Open | Purpose::Older => {
                let state = self.chat(chat);
                if let Some(first) = message_list.first() {
                    state.oldest_seq = Some(match state.oldest_seq {
                        Some(oldest) => oldest.min(first.seq),
                        None => first.seq,
                    });
                }
                let has_older = match message_list.first() {
                    Some(first) => first.seq > 1,
                    None => false,
                };
                self.open_list(chat, message_list, true, true);
                self.event(Event::PageLoaded { chat, has_older });
            }
            Purpose::Fill => {
                let count = message_list.len();
                let state = self.chat(chat);
                state.busy = Busy::Idle;
                for message in &message_list {
                    state.top = state.top.max(message.seq);
                }
                self.handle(chat, message_list);
                let state = self.chat(chat);
                if state.busy != Busy::Idle {
                    return;
                }
                if state.catch_up_next {
                    state.catch_up_next = false;
                    return self.catch_up(chat);
                }
                let known = state.known.unwrap_or(0);
                if count == 0 {
                    // Nothing more is stored.
                    state.top = known;
                } else if known < state.top || count as u64 >= u64::from(PAGE) {
                    return self.fill(chat);
                }
                self.settle(chat);
            }
            Purpose::CatchUp { mut found } => {
                let current = match self.device.as_ref() {
                    Some(device) => device.epoch(chat).ok(),
                    None => None,
                };
                let Some(current) = current else {
                    self.chat(chat).busy = Busy::Idle;
                    return self.settle(chat);
                };
                let mut reached = message_list.is_empty();
                for message in message_list.iter().rev() {
                    if message.kind != Some(Kind::Commit) {
                        continue;
                    }
                    let Some(epoch) = message.epoch else {
                        continue;
                    };
                    if epoch <= current {
                        reached = true;
                        break;
                    }
                    found.push(message.clone());
                    if epoch == current + 1 {
                        reached = true;
                        break;
                    }
                }
                if let Some(first) = message_list.first()
                    && first.seq <= 1
                {
                    reached = true;
                }
                if !reached && let Some(first) = message_list.first() {
                    let before = first.seq;
                    if !self.page(chat, 0, Some(before), Purpose::CatchUp { found }) {
                        self.chat(chat).busy = Busy::Idle;
                    }
                    return;
                }
                self.chat(chat).busy = Busy::Idle;
                found.reverse();
                let mut changed = false;
                for message in found {
                    if self.match_flight(chat, &message) {
                        continue;
                    }
                    if let Applied::Changed = self.apply_commit(chat, &message, true) {
                        changed = true;
                    }
                }
                if changed {
                    self.after_commits(chat);
                }
                self.settle(chat);
            }
        }
    }

    /// Opens what it can. With `wait_device`, holds what comes from a device not yet known for one
    /// fresh device list. With `wait_key`, holds what waits for its epoch, the chat's join or its
    /// restore. Anything else is shown as it is.
    fn open_list(
        &mut self,
        chat: Ulid,
        message_list: Vec<MessageFrame>,
        wait_device: bool,
        wait_key: bool,
    ) {
        let mut highest = 0;
        let mut need_devices = false;
        let mut need_catch_up = false;
        let joining = self.chat(chat).joining;
        let restoring = self.restoring.contains(&chat);
        for message in message_list {
            highest = highest.max(message.seq);
            let Some(kind) = message.kind else {
                self.event(Event::Removed {
                    chat,
                    seq: message.seq,
                });
                continue;
            };
            if kind == Kind::Commit {
                continue;
            }
            let Some(device) = message.device else {
                self.problem(&message, None, Problem::Malformed);
                continue;
            };
            let Some(known) = self.devices.get(&device).copied() else {
                if wait_device {
                    need_devices = true;
                    self.chat(chat).unknown.push(message);
                } else {
                    self.problem(&message, None, Problem::UnknownDevice);
                }
                continue;
            };
            if kind == Kind::DeviceNotice {
                self.notice(&message, device, known);
                continue;
            }
            let signature = match &message.signature {
                Some(signature) => fixed::<64>(&signature.0),
                None => None,
            };
            let (Some(header), Some(signature)) = (message.header(), signature) else {
                self.problem(&message, Some(known.account), Problem::Malformed);
                continue;
            };
            let body = match &message.body {
                Some(body) => body.0.clone(),
                None => Vec::new(),
            };
            if verify_message(&header, &body, &signature, &known.public).is_err() {
                self.problem(&message, Some(known.account), Problem::BadSignature);
                continue;
            }
            if kind == Kind::Delete {
                match message.pointer_seq {
                    Some(target_seq) => self.event(Event::Deleted {
                        chat,
                        seq: message.seq,
                        target_seq,
                    }),
                    None => self.problem(&message, Some(known.account), Problem::Malformed),
                }
                continue;
            }
            let text = if !header.encrypted {
                text_of(&body)
            } else {
                let Some(own) = self.device.as_ref() else {
                    return;
                };
                let current = own.epoch(chat).ok();
                match own.open(&header, &body, &signature, &known.public) {
                    Ok(plaintext) => text_of(&plaintext),
                    Err(_) if wait_key && (restoring || (current.is_none() && joining)) => {
                        self.chat(chat).waiting.push(message);
                        continue;
                    }
                    Err(_)
                        if wait_key
                            && matches!(current, Some(current) if header.epoch.unwrap_or(0) > current) =>
                    {
                        need_catch_up = true;
                        self.chat(chat).waiting.push(message);
                        continue;
                    }
                    Err(_) => None,
                }
            };
            let mine = self.account == Some(known.account);
            let shown = shown(&message, Some(known.account), mine, text);
            match (kind, message.pointer_seq) {
                (Kind::Edit, Some(target_seq)) => self.event(Event::Edited {
                    message: shown,
                    target_seq,
                }),
                _ => self.event(Event::Message { message: shown }),
            }
        }
        if need_devices && !self.chat(chat).fetching_devices {
            self.devices_of(chat);
        }
        if need_catch_up {
            self.catch_up(chat);
        }
        let now = self.now;
        let state = self.chat(chat);
        if highest > state.received.wanted {
            state.received.wanted = highest;
            state.received.at = now;
            self.report(chat);
        }
    }

    fn problem(&mut self, message: &MessageFrame, account: Option<Ulid>, problem: Problem) {
        let mine = account.is_some() && account == self.account;
        let mut shown = shown(message, account, mine, None);
        shown.problem = Some(problem);
        self.event(Event::Message { message: shown });
    }

    /// A new device's notice, whose signature is the device's stamp from this account's master
    /// key.
    fn notice(&mut self, message: &MessageFrame, device: Ulid, known: KnownDevice) {
        let stamp = match &message.signature {
            Some(signature) => fixed::<64>(&signature.0),
            None => None,
        };
        let mut proven = false;
        if let (Some(stamp), Some(own)) = (stamp, self.device.as_mut())
            && Some(known.account) == self.account
        {
            let record = DeviceRecord {
                account: known.account,
                device,
                device_public: known.public,
                master_public: known.master,
                stamp,
            };
            proven = own.check_device(&record).is_ok();
        }
        let problem = if proven {
            None
        } else {
            Some(Problem::BadSignature)
        };
        self.event(Event::Notice {
            chat: message.chat,
            seq: message.seq,
            device,
            saved_at: message.server_saved_at.unwrap_or(0),
            problem,
        });
    }

    /// Checks a device's ending statement, then commits its removal from every chat it is in.
    fn revoked(&mut self, account: Ulid, ended: Ulid, statement: &[u8]) {
        let Some(signature) = fixed::<64>(statement) else {
            return self.error(
                "bad_signature",
                "A device's ending statement does not verify",
            );
        };
        let master = match self.accounts.get(&account) {
            Some(info) => match &info.master_public {
                Some(master) => fixed::<32>(&master.0),
                None => None,
            },
            None => {
                // Checked once the account's keys arrive.
                self.revocations.push((account, ended, statement.to_vec()));
                return self.ask_account(account);
            }
        };
        let mut proven = false;
        if let Some(master) = master {
            proven = verify_revocation(&master, &account, &ended, &signature).is_ok();
        }
        if !proven && let Some(known) = self.devices.get(&ended) {
            proven = known.account == account
                && verify_logout(&known.public, &account, &ended, &signature).is_ok();
        }
        if !proven {
            return self.error(
                "bad_signature",
                "A device's ending statement does not verify",
            );
        }
        let Some(device) = self.device.as_ref() else {
            return;
        };
        if device.record().device == ended {
            return self.ended();
        }
        let mut chat_list = Vec::new();
        for chat in self.chats.keys() {
            if let Ok(member_list) = device.member_list(*chat) {
                for member in member_list {
                    if member.device == ended {
                        chat_list.push(*chat);
                    }
                }
            }
        }
        chat_list.sort();
        for chat in chat_list {
            self.chat(chat).plan_list.push(Plan::Remove {
                device_list: vec![ended],
            });
        }
    }

    /// With the words' keys, unwraps every key this account was given in this chat.
    fn restore_chat(&mut self, chat: Ulid) {
        if self.keys.is_some() {
            self.restoring.insert(chat);
            self.get(format!("/api/v1/chats/{chat}/wraps"), Asked::Wraps { chat });
        }
    }

    /// Once no chat is left to restore, the words' keys go.
    fn restore_done(&mut self, chat: Ulid) {
        if self.restoring.remove(&chat) && self.restoring.is_empty() {
            self.keys = None;
        }
    }

    /// This chat's restore cannot finish, so what waited for it is shown as it is.
    fn restored_without(&mut self, chat: Ulid) {
        if self.restoring.contains(&chat) {
            self.restore_done(chat);
            self.release(chat);
        }
    }

    // ---------------------------------------------------------------------------------------------
    // REST replies
    // ---------------------------------------------------------------------------------------------

    fn replied(&mut self, id: u64, status: u16, body: &str) {
        let Some(mut request) = self.asked.remove(&id) else {
            return;
        };
        let transient = status == 0 || status == 429 || status >= 500;
        if transient && background(&request.asked) {
            let at = backoff(self.now, request.attempt);
            request.attempt += 1;
            self.later.push((at, request));
            return;
        }
        if !(200..300).contains(&status) {
            let (code, message) = if status == 0 {
                (
                    "offline".to_string(),
                    "The server could not be reached".to_string(),
                )
            } else {
                match serde_json::from_str::<RefusedReply>(body) {
                    Ok(refused) => (refused.code, refused.message),
                    Err(_) => (
                        format!("http_{status}"),
                        "The server's reply could not be read".to_string(),
                    ),
                }
            };
            return self.rest_refused(request, status, &code, &message);
        }
        self.reply(request.asked, body);
    }

    fn reply(&mut self, asked: Asked, body: &str) {
        match asked {
            Asked::Register {
                username,
                password,
                captcha,
            } => {
                if let Some(AccountReply { .. }) = self.data(body) {
                    // Registering used the captcha up. Login checks another one.
                    if captcha.is_some() {
                        self.event(Event::Registered { username });
                    } else {
                        self.log_in(username, password, None);
                    }
                }
            }
            Asked::Setup { password, captcha } => {
                if let Some(SetupReply { username, .. }) = self.data(body) {
                    self.log_in(username, password, captcha);
                }
            }
            Asked::Captcha => {
                if let Some(CaptchaReply { id, image }) = self.data(body) {
                    self.event(Event::Captcha { id, image });
                }
            }
            Asked::Challenge {
                username,
                password,
                captcha,
            } => {
                let Some(ChallengeReply { challenge }) = self.data(body) else {
                    return;
                };
                let Some(device) = self.device.as_ref() else {
                    return;
                };
                let Some(fixed_challenge) = fixed::<32>(&challenge.0) else {
                    return self.error("bad_reply", "The challenge is not 32 bytes");
                };
                let signature = match device.sign_device_login(&fixed_challenge) {
                    Ok(signature) => signature,
                    Err(error) => return self.error("sign_failed", &error.to_string()),
                };
                let proof = DeviceProof {
                    id: device.record().device,
                    challenge,
                    signature: Bytes(signature.to_vec()),
                };
                self.post_login(username, password, captcha, Some(proof));
            }
            Asked::Login { username, .. } => {
                let Some(LoginReply {
                    account,
                    device,
                    token,
                }) = self.data(body)
                else {
                    return;
                };
                if let Some(own) = self.device.as_ref()
                    && own.record().account != account
                {
                    return self.error("wrong_account", "This device belongs to another account");
                }
                let kept = token.map(Secret);
                if kept.is_some() {
                    self.token = kept.clone();
                }
                self.account = Some(account);
                self.username = Some(username.clone());
                self.stopped = false;
                self.session_lost = false;
                self.limited = false;
                self.event(Event::SignedIn {
                    account,
                    username,
                    device,
                });
                if kept.is_some() {
                    self.event(Event::Token { token: kept });
                }
                self.asked_accounts.insert(account);
                self.get(format!("/api/v1/accounts/{account}"), Asked::Me);
            }
            Asked::Me => {
                let Some(info) = self.data::<AccountInfo>(body) else {
                    return;
                };
                let restore = info.master_public.is_some();
                self.account_known(info);
                if self.device.is_some() {
                    if self.socket == Socket::Down {
                        self.connect();
                    }
                } else {
                    self.event(Event::NeedWords { restore });
                }
            }
            Asked::Account { .. } => {
                if let Some(info) = self.data::<AccountInfo>(body) {
                    self.account_known(info);
                }
            }
            Asked::AccountKeys => self.keys_set(),
            Asked::KeysCheck => {
                let Some(info) = self.data::<AccountInfo>(body) else {
                    return;
                };
                let (Some(account), Some(keys)) = (self.account, self.keys.as_ref()) else {
                    return;
                };
                let ours = self.keys_body(keys, account);
                if info.master_public == Some(ours.master_public)
                    && info.account_public == Some(ours.account_public)
                {
                    return self.keys_set();
                }
                self.keys = None;
                self.error("keys_set", "This account already has other words");
            }
            Asked::DeviceId { restore } => {
                let Some(DeviceReply { device: id }) = self.data(body) else {
                    return;
                };
                let (Some(account), Some(keys)) = (self.account, self.keys.as_ref()) else {
                    return;
                };
                let device = match Device::create(keys, account, id) {
                    Ok(device) => device,
                    Err(error) => return self.error("device_failed", &error.to_string()),
                };
                let record = device.record();
                let body = RegisterDeviceBody {
                    device: id,
                    public_key: Bytes(record.device_public.to_vec()),
                    stamp: Bytes(record.stamp.to_vec()),
                    platform: self.platform.platform,
                    os: self.platform.os.clone(),
                    os_version: self.platform.os_version.clone(),
                    web_platform: self.platform.web_platform.clone(),
                    web_platform_version: self.platform.web_platform_version.clone(),
                };
                self.request(
                    "POST",
                    "/api/v1/devices".to_string(),
                    Some(&body),
                    Asked::RegisterDevice {
                        device: Box::new(device),
                        restore,
                    },
                );
            }
            Asked::RegisterDevice { device, restore } => self.registered(*device, restore),
            Asked::KeyPackageCount => {
                let Some(CountReply { count }) = self.data(body) else {
                    return;
                };
                if count >= KEY_PACKAGES {
                    return;
                }
                let Some(device) = self.device.as_ref() else {
                    return;
                };
                let mut body_list = Vec::new();
                for _ in count..KEY_PACKAGES {
                    match device.key_package() {
                        Ok(package) => body_list.push(Bytes(package)),
                        Err(error) => return self.error("key_package_failed", &error.to_string()),
                    }
                }
                self.dirty = true;
                let body = KeyPackagesBody { body_list };
                self.request(
                    "POST",
                    "/api/v1/key_packages".to_string(),
                    Some(&body),
                    Asked::KeyPackageUpload,
                );
            }
            Asked::KeyPackageUpload => {}
            Asked::CreateChat { account } => {
                self.starting.remove(&account);
                let Some(ChatReply { chat, .. }) = self.data(body) else {
                    return;
                };
                let Some(me) = self.account else {
                    return;
                };
                self.event(Event::Started { chat, account });
                let joined = self.joined(chat);
                let state = self.chat(chat);
                state.member_list = vec![me];
                if account != me {
                    state.member_list.push(account);
                    state.chat_type = Some(ChatType::Direct);
                } else {
                    state.chat_type = Some(ChatType::SelfChat);
                }
                if joined {
                    return self.ready(chat);
                }
                if state.joining {
                    return;
                }
                state.joining = true;
                self.get(
                    format!("/api/v1/chats/{chat}/group_info"),
                    Asked::StartInfo { chat },
                );
            }
            Asked::StartInfo { chat } | Asked::JoinInfo { chat } => {
                let Some(GroupInfoReply { group_info, .. }) = self.data(body) else {
                    self.chat(chat).joining = false;
                    self.restored_without(chat);
                    return self.fail_chat(chat, "bad_reply");
                };
                // A Welcome brought this device in meanwhile.
                if self.joined(chat) {
                    return;
                }
                self.chat(chat).plan_list.push(Plan::Join {
                    group_info: group_info.0,
                });
            }
            Asked::Claim {
                chat,
                mut account_list,
                mut package_list,
                ..
            } => {
                if let Some(KeyPackageListReply { key_package_list }) = self.data(body) {
                    let own = self.device.as_ref().map(|device| device.record().device);
                    for package in key_package_list {
                        if Some(package.device) != own {
                            package_list.push(package);
                        }
                    }
                }
                self.claim_next(chat, &mut account_list, package_list);
            }
            Asked::ChatDevices { chat, judged } => {
                self.chat(chat).fetching_devices = false;
                let Some(ChatDeviceListReply { device_list }) = self.data(body) else {
                    return;
                };
                self.chat_devices(chat, device_list, judged);
            }
            Asked::Wraps { chat } => {
                let Some(WrapListReply { wrap_list }) = self.data(body) else {
                    return self.restored_without(chat);
                };
                self.wraps(chat, wrap_list);
            }
            Asked::Search => {
                if let Some(AccountListReply { account_list }) = self.data(body) {
                    self.event(Event::SearchResults { account_list });
                }
            }
            Asked::Devices => {
                if let Some(reply) = self.data::<DevicesReply>(body) {
                    self.event(Event::DeviceList { reply });
                }
            }
            Asked::Terminate { device } => {
                self.event(Event::Terminated { device });
                if !self.limited {
                    return;
                }
                self.limited = false;
                if self.device.is_some() {
                    self.stopped = false;
                    if self.socket == Socket::Down {
                        self.connect();
                    }
                } else if let Some(restore) = self.register_restore.take() {
                    self.request::<EmptyReply>(
                        "POST",
                        "/api/v1/devices/id".to_string(),
                        None,
                        Asked::DeviceId { restore },
                    );
                }
            }
            Asked::LogOut => self.ended(),
            Asked::Features => {
                if let Some(FeaturesReply {
                    registration,
                    captcha,
                }) = self.data(body)
                {
                    self.event(Event::Features {
                        registration,
                        captcha,
                    });
                }
            }
        }
    }

    /// The account holds these words' keys: the device comes next.
    fn keys_set(&mut self) {
        let (Some(account), Some(keys)) = (self.account, self.keys.as_ref()) else {
            return;
        };
        let body = self.keys_body(keys, account);
        if let Some(info) = self.accounts.get_mut(&account) {
            info.master_public = Some(body.master_public);
            info.account_public = Some(body.account_public);
            info.account_stamp = Some(body.account_stamp);
        }
        self.request::<EmptyReply>(
            "POST",
            "/api/v1/devices/id".to_string(),
            None,
            Asked::DeviceId { restore: false },
        );
    }

    fn registered(&mut self, device: Device, restore: bool) {
        let id = device.record().device;
        let account = device.record().account;
        let record = device.record();
        self.devices.insert(
            id,
            KnownDevice {
                account,
                public: record.device_public,
                master: record.master_public,
            },
        );
        self.device = Some(device);
        self.dirty = true;
        if !restore {
            // New words: there is no history to unwrap, so their keys go at once.
            self.keys = None;
        }
        let username = self.username.clone().unwrap_or_default();
        self.event(Event::SignedIn {
            account,
            username,
            device: Some(id),
        });
        self.connect();
    }

    fn data<T: DeserializeOwned>(&mut self, body: &str) -> Option<T> {
        match serde_json::from_str::<Wrapped<T>>(body) {
            Ok(wrapped) => Some(wrapped.data),
            Err(_) => {
                self.error("bad_reply", "The server's reply could not be read");
                None
            }
        }
    }

    fn account_known(&mut self, info: AccountInfo) {
        let account = info.id;
        // A reload keeps the device and loses the name, so the page hears `signed_in` when it comes back.
        let device_id = self.device.as_ref().map(|device| device.record().device);
        if self.account == Some(account) && self.username.is_none() {
            self.username = Some(info.username.clone());
            if let Some(id) = device_id {
                self.event(Event::SignedIn {
                    account,
                    username: info.username.clone(),
                    device: Some(id),
                });
            }
        }
        self.event(Event::Account {
            account: AccountBrief {
                id: info.id,
                username: info.username.clone(),
                display_name: info.display_name.clone(),
            },
        });
        self.accounts.insert(info.id, info);
        let mut pending = Vec::new();
        let mut index = 0;
        while index < self.revocations.len() {
            if self.revocations[index].0 == account {
                pending.push(self.revocations.remove(index));
            } else {
                index += 1;
            }
        }
        for (account, ended, statement) in pending {
            self.revoked(account, ended, &statement);
        }
    }

    fn rest_refused(&mut self, request: Request, status: u16, code: &str, message: &str) {
        let attempt = request.attempt;
        match request.asked {
            Asked::Login {
                username,
                password,
                with_device: true,
            } if code == "device_ended" => {
                self.device = None;
                self.out.push(Output::Wipe);
                self.post_login(username, password, None, None);
            }
            Asked::Register { .. }
            | Asked::Setup { .. }
            | Asked::Challenge { .. }
            | Asked::Login { .. } => self.error(code, message),
            // The socket opened and this count went out before the server closed it. A session
            // with no device is the login screen, and that refusal is not something they did.
            Asked::KeyPackageCount if code == "no_device" => {
                if !self.session_lost {
                    self.session_gone();
                }
            }
            _ if status == 401 => self.session_gone(),
            _ if code == "device_ended" => self.ended(),
            // An earlier try did register it, and only its answer was lost.
            Asked::RegisterDevice { device, restore } if code == "has_device" && attempt > 0 => {
                self.registered(*device, restore)
            }
            // The id expired while nothing answered: ask for another.
            Asked::RegisterDevice { restore, .. } if code == "unknown_device_id" && attempt > 0 => {
                self.request::<EmptyReply>(
                    "POST",
                    "/api/v1/devices/id".to_string(),
                    None,
                    Asked::DeviceId { restore },
                );
            }
            Asked::RegisterDevice { restore, .. } if code == "device_limit" => {
                self.limited = true;
                self.register_restore = Some(restore);
                self.event(Event::DeviceLimit {
                    message: message.to_string(),
                });
            }
            _ if code == "device_limit" => {
                self.limited = true;
                self.stopped = true;
                self.event(Event::DeviceLimit {
                    message: message.to_string(),
                });
            }
            Asked::AccountKeys if code == "keys_set" => {
                let Some(account) = self.account else {
                    return;
                };
                self.get(format!("/api/v1/accounts/{account}"), Asked::KeysCheck);
            }
            Asked::Account { account } => {
                self.asked_accounts.remove(&account);
                self.error(code, message);
            }
            Asked::CreateChat { account } => {
                self.starting.remove(&account);
                self.error(code, message);
            }
            Asked::StartInfo { chat } if code == "no_group_info" => {
                let Some(me) = self.account else {
                    return;
                };
                let mut account_list = Vec::new();
                for account in &self.chat(chat).member_list {
                    if *account != me {
                        account_list.push(*account);
                    }
                }
                account_list.push(me);
                self.claim_next(chat, &mut account_list, Vec::new());
            }
            Asked::JoinInfo { chat } if code == "no_group_info" => {
                self.chat(chat).joining = false;
                self.restore_done(chat);
                if self.chat(chat).chat_type == Some(ChatType::SelfChat) {
                    self.chat(chat).joining = true;
                    let mut account_list = Vec::new();
                    if let Some(me) = self.account {
                        account_list.push(me);
                    }
                    self.claim_next(chat, &mut account_list, Vec::new());
                } else {
                    self.fail_chat(chat, "not_joined");
                    self.release(chat);
                }
            }
            // This account's other devices may have none left, and they join by themselves later.
            // Anyone else having none means the chat cannot start yet.
            Asked::Claim {
                chat,
                account,
                mut account_list,
                package_list,
            } if code == "no_key_packages" && Some(account) == self.account => {
                self.claim_next(chat, &mut account_list, package_list);
            }
            Asked::StartInfo { chat } | Asked::JoinInfo { chat } | Asked::Claim { chat, .. } => {
                self.chat(chat).joining = false;
                self.restore_done(chat);
                self.fail_chat(chat, code);
                self.release(chat);
                self.error(code, message);
            }
            // No list is coming, so what waited for one is shown as from an unknown device.
            Asked::ChatDevices { chat, .. } => {
                let state = self.chat(chat);
                state.fetching_devices = false;
                let waiting = std::mem::take(&mut state.unknown);
                self.open_list(chat, waiting, false, true);
                self.error(code, message);
            }
            Asked::Wraps { chat } => {
                self.restored_without(chat);
                self.error(code, message);
            }
            _ => self.error(code, message),
        }
    }

    /// Claims one package per device of each account in turn, then starts the group.
    fn claim_next(
        &mut self,
        chat: Ulid,
        account_list: &mut Vec<Ulid>,
        package_list: Vec<KeyPackageItem>,
    ) {
        let Some(account) = account_list.first().copied() else {
            self.chat(chat)
                .plan_list
                .push(Plan::Create { package_list });
            return;
        };
        account_list.remove(0);
        self.request::<EmptyReply>(
            "POST",
            format!("/api/v1/accounts/{account}/key_packages/claim"),
            None,
            Asked::Claim {
                chat,
                account,
                account_list: std::mem::take(account_list),
                package_list,
            },
        );
    }

    /// Checks each device's stamp, acts on ended ones, then opens what waited for a device: what
    /// this list was asked for is decided, and what came after waits for the next one.
    fn chat_devices(&mut self, chat: Ulid, device_list: Vec<ChatDevice>, judged: usize) {
        let mut ended_list = Vec::new();
        let mut member_list = Vec::new();
        for item in device_list {
            let (Some(public), Some(master), Some(stamp)) = (
                fixed::<32>(&item.public_key.0),
                fixed::<32>(&item.master_public.0),
                fixed::<64>(&item.stamp.0),
            ) else {
                continue;
            };
            let record = DeviceRecord {
                account: item.account,
                device: item.device,
                device_public: public,
                master_public: master,
                stamp,
            };
            let Some(device) = self.device.as_mut() else {
                return;
            };
            if let Err(error) = device.check_device(&record) {
                self.error("bad_device", &error.to_string());
                continue;
            }
            self.dirty = true;
            if !member_list.contains(&item.account) {
                member_list.push(item.account);
            }
            self.devices.insert(
                item.device,
                KnownDevice {
                    account: item.account,
                    public,
                    master,
                },
            );
            if let Some(statement) = item.statement {
                ended_list.push((item.account, item.device, statement));
            }
        }
        if !member_list.is_empty() {
            self.chat(chat).member_list = member_list;
        }
        for (account, ended, statement) in ended_list {
            let in_group = match self.device.as_ref() {
                Some(device) => match device.member_list(chat) {
                    Ok(list) => list.iter().any(|member| member.device == ended),
                    Err(_) => false,
                },
                None => false,
            };
            if in_group {
                self.revoked(account, ended, &statement.0);
            }
        }
        let state = self.chat(chat);
        let judged = judged.min(state.unknown.len());
        let later = state.unknown.split_off(judged);
        let decided = std::mem::take(&mut state.unknown);
        self.open_list(chat, decided, false, true);
        self.open_list(chat, later, true, true);
    }

    /// Unwraps this account's keys while restoring, and repairs any wrap that is wrong.
    fn wraps(&mut self, chat: Ulid, wrap_list: Vec<WrapEntry>) {
        let account_list = self.account_list(chat);
        let Some(me) = self.account else {
            return;
        };
        let Some(device) = self.device.as_mut() else {
            return;
        };
        if let Some(keys) = self.keys.as_ref() {
            for entry in &wrap_list {
                if entry.account == me
                    && device
                        .unwrap_history(keys, chat, entry.epoch, &entry.wrapped.0)
                        .is_ok()
                {
                    self.dirty = true;
                }
            }
            self.restore_done(chat);
            self.release(chat);
        }
        let Some(device) = self.device.as_mut() else {
            return;
        };
        let Ok(epoch) = device.epoch(chat) else {
            return;
        };
        let mut stored = Vec::new();
        for entry in &wrap_list {
            if entry.epoch == epoch
                && let Some(wrapped) = fixed(&entry.wrapped.0)
            {
                stored.push(crate::mls::AccountWrap {
                    account: entry.account,
                    wrapped,
                });
            }
        }
        let repair_list = match device.check_wraps(chat, &stored, &account_list) {
            Ok(repair_list) => repair_list,
            Err(error) => return self.error("bad_wraps", &error.to_string()),
        };
        if repair_list.is_empty() {
            return;
        }
        let mut wrap_list = Vec::new();
        for wrap in repair_list {
            wrap_list.push(WrapItem {
                account: wrap.account,
                wrapped: Bytes(wrap.wrapped.to_vec()),
            });
        }
        let state = self.chat(chat);
        match &mut state.wraps_out {
            Some((out, list)) if *out == epoch => list.extend(wrap_list),
            _ => state.wraps_out = Some((epoch, wrap_list)),
        }
        self.send_outs(chat);
    }
}
