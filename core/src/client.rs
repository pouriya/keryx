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

use crate::call::{CallIndex, CallKey};
use crate::file::{self, FileInfo};
use crate::mls::{
    AccountKey, Device, DeviceRecord, MlsError, sign_revocation, stamp_account_key, verify_logout,
    verify_message, verify_revocation,
};
use crate::protocol::{
    AccountBrief, AccountInfo, AccountKeysBody, AccountListReply, AccountReply, Answer, Bytes,
    CallBody, CallEvent, CallEventKind, CallOutcome, CallText, CaptchaAnswer, CaptchaReply,
    ChallengeBody, ChallengeReply, ChatDevice, ChatDeviceListReply, ChatItem, ChatReply, ChatType,
    ClientFrame, ContentKind, CountReply, CreateChatBody, CreateFileBody, DeviceProof, DeviceReply,
    DevicesReply, DraftBody, DraftItem, EmptyReply, FeaturesReply, FileReply, GroupInfoReply,
    Header, KeyPackageItem, KeyPackageListReply, KeyPackagesBody, Kind, LoginBody, LoginReply,
    MarkItem, MessageFrame, Question, RefusedReply, RegisterBody, RegisterDeviceBody, SendFrame,
    ServerFrame, SetupBody, SetupReply, SignedBody, StickerListReply, StickerPack, SyncPosition,
    TimesItem, WelcomeItem, WrapEntry, WrapItem, WrapListReply, call_text_header,
};
use crate::queue::{Level, Queue, Target};
use crate::seed::{self, AccountKeys};
use crate::ulid::Ulid;

/// How many key packages a device keeps on the server.
const KEY_PACKAGES: u64 = 10;
/// The waits before each retry, the last repeating: the connection, background requests, a lost
/// join and a failing save.
const RETRY_MS: [i64; 6] = [1_000, 2_000, 4_000, 8_000, 16_000, 30_000];
/// How many messages one read of the archive asks for, when a chat opens or scrolls up.
const PAGE: u32 = 50;

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

/// A picture's small pixels, from which core makes its BlurHash.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Thumbnail {
    pub width: u32,
    pub height: u32,
    #[serde(with = "serde_bytes")]
    #[cfg_attr(feature = "wasm", tsify(type = "Uint8Array"))]
    pub rgba: Vec<u8>,
}

/// One file to send. The platform holds its bytes as `handle`, its own number, and core asks for
/// them chunk by chunk. `kind` is voice, image, music, video, doc or file, and `size` its real size.
/// Music is an MP3 only, whose type is `audio/mpeg`: its `title`, `artist` and `duration` in
/// milliseconds come from its tags, and its cover goes as `thumbnail`, with `width` and `height`.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileSpec {
    pub handle: u64,
    pub kind: ContentKind,
    pub name: String,
    #[serde(rename = "type")]
    pub mime: String,
    pub size: u64,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    #[serde(default)]
    pub thumbnail: Option<Thumbnail>,
    /// An image's or video's own text, in a bundle.
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub artist: Option<String>,
    #[serde(default)]
    pub duration: Option<u64>,
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
    /// `reply` names the message answered, by its sequence id or its pending local id. The message
    /// then waits for that id if it has none yet, and carries it inside its encryption.
    SendText {
        chat: Ulid,
        text: Secret,
        #[serde(default)]
        reply: Option<Pointer>,
    },
    /// One file, or several images and videos as a bundle, with an optional caption. Each goes up
    /// first, and the message joins the queue once all are up.
    SendFiles {
        chat: Ulid,
        files: Vec<FileSpec>,
        #[serde(default)]
        text: Option<Secret>,
    },
    /// Fetches a file into the platform's `handle`, chunk by chunk, each opened here.
    Download {
        file: FileInfo,
        handle: u64,
    },
    /// A file message's edit holds the files it keeps in `files`, and names those it drops in
    /// `remove`.
    Edit {
        chat: Ulid,
        target: Pointer,
        text: Secret,
        #[serde(default)]
        files: Vec<FileInfo>,
        #[serde(default)]
        remove: Vec<Ulid>,
    },
    /// Names the target's files in `remove`, so the server deletes them too.
    Delete {
        chat: Ulid,
        target: Pointer,
        #[serde(default)]
        remove: Vec<Ulid>,
    },
    Seen {
        chat: Ulid,
        up_to_seq: u64,
    },
    Typing {
        chat: Ulid,
        on: bool,
    },
    /// Keeps this chat's draft, or clears it with none. Nothing is queued: a draft is best effort,
    /// and one the connection could not carry is dropped, as the next change replaces it.
    Draft {
        chat: Ulid,
        draft: Option<Draft>,
    },
    /// When each other member received and saw one message, answered as [`Event::Times`].
    Times {
        chat: Ulid,
        seq: u64,
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
    /// Starts a call: one message, whose delivery rings the others. It says what the caller has on.
    StartCall {
        chat: Ulid,
        video: bool,
        voice: bool,
    },
    /// Declines a ringing call, without joining it.
    DeclineCall {
        chat: Ulid,
        call_seq: u64,
    },
    /// Joins a call: [`Output::JoinCall`] hands the platform's call code where to connect and the
    /// key. One call at a time.
    JoinCall {
        chat: Ulid,
        call_seq: u64,
    },
    LeaveCall {
        chat: Ulid,
    },
    /// Text typed in the joined call, sealed here and sent on the device's `text` track.
    CallText {
        chat: Ulid,
        call_seq: u64,
        text: Secret,
    },
    /// One of the server's stickers, by its id, sealed so the server never sees which.
    SendSticker {
        chat: Ulid,
        sticker: Ulid,
        #[serde(default)]
        reply: Option<Pointer>,
    },
    /// A tree of questions, with an optional intro. Clients have no screen for it: an agent sends
    /// them. Refused here unless the tree holds together.
    SendQuestion {
        chat: Ulid,
        #[serde(default)]
        text: Option<Secret>,
        question: Question,
    },
    /// This account's answer. `question` is the question as shown, checked here against the answer
    /// before it goes.
    SendAnswer {
        chat: Ulid,
        question: Question,
        answer: Answer,
    },
    /// Replaces this account's answer, checked the same way.
    EditAnswer {
        chat: Ulid,
        target: Pointer,
        question: Question,
        answer: Answer,
    },
    /// The server's sticker packs, answered as [`Event::Stickers`].
    Stickers,
    /// One sticker's WebP, answered as [`Event::StickerImage`].
    StickerImage {
        sticker: Ulid,
    },
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
    /// The bytes [`Output::ReadChunk`] asked for.
    ChunkRead {
        handle: u64,
        n: u64,
        #[serde(with = "serde_bytes")]
        #[cfg_attr(feature = "wasm", tsify(type = "Uint8Array"))]
        bytes: Vec<u8>,
    },
    /// [`Output::GetChunk`]'s reply: its status, 0 when nothing answered, and its raw bytes.
    Fetched {
        id: u64,
        status: u16,
        #[serde(with = "serde_bytes")]
        #[cfg_attr(feature = "wasm", tsify(type = "Uint8Array"))]
        bytes: Vec<u8>,
    },
    /// A frame from the joined call's `events` track.
    CallEvent {
        event: CallEvent,
    },
    /// [`Output::SaveRows`] wrote all of its rows.
    RowsSaved {
        id: u64,
    },
    /// [`Output::SaveRows`] could not write them. Nothing of it may stay.
    RowsSaveFailed {
        id: u64,
    },
    /// [`Output::ReadRows`]'s answer: the chat's messages, oldest first, and the rows of every call
    /// among them.
    RowsRead {
        id: u64,
        message_list: Vec<MessageFrame>,
        call_list: Vec<CallEvent>,
    },
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
    /// Read `length` bytes at `offset` of the file the platform holds as `handle`, and answer with
    /// [`Input::ChunkRead`].
    ReadChunk {
        handle: u64,
        n: u64,
        offset: u64,
        length: u64,
    },
    /// A PUT of these raw bytes. Its reply comes back as [`Input::Reply`] with the same id.
    PutChunk {
        id: u64,
        path: String,
        bearer: Option<Secret>,
        #[serde(with = "serde_bytes")]
        #[cfg_attr(feature = "wasm", tsify(type = "Uint8Array"))]
        bytes: Vec<u8>,
    },
    /// A GET whose reply is raw bytes. It comes back as [`Input::Fetched`] with the same id.
    GetChunk {
        id: u64,
        path: String,
        bearer: Option<Secret>,
    },
    /// Write these opened bytes at `offset` of the file the platform holds as `handle`.
    WriteChunk {
        handle: u64,
        n: u64,
        offset: u64,
        #[serde(with = "serde_bytes")]
        #[cfg_attr(feature = "wasm", tsify(type = "Uint8Array"))]
        bytes: Vec<u8>,
    },
    /// Connect to a call's MoQ over a WebSocket at `path`, publish as this device and read the
    /// others. The key and the members' MLS indexes are for the Rust that seals and opens frames,
    /// and never go into a platform's own language.
    JoinCall {
        chat: Ulid,
        call_seq: u64,
        path: String,
        bearer: Option<Secret>,
        key: CallKey,
        index_list: Vec<CallIndex>,
    },
    /// The joined call's chat moved to a new epoch: seal with this key from now on, and open with
    /// it too.
    CallKey {
        chat: Ulid,
        key: CallKey,
        index_list: Vec<CallIndex>,
    },
    LeaveCall {
        chat: Ulid,
    },
    /// Publish this on the device's `text` track in the joined call.
    CallText {
        chat: Ulid,
        call_seq: u64,
        frame: CallText,
    },
    /// Keep these rows in the archive, exactly as they are, in one transaction, then answer with
    /// [`Input::RowsSaved`] or [`Input::RowsSaveFailed`]. A row already kept stays, except that a
    /// removed message, with only `chat`, `seq` and `server_saved_at`, replaces its copy, and a
    /// removed call message takes that call's rows with it.
    SaveRows {
        id: u64,
        message_list: Vec<MessageFrame>,
        call_list: Vec<CallEvent>,
    },
    /// Read up to `limit` of the chat's newest messages from the archive, below `before_seq` if
    /// given, with the rows of every call among them, and answer with [`Input::RowsRead`].
    ReadRows {
        id: u64,
        chat: Ulid,
        before_seq: Option<u64>,
        limit: u32,
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

/// Unsent text in a chat, with the message it replies to or edits, if any.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify), tsify(missing_as_null))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Draft {
    pub text: Secret,
    #[serde(default)]
    pub reply: Option<u64>,
    #[serde(default)]
    pub edit: Option<u64>,
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
    /// exist, it was sent while this account was out of the chat. A file message's caption, so
    /// with files and no caption it is `None` too, and `files` says it was opened.
    pub text: Option<Secret>,
    /// The sequence id of the message this one answers, from inside its encryption, so the server
    /// never sees who replied to what.
    #[serde(default)]
    pub reply: Option<u64>,
    /// A file message's files, or a bundle's.
    #[serde(default)]
    pub files: Vec<FileInfo>,
    /// A call's start: what the caller had on.
    #[serde(default)]
    pub call: Option<CallBody>,
    /// Missed or answered, from the call's `end` row, once this client has it.
    #[serde(default)]
    pub call_outcome: Option<CallOutcome>,
    /// The `end` row's time.
    #[serde(default)]
    pub call_ended_at: Option<i64>,
    /// A sticker message's sticker.
    #[serde(default)]
    pub sticker: Option<Ulid>,
    /// A question message's tree, once it holds together. Its intro is `text`.
    #[serde(default)]
    pub question: Option<Question>,
    /// An answer message's answer.
    #[serde(default)]
    pub answer: Option<Answer>,
    pub problem: Option<Problem>,
}

/// Something that happened inside a call, to show in it or in its replay.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify), tsify(missing_as_null))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShownCallEvent {
    pub chat: Ulid,
    pub call_seq: u64,
    pub n: u64,
    pub kind: CallEventKind,
    pub device: Ulid,
    pub account: Option<Ulid>,
    pub mine: bool,
    pub at: i64,
    /// Text's own clock, as sent.
    pub sent_at: Option<i64>,
    /// An end's missed or answered. The list names the call from this.
    pub outcome: Option<CallOutcome>,
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
    /// `name` is the server's name, when one is set.
    Features {
        registration: bool,
        captcha: bool,
        name: Option<String>,
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
    /// This account's devices known so far, so the platform counts unread without its own
    /// messages. Sent again whenever one more is known.
    OwnDevices {
        device_list: Vec<Ulid>,
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
        /// What kind of content it is, and none for an edit or a delete.
        #[serde(default)]
        content_kind: Option<ContentKind>,
        text: Option<Secret>,
        /// A file message's files, by the platform's handles.
        #[serde(default)]
        handles: Vec<u64>,
        /// A sticker message's sticker.
        #[serde(default)]
        sticker: Option<Ulid>,
        /// An answer, or an answer's edit.
        #[serde(default)]
        answer: Option<Answer>,
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
    /// This chat's draft, from the list or from another of this account's devices, or none once
    /// it was cleared. `saved_at` is the server's time, so a device keeps the newer one.
    Draft {
        chat: Ulid,
        draft: Option<Draft>,
        saved_at: i64,
    },
    /// The answer to `Times`: each member but the sender, by the server's clock.
    Times {
        chat: Ulid,
        seq: u64,
        member_list: Vec<TimesItem>,
    },
    /// The server's sticker packs.
    Stickers {
        pack_list: Vec<StickerPack>,
    },
    /// One sticker's WebP. Not encrypted: everyone on the server can see it.
    StickerImage {
        sticker: Ulid,
        bytes: Bytes,
    },
    /// A sticker the server no longer has.
    StickerMissing {
        sticker: Ulid,
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
    /// `done` of `total` chunks of the platform's `handle`, going up or coming down.
    Progress {
        handle: u64,
        done: u64,
        total: u64,
    },
    /// All of `handle` is written.
    Downloaded {
        handle: u64,
    },
    /// An upload or download stopped for good. An upload's message is not sent.
    TransferFailed {
        handle: u64,
        code: String,
    },
    /// Something happened inside a call: live, or from its replay.
    CallEvent {
        event: ShownCallEvent,
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
    /// A text, a file message, an edit or a delete.
    Content {
        chat: Ulid,
        kind: Kind,
        content_kind: Option<ContentKind>,
        text: Option<Secret>,
        files: Vec<FileInfo>,
        remove: Vec<Ulid>,
        call: Option<CallBody>,
        fields: Option<Secret>,
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
    CreateFile {
        upload: u64,
    },
    PutChunk {
        upload: u64,
        n: u64,
    },
    GetChunk {
        handle: u64,
        n: u64,
    },
    Features,
    Stickers,
    StickerImage {
        sticker: Ulid,
    },
}

/// A REST request, kept until it is answered, to go again if nothing answered.
struct Request {
    method: String,
    path: String,
    body: Option<Secret>,
    /// A chunk to PUT, as raw bytes.
    bytes: Option<Vec<u8>>,
    /// The reply is raw bytes: a chunk.
    raw: bool,
    asked: Asked,
    attempt: usize,
}

/// What a content item carries, beside its chat, kind and target.
#[derive(Default)]
struct Content {
    content_kind: Option<ContentKind>,
    text: Option<Secret>,
    files: Vec<FileInfo>,
    remove: Vec<Ulid>,
    /// A call's start body.
    call: Option<CallBody>,
    /// More of the plaintext's fields, as a JSON object: a sticker, a question's tree or an answer.
    fields: Option<Secret>,
    /// The platform's handles for `files`, for its pending message.
    handles: Vec<u64>,
    /// A sticker message's sticker, for its pending message.
    sticker: Option<Ulid>,
    /// An answer or an answer's edit, for its pending message.
    answer: Option<Answer>,
}

/// The call this device has joined.
struct ActiveCall {
    chat: Ulid,
    call_seq: u64,
    epoch: u64,
}

/// A file message's files going up, one at a time, before the message joins the queue.
struct Upload {
    chat: Ulid,
    content_kind: ContentKind,
    text: Option<Secret>,
    files: Vec<UpFile>,
    /// The file going up now.
    current: usize,
}

struct UpFile {
    handle: u64,
    info: FileInfo,
    key: zeroize::Zeroizing<[u8; 16]>,
    /// The next chunk to read and put.
    next: u64,
}

/// A file coming down into the platform's handle.
struct Download {
    info: FileInfo,
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
    /// A `sync` page, of every chat or only `chat`.
    Sync {
        chat: Option<Ulid>,
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

/// A `sync` under way: one at a time, of every chat or only `chat`.
struct SyncRun {
    chat: Option<Ulid>,
    /// Its first page carries the chats list.
    first: bool,
}

/// Rows handed to the platform to keep. Once kept, `since` moves to `last`, and a `sync` page goes
/// on to its next one or ends.
struct RowsSave {
    last: Option<SyncPosition>,
    page: Option<PageSaved>,
}

/// What follows a kept `sync` page, if the connection it came on is still the current one.
struct PageSaved {
    chat: Option<Ulid>,
    more: bool,
    generation: u64,
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
    /// Where a `sync` started when it went, so the one after a reconnect finds it.
    since: Option<i64>,
}

/// A device whose stamp has been checked.
#[derive(Clone, Copy)]
struct KnownDevice {
    account: Ulid,
    public: [u8; 32],
    master: [u8; 32],
}

/// What the snapshot's shelf holds, as JSON: enough to show the chat list, its previews and every
/// opened message with no network. Everything in it is checked again when the server answers.
#[derive(Default, Serialize, Deserialize)]
struct Shelf {
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    chat_list: Vec<ChatItem>,
    #[serde(default)]
    account_list: Vec<AccountInfo>,
    #[serde(default)]
    device_list: Vec<ShelfDevice>,
}

#[derive(Serialize, Deserialize)]
struct ShelfDevice {
    device: Ulid,
    account: Ulid,
    public: Bytes,
    master: Bytes,
}

#[derive(Default, PartialEq, Eq)]
enum Busy {
    #[default]
    Idle,
    /// A `sync` covering this chat runs, and live frames wait for it.
    Syncing,
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
}

#[derive(Default)]
struct ChatState {
    chat_type: Option<ChatType>,
    /// The accounts in it, this one included.
    member_list: Vec<Ulid>,
    /// The last sequence id handled with nothing missing below it, once known.
    known: Option<u64>,
    /// The server time of `known`'s message, where a `sync` of this chat alone starts.
    known_at: Option<i64>,
    oldest_seq: Option<u64>,
    busy: Busy,
    /// Sync, then match what was in flight before resending it.
    verify: bool,
    /// Live frames kept while busy, handled in order after.
    buffer: Vec<MessageFrame>,
    /// Messages waiting for their epoch or for the chat to be joined.
    waiting: Vec<MessageFrame>,
    /// Messages waiting for their device's key, oldest first.
    unknown: Vec<MessageFrame>,
    /// Call text waiting for its device's key, the same way.
    unknown_calls: Vec<CallEvent>,
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
    /// The last chat list the server sent, previews included, kept in the snapshot's shelf.
    chat_cache: Vec<ChatItem>,
    /// Ending statements waiting for their account's master key.
    revocations: Vec<(Ulid, Ulid, Vec<u8>)>,
    uploads: HashMap<u64, Upload>,
    next_upload: u64,
    downloads: HashMap<u64, Download>,
    call: Option<ActiveCall>,
    /// Call events already shown, since one can come over the WebSocket and on the call's track.
    call_events_shown: HashSet<(Ulid, u64, u64)>,
    /// Each call's outcome and end time, from its `end` row, by chat and call.
    call_ends: HashMap<(Ulid, u64), (CallOutcome, i64)>,
    /// Where the next `sync` starts, as the device's snapshot keeps it.
    since: Option<i64>,
    sync_run: Option<SyncRun>,
    /// `sync`s asked for while one ran, by chat or `None` for every chat.
    sync_next: Vec<Option<Ulid>>,
    /// Counts connections, so a page kept after its connection closed asks for nothing more.
    sync_generation: u64,
    /// Chats the list said were at a newer epoch than this device, checked once a `sync` ends.
    behind: Vec<(Ulid, u64)>,
    rows_saving: HashMap<u64, RowsSave>,
    /// Archive reads under way: the chat, and whether it is older messages.
    reading: HashMap<u64, Ulid>,
    /// This account's devices, as handed to the platform.
    own_devices: Vec<Ulid>,
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
            | Asked::Stickers
            | Asked::StickerImage { .. }
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

/// What a message holds once opened.
#[derive(Default)]
struct Opened {
    text: Option<Secret>,
    files: Vec<FileInfo>,
    call: Option<CallBody>,
    reply: Option<u64>,
    sticker: Option<Ulid>,
    question: Option<Question>,
    answer: Option<Answer>,
}

/// A message's text and files, `{"text": "...", "files": [...]}`, each optional, a call's start,
/// `{"video": ..., "voice": ...}`, a sticker, `{"sticker": "..."}`, a question's tree, `{"text"?,
/// "first", "nodes"}`, or an answer, `{"question", "answers", "skipped"?}`. A tree that does not hold
/// together is left out.
fn content_of(plaintext: &[u8]) -> Opened {
    #[derive(Deserialize)]
    struct Plain {
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        files: Vec<FileInfo>,
        #[serde(default)]
        video: Option<bool>,
        #[serde(default)]
        voice: Option<bool>,
        #[serde(default)]
        reply: Option<u64>,
        #[serde(default)]
        sticker: Option<Ulid>,
        #[serde(default)]
        first: Option<String>,
        #[serde(default)]
        nodes: Option<Vec<crate::protocol::QuestionNode>>,
        #[serde(default)]
        question: Option<u64>,
        #[serde(default)]
        answers: Vec<crate::protocol::AnswerItem>,
        #[serde(default)]
        skipped: bool,
    }
    match serde_json::from_slice::<Plain>(plaintext) {
        Ok(plain) => {
            let question = match (plain.first, plain.nodes) {
                (Some(first), Some(nodes)) => {
                    let question = Question { first, nodes };
                    match question.check() {
                        Ok(()) => Some(question),
                        Err(_) => None,
                    }
                }
                _ => None,
            };
            let answer = match plain.question {
                Some(seq) => Some(Answer {
                    question: seq,
                    answers: plain.answers,
                    skipped: plain.skipped,
                }),
                None => None,
            };
            Opened {
                reply: plain.reply,
                text: plain.text.map(Secret),
                files: plain.files,
                call: match (plain.video, plain.voice) {
                    (Some(video), Some(voice)) => Some(CallBody { video, voice }),
                    _ => None,
                },
                sticker: plain.sticker,
                question,
                answer,
            }
        }
        Err(_) => Opened::default(),
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

/// `end` is the call's outcome and end time, for a call's message whose `end` row this client has.
fn shown(
    message: &MessageFrame,
    account: Option<Ulid>,
    mine: bool,
    opened: Opened,
    end: Option<(CallOutcome, i64)>,
) -> Shown {
    let Opened {
        text,
        files,
        call,
        reply,
        sticker,
        question,
        answer,
    } = opened;
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
        reply,
        files,
        call,
        call_outcome: end.map(|(outcome, _)| outcome),
        call_ended_at: end.map(|(_, at)| at),
        sticker,
        question,
        answer,
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
            chat_cache: Vec::new(),
            revocations: Vec::new(),
            uploads: HashMap::new(),
            next_upload: 0,
            downloads: HashMap::new(),
            call: None,
            call_events_shown: HashSet::new(),
            call_ends: HashMap::new(),
            since: None,
            sync_run: None,
            sync_next: Vec::new(),
            sync_generation: 0,
            behind: Vec::new(),
            rows_saving: HashMap::new(),
            reading: HashMap::new(),
            own_devices: Vec::new(),
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
            Input::ChunkRead { handle, n, bytes } => self.chunk_read(handle, n, bytes),
            Input::Fetched { id, status, bytes } => self.fetched(id, status, &bytes),
            Input::CallEvent { event } => self.call_row(event, true),
            Input::RowsSaved { id } => self.rows_saved(id),
            Input::RowsSaveFailed { id } => self.rows_save_failed(id),
            Input::RowsRead {
                id,
                message_list,
                call_list,
            } => self.rows_read(id, message_list, call_list),
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
        self.rekey_call();
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
            bytes: None,
            raw: false,
            asked,
            attempt: 0,
        });
    }

    fn issue(&mut self, request: Request) {
        let id = self.next_id;
        self.next_id += 1;
        let bearer = self.bearer();
        let path = request.path.clone();
        self.out.push(match (&request.bytes, request.raw) {
            (Some(bytes), _) => Output::PutChunk {
                id,
                path,
                bearer,
                bytes: bytes.clone(),
            },
            (None, true) => Output::GetChunk { id, path, bearer },
            (None, false) => Output::Request {
                id,
                method: request.method.clone(),
                path,
                body: request.body.clone(),
                bearer,
            },
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
        let shelf = Shelf {
            username: self.username.clone(),
            chat_list: self.chat_cache.clone(),
            account_list: self.accounts.values().cloned().collect(),
            device_list: self
                .devices
                .iter()
                .map(|(device, known)| ShelfDevice {
                    device: *device,
                    account: known.account,
                    public: Bytes(known.public.to_vec()),
                    master: Bytes(known.master.to_vec()),
                })
                .collect(),
        };
        let Some(device) = self.device.as_mut() else {
            return;
        };
        if let Ok(bytes) = serde_json::to_vec(&shelf) {
            device.set_shelf(bytes);
        }
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
            Command::ChatList => self.start_sync(None, self.since),
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
            Command::SendText { chat, text, reply } => self.push_content(
                chat,
                Kind::Content,
                reply,
                Content {
                    content_kind: Some(ContentKind::Text),
                    text: Some(text),
                    ..Content::default()
                },
            ),
            Command::SendFiles { chat, files, text } => self.send_files(chat, files, text),
            Command::Download { file, handle } => self.download(file, handle),
            Command::Edit {
                chat,
                target,
                text,
                files,
                remove,
            } => self.push_content(
                chat,
                Kind::Edit,
                Some(target),
                Content {
                    text: Some(text),
                    files,
                    remove,
                    ..Content::default()
                },
            ),
            Command::Delete {
                chat,
                target,
                remove,
            } => self.push_content(
                chat,
                Kind::Delete,
                Some(target),
                Content {
                    remove,
                    ..Content::default()
                },
            ),
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
            Command::Draft { chat, draft } => self.keep_draft(chat, draft),
            Command::Times { chat, seq } => {
                let reference = reference();
                let frame = ClientFrame::Times {
                    reference,
                    chat,
                    seq,
                };
                if !self.ask(frame, reference, Waiting::Done) {
                    self.error("offline", "Not connected, so the times could not be read");
                }
            }
            Command::Devices => self.get("/api/v1/devices".to_string(), Asked::Devices),
            Command::SendSticker {
                chat,
                sticker,
                reply,
            } => {
                let fields = serde_json::json!({ "sticker": sticker }).to_string();
                self.push_content(
                    chat,
                    Kind::Content,
                    reply,
                    Content {
                        content_kind: Some(ContentKind::Sticker),
                        fields: Some(Secret(fields)),
                        sticker: Some(sticker),
                        ..Content::default()
                    },
                );
            }
            Command::SendQuestion {
                chat,
                text,
                question,
            } => {
                if let Err(error) = question.check() {
                    return self.error("bad_question", &error.to_string());
                }
                let fields = match serde_json::to_string(&question) {
                    Ok(fields) => fields,
                    Err(_) => return self.error("internal", "Could not write the question"),
                };
                self.push_content(
                    chat,
                    Kind::Content,
                    None,
                    Content {
                        content_kind: Some(ContentKind::Question),
                        text,
                        fields: Some(Secret(fields)),
                        ..Content::default()
                    },
                );
            }
            Command::SendAnswer {
                chat,
                question,
                answer,
            } => self.answer(chat, None, &question, &answer),
            Command::EditAnswer {
                chat,
                target,
                question,
                answer,
            } => self.answer(chat, Some(target), &question, &answer),
            Command::Stickers => self.get("/api/v1/stickers".to_string(), Asked::Stickers),
            Command::StickerImage { sticker } => self.issue(Request {
                method: "GET".to_string(),
                path: format!("/api/v1/stickers/{sticker}"),
                body: None,
                bytes: None,
                raw: true,
                asked: Asked::StickerImage { sticker },
                attempt: 0,
            }),
            Command::StartCall { chat, video, voice } => self.push_content(
                chat,
                Kind::Content,
                None,
                Content {
                    content_kind: Some(ContentKind::Call),
                    call: Some(CallBody { video, voice }),
                    ..Content::default()
                },
            ),
            Command::DeclineCall { chat, call_seq } => {
                let reference = reference();
                let frame = ClientFrame::CallEvent {
                    reference,
                    chat,
                    call_seq,
                    kind: CallEventKind::Decline,
                };
                if !self.ask(frame, reference, Waiting::Done) {
                    self.error("offline", "Not connected, so the call was not declined");
                }
            }
            Command::JoinCall { chat, call_seq } => self.join_call(chat, call_seq),
            Command::LeaveCall { chat } => {
                if self.call.as_ref().is_some_and(|call| call.chat == chat) {
                    self.call = None;
                    self.out.push(Output::LeaveCall { chat });
                }
            }
            Command::CallText {
                chat,
                call_seq,
                text,
            } => self.call_text(chat, call_seq, text),
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

    /// Reads the chat's newest messages from the archive, below `before` if given.
    fn read_rows(&mut self, chat: Ulid, before: Option<u64>) {
        let id = self.next_id;
        self.next_id += 1;
        self.reading.insert(id, chat);
        self.out.push(Output::ReadRows {
            id,
            chat,
            before_seq: before,
            limit: PAGE,
        });
    }

    fn open_chat(&mut self, chat: Ulid) {
        self.chat(chat).oldest_seq = None;
        self.read_rows(chat, None);
    }

    fn load_older(&mut self, chat: Ulid) {
        let before = self.chat(chat).oldest_seq;
        if matches!(before, Some(before) if before <= 1) {
            return self.event(Event::PageLoaded {
                chat,
                has_older: false,
            });
        }
        self.read_rows(chat, before);
    }

    /// The archive's answer to `read_rows`: call rows first, so their messages show their ends.
    fn rows_read(&mut self, id: u64, message_list: Vec<MessageFrame>, call_list: Vec<CallEvent>) {
        let Some(chat) = self.reading.remove(&id) else {
            return;
        };
        for event in call_list {
            self.call_row(event, false);
        }
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

    /// An answer, or an edit of one when `target` is set, once it follows its question.
    fn answer(
        &mut self,
        chat: Ulid,
        target: Option<Pointer>,
        question: &Question,
        answer: &Answer,
    ) {
        if let Err(error) = question.check_answer(answer) {
            return self.error("bad_answer", &error.to_string());
        }
        let fields = match serde_json::to_string(answer) {
            Ok(fields) => fields,
            Err(_) => return self.error("internal", "Could not write the answer"),
        };
        let (kind, content_kind) = match target {
            Some(_) => (Kind::Edit, None),
            None => (Kind::Content, Some(ContentKind::Answer)),
        };
        self.push_content(
            chat,
            kind,
            target,
            Content {
                content_kind,
                fields: Some(Secret(fields)),
                answer: Some(answer.clone()),
                ..Content::default()
            },
        );
    }

    fn push_content(&mut self, chat: Ulid, kind: Kind, target: Option<Pointer>, content: Content) {
        if self.device.is_none() {
            return self.error("no_device", "This client has no device");
        }
        let queue_target = match target {
            Some(Pointer::Seq(seq)) => Target::Seq(seq),
            Some(Pointer::Local(local)) => Target::Pending(local),
            None => Target::None,
        };
        let Content {
            content_kind,
            text,
            files,
            remove,
            call,
            fields,
            handles,
            sticker,
            answer,
        } = content;
        let item = Item::Content {
            chat,
            kind,
            content_kind,
            text: text.clone(),
            files,
            remove,
            call,
            fields,
        };
        let local = self.queue.push(Level::Content, item, queue_target);
        self.event(Event::Pending {
            chat,
            local,
            kind,
            target,
            content_kind,
            text,
            handles,
            sticker,
            answer,
        });
    }

    // ---------------------------------------------------------------------------------------------
    // Files
    // ---------------------------------------------------------------------------------------------

    /// One file, or a bundle of images and videos. Each file gets its own key, and goes up before
    /// the message joins the queue, so text sent meanwhile is not held behind it.
    fn send_files(&mut self, chat: Ulid, specs: Vec<FileSpec>, text: Option<Secret>) {
        if self.device.is_none() {
            return self.error("no_device", "This client has no device");
        }
        let bundle = specs.len() > 1;
        let fits = match specs.as_slice() {
            [] => false,
            [one] => matches!(
                one.kind,
                ContentKind::Voice
                    | ContentKind::Image
                    | ContentKind::Music
                    | ContentKind::Video
                    | ContentKind::Doc
                    | ContentKind::File
            ),
            many => {
                many.len() <= file::BUNDLE_FILES
                    && many
                        .iter()
                        .all(|spec| matches!(spec.kind, ContentKind::Image | ContentKind::Video))
            }
        };
        if !fits {
            return self.error(
                "bad_files",
                "A message holds one file, or up to 20 images and videos",
            );
        }
        if specs
            .iter()
            .any(|spec| spec.kind == ContentKind::Music && spec.mime != file::MUSIC_TYPE)
        {
            return self.error("bad_files", "Music is an MP3 only, of type audio/mpeg");
        }
        let content_kind = match specs.first() {
            Some(spec) if !bundle => spec.kind,
            _ => ContentKind::MediaBundle,
        };
        let mut files = Vec::with_capacity(specs.len());
        for spec in specs {
            let key = match file::new_key() {
                Ok(key) => key,
                Err(error) => return self.error("internal", &error.to_string()),
            };
            let blurhash = match &spec.thumbnail {
                Some(thumbnail) => {
                    file::blurhash_of(thumbnail.width, thumbnail.height, &thumbnail.rgba).ok()
                }
                None => None,
            };
            let music = spec.kind == ContentKind::Music;
            files.push(UpFile {
                handle: spec.handle,
                info: FileInfo {
                    id: Ulid::from_bytes([0; 16]),
                    kind: spec.kind,
                    name: spec.name,
                    mime: spec.mime,
                    size: spec.size,
                    key: Bytes(key.to_vec()),
                    chunks: file::chunks_for(spec.size),
                    width: spec.width,
                    height: spec.height,
                    blurhash,
                    description: spec.description,
                    title: spec.title.filter(|_| music),
                    artist: spec.artist.filter(|_| music),
                    duration: spec.duration.filter(|_| music),
                },
                key,
                next: 0,
            });
        }
        let upload = self.next_upload;
        self.next_upload += 1;
        self.uploads.insert(
            upload,
            Upload {
                chat,
                content_kind,
                text,
                files,
                current: 0,
            },
        );
        self.create_file(upload);
    }

    /// Asks the server for the current file's id.
    fn create_file(&mut self, upload: u64) {
        let Some(going) = self.uploads.get(&upload) else {
            return;
        };
        let Some(current) = going.files.get(going.current) else {
            return;
        };
        let body = CreateFileBody {
            kind: current.info.kind,
            size: file::sealed_size(current.info.size),
            chunks: current.info.chunks,
        };
        let path = format!("/api/v1/chats/{}/files", going.chat);
        self.request("POST", path, Some(&body), Asked::CreateFile { upload });
    }

    fn file_created(&mut self, upload: u64, id: Ulid) {
        let Some(going) = self.uploads.get_mut(&upload) else {
            return;
        };
        let Some(current) = going.files.get_mut(going.current) else {
            return;
        };
        current.info.id = id;
        let (handle, total) = (current.handle, current.info.chunks);
        self.event(Event::Progress {
            handle,
            done: 0,
            total,
        });
        self.read_next(upload);
    }

    /// Asks the platform for the current file's next chunk.
    fn read_next(&mut self, upload: u64) {
        let Some(going) = self.uploads.get(&upload) else {
            return;
        };
        let Some(current) = going.files.get(going.current) else {
            return;
        };
        let n = current.next;
        self.out.push(Output::ReadChunk {
            handle: current.handle,
            n,
            offset: n * file::CHUNK,
            length: file::chunk_plain(current.info.size, n),
        });
    }

    /// The platform read a chunk: it is sealed and put.
    fn chunk_read(&mut self, handle: u64, n: u64, bytes: Vec<u8>) {
        let bytes = zeroize::Zeroizing::new(bytes);
        let mut found = None;
        for (upload, going) in &self.uploads {
            if let Some(current) = going.files.get(going.current)
                && current.handle == handle
                && current.next == n
            {
                found = Some(*upload);
            }
        }
        let Some(upload) = found else {
            return;
        };
        let Some(going) = self.uploads.get(&upload) else {
            return;
        };
        let Some(current) = going.files.get(going.current) else {
            return;
        };
        if bytes.len() as u64 != file::chunk_plain(current.info.size, n) {
            return self.upload_stopped(upload, "read_failed");
        }
        let last = n + 1 == current.info.chunks;
        let sealed = match file::seal_chunk(current.key.as_ref(), n, last, &bytes) {
            Ok(sealed) => sealed,
            Err(_) => return self.upload_stopped(upload, "seal_failed"),
        };
        let path = format!("/api/v1/files/{}/chunks/{n}", current.info.id);
        self.issue(Request {
            method: "PUT".to_string(),
            path,
            body: None,
            bytes: Some(sealed),
            raw: false,
            asked: Asked::PutChunk { upload, n },
            attempt: 0,
        });
    }

    /// A chunk is up: the next one, the next file, or once all are up, the message.
    fn chunk_put(&mut self, upload: u64, n: u64) {
        let Some(going) = self.uploads.get_mut(&upload) else {
            return;
        };
        let Some(current) = going.files.get_mut(going.current) else {
            return;
        };
        current.next = n + 1;
        let (handle, total, done) = (current.handle, current.info.chunks, current.next);
        if done < total {
            self.event(Event::Progress {
                handle,
                done,
                total,
            });
            return self.read_next(upload);
        }
        going.current += 1;
        let more = going.current < going.files.len();
        self.event(Event::Progress {
            handle,
            done,
            total,
        });
        if more {
            return self.create_file(upload);
        }
        let Some(going) = self.uploads.remove(&upload) else {
            return;
        };
        let handles = going.files.iter().map(|up| up.handle).collect();
        let files = going.files.into_iter().map(|up| up.info).collect();
        self.push_content(
            going.chat,
            Kind::Content,
            None,
            Content {
                content_kind: Some(going.content_kind),
                text: going.text,
                files,
                remove: Vec::new(),
                call: None,
                fields: None,
                handles,
                sticker: None,
                answer: None,
            },
        );
    }

    /// An upload stopped for good: every file of its message is told, and the message is not sent.
    fn upload_stopped(&mut self, upload: u64, code: &str) {
        let Some(going) = self.uploads.remove(&upload) else {
            return;
        };
        for up in going.files {
            self.event(Event::TransferFailed {
                handle: up.handle,
                code: code.to_string(),
            });
        }
    }

    fn download(&mut self, info: FileInfo, handle: u64) {
        let total = info.chunks;
        self.downloads.insert(handle, Download { info });
        self.event(Event::Progress {
            handle,
            done: 0,
            total,
        });
        self.fetch(handle, 0);
    }

    fn fetch(&mut self, handle: u64, n: u64) {
        let Some(coming) = self.downloads.get(&handle) else {
            return;
        };
        let path = format!("/api/v1/files/{}/chunks/{n}", coming.info.id);
        self.issue(Request {
            method: "GET".to_string(),
            path,
            body: None,
            bytes: None,
            raw: true,
            asked: Asked::GetChunk { handle, n },
            attempt: 0,
        });
    }

    /// A chunk came down: it is opened, handed to the platform to write, and the next is fetched.
    fn chunk_got(&mut self, handle: u64, n: u64, sealed: &[u8]) {
        let Some(coming) = self.downloads.get(&handle) else {
            return;
        };
        let total = coming.info.chunks;
        let last = n + 1 == total;
        let plain = match file::open_chunk(&coming.info.key.0, n, last, sealed) {
            Ok(plain) => plain,
            Err(_) => return self.download_failed(handle, "bad_file"),
        };
        self.out.push(Output::WriteChunk {
            handle,
            n,
            offset: n * file::CHUNK,
            bytes: plain.to_vec(),
        });
        self.event(Event::Progress {
            handle,
            done: n + 1,
            total,
        });
        if last {
            self.downloads.remove(&handle);
            return self.event(Event::Downloaded { handle });
        }
        self.fetch(handle, n + 1);
    }

    fn download_failed(&mut self, handle: u64, code: &str) {
        if self.downloads.remove(&handle).is_some() {
            self.event(Event::TransferFailed {
                handle,
                code: code.to_string(),
            });
        }
    }

    fn fetched(&mut self, id: u64, status: u16, bytes: &[u8]) {
        let Some(request) = self.asked.get(&id) else {
            return;
        };
        if let Asked::GetChunk { handle, n } = request.asked
            && (200..300).contains(&status)
        {
            self.asked.remove(&id);
            return self.chunk_got(handle, n, bytes);
        }
        if let Asked::StickerImage { sticker } = request.asked {
            if (200..300).contains(&status) {
                self.asked.remove(&id);
                return self.event(Event::StickerImage {
                    sticker,
                    bytes: Bytes(bytes.to_vec()),
                });
            }
            // Removed from the server, so its messages show it as unavailable.
            if status == 404 {
                self.asked.remove(&id);
                return self.event(Event::StickerMissing { sticker });
            }
        }
        self.replied(id, status, &String::from_utf8_lossy(bytes));
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
        let body = KeyPackagesBody {
            body_list: Vec::new(),
        };
        self.request(
            "POST",
            "/api/v1/key_packages".to_string(),
            Some(&body),
            Asked::KeyPackageCount,
        );
        // What went out before the connection dropped is looked for before anything new goes, in
        // one `sync` from where it started when the oldest of it went.
        let mut since = self.since;
        let mut verify_list: Vec<Ulid> = Vec::new();
        for flight in self.in_flight.values() {
            since = match (since, flight.since) {
                (Some(since), Some(flight)) => Some(since.min(flight)),
                _ => None,
            };
            if !verify_list.contains(&flight.chat) {
                verify_list.push(flight.chat);
            }
        }
        for chat in verify_list {
            self.chat(chat).verify = true;
            self.verifying += 1;
        }
        self.sync_next.clear();
        self.start_sync(None, since);
        let mut chat_list: Vec<Ulid> = self.chats.keys().copied().collect();
        chat_list.sort();
        for chat in chat_list {
            self.send_outs(chat);
            self.report(chat);
        }
    }

    fn closed(&mut self, code: u16, reason: &str) {
        let was_up = self.socket == Socket::Up;
        self.socket = Socket::Down;
        self.verifying = 0;
        self.ping = None;
        self.waiting.clear();
        // A `sync` starts again on the next connection, so what it kept goes, and a page still being
        // kept asks for nothing more.
        self.sync_run = None;
        self.sync_next.clear();
        self.sync_generation += 1;
        self.behind.clear();
        for state in self.chats.values_mut() {
            state.busy = Busy::Idle;
            state.buffer.clear();
            state.verify = false;
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
        self.uploads.clear();
        self.downloads.clear();
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
                self.since = imported.device.since();
                self.device = Some(imported.device);
                if self.auth == Auth::Token && self.token.is_none() {
                    return self.show_login();
                }
                self.unshelve();
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

    /// Shows what the last run knew, before any network: the person's name, the chat list with its
    /// previews, and the devices and names that open the archive's messages. The server's own list
    /// replaces it as soon as it answers. A shelf that cannot be read is ignored.
    fn unshelve(&mut self) {
        let Some(device) = self.device.as_ref() else {
            return;
        };
        let Ok(shelf) = serde_json::from_slice::<Shelf>(device.shelf()) else {
            return;
        };
        let (Some(me), Some(own)) = (
            self.account,
            self.device.as_ref().map(|d| d.record().device),
        ) else {
            return;
        };
        for info in shelf.account_list {
            self.event(Event::Account {
                account: AccountBrief {
                    id: info.id,
                    username: info.username.clone(),
                    display_name: info.display_name.clone(),
                },
            });
            self.accounts.entry(info.id).or_insert(info);
        }
        for item in shelf.device_list {
            if let (Some(public), Some(master)) =
                (fixed::<32>(&item.public.0), fixed::<32>(&item.master.0))
            {
                self.devices.entry(item.device).or_insert(KnownDevice {
                    account: item.account,
                    public,
                    master,
                });
            }
        }
        if self.username.is_none()
            && let Some(username) = shelf.username
        {
            self.username = Some(username.clone());
            self.event(Event::SignedIn {
                account: me,
                username,
                device: Some(own),
            });
        }
        self.chat_cache = shelf.chat_list.clone();
        let mut shown = Vec::with_capacity(shelf.chat_list.len());
        let mut preview_list = Vec::new();
        for mut item in shelf.chat_list {
            let state = self.chats.entry(item.chat).or_default();
            state.chat_type = Some(item.chat_type);
            let mut member_list = vec![me];
            for mark in &item.mark_list {
                if !member_list.contains(&mark.account) {
                    member_list.push(mark.account);
                }
            }
            if state.member_list.is_empty() || member_list.len() > 1 {
                state.member_list = member_list;
            }
            if let Some(message) = item.last_content.take() {
                preview_list.push(*message);
            }
            shown.push(item);
        }
        self.event(Event::Chats { chat_list: shown });
        for message in preview_list {
            let chat = message.chat;
            self.open_list(chat, vec![message], false, false);
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
                self.in_flight.insert(
                    local,
                    Flight {
                        chat,
                        frame: (*frame).clone(),
                        plan: Some(plan.clone()),
                        since: self.since,
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
                Item::Content {
                    chat,
                    kind,
                    content_kind,
                    text,
                    files,
                    remove,
                    call,
                    fields,
                } => match self.can_seal(chat) {
                    Seal::Now => {
                        let content = Content {
                            content_kind,
                            text,
                            files,
                            remove,
                            call,
                            fields,
                            handles: Vec::new(),
                            sticker: None,
                            answer: None,
                        };
                        self.send_content(outgoing.id, chat, kind, outgoing.pointer_seq, content);
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
        content: Content,
    ) {
        let Content {
            content_kind,
            text,
            files,
            remove,
            call,
            fields,
            ..
        } = content;
        let Some(device) = self.device.as_ref() else {
            return;
        };
        let epoch = match device.epoch(chat) {
            Ok(epoch) => epoch,
            Err(_) => return self.fail(chat, local, "not_joined"),
        };
        let content_kind = if kind == Kind::Content {
            content_kind.or(Some(ContentKind::Text))
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
            // A reply's target is inside the encryption: the server must not hold the reply graph.
            pointer_seq: if kind == Kind::Content {
                None
            } else {
                pointer_seq
            },
            call_seq: None,
            client_sent_at: self.now,
        };
        let persist = if kind == Kind::Content {
            files.iter().map(|info| info.id).collect()
        } else {
            Vec::new()
        };
        let sealed = match (kind, text) {
            (Kind::Content, _) if call.is_some() => {
                let plaintext = match serde_json::to_vec(&call) {
                    Ok(plaintext) => plaintext,
                    Err(_) => return self.fail(chat, local, "internal"),
                };
                device.seal(&header, &plaintext)
            }
            (Kind::Content | Kind::Edit, text) => {
                let mut plain = serde_json::Map::new();
                if let (Kind::Content, Some(reply)) = (kind, pointer_seq) {
                    plain.insert("reply".to_string(), serde_json::Value::from(reply));
                }
                if let Some(mut text) = text {
                    let text = std::mem::take(&mut text.0);
                    plain.insert("text".to_string(), serde_json::Value::String(text));
                }
                if !files.is_empty() {
                    match serde_json::to_value(&files) {
                        Ok(files) => {
                            plain.insert("files".to_string(), files);
                        }
                        Err(_) => return self.fail(chat, local, "internal"),
                    }
                }
                if let Some(mut fields) = fields {
                    let parsed = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(
                        &fields.0,
                    );
                    fields.0.zeroize();
                    match parsed {
                        Ok(more) => {
                            for (key, value) in more {
                                plain.insert(key, value);
                            }
                        }
                        Err(_) => return self.fail(chat, local, "internal"),
                    }
                }
                let plaintext = match serde_json::to_vec(&plain) {
                    Ok(plaintext) => zeroize::Zeroizing::new(plaintext),
                    Err(_) => return self.fail(chat, local, "internal"),
                };
                device.seal(&header, &plaintext)
            }
            _ => match device.sign(&header, b"") {
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
            pointer_seq: header.pointer_seq,
            client_sent_at: self.now,
            body: Bytes(body),
            signature: Bytes(signature.to_vec()),
            wrap_list: Vec::new(),
            welcome_list: Vec::new(),
            group_info: None,
            persist,
            remove,
        };
        self.in_flight.insert(
            local,
            Flight {
                chat,
                frame: frame.clone(),
                plan: None,
                since: self.since,
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
    // Calls
    // ---------------------------------------------------------------------------------------------

    /// Hands the platform's call code where to connect and the current key. Leaves any other call
    /// first, as there is one at a time.
    fn join_call(&mut self, chat: Ulid, call_seq: u64) {
        let Some(device) = self.device.as_ref() else {
            return self.error("no_device", "This client has no device");
        };
        let (key, index_list) = match device.call_key(chat) {
            Ok(found) => found,
            Err(_) => return self.error("not_joined", "This device is not in the chat yet"),
        };
        if let Some(call) = self.call.take() {
            self.out.push(Output::LeaveCall { chat: call.chat });
        }
        self.call = Some(ActiveCall {
            chat,
            call_seq,
            epoch: key.epoch,
        });
        let bearer = self.bearer();
        self.out.push(Output::JoinCall {
            chat,
            call_seq,
            path: format!("/api/v1/calls/{chat}/{call_seq}/moq"),
            bearer,
            key,
            index_list,
        });
    }

    /// A new epoch in the joined call's chat means a new key, for sealing and opening.
    fn rekey_call(&mut self) {
        let (Some(call), Some(device)) = (self.call.as_mut(), self.device.as_ref()) else {
            return;
        };
        let Ok(epoch) = device.epoch(call.chat) else {
            return;
        };
        if epoch == call.epoch {
            return;
        }
        let Ok((key, index_list)) = device.call_key(call.chat) else {
            return;
        };
        call.epoch = key.epoch;
        let chat = call.chat;
        self.out.push(Output::CallKey {
            chat,
            key,
            index_list,
        });
    }

    /// Seals text for the joined call, signed with `call_seq` in its header so it can never pass
    /// for a chat message.
    fn call_text(&mut self, chat: Ulid, call_seq: u64, mut text: Secret) {
        if !self
            .call
            .as_ref()
            .is_some_and(|call| call.chat == chat && call.call_seq == call_seq)
        {
            return self.error("not_in_call", "Join the call before writing in it");
        }
        let Some(device) = self.device.as_ref() else {
            return self.error("no_device", "This client has no device");
        };
        let Ok(epoch) = device.epoch(chat) else {
            return self.error("not_joined", "This device is not in the chat yet");
        };
        let header = call_text_header(chat, device.record().device, call_seq, epoch, self.now);
        let mut plain = serde_json::Map::new();
        plain.insert(
            "text".to_string(),
            serde_json::Value::String(std::mem::take(&mut text.0)),
        );
        let plaintext = match serde_json::to_vec(&plain) {
            Ok(plaintext) => zeroize::Zeroizing::new(plaintext),
            Err(_) => return self.error("internal", "Could not write the text"),
        };
        let (body, signature) = match device.seal(&header, &plaintext) {
            Ok(sealed) => sealed,
            Err(_) => return self.error("seal_failed", "Could not seal the text"),
        };
        self.out.push(Output::CallText {
            chat,
            call_seq,
            frame: CallText {
                client_at: header.client_sent_at,
                epoch,
                signature: Bytes(signature.to_vec()),
                body: Bytes(body),
            },
        });
    }

    /// Shows one thing that happened inside a call, once, opening text with the chat's keys. Text
    /// from a device not yet known waits for one fresh device list, as messages do.
    fn call_event(&mut self, event: CallEvent) {
        self.call_event_with(event, true);
    }

    fn call_event_with(&mut self, event: CallEvent, wait_device: bool) {
        if wait_device
            && event.kind == CallEventKind::Text
            && !self.devices.contains_key(&event.device)
            && !self
                .call_events_shown
                .contains(&(event.chat, event.call_seq, event.n))
        {
            let chat = event.chat;
            self.chat(chat).unknown_calls.push(event);
            return self.devices_of(chat);
        }
        if !self
            .call_events_shown
            .insert((event.chat, event.call_seq, event.n))
        {
            return;
        }
        let known = self.devices.get(&event.device).copied();
        let account = known.map(|known| known.account);
        let mut shown = ShownCallEvent {
            chat: event.chat,
            call_seq: event.call_seq,
            n: event.n,
            kind: event.kind,
            device: event.device,
            account,
            mine: account.is_some() && account == self.account,
            at: event.server_at,
            sent_at: event.client_at,
            outcome: event.outcome,
            text: None,
            problem: None,
        };
        if event.kind == CallEventKind::Text {
            let opened = self.open_call_text(&event, known);
            match opened {
                Ok(text) => shown.text = text,
                Err(problem) => shown.problem = Some(problem),
            }
        }
        self.event(Event::CallEvent { event: shown });
    }

    fn open_call_text(
        &mut self,
        event: &CallEvent,
        known: Option<KnownDevice>,
    ) -> Result<Option<Secret>, Problem> {
        let Some(known) = known else {
            self.devices_of(event.chat);
            return Err(Problem::UnknownDevice);
        };
        let signature = match &event.signature {
            Some(signature) => fixed::<64>(&signature.0),
            None => None,
        };
        let (Some(header), Some(signature), Some(body)) = (event.header(), signature, &event.body)
        else {
            return Err(Problem::Malformed);
        };
        if verify_message(&header, &body.0, &signature, &known.public).is_err() {
            return Err(Problem::BadSignature);
        }
        let Some(own) = self.device.as_ref() else {
            return Ok(None);
        };
        match own.open(&header, &body.0, &signature, &known.public) {
            Ok(plaintext) => Ok(content_of(&plaintext).text),
            Err(_) => Ok(None),
        }
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
                ..
            } => {
                if let Some(waiting) = self.waiting.remove(&reference) {
                    self.refused(waiting, &code, &message);
                }
            }
            ServerFrame::Message(message) => self.live(*message),
            ServerFrame::Chats {
                reference,
                chat_list,
                message_list,
                call_list,
                more,
            } => self.synced(reference, chat_list, message_list, call_list, more),
            ServerFrame::Welcome { chat, body } => self.welcome(chat, &body.0),
            ServerFrame::Typing { chat, account, on } => {
                self.event(Event::Typing { chat, account, on })
            }
            ServerFrame::Marks { chat, mark } => self.event(Event::Marks { chat, mark }),
            ServerFrame::Draft { chat, draft } => self.show_draft(chat, draft),
            ServerFrame::Times {
                reference,
                chat,
                seq,
                member_list,
            } => {
                self.waiting.remove(&reference);
                self.event(Event::Times {
                    chat,
                    seq,
                    member_list,
                });
            }
            ServerFrame::Revoked {
                account,
                device,
                statement,
            } => self.revoked(account, device, &statement.0),
            ServerFrame::CallEvent(event) => self.call_row(*event, true),
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

    /// What was sent got its sequence id, from the server's answer or found in a `sync` page.
    fn stored(&mut self, waiting: Waiting, seq: u64, saved_at: i64) {
        match waiting {
            Waiting::Content { chat, local } => {
                self.keep_own(local, seq, saved_at);
                self.queue.confirm(local, seq);
                self.advance(chat, seq, saved_at);
                self.event(Event::Sent {
                    chat,
                    local,
                    seq,
                    saved_at,
                });
            }
            Waiting::Commit { chat, local, plan } => {
                self.commit_stored(chat, local, plan, seq, saved_at)
            }
            _ => {}
        }
    }

    /// This device's own message, stored: the server never sends it back, so it is kept here as
    /// the row others receive.
    fn keep_own(&mut self, local: u64, seq: u64, saved_at: i64) {
        let Some(flight) = self.in_flight.remove(&local) else {
            return;
        };
        let device = self.device.as_ref().map(|device| device.record().device);
        let frame = flight.frame;
        let row = MessageFrame {
            chat: frame.chat,
            seq,
            device,
            kind: Some(frame.kind),
            content_kind: frame.content_kind,
            encrypted: Some(frame.encrypted),
            epoch: frame.epoch,
            pointer_seq: frame.pointer_seq,
            client_sent_at: Some(frame.client_sent_at),
            server_saved_at: Some(saved_at),
            body: Some(frame.body),
            signature: Some(frame.signature),
        };
        let last = SyncPosition {
            at: saved_at,
            chat: frame.chat,
            seq,
            n: 0,
        };
        self.save_rows(vec![row], Vec::new(), Some(last), None);
    }

    /// This client's own message took `seq`: it is handled, or shows that something was missed.
    fn advance(&mut self, chat: Ulid, seq: u64, saved_at: i64) {
        let state = self.chat(chat);
        match state.known {
            None => {
                state.known = Some(seq);
                state.known_at = Some(saved_at);
            }
            Some(known) if seq == known + 1 => {
                state.known = Some(seq);
                state.known_at = Some(saved_at);
            }
            Some(known) if seq > known + 1 => {
                let since = state.known_at.or(self.since);
                self.start_sync(Some(chat), since);
            }
            Some(_) => {}
        }
    }

    fn commit_stored(&mut self, chat: Ulid, local: u64, plan: Plan, seq: u64, saved_at: i64) {
        self.keep_own(local, seq, saved_at);
        self.queue.confirm(local, seq);
        self.commit_busy = false;
        self.advance(chat, seq, saved_at);
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
                        // The winning commit is fetched, then this plan runs again on top of it.
                        self.chat(chat).retry = Some(plan);
                        let since = self.chat(chat).known_at.or(self.since);
                        self.start_sync(Some(chat), since);
                    }
                }
            }
            Waiting::Sync { chat } => {
                if let Some(chat) = chat {
                    let state = self.chat(chat);
                    state.retry = None;
                    state.rotating = false;
                    self.fail_chat(chat, code);
                }
                self.error(code, message);
                self.sync_ended();
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
            Waiting::Done => self.error(code, message),
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

    /// Seals a draft and sends it, or clears it. A draft is best effort, so nothing waits for an
    /// answer, and a chat this device cannot seal for keeps no draft on the server.
    fn keep_draft(&mut self, chat: Ulid, draft: Option<Draft>) {
        let body = match draft {
            None => None,
            Some(draft) => {
                let Some(device) = self.device.as_ref() else {
                    return;
                };
                let mut plain = serde_json::Map::new();
                plain.insert(
                    "text".to_string(),
                    serde_json::Value::String(draft.text.0.clone()),
                );
                if let Some(reply) = draft.reply {
                    plain.insert("reply".to_string(), reply.into());
                }
                if let Some(edit) = draft.edit {
                    plain.insert("edit".to_string(), edit.into());
                }
                let Ok(plaintext) = serde_json::to_vec(&plain).map(zeroize::Zeroizing::new) else {
                    return;
                };
                let Ok((epoch, body)) = device.seal_draft(chat, &plaintext) else {
                    return;
                };
                Some(DraftBody {
                    epoch,
                    body: Bytes(body),
                })
            }
        };
        let reference = reference();
        let frame = ClientFrame::Draft {
            reference,
            chat,
            draft: body,
        };
        self.ask(frame, reference, Waiting::Done);
    }

    /// Opens a draft the server holds, for the platform to show. One this device cannot open, for
    /// want of the epoch's key, is not shown.
    fn show_draft(&mut self, chat: Ulid, item: Option<DraftItem>) {
        let Some(item) = item else {
            self.event(Event::Draft {
                chat,
                draft: None,
                saved_at: self.now,
            });
            return;
        };
        let Some(device) = self.device.as_ref() else {
            return;
        };
        let Ok(plaintext) = device.open_draft(chat, item.epoch, &item.body.0) else {
            return;
        };
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&plaintext) else {
            return;
        };
        let Some(text) = value.get("text").and_then(serde_json::Value::as_str) else {
            return;
        };
        let draft = Draft {
            text: Secret(text.to_string()),
            reply: value.get("reply").and_then(serde_json::Value::as_u64),
            edit: value.get("edit").and_then(serde_json::Value::as_u64),
        };
        self.event(Event::Draft {
            chat,
            draft: Some(draft),
            saved_at: item.saved_at,
        });
    }

    fn chats_listed(&mut self, chat_list: Vec<ChatItem>) {
        let Some(me) = self.account else {
            return;
        };
        self.chat_cache = chat_list.clone();
        self.dirty = true;
        let mut shown = Vec::with_capacity(chat_list.len());
        let mut preview_list = Vec::new();
        let mut draft_list = Vec::new();
        let mut join_list = Vec::new();
        for mut item in chat_list {
            let own_epoch = match self.device.as_ref() {
                Some(device) => device.epoch(item.chat).ok(),
                None => None,
            };
            let state = self.chats.entry(item.chat).or_default();
            state.chat_type = Some(item.chat_type);
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
            if let Some(draft) = item.draft.take() {
                draft_list.push((item.chat, draft));
            }
            match (own_epoch, item.epoch) {
                // The `sync` this list came with brings the commits; any still missing once it
                // ends mean this device rejoins.
                (Some(own), Some(epoch)) if epoch > own => self.behind.push((item.chat, epoch)),
                (Some(_), _) => {}
                (None, _) if !state.joining => join_list.push(item.chat),
                (None, _) => {}
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
        for (chat, draft) in draft_list {
            self.show_draft(chat, Some(draft));
        }
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
    }

    /// Asks for the messages and call rows saved after `since`, of every chat or only `chat`, unless
    /// a `sync` already runs, in which case this one follows it. Live frames of the chats it covers
    /// wait for it.
    fn start_sync(&mut self, chat: Option<Ulid>, since: Option<i64>) {
        if self.sync_run.is_some() {
            if !self.sync_next.contains(&chat) {
                self.sync_next.push(chat);
            }
            return;
        }
        let reference = reference();
        let frame = ClientFrame::Sync {
            reference,
            since,
            after: None,
            chat,
        };
        if !self.ask(frame, reference, Waiting::Sync { chat }) {
            return;
        }
        self.sync_run = Some(SyncRun { chat, first: true });
        match chat {
            Some(chat) => self.chat(chat).busy = Busy::Syncing,
            None => {
                for state in self.chats.values_mut() {
                    state.busy = Busy::Syncing;
                }
            }
        }
    }

    /// Whether a `sync` covering this chat runs.
    fn syncing(&self, chat: Ulid) -> bool {
        match &self.sync_run {
            Some(run) => run.chat.is_none_or(|only| only == chat),
            None => false,
        }
    }

    /// A `sync` page: its call rows are noted, its messages handled chat by chat in order, then all
    /// of it goes to the platform to keep, and the next page waits for that.
    fn synced(
        &mut self,
        reference: Ulid,
        chat_list: Vec<ChatItem>,
        message_list: Vec<MessageFrame>,
        call_list: Vec<CallEvent>,
        more: bool,
    ) {
        let Some(Waiting::Sync { chat: scope }) = self.waiting.remove(&reference) else {
            return;
        };
        let first = match self.sync_run.as_mut() {
            Some(run) => std::mem::replace(&mut run.first, false),
            None => return,
        };
        if first && scope.is_none() {
            self.chats_listed(chat_list);
            // Chats first heard of in the list wait for this `sync` too.
            for state in self.chats.values_mut() {
                state.busy = Busy::Syncing;
            }
        }
        let mut last: Option<SyncPosition> = None;
        for message in &message_list {
            let position = SyncPosition {
                at: message.server_saved_at.unwrap_or(0),
                chat: message.chat,
                seq: message.seq,
                n: 0,
            };
            last = last.max(Some(position));
        }
        for event in &call_list {
            let position = SyncPosition {
                at: event.server_at,
                chat: event.chat,
                seq: event.call_seq,
                n: event.n,
            };
            last = last.max(Some(position));
        }
        for event in &call_list {
            self.call_row(event.clone(), false);
        }
        let mut by_chat: Vec<(Ulid, Vec<MessageFrame>)> = Vec::new();
        for message in &message_list {
            match by_chat.iter_mut().find(|(chat, _)| *chat == message.chat) {
                Some((_, list)) => list.push(message.clone()),
                None => by_chat.push((message.chat, vec![message.clone()])),
            }
        }
        for (chat, mut list) in by_chat {
            list.sort_by_key(|message| message.seq);
            self.handle(chat, list);
        }
        if last.is_none() {
            return self.sync_ended();
        }
        let page = PageSaved {
            chat: scope,
            more,
            generation: self.sync_generation,
        };
        self.save_rows(message_list, call_list, last, Some(page));
    }

    /// Hands rows to the platform to keep.
    fn save_rows(
        &mut self,
        message_list: Vec<MessageFrame>,
        call_list: Vec<CallEvent>,
        last: Option<SyncPosition>,
        page: Option<PageSaved>,
    ) {
        let id = self.next_id;
        self.next_id += 1;
        self.rows_saving.insert(id, RowsSave { last, page });
        self.out.push(Output::SaveRows {
            id,
            message_list,
            call_list,
        });
    }

    /// The platform kept rows: `since` moves, and a `sync` page goes on to its next one or ends.
    fn rows_saved(&mut self, id: u64) {
        let Some(save) = self.rows_saving.remove(&id) else {
            return;
        };
        if let Some(last) = save.last
            && self.since.is_none_or(|since| since < last.at)
        {
            self.since = Some(last.at);
            if let Some(device) = self.device.as_mut() {
                device.set_since(last.at);
                self.dirty = true;
            }
        }
        let Some(page) = save.page else {
            return;
        };
        if page.generation != self.sync_generation || self.sync_run.is_none() {
            return;
        }
        if !page.more {
            return self.sync_ended();
        }
        let Some(after) = save.last else {
            return self.sync_ended();
        };
        let reference = reference();
        let frame = ClientFrame::Sync {
            reference,
            since: None,
            after: Some(after),
            chat: page.chat,
        };
        if !self.ask(frame, reference, Waiting::Sync { chat: page.chat }) {
            self.sync_ended();
        }
    }

    /// The platform could not keep rows. `since` stays, so the next `sync` brings them again.
    fn rows_save_failed(&mut self, id: u64) {
        let Some(save) = self.rows_saving.remove(&id) else {
            return;
        };
        self.error(
            "archive_failed",
            "These messages could not be kept on this device",
        );
        if let Some(page) = save.page
            && page.generation == self.sync_generation
        {
            self.sync_ended();
        }
    }

    /// The `sync` ended: its chats settle, a chat still behind its listed epoch rejoins, and a
    /// `sync` asked for meanwhile starts.
    fn sync_ended(&mut self) {
        let Some(run) = self.sync_run.take() else {
            return;
        };
        let mut chat_list: Vec<Ulid> = match run.chat {
            Some(chat) => vec![chat],
            None => self.chats.keys().copied().collect(),
        };
        chat_list.sort();
        for chat in &chat_list {
            self.chat(*chat).busy = Busy::Idle;
        }
        for chat in chat_list {
            self.settle(chat);
            // Messages still waiting for a newer epoch than this device holds have no commit
            // left to bring it.
            let current = match self.device.as_ref() {
                Some(device) => device.epoch(chat).ok(),
                None => None,
            };
            if let Some(current) = current
                && self
                    .chat(chat)
                    .waiting
                    .iter()
                    .any(|message| message.epoch.unwrap_or(0) > current)
            {
                self.rejoin(chat);
            }
        }
        if run.chat.is_none() {
            for (chat, epoch) in std::mem::take(&mut self.behind) {
                let current = match self.device.as_ref() {
                    Some(device) => device.epoch(chat).ok(),
                    None => None,
                };
                if matches!(current, Some(current) if current < epoch) {
                    self.rejoin(chat);
                }
            }
        }
        if !self.sync_next.is_empty() {
            let next = self.sync_next.remove(0);
            let since = match next {
                Some(chat) => self.chat(chat).known_at.or(self.since),
                None => self.since,
            };
            self.start_sync(next, since);
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
    }

    /// A message pushed live. Handled and kept at once if it is the next one, and otherwise held
    /// while a `sync` of its chat brings what is missing below it.
    fn live(&mut self, message: MessageFrame) {
        let chat = message.chat;
        if self.syncing(chat) {
            self.chat(chat).buffer.push(message);
            return;
        }
        let state = self.chat(chat);
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
            let since = state.known_at.or(self.since);
            return self.start_sync(Some(chat), since);
        }
        let last = SyncPosition {
            at: message.server_saved_at.unwrap_or(0),
            chat,
            seq: message.seq,
            n: 0,
        };
        self.handle(chat, vec![message.clone()]);
        self.save_rows(vec![message], Vec::new(), Some(last), None);
    }

    /// A device whose stamp checked out: if it is this account's, the platform hears of it, so it
    /// counts unread without this account's own messages.
    fn own_device(&mut self, account: Ulid, device: Ulid) {
        if self.account == Some(account) && !self.own_devices.contains(&device) {
            self.own_devices.push(device);
            self.event(Event::OwnDevices {
                device_list: self.own_devices.clone(),
            });
        }
    }

    /// One row of a call: its end noted, shown once, and, when it came live, kept.
    fn call_row(&mut self, event: CallEvent, live: bool) {
        if event.kind == CallEventKind::End {
            let outcome = event.outcome.unwrap_or(CallOutcome::Missed);
            self.call_ends
                .insert((event.chat, event.call_seq), (outcome, event.server_at));
        }
        self.call_event(event.clone());
        if live {
            let last = SyncPosition {
                at: event.server_at,
                chat: event.chat,
                seq: event.call_seq,
                n: event.n,
            };
            self.save_rows(Vec::new(), vec![event], Some(last), None);
        }
    }

    /// Handles messages in order from the next one: commits applied, what was in flight matched,
    /// and the rest opened.
    fn handle(&mut self, chat: Ulid, message_list: Vec<MessageFrame>) {
        let mut opened = Vec::new();
        let mut changed = false;
        for message in message_list {
            if let Some(known) = self.chat(chat).known
                && message.seq <= known
            {
                continue;
            }
            let state = self.chat(chat);
            state.known = Some(message.seq);
            state.known_at = message.server_saved_at.or(state.known_at);
            if self.match_flight(chat, &message) {
                continue;
            }
            if message.kind == Some(Kind::Commit) {
                if let Applied::Changed = self.apply_commit(chat, &message) {
                    changed = true;
                }
            } else {
                opened.push(message);
            }
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
    fn apply_commit(&mut self, chat: Ulid, message: &MessageFrame) -> Applied {
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
        // Commits are missing below this one, though rows come in order: this device's state is
        // behind what it has kept, so it rejoins.
        if epoch > current + 1 {
            self.rejoin(chat);
            return Applied::Nothing;
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
            let content = if !header.encrypted {
                content_of(&body)
            } else {
                let Some(own) = self.device.as_ref() else {
                    return;
                };
                let current = own.epoch(chat).ok();
                match own.open(&header, &body, &signature, &known.public) {
                    Ok(plaintext) => content_of(&plaintext),
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
                    Err(_) => Opened::default(),
                }
            };
            let mine = self.account == Some(known.account);
            let end = self.call_ends.get(&(chat, message.seq)).copied();
            let shown = shown(&message, Some(known.account), mine, content, end);
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
            let since = self.chat(chat).known_at.or(self.since);
            self.start_sync(Some(chat), since);
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
        let mut shown = shown(message, account, mine, Opened::default(), None);
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
            Asked::CreateFile { upload } => {
                if let Some(FileReply { file }) = self.data(body) {
                    self.file_created(upload, file);
                }
            }
            Asked::PutChunk { upload, n } => self.chunk_put(upload, n),
            // A chunk's reply arrives as `Fetched`, so only a refusal's text reaches here.
            Asked::GetChunk { handle, .. } => self.download_failed(handle, "bad_reply"),
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
            Asked::Stickers => {
                if let Some(StickerListReply { pack_list }) = self.data(body) {
                    self.event(Event::Stickers { pack_list });
                }
            }
            // An image's reply arrives as `Fetched`, so only a refusal's text reaches here.
            Asked::StickerImage { .. } => self.error("bad_reply", "The sticker could not be read"),
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
                    name,
                }) = self.data(body)
                {
                    self.event(Event::Features {
                        registration,
                        captcha,
                        name,
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
        self.own_device(account, id);
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
        self.dirty = true;
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
            Asked::CreateFile { upload } | Asked::PutChunk { upload, .. } => {
                self.upload_stopped(upload, code)
            }
            Asked::GetChunk { handle, .. } => self.download_failed(handle, code),
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
                let calls = std::mem::take(&mut state.unknown_calls);
                self.open_list(chat, waiting, false, true);
                for event in calls {
                    self.call_event_with(event, false);
                }
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
            self.own_device(item.account, item.device);
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
        let calls = std::mem::take(&mut state.unknown_calls);
        self.open_list(chat, decided, false, true);
        self.open_list(chat, later, true, true);
        for event in calls {
            self.call_event_with(event, false);
        }
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
