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
use base64ct::{Base64UrlUnpadded, Encoding};

use crate::mls::{
    AccountKey, Device, DeviceRecord, InviteKey, MlsError, sign_revocation, stamp_account_key,
    stamp_invite_key, verify_logout, verify_message, verify_revocation,
};
use crate::protocol::{
    AccountBrief, AccountInfo, AccountKeysBody, AccountListReply, AccountReply, Ack, ActionKind,
    AgentClaims, AgentEndBody, AgentItem, AgentListReply, AgentLoginBody, AgentStatement,
    AgentTokenReply, Answer, Bytes, CallBody, CallEvent, CallEventKind, CallOutcome, CallText,
    CaptchaAnswer, CaptchaReply, ChallengeBody, ChallengeReply, ChatAction, ChatDevice,
    ChatDeviceListReply, ChatEvent, ChatFolders, ChatItem, ChatOptions, ChatProfileBody,
    ChatProfileItem, ChatProfilePlain, ChatReply, ChatRole, ChatType, ClaimBody, ClientFrame,
    CommitAction, ContentKind, CountReply, CreateAgentBody, CreateChatBody, CreateFileBody,
    CreatedAgentReply, DeviceProof, DeviceReply, DevicesReply, DraftBody, DraftItem, EmptyReply,
    FeaturesReply, FileReply, FindableBody, FindableItem, FolderItem, FolderSuggestion, FoundChat,
    GroupInfoReply, Header, InvitationItem, InvitationPlain, InvitationShown, InviteStatement,
    KeyPackageItem, KeyPackageListReply, KeyPackagesBody, Kind, KvChange, KvItem, LineAction,
    LineBody, LoginBody, LoginReply, MarkItem, MemberItem, MessageFrame, NamesBody, PhotoReply,
    PresenceItem, ProfileBody, ProfileKeyItem, ProfileKeyShare, ProfileLink, ProfilePhoto,
    ProfileReply, PutProfileBody, Question, RefusedReply, RegisterBody, RegisterDeviceBody,
    RequestItem, RequestListReply, RequestState, SelfChatReply, SendFrame, ServerFrame, SetupBody,
    SetupReply, SignedBody, SignedProfile, StickerListReply, StickerPack, Suggestion, SyncPosition,
    TimesItem, TopicItem, TopicName, TopicOptionsBody, TopicProfileItem, TopicProfilePlain,
    TopicsBody, UpdateAgentBody, VersionReply, WelcomeItem, WrapEntry, WrapItem, WrapListReply,
    call_text_header,
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
/// How many rows one read of the archive asks for while a search scans it.
const SCAN_ROWS: u32 = 200;
/// How many found messages a search hands over before it stops and waits for more to be asked,
/// chosen by the author.
const SEARCH_HITS: usize = 20;
/// A profile's bio, in characters, and its links, each name and address in characters. See the
/// root AGENTS.md.
const BIO_CHARS: usize = 140;
/// An agent's activity text, chosen by the author, so a modified sender cannot flood the folded list.
const ACTIVITY_CHARS: usize = 140;
const LINKS: usize = 3;
const LINK_NAME_CHARS: usize = 32;
const LINK_URL_CHARS: usize = 512;
/// A group's or channel's name, after trimming, as a display name's.
const CHAT_NAME_CHARS: usize = 64;
/// A topic's name, after trimming, as a folder's.
const TOPIC_NAME_CHARS: usize = 32;
/// A group's, channel's or topic's rules, in Unicode characters.
const RULES_CHARS: usize = 2000;
/// An invitation's note from its inviter.
const INVITE_NOTE_CHARS: usize = 500;

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

/// A new photo for a group or channel: a square JPEG, and its small pixels for the BlurHash.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhotoInput {
    #[serde(with = "serde_bytes")]
    #[cfg_attr(feature = "wasm", tsify(type = "Uint8Array"))]
    pub jpeg: Vec<u8>,
    pub thumbnail: Thumbnail,
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

/// One of this account's settings, opened: its name and its JSON value. Every value is sealed under
/// the profile key, and `lang`, `theme`, `recent.stickers` and `recent.reactions` are checked here.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Setting {
    pub key: String,
    #[cfg_attr(feature = "wasm", tsify(type = "unknown"))]
    pub value: serde_json::Value,
}

/// One group of [`Event::Emoji`], in Unicode's order.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmojiGroup {
    /// `smileys_and_emotion`, `people_and_body`, `animals_and_nature`, `food_and_drink`,
    /// `travel_and_places`, `activities`, `objects`, `symbols` or `flags`.
    pub group: String,
    pub emoji_list: Vec<EmojiItem>,
}

/// One emoji a reaction may be, with what a search matches: its English name and shortcodes.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmojiItem {
    pub emoji: String,
    pub name: String,
    pub shortcode_list: Vec<String>,
    /// Its skin tones, without the emoji itself, and empty when it has none.
    pub tone_list: Vec<String>,
}

/// The most recent stickers `recent.stickers` holds, newest first.
pub const RECENT_STICKERS: usize = 20;
/// The most recent reactions `recent.reactions` holds, newest first.
pub const RECENT_REACTIONS: usize = 8;

/// A setting's name, as the server takes it: 1 to 64 of a-z, 0-9, _ and .
fn setting_name_fits(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 64
        && key.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_' || byte == b'.'
        })
}

/// Whether a chat is a group or a channel, which have roles, topics and the rest.
fn grouped(chat_type: ChatType) -> bool {
    matches!(chat_type, ChatType::Group | ChatType::Channel)
}

/// Whether a setting is left unsealed because the server acts on it: whether others may add this
/// account to a group or a channel straight in.
fn unsealed(key: &str) -> bool {
    matches!(key, "privacy.add.group" | "privacy.add.channel")
}

/// Whether a setting's value is one this client may keep: the ones it knows are checked, and any
/// other is any JSON.
fn setting_fits(setting: &Setting) -> bool {
    let value = &setting.value;
    let strings = |most: usize| match value.as_array() {
        Some(list) => {
            list.len() <= most
                && list
                    .iter()
                    .all(|item| item.as_str().is_some_and(|text| !text.is_empty()))
        }
        None => false,
    };
    setting_name_fits(&setting.key)
        && match setting.key.as_str() {
            "lang" => matches!(value.as_str(), Some("en" | "fa")),
            "theme" => matches!(value.as_str(), Some("light" | "dark" | "system")),
            "recent.stickers" => {
                strings(RECENT_STICKERS)
                    && value.as_array().is_some_and(|list| {
                        list.iter()
                            .all(|item| item.as_str().and_then(|id| Ulid::parse(id).ok()).is_some())
                    })
            }
            "recent.reactions" => strings(RECENT_REACTIONS),
            "privacy.add.group" | "privacy.add.channel" => {
                matches!(value.as_str(), Some("everyone" | "nobody"))
            }
            _ => true,
        }
}

/// `newer` on top of `older`: a key set or removed in `newer` wins.
fn merge_change(older: Option<KvChange>, newer: KvChange) -> KvChange {
    let Some(mut merged) = older else {
        return newer;
    };
    for item in newer.set {
        merged.remove.retain(|key| *key != item.key);
        merged.set.retain(|kept| kept.key != item.key);
        merged.set.push(item);
    }
    for key in newer.remove {
        merged.set.retain(|kept| kept.key != key);
        if !merged.remove.contains(&key) {
            merged.remove.push(key);
        }
    }
    merged
}

/// Applies a change to sealed settings kept sorted by name.
fn apply_change(kv: &mut Vec<KvItem>, set: &[KvItem], remove: &[String]) {
    kv.retain(|item| !remove.contains(&item.key));
    for item in set {
        match kv.binary_search_by(|kept| kept.key.cmp(&item.key)) {
            Ok(at) => kv[at] = item.clone(),
            Err(at) => kv.insert(at, item.clone()),
        }
    }
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
    /// An agent logs in with the token its owner was shown, then answers [`Event::NeedWords`] with
    /// [`Command::Restore`]. A session that ends logs in again with it, by itself.
    LogInAgent {
        token: Secret,
    },
    /// The owner makes an agent: 12 new words, the account, and the agent's keys published from
    /// them. Answered with [`Event::AgentCreated`], whose token and words are shown once.
    CreateAgent {
        username: String,
        display_name: String,
        #[serde(default)]
        permission_list: Vec<String>,
        #[serde(default)]
        admin: bool,
    },
    /// The owner's agents, answered with [`Event::Agents`].
    Agents,
    /// Changes an agent's display name, ticks or admin, then answers with [`Event::Agents`].
    UpdateAgent {
        agent: Ulid,
        #[serde(default)]
        display_name: Option<String>,
        #[serde(default)]
        permission_list: Option<Vec<String>>,
        #[serde(default)]
        admin: Option<bool>,
    },
    /// A new token for an agent, its device ended with a statement signed with the agent's words,
    /// which are never kept. Answered with [`Event::AgentToken`].
    RenewAgentToken {
        agent: Ulid,
        words: Secret,
    },
    /// No token for an agent until a renew, its device ended the same way.
    RevokeAgentToken {
        agent: Ulid,
        words: Secret,
    },
    /// Disables an agent for good: its token and device end, and it leaves every group and channel.
    DisableAgent {
        agent: Ulid,
        words: Secret,
    },
    /// An agent's bio and links, keeping its photo, signed by this device. Its profile must have
    /// loaded, which [`Command::Agents`] asks for.
    AgentProfile {
        agent: Ulid,
        #[serde(default)]
        bio: Option<String>,
        #[serde(default)]
        link_list: Vec<ProfileLink>,
    },
    /// A new photo for an agent, as [`Command::SetPhoto`] is for this account.
    AgentPhoto {
        agent: Ulid,
        #[serde(with = "serde_bytes")]
        #[cfg_attr(feature = "wasm", tsify(type = "Uint8Array"))]
        jpeg: Vec<u8>,
        thumbnail: Thumbnail,
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
    /// Looks through this device's archive for messages holding `text`, newest first, and stops
    /// after a page of hits, handed over as [`Event::MessageHits`]. Replaces any search under way.
    SearchMessages {
        text: Secret,
    },
    /// The next page of the search under way, older than the last.
    SearchOlder,
    /// `reply` names the message answered, by its sequence id or its pending local id. The message
    /// then waits for that id if it has none yet, and carries it inside its encryption.
    SendText {
        chat: Ulid,
        text: Secret,
        #[serde(default)]
        reply: Option<Pointer>,
        /// A group's or channel's topic, none for General.
        #[serde(default)]
        topic: Option<Ulid>,
        /// The main-list message whose thread this goes in.
        #[serde(default)]
        thread: Option<u64>,
        /// The accounts the text mentions as `@username`, each a member, kept inside the encryption.
        #[serde(default)]
        mentions: Vec<Ulid>,
    },
    /// One file, or several images and videos as a bundle, with an optional caption. Each goes up
    /// first, and the message joins the queue once all are up.
    SendFiles {
        chat: Ulid,
        files: Vec<FileSpec>,
        #[serde(default)]
        text: Option<Secret>,
        #[serde(default)]
        topic: Option<Ulid>,
        #[serde(default)]
        thread: Option<u64>,
        /// The message it answers.
        #[serde(default)]
        reply: Option<Pointer>,
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
    /// Seen up to a message: in the main list, or in a group's or channel's topic or thread.
    Seen {
        chat: Ulid,
        up_to_seq: u64,
        #[serde(default)]
        topic: Option<Ulid>,
        #[serde(default)]
        thread: Option<u64>,
    },
    /// In a channel, typing goes out only inside a thread.
    Typing {
        chat: Ulid,
        on: bool,
        #[serde(default)]
        topic: Option<Ulid>,
        #[serde(default)]
        thread: Option<u64>,
    },
    /// Keeps this chat's draft, or clears it with none. Nothing is queued: a draft is best effort,
    /// and one the connection could not carry is dropped, as the next change replaces it.
    Draft {
        chat: Ulid,
        draft: Option<Draft>,
    },
    /// A new folder's id, from the server, answered as [`Event::FolderId`]. It saves nothing: the
    /// folder is saved with the [`Command::Folders`] that names it, within 5 minutes.
    FolderId,
    /// This account's whole arrangement of folders, with plain names: only a changed name is sealed
    /// again. Shown at once as [`Event::Folders`], and kept until the server takes it, so one made
    /// offline goes on the next connection and wins over what other devices did meanwhile. One the
    /// server refuses goes back to the server's, with an [`Event::Error`] saying why.
    Folders {
        folder_list: Vec<Folder>,
        default: Ulid,
    },
    /// When each other member received and saw one message, answered as [`Event::Times`], a page at
    /// a time: `after` is the last account of the page before.
    Times {
        chat: Ulid,
        seq: u64,
        after: Option<Ulid>,
    },
    /// The app came on screen or left it, or an agent's trouble began or cleared. Kept, sent as
    /// `presence` now if connected, and on every connection as its first frame. `text` is sealed
    /// here under the account's profile key.
    InApp {
        in_app: bool,
        #[serde(default)]
        trouble: bool,
        #[serde(default)]
        text: Option<String>,
    },
    /// Someone's presence, asked once, for a person sharing no chat: answered as
    /// [`Event::Presence`].
    PresenceOf {
        account: Ulid,
    },
    /// Changes this account's settings: each value sealed here, shown at once as [`Event::Kv`], and
    /// kept until the server takes it. The server wins: a change it refuses as based on an older
    /// version gives way to its set.
    Kv {
        #[serde(default)]
        set: Vec<Setting>,
        #[serde(default)]
        remove: Vec<String>,
    },
    Devices,
    /// Needs the words, from any device.
    Terminate {
        device: Ulid,
        words: Secret,
    },
    LogOut,
    /// Retry the connection and anything waiting to retry, now. A socket still up is reported
    /// with `Online` again, and syncs.
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
        #[serde(default)]
        topic: Option<Ulid>,
        #[serde(default)]
        thread: Option<u64>,
    },
    /// What an agent is doing, one step's intention for this chat: `{"text"}`, 1 to 140 characters.
    SendActivity {
        chat: Ulid,
        text: String,
        #[serde(default)]
        topic: Option<Ulid>,
        #[serde(default)]
        thread: Option<u64>,
    },
    /// A reaction on a message, refused here unless `emoji` is one the `emojis` crate knows. `on`
    /// is inside the encryption, as a reply's is, and it waits for a pending message's sequence id.
    SendReaction {
        chat: Ulid,
        emoji: String,
        on: Pointer,
        /// The topic of the message it is on, none for General, so the server can apply the
        /// topic's own rules.
        #[serde(default)]
        topic: Option<Ulid>,
        /// The main-list message whose thread the message it is on is in.
        #[serde(default)]
        thread: Option<u64>,
    },
    /// Every emoji a reaction may be, answered as [`Event::Emoji`].
    Emoji,
    /// A tree of questions, with an optional intro. Clients have no screen for it: an agent sends
    /// them. Refused here unless the tree holds together.
    SendQuestion {
        chat: Ulid,
        #[serde(default)]
        text: Option<Secret>,
        question: Question,
        /// The message it answers, a group's or channel's topic, and the thread it goes in.
        #[serde(default)]
        reply: Option<Pointer>,
        #[serde(default)]
        topic: Option<Ulid>,
        #[serde(default)]
        thread: Option<u64>,
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
    /// What to do with this chat, with an optional text. Clients have no screen for it: an agent
    /// sends them. Refused here unless its action is a known one that holds together.
    SendSuggestion {
        chat: Ulid,
        #[serde(default)]
        text: Option<Secret>,
        suggestion: Suggestion,
        /// The message it answers, a group's or channel's topic, and the thread it goes in.
        #[serde(default)]
        reply: Option<Pointer>,
        #[serde(default)]
        topic: Option<Ulid>,
        #[serde(default)]
        thread: Option<u64>,
    },
    /// Accepts the suggestion at `seq`, as shown. Its folder is found by its name folded as search
    /// folds, the first in tab order, or made last among the tabs, which needs a connection for its
    /// id. The arrangement is saved as [`Command::Folders`] saves it, and once the server took it the
    /// ack goes, saying what changed. One the server refuses sends no ack, with an
    /// [`Event::Error`] saying why. Nothing to change sends the ack at once.
    AcceptSuggestion {
        chat: Ulid,
        seq: u64,
        suggestion: Suggestion,
    },
    /// Declines the suggestion at `seq`: the ack alone.
    DeclineSuggestion {
        chat: Ulid,
        seq: u64,
    },
    /// The server's sticker packs, answered as [`Event::Stickers`].
    Stickers,
    /// One sticker's WebP, answered as [`Event::StickerImage`].
    StickerImage {
        sticker: Ulid,
    },
    /// An account's profile, answered as [`Event::Profile`]: at once with what this device knows,
    /// and again once the server's is opened.
    Profile {
        account: Ulid,
    },
    /// The photo a shown profile names, opened, answered as [`Event::Photo`].
    Photo {
        account: Ulid,
    },
    /// This account's bio and links, keeping its photo. Checked here before it goes.
    SaveProfile {
        #[serde(default)]
        bio: Option<String>,
        #[serde(default)]
        link_list: Vec<ProfileLink>,
    },
    /// A new photo for this account: a square JPEG, and its small pixels for the BlurHash. It goes
    /// up sealed, then the profile names it, and the old one is deleted.
    SetPhoto {
        #[serde(with = "serde_bytes")]
        #[cfg_attr(feature = "wasm", tsify(type = "Uint8Array"))]
        jpeg: Vec<u8>,
        thumbnail: Thumbnail,
    },
    RemovePhoto,
    /// This account's display name, username or both. A new username needs the password.
    SetNames {
        #[serde(default)]
        display_name: Option<String>,
        #[serde(default)]
        username: Option<String>,
        #[serde(default)]
        password: Option<Secret>,
    },
    /// A group or channel with this account its owner, then its sealed profile, its options, and the
    /// people in `account_list` added in one commit. Answered with [`Event::Started`] once it has
    /// its id, and [`Event::ChatReady`] once its group runs.
    CreateChat {
        chat_type: ChatType,
        name: String,
        #[serde(default)]
        bio: Option<String>,
        #[serde(default)]
        link_list: Vec<ProfileLink>,
        #[serde(default)]
        account_list: Vec<Ulid>,
        /// Unset takes the server's default.
        #[serde(default)]
        topics: Option<bool>,
        /// Listed in the directory under its name and bio, which the server then reads.
        #[serde(default)]
        findable: bool,
        /// Whether plain members may add people is `member.add` in `options.allow`.
        #[serde(default)]
        options: ChatOptions,
        /// Uploaded before anyone is added, so the first profile names it. If it fails, the chat is
        /// made without it and `photo_failed` is raised.
        #[serde(default)]
        photo: Option<PhotoInput>,
        /// Sealed in the invitation of anyone in `account_list` added by invitation only.
        #[serde(default)]
        note: Option<String>,
    },
    /// A new photo for a group or channel, which mirrors [`Command::SetPhoto`]: it goes up sealed
    /// as one of the chat's files, then the profile names it, and the old one is removed.
    ChatPhoto {
        chat: Ulid,
        #[serde(with = "serde_bytes")]
        #[cfg_attr(feature = "wasm", tsify(type = "Uint8Array"))]
        jpeg: Vec<u8>,
        thumbnail: Thumbnail,
    },
    /// The photo a group's or channel's shown profile names, opened, answered as
    /// [`Event::ChatPhoto`].
    ChatPhotoGet {
        chat: Ulid,
    },
    /// Adds accounts to a group or channel in one commit, each of their devices with a key package.
    /// Anyone added by invitation only is invited instead, with `note` sealed in the invitation.
    AddMembers {
        chat: Ulid,
        account_list: Vec<Ulid>,
        #[serde(default)]
        note: Option<String>,
    },
    /// Takes accounts out of a group or channel in one commit.
    RemoveMembers {
        chat: Ulid,
        account_list: Vec<Ulid>,
    },
    /// Makes a member an owner, a moderator or a plain member, by an owner, on the server alone.
    SetRole {
        chat: Ulid,
        account: Ulid,
        role: ChatRole,
    },
    /// Leaves a group or channel: at once on the server, and the next member online commits it.
    Leave {
        chat: Ulid,
    },
    /// Removes a whole group or channel for everyone, by an owner.
    RemoveChat {
        chat: Ulid,
    },
    /// Asks to join a findable group or channel, with a note the server reads.
    AskToJoin {
        chat: Ulid,
        #[serde(default)]
        note: Option<String>,
    },
    CancelAsk {
        chat: Ulid,
    },
    /// Lets a join request in: an add, by someone who may add people.
    LetIn {
        chat: Ulid,
        account: Ulid,
    },
    DeclineAsk {
        chat: Ulid,
        account: Ulid,
    },
    /// A chat's pending join requests, answered as [`Event::Requests`].
    Requests {
        chat: Ulid,
    },
    /// Accepts or declines this account's invitation. Accepting joins by external commit, with the
    /// inviter's signed invitation.
    AnswerInvite {
        chat: Ulid,
        accept: bool,
    },
    /// A new topic's id, answered as [`Event::TopicId`], to be named in [`Command::Topics`].
    TopicId {
        chat: Ulid,
    },
    /// A group's or channel's members and their roles, answered as [`Event::Members`]. Asked when
    /// its members screen opens; the platform keeps it current from the chat's events.
    Members {
        chat: Ulid,
    },
    /// History rows the platform read from its archive, handed back one [`Event::ChatEvent`] each
    /// with their copies opened, so a History page shows what a delete removed or an edit changed.
    /// Rows of another chat are skipped.
    OpenHistory {
        chat: Ulid,
        event_list: Vec<ChatEvent>,
    },
    /// Whether topics show, and the live topics with their names, in order. A topic left out is
    /// removed with everything in it. Its names go into the chat's sealed profile.
    Topics {
        chat: Ulid,
        on: bool,
        topic_list: Vec<TopicName>,
    },
    /// A group's or channel's name, bio, links and rules, sealed under its key with its topics'
    /// names.
    ChatProfile {
        chat: Ulid,
        name: String,
        #[serde(default)]
        bio: Option<String>,
        #[serde(default)]
        link_list: Vec<ProfileLink>,
        /// At most 2,000 characters.
        #[serde(default)]
        rules: Option<String>,
    },
    /// A topic's bio, links and rules, General's under `GENERAL_TOPIC`, sealed under the chat's
    /// profile key, keeping its photo. Its name stays in the chat's topic list.
    TopicProfile {
        chat: Ulid,
        topic: Ulid,
        #[serde(default)]
        bio: Option<String>,
        #[serde(default)]
        link_list: Vec<ProfileLink>,
        #[serde(default)]
        rules: Option<String>,
    },
    /// A new photo for a topic, which goes up as one of the chat's files, as a chat's does.
    TopicPhoto {
        chat: Ulid,
        topic: Ulid,
        #[serde(with = "serde_bytes")]
        #[cfg_attr(feature = "wasm", tsify(type = "Uint8Array"))]
        jpeg: Vec<u8>,
        thumbnail: Thumbnail,
    },
    /// The photo a topic's shown profile names, opened, answered as [`Event::TopicPhoto`].
    TopicPhotoGet {
        chat: Ulid,
        topic: Ulid,
    },
    /// A topic's own options, by its moderators and the chat's owners and moderators.
    TopicOptions {
        chat: Ulid,
        topic: Ulid,
        options: TopicOptionsBody,
    },
    /// Makes a member a topic's moderator, or a member there again, by the chat's owners: `role`
    /// is moderator or member.
    TopicRole {
        chat: Ulid,
        topic: Ulid,
        account: Ulid,
        role: ChatRole,
    },
    /// What a group's or channel's members may do, by owners.
    ChatOptions {
        chat: Ulid,
        options: ChatOptions,
    },
    /// Whether the directory lists a group or channel, under a name and bio the server reads.
    Findable {
        chat: Ulid,
        findable: bool,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        bio: Option<String>,
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
    /// A command whose outcome the platform wants told: once everything it asked the server and
    /// the platform has been answered, [`Event::Answered`] comes with the same `tag`. Chosen by the
    /// author for keryx-agent, whose scripts wait for each write's answer.
    Tagged {
        tag: u64,
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
    /// The two snapshot slots kept for the account an [`Output::Open`] named, or none when this
    /// platform keeps none for it. The login it waited for goes on.
    Opened {
        #[serde(with = "serde_bytes")]
        #[cfg_attr(feature = "wasm", tsify(type = "Uint8Array | null"))]
        first: Option<Vec<u8>>,
        #[serde(with = "serde_bytes")]
        #[cfg_attr(feature = "wasm", tsify(type = "Uint8Array | null"))]
        second: Option<Vec<u8>>,
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
    /// [`Output::ReadRows`]'s answer: the chat's messages, oldest first, the rows of every call
    /// among them, and the chat's events in the same span, oldest first.
    RowsRead {
        id: u64,
        message_list: Vec<MessageFrame>,
        call_list: Vec<CallEvent>,
        #[serde(default)]
        chat_event_list: Vec<ChatEvent>,
    },
    /// [`Output::ScanRows`]'s answer: the messages, newest first, and how many rows the archive
    /// holds in all, for the search's progress.
    RowsScanned {
        id: u64,
        message_list: Vec<MessageFrame>,
        total: u64,
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
    /// A login with no device loaded found its account: load that account's slots, if this
    /// platform keeps any, and answer with [`Input::Opened`].
    Open {
        account: Ulid,
    },
    /// Call `step` with [`Input::Timer`] at this time. A later one replaces it.
    WakeAt {
        at: i64,
    },
    Event {
        event: Box<Event>,
    },
    /// Read `length` bytes at `offset` of the file the platform holds as `handle`, and answer with
    /// [`Input::ChunkRead`].
    ReadChunk {
        handle: u64,
        n: u64,
        offset: u64,
        length: u64,
    },
    /// A PUT or POST, as `method` says, of these raw bytes. Its reply comes back as
    /// [`Input::Reply`] with the same id.
    PutChunk {
        id: u64,
        method: String,
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
    /// removed message, with only `chat`, `seq` and `server_saved_at`, replaces its copy, and takes
    /// with it the edits pointing at it, as the server wipes them too, and a removed call message
    /// that call's rows. A group's or channel's events are kept by their chat and `n`.
    SaveRows {
        id: u64,
        message_list: Vec<MessageFrame>,
        call_list: Vec<CallEvent>,
        chat_event_list: Vec<ChatEvent>,
    },
    /// Read up to `limit` of the chat's newest messages from the archive, below `before_seq` if
    /// given, with the rows of every call among them, and the chat's events whose server time is
    /// from the first message's on, or from the start when that is the chat's first, up to the
    /// message at `before_seq`'s, and answer with [`Input::RowsRead`].
    ReadRows {
        id: u64,
        chat: Ulid,
        before_seq: Option<u64>,
        limit: u32,
    },
    /// Read up to `limit` messages of every chat, newest by server time first, then by chat and
    /// sequence id, all below `before` when given, and answer with [`Input::RowsScanned`].
    ScanRows {
        id: u64,
        before: Option<ScanAt>,
        limit: u32,
    },
}

/// Where a search's scan of the archive has reached: a message's server time, chat and sequence id.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ScanAt {
    pub saved_at: i64,
    pub chat: Ulid,
    pub seq: u64,
}

/// A message a search found: the words it matched in, split around the match.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify), tsify(missing_as_null))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hit {
    pub chat: Ulid,
    pub seq: u64,
    pub account: Option<Ulid>,
    pub mine: bool,
    pub saved_at: i64,
    pub before: Secret,
    pub matched: Secret,
    pub after: Secret,
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

/// One of this account's folders. `all` is the one with no name and no `chat_list`, and holds every
/// chat. A name this device cannot open shows empty.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify), tsify(missing_as_null))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Folder {
    pub id: Ulid,
    #[serde(default)]
    pub name: Option<Secret>,
    #[serde(default)]
    pub chat_list: Option<Vec<Ulid>>,
    /// Pinned chats, first on top.
    #[serde(default)]
    pub pin_list: Vec<Ulid>,
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
    /// A reaction's emoji, one the `emojis` crate knows, with `on`.
    #[serde(default)]
    pub emoji: Option<String>,
    /// The sequence id of the message a reaction is on, from inside its encryption.
    #[serde(default)]
    pub on: Option<u64>,
    /// A question message's tree, once it holds together. Its intro is `text`.
    #[serde(default)]
    pub question: Option<Question>,
    /// An answer message's answer.
    #[serde(default)]
    pub answer: Option<Answer>,
    /// A suggestion message's action, once it holds together, or `Unknown` from a newer keryx. Its
    /// text is `text`.
    #[serde(default)]
    pub suggestion: Option<Box<Suggestion>>,
    /// An ack message's ack.
    #[serde(default)]
    pub ack: Option<Box<Ack>>,
    /// A group's or channel's topic, none for General.
    #[serde(default)]
    pub topic: Option<Ulid>,
    /// The main-list message whose thread this is in.
    #[serde(default)]
    pub thread_seq: Option<u64>,
    /// The accounts a group's or channel's text mentions, from inside its encryption.
    #[serde(default)]
    pub mentions: Vec<Ulid>,
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
    /// A membership line the server wrote in a group: `account`, through `device`, did `action` to
    /// `account_list`, or to itself when that is empty. `mine` when this account acted, so it is
    /// not unread. The server is trusted for it, as for roles, leaving and removals.
    Line {
        chat: Ulid,
        seq: u64,
        device: Ulid,
        account: Ulid,
        action: LineAction,
        account_list: Vec<Ulid>,
        saved_at: i64,
        mine: bool,
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
        /// A reaction's emoji. What it is on is `target`.
        #[serde(default)]
        emoji: Option<String>,
        /// An answer, or an answer's edit.
        #[serde(default)]
        answer: Option<Answer>,
        /// An ack, so the platform shows its suggestion handled at once.
        #[serde(default)]
        ack: Option<Ack>,
        /// A group's or channel's topic, none for General.
        #[serde(default)]
        topic: Option<Ulid>,
        /// The main-list message whose thread it goes in.
        #[serde(default)]
        thread: Option<u64>,
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
        /// Only for `slow_mode`: how many seconds until this member may write there again, chosen
        /// by the author, so the app can count down.
        seconds: Option<u64>,
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
    /// The answer to [`Command::FolderId`]: a new folder's id, kept for this account for 5 minutes.
    FolderId {
        folder: Ulid,
    },
    /// This account's folders, here or from the server: the whole arrangement in tab order, names
    /// opened, and the one to open on.
    Folders {
        folder_list: Vec<Folder>,
        default: Ulid,
    },
    /// The answer to `Times`: a page of the members but the sender, by the server's clock. `more`
    /// says another page follows.
    Times {
        chat: Ulid,
        seq: u64,
        member_list: Vec<TimesItem>,
        more: bool,
    },
    /// Where someone sharing a chat is, after this device's first `presence` on each connection and
    /// on every change, or the answer to [`Command::PresenceOf`]. What the account may not see is
    /// missing.
    Presence {
        presence: PresenceItem,
        /// Its text, opened. Left out when it does not open.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
    /// This account's settings, opened, whenever they change here or on the server.
    Kv {
        setting_list: Vec<Setting>,
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
    /// The answer to [`Command::Emoji`]: every emoji a reaction may be, by group, in Unicode's order.
    Emoji {
        group_list: Vec<EmojiGroup>,
    },
    /// An account's profile as far as this device can open it, sent again whenever it changes.
    Profile {
        profile: ShownProfile,
    },
    /// The JPEG of the photo a profile names, opened.
    Photo {
        account: Ulid,
        photo: Ulid,
        bytes: Bytes,
    },
    /// This account's profile, photo or names were saved.
    ProfileSaved,
    /// A new agent, its token and its 12 words, to be put in its `agent.toml`. Shown once: nothing
    /// here keeps the words.
    AgentCreated {
        account: Ulid,
        username: String,
        token: Secret,
        words: Secret,
    },
    /// The owner's agents.
    Agents {
        agent_list: Vec<AgentItem>,
    },
    /// An agent's renewed token, shown once.
    AgentToken {
        agent: Ulid,
        token: Secret,
    },
    Typing {
        chat: Ulid,
        account: Ulid,
        on: bool,
        /// The topic typed in, none for General or a chat without topics.
        topic: Option<Ulid>,
        /// The thread typed in, by the seq of the message that opened it.
        thread: Option<u64>,
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
    /// The directory's answer: accounts, and findable groups and channels, which can be asked to join.
    SearchResults {
        account_list: Vec<AccountBrief>,
        chat_list: Vec<FoundChat>,
    },
    /// What one read of the archive found for the search under way, and how far it has looked:
    /// `scanned` of `total` rows. `done` once this page of hits is full or the archive is
    /// through, and `more` while older rows are left for [`Command::SearchOlder`]. `text` is what
    /// was looked for, so a platform can drop what an earlier search still had on its way.
    MessageHits {
        text: Secret,
        hit_list: Vec<Hit>,
        scanned: u64,
        total: u64,
        done: bool,
        more: bool,
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
    /// Something the server wrote happened in a group or channel: a line every member sees, history
    /// for owners and moderators, or a line for this account alone.
    ChatEvent {
        event: ChatEvent,
        /// A delete's or edit's copies of the message and its edits as they were, oldest first,
        /// opened and their signatures checked. One this device cannot open or check is left out.
        #[serde(default)]
        copy_list: Vec<Shown>,
    },
    /// The answer to [`Command::Requests`].
    Requests {
        chat: Ulid,
        request_list: Vec<RequestItem>,
    },
    /// Someone's join request to a chat this account may let people into arrived or changed.
    JoinRequest {
        request: RequestItem,
    },
    /// This account's own join request, as its row in the chat list shows it: waiting, declined,
    /// let in, or gone, taken back on another device or expired. `name` is the chat's findable
    /// name, as `chats`' `asked_list` has it.
    Asked {
        chat: Ulid,
        name: Option<String>,
        state: RequestState,
        at: i64,
    },
    /// An invitation for this account, its name and BlurHash opened, or with none, gone: answered
    /// on another device or expired.
    Invitation {
        chat: Ulid,
        invitation: Option<InvitationShown>,
    },
    /// The answer to [`Command::Members`].
    Members {
        chat: Ulid,
        member_list: Vec<MemberItem>,
    },
    /// The answer to [`Command::TopicId`].
    TopicId {
        chat: Ulid,
        topic: Ulid,
    },
    /// A group's or channel's topics changed, with their names as its profile has them.
    Topics {
        chat: Ulid,
        on: bool,
        topic_list: Vec<TopicName>,
    },
    /// A group's or channel's profile, opened.
    ChatProfile {
        chat: Ulid,
        profile: ShownChatProfile,
    },
    /// The JPEG of the photo a group's or channel's profile names, opened.
    ChatPhoto {
        chat: Ulid,
        photo: Ulid,
        bytes: Bytes,
    },
    ChatOptions {
        chat: Ulid,
        options: ChatOptions,
    },
    /// A topic's own options and moderators, General's under `GENERAL_TOPIC`. One hidden from
    /// this account comes as hidden with nothing else, and the platform leaves it out.
    TopicOptions {
        chat: Ulid,
        topic: Ulid,
        options: TopicOptionsBody,
        mod_list: Vec<Ulid>,
    },
    /// A topic's profile, opened.
    TopicProfile {
        chat: Ulid,
        topic: Ulid,
        profile: ShownTopicProfile,
    },
    /// The JPEG of the photo a topic's profile names, opened.
    TopicPhoto {
        chat: Ulid,
        topic: Ulid,
        photo: Ulid,
        bytes: Bytes,
    },
    /// Whether the directory lists a group or channel, with the name and bio it shows; none when it
    /// no longer does.
    Findable {
        chat: Ulid,
        findable: Option<FindableItem>,
    },
    /// Everything an [`Input::Tagged`] command asked has been answered: `seq` when it stored a
    /// message, and `code`, with `message` and a slow mode's `seconds`, when something refused it.
    Answered {
        tag: u64,
        seq: Option<u64>,
        code: Option<String>,
        message: Option<String>,
        seconds: Option<u64>,
    },
    Online,
    Offline,
    LoggedOut,
    Error {
        code: String,
        message: String,
    },
}

/// A group's or channel's profile, opened, for the platform to show.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify), tsify(missing_as_null))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShownChatProfile {
    pub name: String,
    pub bio: Option<String>,
    pub link_list: Vec<ProfileLink>,
    pub rules: Option<String>,
    pub topic_list: Vec<TopicName>,
    /// What to draw while [`Command::ChatPhotoGet`] fetches it.
    pub photo: Option<ShownChatPhoto>,
    pub version: u64,
}

/// A topic's profile, opened, for the platform to show.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify), tsify(missing_as_null))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShownTopicProfile {
    pub bio: Option<String>,
    pub link_list: Vec<ProfileLink>,
    pub rules: Option<String>,
    /// What to draw while [`Command::TopicPhotoGet`] fetches it.
    pub photo: Option<ShownChatPhoto>,
    pub version: u64,
}

/// The photo a group's or channel's profile names: its file, and its BlurHash.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShownChatPhoto {
    pub id: Ulid,
    pub blurhash: String,
}

/// An account's profile, for the platform to show.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify), tsify(missing_as_null))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShownProfile {
    pub account: Ulid,
    pub username: String,
    pub display_name: Option<String>,
    pub version: u64,
    pub bio: Option<String>,
    pub link_list: Vec<ProfileLink>,
    /// What to draw while [`Command::Photo`] fetches it.
    pub photo: Option<ShownPhoto>,
    /// An agent account, with its owner, for its badge.
    pub agent: bool,
    pub owner: Option<Ulid>,
}

#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShownPhoto {
    pub id: Ulid,
    pub blurhash: String,
}

/// What this device knows of an account's profile, kept in the shelf.
#[derive(Clone, Serialize, Deserialize)]
struct ProfileState {
    account: Ulid,
    version: u64,
    username: String,
    #[serde(default)]
    display_name: Option<String>,
    /// The server's sealed profile, kept until it opens: its key may come later.
    #[serde(default)]
    signed: Option<SignedProfile>,
    /// The photo the server holds. A sealed profile naming another one shows none.
    #[serde(default)]
    photo: Option<Ulid>,
    #[serde(default)]
    body: Option<ProfileBody>,
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
        topic: Option<Ulid>,
        thread_seq: Option<u64>,
    },
    Typing {
        chat: Ulid,
        on: bool,
        topic: Option<Ulid>,
        thread_seq: Option<u64>,
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
    /// Devices added. In a group or channel, `added` names each new account with its devices'
    /// packages, and the server is told of them; empty for devices of accounts already in.
    Add {
        package_list: Vec<KeyPackageItem>,
        added: Vec<(Ulid, Vec<Ulid>)>,
    },
    /// Devices taken out. `removed` names the accounts a group's or channel's member takes out, and
    /// the server is told of them; empty for devices that ended and accounts that already left.
    Remove {
        device_list: Vec<Ulid>,
        removed: Vec<Ulid>,
    },
    Rotate,
    /// Joining by external commit, with the invitation when this account is not in the group yet.
    Join {
        group_info: Vec<u8>,
        invite: Option<InviteStatement>,
    },
}

/// A group's or channel's start, kept until its group runs: its profile, its options and who to
/// add.
struct GroupStart {
    profile: ChatProfilePlain,
    options: ChatOptions,
    account_list: Vec<Ulid>,
    /// Its photo, waiting to go up before epoch 0.
    photo: Option<PreparedPhoto>,
    /// Sealed in the invitation of anyone added by invitation only.
    note: Option<String>,
}

/// A group's or channel's photo sealed as one chunk under its own key, ready to go up.
struct PreparedPhoto {
    sealed: Vec<u8>,
    key: zeroize::Zeroizing<[u8; 16]>,
    size: u64,
    blurhash: String,
}

/// Checks a photo and seals it under a new key, as an account's photo is.
fn prepare_photo(jpeg: &[u8], thumbnail: &Thumbnail) -> Result<PreparedPhoto, &'static str> {
    if jpeg.is_empty() || jpeg.len() as u64 > file::CHUNK {
        return Err("That photo is too large");
    }
    let Ok(blurhash) = file::blurhash_of(thumbnail.width, thumbnail.height, &thumbnail.rgba) else {
        return Err("That photo's thumbnail is not one");
    };
    let Ok(key) = file::new_key() else {
        return Err("Could not make the photo's key");
    };
    let Ok(sealed) = file::seal_chunk(key.as_ref(), 0, true, jpeg) else {
        return Err("Could not seal the photo");
    };
    Ok(PreparedPhoto {
        sealed,
        key,
        size: jpeg.len() as u64,
        blurhash,
    })
}

/// Accounts being added to a group or channel, their devices' packages claimed an account at a
/// time.
#[derive(Default)]
struct AddClaim {
    account_list: Vec<Ulid>,
    package_list: Vec<KeyPackageItem>,
    added: Vec<(Ulid, Vec<Ulid>)>,
}

/// Rows of the archive: a `sync` page's, one arriving live, or a page read back.
struct Rows {
    message_list: Vec<MessageFrame>,
    call_list: Vec<CallEvent>,
    chat_event_list: Vec<ChatEvent>,
}

impl Rows {
    fn messages(message_list: Vec<MessageFrame>) -> Self {
        Self {
            message_list,
            call_list: Vec::new(),
            chat_event_list: Vec::new(),
        }
    }
}

/// What a tagged command asked that is answered later: a frame by its `ref`, a REST request or a
/// save by its id, or a message by its local id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum TagKey {
    Frame(Ulid),
    Request(u64),
    Save(u64),
    Rows(u64),
    Local(u64),
}

/// A tagged command not yet answered: how much it still waits for, and what was answered so far.
#[derive(Default)]
struct TagState {
    open: usize,
    seq: Option<u64>,
    code: Option<String>,
    message: Option<String>,
    seconds: Option<u64>,
}

/// One thing a step's outputs say about tagged commands.
enum Tagging {
    Open(TagKey),
    Sent(u64, u64),
    Failed(u64, String, Option<u64>),
    Error(String, String),
}

/// What a REST request was for.
/// A login that found its account and waits for the platform to load that account's slots.
struct Opening {
    account: Ulid,
    credentials: Credentials,
    challenge: Bytes,
}

/// What an owner does to an agent's token, each ending its device first.
#[derive(Clone, Copy)]
enum AgentEnd {
    Renew,
    Revoke,
    Disable,
}

/// The account an agent's token names, read from its claims without checking them: the server does.
fn agent_of_token(token: &str) -> Option<Ulid> {
    let payload = token.split('.').nth(1)?;
    let bytes = Base64UrlUnpadded::decode_vec(payload).ok()?;
    let claims: AgentClaims = serde_json::from_slice(&bytes).ok()?;
    Some(claims.sub)
}

/// What a login proves itself with: a person's password, or an agent's token.
#[derive(Clone)]
enum Credentials {
    Password {
        username: String,
        password: Secret,
        captcha: Option<CaptchaAnswer>,
    },
    Agent {
        token: Secret,
    },
}

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
        credentials: Credentials,
    },
    Login {
        credentials: Credentials,
        with_device: bool,
    },
    /// This account's own info. `signed_in` holds the login's device while [`Event::SignedIn`] is
    /// still owed, as an agent's login does not know its username until here.
    Me {
        signed_in: Option<Option<Ulid>>,
    },
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
    CreateGroup {
        chat_type: ChatType,
        start: Box<GroupStart>,
    },
    /// A group's or channel's photo: its file made, then its one chunk put. `starting` is a new
    /// chat's, whose group starts once it is up or has failed.
    /// A topic's photo when `topic` is set, never starting.
    ChatPhotoFile {
        chat: Ulid,
        topic: Option<Ulid>,
        photo: Box<PreparedPhoto>,
        starting: bool,
    },
    ChatPhotoChunk {
        chat: Ulid,
        topic: Option<Ulid>,
        file: Ulid,
        photo: Box<PreparedPhoto>,
        starting: bool,
    },
    ChatPhotoGet {
        chat: Ulid,
        topic: Option<Ulid>,
        photo: Ulid,
    },
    Requests {
        chat: Ulid,
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
    /// One account's packages, for adding it to a group or channel.
    AddClaim {
        chat: Ulid,
        account: Ulid,
        claim: AddClaim,
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
    /// The new agent's account, with its words to publish keys from and show once.
    CreateAgent {
        username: String,
        words: Secret,
    },
    AgentKeys {
        account: Ulid,
        username: String,
        token: Secret,
        words: Secret,
    },
    Agents,
    /// An agent was changed, so the list is asked again.
    AgentChanged,
    /// The list, to find the agent's device a renew, revoke or disable must end.
    AgentDevice {
        agent: Ulid,
        words: Secret,
        end: AgentEnd,
    },
    AgentEnded {
        agent: Ulid,
        end: AgentEnd,
    },
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
    Profile {
        account: Ulid,
    },
    PutProfile {
        account: Ulid,
        body: Box<ProfileBody>,
    },
    AddPhoto {
        account: Ulid,
        key: zeroize::Zeroizing<[u8; 16]>,
        size: u64,
        blurhash: String,
    },
    Photo {
        account: Ulid,
        photo: Ulid,
    },
    Names,
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
    /// A reaction's emoji, for its pending message.
    emoji: Option<String>,
    /// An answer or an answer's edit, for its pending message.
    answer: Option<Answer>,
    /// An ack, for its pending message.
    ack: Option<Ack>,
    /// A group's or channel's topic, signed in the header. None for General.
    topic: Option<Ulid>,
    /// The main-list message whose thread this goes in, signed in the header.
    thread_seq: Option<u64>,
}

/// An ack waiting for the server to take the arrangement it accepted.
struct WaitingAck {
    folders: ChatFolders,
    chat: Ulid,
    ack: Ack,
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
    /// The message it answers.
    reply: Option<Pointer>,
    topic: Option<Ulid>,
    thread_seq: Option<u64>,
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
    /// A new folder's id.
    FolderId,
    /// A new folder's id for the suggestion at `seq` being accepted.
    SuggestionFolder {
        chat: Ulid,
        seq: u64,
        folder: FolderSuggestion,
    },
    /// A `chat_folders` frame: kept until the server takes this arrangement.
    Folders {
        folders: ChatFolders,
    },
    /// A `kv` frame: this account's settings.
    Kv,
    /// A `kv_set` frame: the change in flight.
    KvSet,
    /// A new topic's id.
    TopicId,
    /// A chat's members.
    Members,
    /// This device left: once the server took it, the chat goes.
    Left {
        chat: Ulid,
    },
    /// A `topics` frame from this device: once the server took it, this device is shown the new
    /// topics and a chat that lost a topic is synced, as the push of it goes to every device but
    /// this one.
    TopicsSaved {
        chat: Ulid,
        on: bool,
        topic_list: Vec<TopicName>,
        removed: bool,
    },
    /// A `chat_options` frame from this device: once the server took it, this device is shown
    /// them, as their push goes to every device but this one.
    OptionsSaved {
        chat: Ulid,
        options: ChatOptions,
    },
    /// A `findable` frame from this device, shown to it once the server took it, the same way.
    FindableSaved {
        chat: Ulid,
        findable: Option<FindableItem>,
    },
    /// This account's answer to an invitation: accepted, its device joins.
    InviteAnswer {
        chat: Ulid,
        accept: bool,
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
    #[serde(default)]
    profile_list: Vec<ProfileState>,
    /// The server's last arrangement of folders, so a start with no network shows it.
    #[serde(default)]
    folders: Option<ChatFolders>,
    /// An arrangement the server has not taken yet.
    #[serde(default)]
    folders_out: Option<ChatFolders>,
    /// This account's settings, still sealed, with any change of this device's own on top.
    #[serde(default)]
    kv: Vec<KvItem>,
    #[serde(default)]
    kv_version: Option<u64>,
    /// A change the server has not taken yet.
    #[serde(default)]
    kv_out: Option<KvChange>,
    /// This account's own join requests and invitations as last shown, so the next `chats` list
    /// that leaves one out shows it gone, after a restart too.
    #[serde(default)]
    asked_list: Vec<RequestItem>,
    #[serde(default)]
    invitation_list: Vec<InvitationItem>,
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

/// A new group's or channel's first profile, its profile key sealed at epoch 0, and its options, all
/// going with epoch 0's group info.
#[derive(Debug, Clone)]
struct StartProfile {
    body: ChatProfileBody,
    key: Vec<u8>,
    options: Option<ChatOptions>,
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
    /// Something was refused as sealed under an older key, and a `sync` brings the commits it
    /// missed before anything is sealed again.
    catching_up: bool,
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
    /// A group's or channel's start, once its group runs.
    start: Option<GroupStart>,
    /// A new group's or channel's first profile and options, going up with epoch 0's group info.
    start_profile: Option<StartProfile>,
    /// The server's list of its members' devices has come at least once, so a group's or
    /// channel's commits wrap to everyone in it.
    members_known: bool,
    /// A group's or channel's profile, opened, and its version.
    profile: Option<(ChatProfilePlain, u64)>,
    /// Whether its topics show, and its live topics in order.
    topics: Option<TopicsBody>,
    /// A profile sealed at an epoch this device could not open yet, tried again once it is ready.
    profile_waiting: Option<ChatProfileItem>,
    /// Each topic's own settings this account may see, General's under `GENERAL_TOPIC`.
    topic_settings: HashMap<Ulid, TopicItem>,
    /// Topics hidden from this account, whose messages the server withheld until they show again.
    hidden_topics: HashSet<Ulid>,
    /// Topics shown again, whose withheld messages the next `sync` of this chat opens though they
    /// sit below `known`, with every edit, since an edit's topic is its target's.
    refill: HashSet<Ulid>,
    /// Topics' profiles, opened, and their versions.
    topic_profiles: HashMap<Ulid, (TopicProfilePlain, u64)>,
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
    /// The tag of the [`Input::Tagged`] command this step works for, taken from the input or from
    /// what the input answers.
    tagging: Option<u64>,
    /// What tagged commands asked and wait for, each with its command's tag.
    tag_keys: HashMap<TagKey, u64>,
    /// Each tagged command not yet answered.
    tagged: HashMap<u64, TagState>,
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
    /// An agent's token, so a lost session logs in again by itself.
    agent_token: Option<Secret>,
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
    /// Invitations waiting for their account's keys, which their name is sealed to.
    inviting: Vec<(Ulid, Ulid)>,
    /// The note an add's invitations carry, by chat and account, until each is sealed.
    invite_notes: HashMap<(Ulid, Ulid), String>,
    /// Every device whose stamp has been checked.
    devices: HashMap<Ulid, KnownDevice>,
    /// The last chat list the server sent, previews included, kept in the snapshot's shelf.
    chat_cache: Vec<ChatItem>,
    /// The server's last arrangement of folders, still sealed.
    folders: Option<ChatFolders>,
    /// An arrangement the server has not taken yet, sent on every connection until it is.
    folders_out: Option<ChatFolders>,
    /// Acks of accepted suggestions, each sent once the server takes its arrangement. Memory only,
    /// like the queue: one lost costs a second tap, which changes nothing more.
    acks_waiting: Vec<WaitingAck>,
    /// Whether the app is on screen, as the platform last said, sent as `presence`.
    in_app: bool,
    /// Having trouble, and its text, as the platform last said, sent with `in_app`.
    trouble: bool,
    presence_text: Option<String>,
    /// This account's settings, still sealed, sorted by name, with this device's changes on top.
    kv: Vec<KvItem>,
    /// The server's version of them, once it has said.
    kv_version: Option<u64>,
    /// Changes not yet sent, which go once the server's version is known on this connection.
    kv_out: Option<KvChange>,
    /// The change sent and not yet answered.
    kv_flight: Option<KvChange>,
    /// This connection's `kv` has been answered, so a change can be based on its version.
    kv_ready: bool,
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
    /// The search through the archive, while one is under way or can go on.
    scan: Option<Scan>,
    /// This account's devices, as handed to the platform.
    own_devices: Vec<Ulid>,
    profiles: HashMap<Ulid, ProfileState>,
    /// Profiles asked for and not yet answered.
    profile_asked: HashSet<Ulid>,
    /// Photos asked for and not yet answered.
    photo_asked: HashSet<Ulid>,
    /// This account's own profile key shared by a frame, by chat and epoch, so a list that has not
    /// caught up yet does not send it again.
    profile_shared: HashSet<(Ulid, u64)>,
    /// Keys pushed for a chat this device has not joined yet, opened once it is ready.
    profile_keys_waiting: HashMap<Ulid, Vec<ProfileKeyItem>>,
    /// This account's invitations, by chat, kept for the signature its join carries.
    invitations: HashMap<Ulid, InvitationItem>,
    /// This account's own join requests, waiting or declined, by chat, so one the server stops
    /// listing is shown gone.
    own_requests: HashMap<Ulid, RequestItem>,
    /// A login waiting for [`Input::Opened`], once [`Output::Open`] went out.
    opening: Option<Opening>,
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

/// Whether a bio and links are within the limits, and each link's address is a web one.
/// A group's or channel's profile trimmed and checked by an account's limits, with a name.
/// An invitation's note, trimmed, with an empty one dropped, as the server takes a join request's.
fn checked_note(note: Option<String>) -> Result<Option<String>, &'static str> {
    let Some(note) = note else {
        return Ok(None);
    };
    let note = note.trim().to_string();
    if note.chars().count() > INVITE_NOTE_CHARS {
        return Err("An invitation's note is at most 500 characters");
    }
    Ok((!note.is_empty()).then_some(note))
}

fn checked_chat_profile(mut profile: ChatProfilePlain) -> Result<ChatProfilePlain, &'static str> {
    profile.name = profile.name.trim().to_string();
    let length = profile.name.chars().count();
    if length == 0 || length > CHAT_NAME_CHARS {
        return Err("A name is 1 to 64 characters");
    }
    profile.bio = profile
        .bio
        .map(|bio| bio.trim().to_string())
        .filter(|bio| !bio.is_empty());
    profile.link_list = profile
        .link_list
        .into_iter()
        .map(|link| ProfileLink {
            name: link.name.trim().to_string(),
            url: link.url.trim().to_string(),
        })
        .collect();
    profile.rules = checked_rules(profile.rules)?;
    check_profile(&ProfileBody {
        bio: profile.bio.clone(),
        link_list: profile.link_list.clone(),
        photo: None,
    })?;
    Ok(profile)
}

/// A topic's bio, links and rules, trimmed and checked as a chat's are.
fn checked_topic_profile(
    mut profile: TopicProfilePlain,
) -> Result<TopicProfilePlain, &'static str> {
    profile.bio = profile
        .bio
        .map(|bio| bio.trim().to_string())
        .filter(|bio| !bio.is_empty());
    profile.link_list = profile
        .link_list
        .into_iter()
        .map(|link| ProfileLink {
            name: link.name.trim().to_string(),
            url: link.url.trim().to_string(),
        })
        .collect();
    profile.rules = checked_rules(profile.rules)?;
    check_profile(&ProfileBody {
        bio: profile.bio.clone(),
        link_list: profile.link_list.clone(),
        photo: None,
    })?;
    Ok(profile)
}

/// Rules trimmed, none when empty, and at most 2,000 characters.
fn checked_rules(rules: Option<String>) -> Result<Option<String>, &'static str> {
    let rules = rules
        .map(|rules| rules.trim().to_string())
        .filter(|rules| !rules.is_empty());
    if rules
        .as_ref()
        .is_some_and(|rules| rules.chars().count() > RULES_CHARS)
    {
        return Err("Rules are at most 2,000 characters");
    }
    Ok(rules)
}

fn check_profile(body: &ProfileBody) -> Result<(), &'static str> {
    if body
        .bio
        .as_ref()
        .is_some_and(|bio| bio.chars().count() > BIO_CHARS)
    {
        return Err("A bio is at most 140 characters");
    }
    if body.link_list.len() > LINKS {
        return Err("A profile has at most 3 links");
    }
    for link in &body.link_list {
        let name = link.name.chars().count();
        if name == 0 || name > LINK_NAME_CHARS {
            return Err("A link's name is 1 to 32 characters");
        }
        if link.url.chars().count() > LINK_URL_CHARS {
            return Err("A link's address is at most 512 characters");
        }
        let lower = link.url.to_ascii_lowercase();
        let rest = lower
            .strip_prefix("https://")
            .or_else(|| lower.strip_prefix("http://"));
        if rest.is_none_or(str::is_empty) {
            return Err("A link's address starts with https:// or http://");
        }
    }
    Ok(())
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
            | Asked::PutProfile { .. }
            | Asked::AddPhoto { .. }
            | Asked::ChatPhotoFile { .. }
            | Asked::ChatPhotoChunk { .. }
            | Asked::Names
            | Asked::CreateAgent { .. }
            | Asked::AgentKeys { .. }
            | Asked::Agents
            | Asked::AgentChanged
            | Asked::AgentDevice { .. }
            | Asked::AgentEnded { .. }
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

/// A search through the archive: what it looks for, folded, how far it has read, and which
/// messages it has already judged by their newest text.
struct Scan {
    text: Secret,
    needle: Vec<char>,
    /// The archive read under way, whose answer is the only one taken.
    reading: Option<u64>,
    before: Option<ScanAt>,
    scanned: u64,
    /// Hits in the page under way.
    found: usize,
    /// Messages already judged, by a newer edit or a delete.
    edited: HashSet<(Ulid, u64)>,
    more: bool,
}

/// Text as a search compares it, chosen by the author: lower case, Arabic ي, ى and ك as Persian ی
/// and ک, Persian and Arabic digits as ASCII ones, and no ZWNJ or short vowel marks. Each folded
/// character carries the index of the character it came from.
fn fold(text: &str) -> (Vec<char>, Vec<usize>) {
    let mut folded = Vec::with_capacity(text.len());
    let mut from = Vec::with_capacity(text.len());
    for (at, character) in text.chars().enumerate() {
        let one = match character {
            '\u{200C}' | '\u{064B}'..='\u{065F}' | '\u{0670}' => continue,
            '\u{064A}' | '\u{0649}' => '\u{06CC}',
            '\u{0643}' => '\u{06A9}',
            '\u{0660}'..='\u{0669}' => {
                char::from_digit(u32::from(character) - 0x0660, 10).unwrap_or(character)
            }
            '\u{06F0}'..='\u{06F9}' => {
                char::from_digit(u32::from(character) - 0x06F0, 10).unwrap_or(character)
            }
            other => other,
        };
        for lower in one.to_lowercase() {
            folded.push(lower);
            from.push(at);
        }
    }
    (folded, from)
}

/// A folder's name as accepting a suggestion matches it: trimmed, then folded as search folds. A
/// client asks it to see which folder accepting will choose, before it does.
pub fn fold_name(name: &str) -> String {
    fold(name.trim()).0.into_iter().collect()
}

/// The field split around the first place it holds `needle`, both folded, or nothing.
fn found_in(field: &str, needle: &[char]) -> Option<(String, String, String)> {
    let (folded, from) = fold(field);
    let start = folded
        .windows(needle.len())
        .position(|window| window == needle)?;
    let first = from[start];
    let last = from[start + needle.len() - 1] + 1;
    let characters: Vec<char> = field.chars().collect();
    Some((
        characters[..first].iter().collect(),
        characters[first..last].iter().collect(),
        characters[last..].iter().collect(),
    ))
}

/// What a message holds once opened.
#[derive(Default)]
struct Opened {
    text: Option<Secret>,
    files: Vec<FileInfo>,
    call: Option<CallBody>,
    reply: Option<u64>,
    sticker: Option<Ulid>,
    emoji: Option<String>,
    on: Option<u64>,
    question: Option<Question>,
    answer: Option<Answer>,
    suggestion: Option<Suggestion>,
    ack: Option<Ack>,
    mentions: Vec<Ulid>,
}

/// A message's text and files, `{"text": "...", "files": [...]}`, each optional, a call's start,
/// `{"video": ..., "voice": ...}`, a sticker, `{"sticker": "..."}`, a reaction, `{"emoji", "on"}`,
/// a question's tree, `{"text"?, "first", "nodes"}`, or an answer, `{"question", "answers",
/// "skipped"?}`. A tree that does not hold together is left out, and so is a reaction whose emoji the
/// `emojis` crate does not know. A suggestion's action, `{"text"?, <action>}`, and an ack,
/// `{"suggestion", "accepted", "created"?, "left"?}`, are read by `content_kind`, and left out when
/// they do not hold together.
fn content_of(plaintext: &[u8], content_kind: Option<ContentKind>) -> Opened {
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
        emoji: Option<String>,
        #[serde(default)]
        on: Option<u64>,
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
        #[serde(default)]
        mentions: Vec<Ulid>,
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
            // Written as the crate writes it, so a chip is one per emoji however it was typed.
            let (emoji, on) = match (plain.emoji.as_deref().and_then(emojis::get), plain.on) {
                (Some(emoji), Some(on)) => (Some(emoji.as_str().to_string()), Some(on)),
                _ => (None, None),
            };
            // An activity's text is 1 to 140 characters, or it is left out.
            let mut text = plain.text;
            if content_kind == Some(ContentKind::Activity)
                && let Some(activity) = &text
                && !(1..=ACTIVITY_CHARS).contains(&activity.chars().count())
            {
                text = None;
            }
            Opened {
                reply: plain.reply,
                text: text.map(Secret),
                files: plain.files,
                call: match (plain.video, plain.voice) {
                    (Some(video), Some(voice)) => Some(CallBody { video, voice }),
                    _ => None,
                },
                sticker: plain.sticker,
                emoji,
                on,
                question,
                answer,
                suggestion: match content_kind {
                    Some(ContentKind::Suggestion) => Suggestion::from_plaintext(plaintext),
                    _ => None,
                },
                ack: match content_kind {
                    Some(ContentKind::Ack) => serde_json::from_slice::<Ack>(plaintext)
                        .ok()
                        .filter(|ack| ack.check().is_ok()),
                    _ => None,
                },
                mentions: plain.mentions,
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
        emoji,
        on,
        question,
        answer,
        suggestion,
        ack,
        mentions,
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
        emoji,
        on,
        question,
        answer,
        suggestion: suggestion.map(Box::new),
        ack: ack.map(Box::new),
        topic: message.topic,
        thread_seq: message.thread_seq,
        mentions,
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
            agent_token: None,
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
            tagging: None,
            tag_keys: HashMap::new(),
            tagged: HashMap::new(),
            queue: Queue::default(),
            in_flight: HashMap::new(),
            commit_busy: false,
            verifying: 0,
            chats: HashMap::new(),
            accounts: HashMap::new(),
            asked_accounts: HashSet::new(),
            inviting: Vec::new(),
            invite_notes: HashMap::new(),
            devices: HashMap::new(),
            chat_cache: Vec::new(),
            folders: None,
            folders_out: None,
            acks_waiting: Vec::new(),
            in_app: false,
            trouble: false,
            presence_text: None,
            kv: Vec::new(),
            kv_version: None,
            kv_out: None,
            kv_flight: None,
            kv_ready: false,
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
            scan: None,
            own_devices: Vec::new(),
            profiles: HashMap::new(),
            profile_asked: HashSet::new(),
            photo_asked: HashSet::new(),
            profile_shared: HashSet::new(),
            profile_keys_waiting: HashMap::new(),
            invitations: HashMap::new(),
            own_requests: HashMap::new(),
            opening: None,
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
        self.tagging = match &input {
            Input::Tagged { tag, .. } => {
                self.tagged.entry(*tag).or_default();
                Some(*tag)
            }
            Input::Reply { id, .. } | Input::Fetched { id, .. } => self.untag(TagKey::Request(*id)),
            Input::Saved { id } | Input::SaveFailed { id } => self.untag(TagKey::Save(*id)),
            Input::RowsSaved { id } | Input::RowsSaveFailed { id } => self.untag(TagKey::Rows(*id)),
            _ => None,
        };
        match input {
            Input::Command { command } | Input::Tagged { command, .. } => self.command(command),
            Input::Loaded {
                first,
                second,
                resume,
                token,
            } => self.loaded(first, second, resume, token),
            Input::Opened { first, second } => self.opened(first, second),
            Input::Saved { id } => self.saved(id),
            Input::SaveFailed { id } => self.save_failed(id),
            Input::Connected => self.connected(),
            Input::Closed { code, reason } => self.closed(code, &reason),
            Input::Frame { text } => {
                self.last_heard = now;
                self.ping_sent_at = None;
                match serde_json::from_str::<ServerFrame>(&text) {
                    Ok(frame) => {
                        if let Some(reference) = frame.reference()
                            && let Some(tag) = self.untag(TagKey::Frame(reference))
                        {
                            self.tagging = Some(tag);
                        }
                        self.frame(frame)
                    }
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
                chat_event_list,
            } => self.rows_read(
                id,
                Rows {
                    message_list,
                    call_list,
                    chat_event_list,
                },
            ),
            Input::RowsScanned {
                id,
                message_list,
                total,
            } => self.rows_scanned(id, message_list, total),
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
        self.tag_outputs();
        std::mem::take(&mut self.out)
    }

    /// Takes what a tagged command waited for off its list, and gives its tag.
    fn untag(&mut self, key: TagKey) -> Option<u64> {
        let tag = self.tag_keys.remove(&key)?;
        if let Some(state) = self.tagged.get_mut(&tag) {
            state.open = state.open.saturating_sub(1);
        }
        Some(tag)
    }

    /// Puts what this step asked on its tagged command's list.
    fn tag_open(&mut self, key: TagKey) {
        if let Some(tag) = self.tagging
            && let Some(state) = self.tagged.get_mut(&tag)
        {
            state.open += 1;
            self.tag_keys.insert(key, tag);
        }
    }

    /// Reads this step's outputs for what tagged commands asked and how they were answered, then
    /// tells each command whose list is empty.
    fn tag_outputs(&mut self) {
        let mut found = Vec::new();
        for output in &self.out {
            match output {
                Output::Request { id, .. }
                | Output::PutChunk { id, .. }
                | Output::GetChunk { id, .. } => found.push(Tagging::Open(TagKey::Request(*id))),
                Output::Save { id, .. } => found.push(Tagging::Open(TagKey::Save(*id))),
                Output::SaveRows { id, .. } => found.push(Tagging::Open(TagKey::Rows(*id))),
                Output::Event { event } => match event.as_ref() {
                    Event::Pending { local, .. } => {
                        found.push(Tagging::Open(TagKey::Local(*local)));
                    }
                    Event::Sent { local, seq, .. } => found.push(Tagging::Sent(*local, *seq)),
                    Event::Failed {
                        local,
                        code,
                        seconds,
                        ..
                    } => found.push(Tagging::Failed(*local, code.clone(), *seconds)),
                    Event::Error { code, message } => {
                        found.push(Tagging::Error(code.clone(), message.clone()));
                    }
                    Event::TransferFailed { code, .. } => {
                        found.push(Tagging::Error(code.clone(), String::new()));
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        for item in found {
            match item {
                Tagging::Open(key) => self.tag_open(key),
                Tagging::Sent(local, seq) => {
                    if let Some(tag) = self.untag(TagKey::Local(local))
                        && let Some(state) = self.tagged.get_mut(&tag)
                    {
                        state.seq = Some(seq);
                    }
                }
                Tagging::Failed(local, code, seconds) => {
                    if let Some(tag) = self.untag(TagKey::Local(local))
                        && let Some(state) = self.tagged.get_mut(&tag)
                        && state.code.is_none()
                    {
                        state.code = Some(code);
                        state.seconds = seconds;
                    }
                }
                Tagging::Error(code, message) => {
                    if let Some(tag) = self.tagging
                        && let Some(state) = self.tagged.get_mut(&tag)
                        && state.code.is_none()
                    {
                        state.code = Some(code);
                        state.message = Some(message).filter(|message| !message.is_empty());
                    }
                }
            }
        }
        self.tagging = None;
        let mut answered: Vec<u64> = Vec::new();
        for (tag, state) in &self.tagged {
            if state.open == 0 {
                answered.push(*tag);
            }
        }
        answered.sort_unstable();
        for tag in answered {
            if let Some(state) = self.tagged.remove(&tag) {
                self.event(Event::Answered {
                    tag,
                    seq: state.seq,
                    code: state.code,
                    message: state.message,
                    seconds: state.seconds,
                });
            }
        }
    }

    // ---------------------------------------------------------------------------------------------
    // Plumbing
    // ---------------------------------------------------------------------------------------------

    fn event(&mut self, event: Event) {
        self.out.push(Output::Event {
            event: Box::new(event),
        });
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
                method: request.method.clone(),
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
            self.tag_open(TagKey::Frame(reference));
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
            profile_list: self.profiles.values().cloned().collect(),
            folders: self.folders.clone(),
            folders_out: self.folders_out.clone(),
            kv: self.kv.clone(),
            kv_version: self.kv_version,
            kv_out: match (self.kv_flight.clone(), self.kv_out.clone()) {
                (flight, Some(out)) => Some(merge_change(flight, out)),
                (flight, None) => flight,
            },
            asked_list: self.own_requests.values().cloned().collect(),
            invitation_list: self.invitations.values().cloned().collect(),
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
            Command::LogInAgent { token } => self.log_in_agent(token),
            Command::CreateAgent {
                username,
                display_name,
                permission_list,
                admin,
            } => self.create_agent(username, display_name, permission_list, admin),
            Command::Agents => self.get("/api/v1/agents".to_string(), Asked::Agents),
            Command::UpdateAgent {
                agent,
                display_name,
                permission_list,
                admin,
            } => {
                let body = UpdateAgentBody {
                    display_name,
                    permission_list,
                    admin,
                };
                self.request(
                    "PUT",
                    format!("/api/v1/agents/{agent}"),
                    Some(&body),
                    Asked::AgentChanged,
                );
            }
            Command::RenewAgentToken { agent, words } => {
                self.end_agent(agent, words, AgentEnd::Renew);
            }
            Command::RevokeAgentToken { agent, words } => {
                self.end_agent(agent, words, AgentEnd::Revoke);
            }
            Command::DisableAgent { agent, words } => {
                self.end_agent(agent, words, AgentEnd::Disable);
            }
            Command::AgentProfile {
                agent,
                bio,
                link_list,
            } => {
                let Some(mut body) = self.profile_body(agent) else {
                    return;
                };
                body.bio = bio
                    .map(|bio| bio.trim().to_string())
                    .filter(|bio| !bio.is_empty());
                body.link_list = link_list
                    .into_iter()
                    .map(|link| ProfileLink {
                        name: link.name.trim().to_string(),
                        url: link.url.trim().to_string(),
                    })
                    .collect();
                if let Err(message) = check_profile(&body) {
                    return self.error("bad_profile", message);
                }
                self.put_profile(agent, body);
            }
            Command::AgentPhoto {
                agent,
                jpeg,
                thumbnail,
            } => self.set_photo(agent, &jpeg, &thumbnail),
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
                let body = CreateChatBody {
                    account: Some(account),
                    chat_type: None,
                    topics: None,
                    findable: None,
                };
                self.request(
                    "POST",
                    "/api/v1/chats".to_string(),
                    Some(&body),
                    Asked::CreateChat { account },
                );
            }
            Command::SearchMessages { text } => {
                let (needle, _) = fold(text.0.trim());
                self.scan = None;
                if !needle.is_empty() {
                    self.scan = Some(Scan {
                        text: text.clone(),
                        needle,
                        reading: None,
                        before: None,
                        scanned: 0,
                        found: 0,
                        edited: HashSet::new(),
                        more: true,
                    });
                    self.scan_rows();
                }
            }
            Command::SearchOlder => {
                if let Some(scan) = self.scan.as_mut()
                    && scan.reading.is_none()
                    && scan.more
                {
                    scan.found = 0;
                    self.scan_rows();
                }
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
            Command::SendText {
                chat,
                text,
                reply,
                topic,
                thread,
                mentions,
            } => {
                // Mentions stay inside the encryption, each a member of the chat.
                let mut fields = None;
                if !mentions.is_empty() {
                    let state = self.chat(chat);
                    if mentions
                        .iter()
                        .any(|account| !state.member_list.contains(account))
                    {
                        return self.error("not_member", "Someone mentioned is not in this chat");
                    }
                    fields = Some(Secret(
                        serde_json::json!({ "mentions": mentions }).to_string(),
                    ));
                }
                self.push_content(
                    chat,
                    Kind::Content,
                    reply,
                    Content {
                        content_kind: Some(ContentKind::Text),
                        text: Some(text),
                        fields,
                        topic,
                        thread_seq: thread,
                        ..Content::default()
                    },
                );
            }
            Command::SendFiles {
                chat,
                files,
                text,
                topic,
                thread,
                reply,
            } => self.send_files(chat, files, text, reply, (topic, thread)),
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
            Command::Seen {
                chat,
                up_to_seq,
                topic,
                thread,
            } if topic.is_some() || thread.is_some() => {
                self.seen_in_place(chat, up_to_seq, topic, thread);
            }
            Command::Seen {
                chat, up_to_seq, ..
            } => {
                let now = self.now;
                let state = self.chat(chat);
                if up_to_seq > state.seen.wanted {
                    state.seen.wanted = up_to_seq;
                    state.seen.at = now;
                }
                self.report(chat);
            }
            Command::Typing {
                chat,
                on,
                topic,
                thread,
            } => {
                // Typing is stale by the time a connection comes back, so it is not kept.
                if self.socket == Socket::Up {
                    let item = Item::Typing {
                        chat,
                        on,
                        topic,
                        thread_seq: thread,
                    };
                    self.queue.push(Level::Typing, item, Target::None);
                }
            }
            Command::Draft { chat, draft } => self.keep_draft(chat, draft),
            Command::FolderId => {
                let reference = reference();
                let frame = ClientFrame::FolderId { reference };
                if !self.ask(frame, reference, Waiting::FolderId) {
                    self.error("offline", "A new folder needs a connection");
                }
            }
            Command::Folders {
                folder_list,
                default,
            } => {
                self.arrange(folder_list, default);
            }
            Command::InApp {
                in_app,
                trouble,
                text,
            } => {
                self.in_app = in_app;
                self.trouble = trouble;
                self.presence_text = text;
                if self.socket == Socket::Up {
                    self.send_presence();
                }
            }
            Command::PresenceOf { account } => {
                let reference = reference();
                let frame = ClientFrame::PresenceOf { reference, account };
                if !self.ask(frame, reference, Waiting::Done) {
                    self.error(
                        "offline",
                        "Not connected, so the presence could not be read",
                    );
                }
            }
            Command::Kv { set, remove } => {
                if set.iter().any(|setting| !setting_fits(setting))
                    || remove.iter().any(|key| !setting_name_fits(key))
                {
                    return self.error("bad_setting", "A setting is not one this client may keep");
                }
                let Some(device) = self.device.as_ref() else {
                    return;
                };
                let mut sealed_list = Vec::with_capacity(set.len());
                for setting in set {
                    let Ok(plain) = serde_json::to_vec(&setting.value) else {
                        return self.error("internal", "A setting could not be written");
                    };
                    if unsealed(&setting.key) {
                        sealed_list.push(KvItem {
                            key: setting.key,
                            value: Bytes(plain),
                        });
                        continue;
                    }
                    match device.seal_kv(&setting.key, &plain) {
                        Ok(sealed) => sealed_list.push(KvItem {
                            key: setting.key,
                            value: Bytes(sealed),
                        }),
                        Err(_) => return self.error("seal", "A setting could not be sealed"),
                    }
                }
                apply_change(&mut self.kv, &sealed_list, &remove);
                let change = KvChange {
                    version: 0,
                    set: sealed_list,
                    remove,
                };
                self.kv_out = Some(merge_change(self.kv_out.take(), change));
                self.dirty = true;
                self.show_kv();
                self.send_kv();
            }
            Command::Times { chat, seq, after } => {
                let reference = reference();
                let frame = ClientFrame::Times {
                    reference,
                    chat,
                    seq,
                    after,
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
                topic,
                thread,
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
                        topic,
                        thread_seq: thread,
                        ..Content::default()
                    },
                );
            }
            Command::SendActivity {
                chat,
                text,
                topic,
                thread,
            } => {
                let count = text.chars().count();
                if count == 0 || count > ACTIVITY_CHARS {
                    return self.error("bad_activity", "An activity is 1 to 140 characters");
                }
                let fields = serde_json::json!({ "text": text }).to_string();
                self.push_content(
                    chat,
                    Kind::Content,
                    None,
                    Content {
                        content_kind: Some(ContentKind::Activity),
                        fields: Some(Secret(fields)),
                        topic,
                        thread_seq: thread,
                        ..Content::default()
                    },
                );
            }
            Command::SendReaction {
                chat,
                emoji,
                on,
                topic,
                thread,
            } => {
                let Some(emoji) = emojis::get(&emoji) else {
                    return self.error("bad_emoji", "That is not an emoji a reaction can be");
                };
                let emoji = emoji.as_str().to_string();
                let fields = serde_json::json!({ "emoji": emoji }).to_string();
                self.push_content(
                    chat,
                    Kind::Content,
                    Some(on),
                    Content {
                        content_kind: Some(ContentKind::Reaction),
                        fields: Some(Secret(fields)),
                        emoji: Some(emoji),
                        topic,
                        thread_seq: thread,
                        ..Content::default()
                    },
                );
            }
            Command::Emoji => {
                let group_list = emojis::Group::iter()
                    .map(|group| EmojiGroup {
                        group: match group {
                            emojis::Group::SmileysAndEmotion => "smileys_and_emotion",
                            emojis::Group::PeopleAndBody => "people_and_body",
                            emojis::Group::AnimalsAndNature => "animals_and_nature",
                            emojis::Group::FoodAndDrink => "food_and_drink",
                            emojis::Group::TravelAndPlaces => "travel_and_places",
                            emojis::Group::Activities => "activities",
                            emojis::Group::Objects => "objects",
                            emojis::Group::Symbols => "symbols",
                            emojis::Group::Flags => "flags",
                        }
                        .to_string(),
                        emoji_list: group
                            .emojis()
                            .map(|emoji| EmojiItem {
                                emoji: emoji.as_str().to_string(),
                                name: emoji.name().to_string(),
                                shortcode_list: emoji.shortcodes().map(str::to_string).collect(),
                                tone_list: match emoji.skin_tones() {
                                    Some(tones) => tones
                                        .skip(1)
                                        .map(|tone| tone.as_str().to_string())
                                        .collect(),
                                    None => Vec::new(),
                                },
                            })
                            .collect(),
                    })
                    .collect();
                self.event(Event::Emoji { group_list });
            }
            Command::SendQuestion {
                chat,
                text,
                question,
                reply,
                topic,
                thread,
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
                    reply,
                    Content {
                        content_kind: Some(ContentKind::Question),
                        text,
                        fields: Some(Secret(fields)),
                        topic,
                        thread_seq: thread,
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
            Command::SendSuggestion {
                chat,
                text,
                suggestion,
                reply,
                topic,
                thread,
            } => {
                if let Some(text) = &text
                    && text.0.chars().count() > crate::protocol::SUGGESTION_TEXT
                {
                    return self.error("bad_suggestion", "A suggestion's text is too long");
                }
                if let Err(error) = suggestion.check() {
                    return self.error("bad_suggestion", &error.to_string());
                }
                let fields = match serde_json::to_string(&suggestion) {
                    Ok(fields) => fields,
                    Err(_) => return self.error("internal", "Could not write the suggestion"),
                };
                self.push_content(
                    chat,
                    Kind::Content,
                    reply,
                    Content {
                        content_kind: Some(ContentKind::Suggestion),
                        text,
                        fields: Some(Secret(fields)),
                        topic,
                        thread_seq: thread,
                        ..Content::default()
                    },
                );
            }
            Command::AcceptSuggestion {
                chat,
                seq,
                suggestion,
            } => match suggestion {
                Suggestion::Folder(folder) => self.accept(chat, seq, folder, None),
                Suggestion::Unknown => {
                    self.error("bad_suggestion", "This suggestion needs a newer keryx")
                }
            },
            Command::DeclineSuggestion { chat, seq } => self.send_ack(
                chat,
                Ack {
                    suggestion: seq,
                    accepted: false,
                    created: None,
                    left: None,
                },
            ),
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
            Command::Profile { account } => {
                if self.profiles.contains_key(&account) {
                    self.show_profile(account);
                } else {
                    self.ask_account(account);
                    self.ask_profile(account);
                }
            }
            Command::Photo { account } => self.ask_photo(account),
            Command::SaveProfile { bio, link_list } => {
                let Some(me) = self.account else {
                    return self.error("not_logged_in", "Log in first");
                };
                let Some(mut body) = self.profile_body(me) else {
                    return;
                };
                body.bio = bio
                    .map(|bio| bio.trim().to_string())
                    .filter(|bio| !bio.is_empty());
                body.link_list = link_list
                    .into_iter()
                    .map(|link| ProfileLink {
                        name: link.name.trim().to_string(),
                        url: link.url.trim().to_string(),
                    })
                    .collect();
                if let Err(message) = check_profile(&body) {
                    return self.error("bad_profile", message);
                }
                self.put_profile(me, body);
            }
            Command::SetPhoto { jpeg, thumbnail } => {
                let Some(me) = self.account else {
                    return self.error("not_logged_in", "Log in first");
                };
                self.set_photo(me, &jpeg, &thumbnail);
            }
            Command::RemovePhoto => {
                let Some(me) = self.account else {
                    return self.error("not_logged_in", "Log in first");
                };
                let Some(mut body) = self.profile_body(me) else {
                    return;
                };
                body.photo = None;
                self.put_profile(me, body);
            }
            Command::SetNames {
                display_name,
                username,
                password,
            } => {
                let Some(account) = self.account else {
                    return self.error("not_logged_in", "Log in first");
                };
                let body = NamesBody {
                    display_name,
                    username,
                    password: password.map(|password| password.0.clone()),
                };
                self.request(
                    "PUT",
                    format!("/api/v1/accounts/{account}/names"),
                    Some(&body),
                    Asked::Names,
                );
            }
            Command::CreateChat {
                chat_type,
                name,
                bio,
                link_list,
                account_list,
                topics,
                findable,
                options,
                photo,
                note,
            } => {
                let note = match checked_note(note) {
                    Ok(note) => note,
                    Err(message) => return self.error("bad_note", message),
                };
                let profile = ChatProfilePlain {
                    name,
                    bio,
                    link_list,
                    ..ChatProfilePlain::default()
                };
                // A photo that cannot be prepared is the same as one that fails to go up.
                let photo =
                    photo.and_then(|photo| match prepare_photo(&photo.jpeg, &photo.thumbnail) {
                        Ok(prepared) => Some(prepared),
                        Err(message) => {
                            self.error("photo_failed", message);
                            None
                        }
                    });
                self.create_group(
                    chat_type,
                    GroupStart {
                        profile,
                        options,
                        account_list,
                        photo,
                        note,
                    },
                    topics,
                    findable,
                );
            }
            Command::ChatPhoto {
                chat,
                jpeg,
                thumbnail,
            } => match prepare_photo(&jpeg, &thumbnail) {
                Ok(photo) => self.upload_chat_photo(chat, None, photo, false),
                Err(message) => self.error("bad_files", message),
            },
            Command::ChatPhotoGet { chat } => self.ask_chat_photo(chat, None),
            Command::AddMembers {
                chat,
                account_list,
                note,
            } => {
                let note = match checked_note(note) {
                    Ok(note) => note,
                    Err(message) => return self.error("bad_note", message),
                };
                self.add_members(chat, account_list, note);
            }
            Command::LetIn { chat, account } => self.add_members(chat, vec![account], None),
            Command::RemoveMembers { chat, account_list } => {
                self.remove_members(chat, account_list)
            }
            Command::SetRole {
                chat,
                account,
                role,
            } => {
                let reference = reference();
                let frame = ClientFrame::ChatRole {
                    reference,
                    chat,
                    account,
                    role,
                };
                self.ask_done(frame, reference, Waiting::Done);
            }
            Command::Leave { chat } => {
                let reference = reference();
                let frame = ClientFrame::Leave { reference, chat };
                self.ask_done(frame, reference, Waiting::Left { chat });
            }
            Command::RemoveChat { chat } => {
                let reference = reference();
                let frame = ClientFrame::ChatRemove { reference, chat };
                self.ask_done(frame, reference, Waiting::Left { chat });
            }
            Command::AskToJoin { chat, note } => {
                let reference = reference();
                let frame = ClientFrame::JoinRequest {
                    reference,
                    chat,
                    note,
                };
                self.ask_done(frame, reference, Waiting::Done);
            }
            Command::CancelAsk { chat } => {
                let reference = reference();
                let frame = ClientFrame::JoinCancel { reference, chat };
                self.ask_done(frame, reference, Waiting::Done);
            }
            Command::DeclineAsk { chat, account } => {
                let reference = reference();
                let frame = ClientFrame::JoinDecline {
                    reference,
                    chat,
                    account,
                };
                self.ask_done(frame, reference, Waiting::Done);
            }
            Command::Requests { chat } => {
                self.get(
                    format!("/api/v1/chats/{chat}/requests"),
                    Asked::Requests { chat },
                );
            }
            Command::AnswerInvite { chat, accept } => {
                // The invitation holds the chat's profile key, which opens its profile with nobody
                // online, and which the join commit shares with this account's other devices.
                let sealed = self
                    .invitations
                    .get(&chat)
                    .map(|item| item.sealed.0.clone());
                if accept
                    && let (Some(sealed), Some(device)) = (sealed, self.device.as_mut())
                    && let Ok(opened) = device.open_invitation(chat, &sealed)
                    && let Ok(plain) = serde_json::from_slice::<InvitationPlain>(&opened)
                    && let Some(key) = fixed::<16>(&plain.key.0)
                {
                    device.set_chat_key(chat, key);
                    self.dirty = true;
                }
                let reference = reference();
                let frame = ClientFrame::InviteAnswer {
                    reference,
                    chat,
                    accept,
                };
                self.ask_done(frame, reference, Waiting::InviteAnswer { chat, accept });
            }
            Command::Members { chat } => {
                let reference = reference();
                let frame = ClientFrame::Members { reference, chat };
                self.ask_done(frame, reference, Waiting::Members);
            }
            Command::OpenHistory { chat, event_list } => {
                for event in event_list {
                    if event.chat == chat {
                        self.show_chat_event(event);
                    }
                }
            }
            Command::TopicId { chat } => {
                let reference = reference();
                let frame = ClientFrame::TopicId { reference, chat };
                self.ask_done(frame, reference, Waiting::TopicId);
            }
            Command::Topics {
                chat,
                on,
                topic_list,
            } => self.set_topics(chat, on, topic_list),
            Command::ChatProfile {
                chat,
                name,
                bio,
                link_list,
                rules,
            } => {
                let profile = ChatProfilePlain {
                    name,
                    bio,
                    link_list,
                    rules,
                    ..ChatProfilePlain::default()
                };
                self.edit_chat_profile(chat, profile);
            }
            Command::TopicProfile {
                chat,
                topic,
                bio,
                link_list,
                rules,
            } => {
                let mut profile = match checked_topic_profile(TopicProfilePlain {
                    bio,
                    link_list,
                    photo: None,
                    rules,
                }) {
                    Ok(profile) => profile,
                    Err(message) => return self.error("bad_profile", message),
                };
                profile.photo = self
                    .chat(chat)
                    .topic_profiles
                    .get(&topic)
                    .and_then(|(current, _)| current.photo.clone());
                self.send_topic_profile(chat, topic, profile);
            }
            Command::TopicPhoto {
                chat,
                topic,
                jpeg,
                thumbnail,
            } => match prepare_photo(&jpeg, &thumbnail) {
                Ok(photo) => self.upload_chat_photo(chat, Some(topic), photo, false),
                Err(message) => self.error("bad_files", message),
            },
            Command::TopicPhotoGet { chat, topic } => self.ask_chat_photo(chat, Some(topic)),
            Command::TopicOptions {
                chat,
                topic,
                options,
            } => {
                let reference = reference();
                let frame = ClientFrame::TopicOptions {
                    reference,
                    chat,
                    topic,
                    options: Box::new(options),
                };
                self.ask_done(frame, reference, Waiting::Done);
            }
            Command::TopicRole {
                chat,
                topic,
                account,
                role,
            } => {
                let reference = reference();
                let frame = ClientFrame::TopicRole {
                    reference,
                    chat,
                    topic,
                    account,
                    role,
                };
                self.ask_done(frame, reference, Waiting::Done);
            }
            Command::ChatOptions { chat, options } => {
                let reference = reference();
                let frame = ClientFrame::ChatOptions {
                    reference,
                    chat,
                    options: options.clone(),
                };
                self.ask_done(frame, reference, Waiting::OptionsSaved { chat, options });
            }
            Command::Findable {
                chat,
                findable,
                name,
                bio,
            } => {
                let reference = reference();
                let shown = match (findable, &name) {
                    (true, Some(name)) => Some(FindableItem {
                        name: name.clone(),
                        bio: bio.clone(),
                    }),
                    _ => None,
                };
                let frame = ClientFrame::Findable {
                    reference,
                    chat,
                    findable: FindableBody {
                        findable,
                        name,
                        bio,
                    },
                };
                self.ask_done(
                    frame,
                    reference,
                    Waiting::FindableSaved {
                        chat,
                        findable: shown,
                    },
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
                // A socket that outlived the network's loss: the platform's own state may have
                // moved off connected, so it hears it again, and one `sync` brings the list that
                // tells it the server answered, with anything the flap cost.
                if self.socket == Socket::Up {
                    self.event(Event::Online);
                    self.start_sync(None, self.since);
                }
            }
        }
    }

    /// Asks for a challenge first, even with no device loaded, since its answer names the account
    /// whose slots a platform keeps by account id.
    fn log_in(&mut self, username: String, password: Secret, captcha: Option<CaptchaAnswer>) {
        self.opening = None;
        let body = ChallengeBody {
            username: Some(username.clone()),
            account: None,
            purpose: "device_login".to_string(),
        };
        let credentials = Credentials::Password {
            username,
            password,
            captcha,
        };
        self.request(
            "POST",
            "/api/v1/challenge".to_string(),
            Some(&body),
            Asked::Challenge { credentials },
        );
    }

    /// An agent logs in with its token, asking its challenge by the account the token names. It is
    /// kept, so a lost session logs in again by itself.
    fn log_in_agent(&mut self, token: Secret) {
        self.opening = None;
        let Some(account) = agent_of_token(&token.0) else {
            return self.error("bad_token", "That is not an agent's token");
        };
        self.agent_token = Some(token.clone());
        let body = ChallengeBody {
            username: None,
            account: Some(account),
            purpose: "device_login".to_string(),
        };
        self.request(
            "POST",
            "/api/v1/challenge".to_string(),
            Some(&body),
            Asked::Challenge {
                credentials: Credentials::Agent { token },
            },
        );
    }

    /// Logs in proving the loaded device with the challenge, or with no device.
    fn prove_and_log_in(&mut self, credentials: Credentials, challenge: Bytes) {
        let Some(device) = self.device.as_ref() else {
            return self.post_login(credentials, None);
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
        self.post_login(credentials, Some(proof));
    }

    /// The slots for the account a login found. A device that does not load is reported, and
    /// the login goes on without it, as a new device.
    fn opened(&mut self, first: Option<Vec<u8>>, second: Option<Vec<u8>>) {
        let Some(Opening {
            account,
            credentials,
            challenge,
        }) = self.opening.take()
        else {
            return;
        };
        if first.is_some() || second.is_some() {
            match Device::import(first.as_deref(), second.as_deref()) {
                Ok(mut imported) if imported.device.record().account == account => {
                    // The queue that held any commit still waiting is gone with the last run.
                    match imported.device.drop_pending_commits() {
                        Ok(dropped) => self.dirty = dropped,
                        Err(error) => self.error("load_failed", &error.to_string()),
                    }
                    self.account = Some(account);
                    self.since = imported.device.since();
                    self.device = Some(imported.device);
                    self.unshelve();
                }
                Ok(_) => self.error("wrong_account", "This device belongs to another account"),
                Err(error) => self.error("load_failed", &error.to_string()),
            }
        }
        self.prove_and_log_in(credentials, challenge);
    }

    fn post_login(&mut self, credentials: Credentials, device: Option<DeviceProof>) {
        let with_device = device.is_some();
        match &credentials {
            Credentials::Password {
                username,
                password,
                captcha,
            } => {
                let body = LoginBody {
                    username: username.clone(),
                    password: password.0.clone(),
                    captcha: captcha.clone(),
                    device,
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
                        credentials,
                        with_device,
                    },
                );
            }
            Credentials::Agent { token } => {
                let body = AgentLoginBody {
                    token: token.0.clone(),
                    device,
                };
                self.request(
                    "POST",
                    "/api/v1/login/agent".to_string(),
                    Some(&body),
                    Asked::Login {
                        credentials,
                        with_device,
                    },
                );
            }
        }
    }

    /// Makes 12 words for the agent first, then its account. Its keys follow from the words once
    /// the server gives its id.
    fn create_agent(
        &mut self,
        username: String,
        display_name: String,
        permission_list: Vec<String>,
        admin: bool,
    ) {
        let words = match seed::generate(12) {
            Ok(words) => Secret(words.to_string()),
            Err(error) => return self.error("bad_words", &error.to_string()),
        };
        let body = CreateAgentBody {
            username: username.clone(),
            display_name,
            permission_list,
            admin,
        };
        self.request(
            "POST",
            "/api/v1/agents".to_string(),
            Some(&body),
            Asked::CreateAgent { username, words },
        );
    }

    /// The agent exists: publish its keys from its words, and keep only its profile key, so this
    /// device can seal its bio and photo. Its master key is never kept.
    fn agent_made(&mut self, account: Ulid, username: String, token: Secret, words: Secret) {
        let keys = match seed::derive(&words.0) {
            Ok(keys) => keys,
            Err(error) => return self.error("bad_words", &error.to_string()),
        };
        let body = self.keys_body(&keys, account);
        if let Some(device) = self.device.as_mut() {
            device.keep_profile_key(account, keys.profile());
            self.dirty = true;
        }
        self.profiles.insert(
            account,
            ProfileState {
                account,
                version: 0,
                username: username.clone(),
                display_name: None,
                signed: None,
                photo: None,
                body: None,
            },
        );
        self.request(
            "PUT",
            format!("/api/v1/accounts/{account}/keys"),
            Some(&body),
            Asked::AgentKeys {
                account,
                username,
                token,
                words,
            },
        );
    }

    /// A renew, revoke or disable asks for the agent's device first, whose termination the words
    /// sign.
    fn end_agent(&mut self, agent: Ulid, words: Secret, end: AgentEnd) {
        if let Err(error) = seed::derive(&words.0) {
            return self.error("bad_words", &error.to_string());
        }
        self.get(
            "/api/v1/agents".to_string(),
            Asked::AgentDevice { agent, words, end },
        );
    }

    fn agent_device_known(
        &mut self,
        agent: Ulid,
        words: Secret,
        end: AgentEnd,
        agent_list: Vec<AgentItem>,
    ) {
        let Some(item) = agent_list.into_iter().find(|item| item.id == agent) else {
            return self.error("no_such_account", "That is not one of your agents");
        };
        let statement = match item.device {
            Some(device) => {
                let keys = match seed::derive(&words.0) {
                    Ok(keys) => keys,
                    Err(error) => return self.error("bad_words", &error.to_string()),
                };
                Some(AgentStatement {
                    device,
                    signature: Bytes(sign_revocation(&keys, &agent, &device).to_vec()),
                })
            }
            None => None,
        };
        let body = AgentEndBody { statement };
        let (method, path) = match end {
            AgentEnd::Renew => ("POST", format!("/api/v1/agents/{agent}/token")),
            AgentEnd::Revoke => ("DELETE", format!("/api/v1/agents/{agent}/token")),
            AgentEnd::Disable => ("POST", format!("/api/v1/agents/{agent}/disable")),
        };
        self.request(method, path, Some(&body), Asked::AgentEnded { agent, end });
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
            invite_public: Bytes(keys.invite_public().to_vec()),
            invite_stamp: Bytes(stamp_invite_key(keys, &account).to_vec()),
        }
    }

    /// Asks for each account not known yet, so the platform can name the people a list shows.
    fn know(&mut self, account_list: impl IntoIterator<Item = Ulid>) {
        for account in account_list {
            if !self.accounts.contains_key(&account) {
                self.ask_account(account);
            }
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

    /// The archive's answer to `read_rows`: call rows first, so their messages show their ends. The
    /// chat's events are shown as lines only, since what they changed is long applied.
    fn rows_read(&mut self, id: u64, rows: Rows) {
        let Rows {
            message_list,
            call_list,
            chat_event_list,
        } = rows;
        let Some(chat) = self.reading.remove(&id) else {
            return;
        };
        for event in call_list {
            self.call_row(event, false);
        }
        for event in chat_event_list {
            self.know([event.actor].into_iter().chain(account_target(&event)));
            self.show_chat_event(event);
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

    /// Asks the archive for the next rows the search under way has not looked at.
    fn scan_rows(&mut self) {
        let id = self.next_id;
        self.next_id += 1;
        let Some(scan) = self.scan.as_mut() else {
            return;
        };
        scan.reading = Some(id);
        self.out.push(Output::ScanRows {
            id,
            before: scan.before,
            limit: SCAN_ROWS,
        });
    }

    /// The archive's answer to `scan_rows`. Each message is checked and opened as when shown, and
    /// matched by its newest text: an edit is met before what it edits, as rows come newest first.
    fn rows_scanned(&mut self, id: u64, message_list: Vec<MessageFrame>, total: u64) {
        let Some(mut scan) = self.scan.take() else {
            return;
        };
        if scan.reading != Some(id) {
            self.scan = Some(scan);
            return;
        }
        scan.reading = None;
        let read = message_list.len();
        let mut hit_list = Vec::new();
        let mut looked: usize = 0;
        for message in &message_list {
            looked += 1;
            scan.before = Some(ScanAt {
                saved_at: message.server_saved_at.unwrap_or(0),
                chat: message.chat,
                seq: message.seq,
            });
            if let Some(hit) = self.scanned_one(&mut scan, message) {
                hit_list.push(hit);
                scan.found += 1;
                if scan.found >= SEARCH_HITS {
                    break;
                }
            }
        }
        scan.scanned += u64::try_from(looked).unwrap_or(u64::MAX);
        scan.more = looked < read || u32::try_from(read).is_ok_and(|read| read >= SCAN_ROWS);
        let done = scan.found >= SEARCH_HITS || !scan.more;
        self.event(Event::MessageHits {
            text: scan.text.clone(),
            hit_list,
            scanned: scan.scanned,
            total: total.max(scan.scanned),
            done,
            more: scan.more,
        });
        self.scan = Some(scan);
        if !done {
            self.scan_rows();
        }
    }

    /// One row of a search: a hit when its newest text, a caption or a file's name, description,
    /// title or artist holds what is looked for. Anything that does not check out is passed over.
    /// A delete passes over what it points at, which the archive may hold until a `sync` wipes it.
    fn scanned_one(&self, scan: &mut Scan, message: &MessageFrame) -> Option<Hit> {
        let kind = message.kind?;
        let shown = match (kind, message.pointer_seq) {
            (Kind::Content, None) => message.seq,
            (Kind::Edit, Some(target)) => target,
            (Kind::Delete, Some(target)) => {
                scan.edited.insert((message.chat, target));
                return None;
            }
            _ => return None,
        };
        if !scan.edited.insert((message.chat, shown)) {
            return None;
        }
        if !matches!(
            message.content_kind,
            None | Some(
                ContentKind::Text
                    | ContentKind::Image
                    | ContentKind::Music
                    | ContentKind::Video
                    | ContentKind::Doc
                    | ContentKind::File
                    | ContentKind::MediaBundle
            )
        ) {
            return None;
        }
        let known = self.devices.get(&message.device?).copied()?;
        let signature = fixed::<64>(&message.signature.as_ref()?.0)?;
        let header = message.header()?;
        let body = message
            .body
            .as_ref()
            .map(|body| body.0.clone())
            .unwrap_or_default();
        verify_message(&header, &body, &signature, &known.public).ok()?;
        let opened = if header.encrypted {
            let plaintext = self
                .device
                .as_ref()?
                .open(&header, &body, &signature, &known.public)
                .ok()?;
            content_of(&plaintext, message.content_kind)
        } else {
            content_of(&body, message.content_kind)
        };
        let mut fields: Vec<&str> = Vec::new();
        if let Some(text) = &opened.text {
            fields.push(&text.0);
        }
        for file in &opened.files {
            if file.kind == ContentKind::Voice {
                continue;
            }
            fields.push(&file.name);
            for extra in [&file.description, &file.title, &file.artist]
                .into_iter()
                .flatten()
            {
                fields.push(extra);
            }
        }
        let (before, matched, after) = fields
            .iter()
            .find_map(|field| found_in(field, &scan.needle))?;
        Some(Hit {
            chat: message.chat,
            seq: shown,
            account: Some(known.account),
            mine: self.account == Some(known.account),
            saved_at: message.server_saved_at.unwrap_or(0),
            before: Secret(before),
            matched: Secret(matched),
            after: Secret(after),
        })
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
            emoji,
            answer,
            ack,
            topic,
            thread_seq,
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
            topic,
            thread_seq,
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
            emoji,
            answer,
            ack,
            topic,
            thread: thread_seq,
        });
    }

    // ---------------------------------------------------------------------------------------------
    // Files
    // ---------------------------------------------------------------------------------------------

    /// One file, or a bundle of images and videos. Each file gets its own key, and goes up before
    /// the message joins the queue, so text sent meanwhile is not held behind it.
    fn send_files(
        &mut self,
        chat: Ulid,
        specs: Vec<FileSpec>,
        text: Option<Secret>,
        reply: Option<Pointer>,
        place: (Option<Ulid>, Option<u64>),
    ) {
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
                reply,
                topic: place.0,
                thread_seq: place.1,
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
            encrypted: None,
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
            going.reply,
            Content {
                content_kind: Some(going.content_kind),
                text: going.text,
                files,
                remove: Vec::new(),
                call: None,
                fields: None,
                handles,
                topic: going.topic,
                thread_seq: going.thread_seq,
                ..Content::default()
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
        if let Asked::ChatPhotoGet { chat, topic, photo } = request.asked {
            if (200..300).contains(&status) {
                self.asked.remove(&id);
                self.photo_asked.remove(&photo);
                return self.chat_photo_got(chat, topic, photo, bytes);
            }
            // Replaced since, so the profile keeps its BlurHash.
            if status == 404 {
                self.asked.remove(&id);
                self.photo_asked.remove(&photo);
                return;
            }
        }
        if let Asked::Photo { account, photo } = request.asked {
            if (200..300).contains(&status) {
                self.asked.remove(&id);
                self.photo_asked.remove(&photo);
                return self.photo_got(account, photo, bytes);
            }
            // Removed by admin or replaced since, so the profile keeps its BlurHash.
            if status == 404 {
                self.asked.remove(&id);
                self.photo_asked.remove(&photo);
                return;
            }
        }
        self.replied(id, status, &String::from_utf8_lossy(bytes));
    }

    // ---------------------------------------------------------------------------------------------
    // Profiles
    // ---------------------------------------------------------------------------------------------

    fn ask_profile(&mut self, account: Ulid) {
        if self.profile_asked.insert(account) {
            self.get(
                format!("/api/v1/accounts/{account}/profile"),
                Asked::Profile { account },
            );
        }
    }

    /// Whether the server's `version` of a profile is newer than what this device holds. An account
    /// that never changed its profile has nothing beyond its names.
    fn profile_moved(&self, account: Ulid, version: u64) -> bool {
        match self.profiles.get(&account) {
            Some(state) => state.version != version,
            None => version > 0,
        }
    }

    /// Opens the profile keys a chat's entry carries, shares this account's own where the chat
    /// lacks it, and asks for every profile whose version moved.
    fn profile_pass(&mut self, chat: Ulid) {
        let Some(me) = self.account else {
            return;
        };
        // Joined by a Welcome before any list named it: the person a chat was started with, whose
        // own key nobody else could share.
        let Some(item) = self.chat_cache.iter().find(|item| item.chat == chat) else {
            return self.share_profile_key(chat, me);
        };
        let key_list = item.profile_key_list.clone();
        self.take_profile_keys(chat, me, key_list);
    }

    /// A `profile_keys` frame: keys just shared in a chat, kept in its cached entry so a reload
    /// has them, then opened.
    fn profile_keys_pushed(&mut self, chat: Ulid, key_list: Vec<ProfileKeyItem>) {
        let Some(me) = self.account else {
            return;
        };
        if let Some(item) = self.chat_cache.iter_mut().find(|item| item.chat == chat) {
            for entry in &key_list {
                match item
                    .profile_key_list
                    .iter_mut()
                    .find(|kept| kept.account == entry.account)
                {
                    Some(kept) => *kept = entry.clone(),
                    None => item.profile_key_list.push(entry.clone()),
                }
            }
            self.dirty = true;
        }
        // A commit's keys can come before the Welcome that lets this device open them.
        if self
            .device
            .as_ref()
            .is_none_or(|device| device.epoch(chat).is_err())
        {
            let waiting = self.profile_keys_waiting.entry(chat).or_default();
            waiting.retain(|kept| key_list.iter().all(|entry| entry.account != kept.account));
            waiting.extend(key_list);
            return;
        }
        self.take_profile_keys(chat, me, key_list);
    }

    fn take_profile_keys(&mut self, chat: Ulid, me: Ulid, key_list: Vec<ProfileKeyItem>) {
        for entry in key_list {
            let account = entry.account;
            if let (Some(epoch), Some(sealed)) = (entry.epoch, entry.sealed.as_ref())
                && let Some(device) = self.device.as_mut()
                && !device.has_profile_key(account)
                && device
                    .take_profile_key(chat, epoch, account, &sealed.0)
                    .is_ok()
            {
                self.dirty = true;
            }
            if account == me && entry.sealed.is_none() {
                self.share_profile_key(chat, me);
            }
            if self.profile_moved(account, entry.version) {
                self.ask_profile(account);
            } else if self
                .profiles
                .get(&account)
                .is_some_and(|state| state.body.is_none() && state.signed.is_some())
            {
                self.open_stored(account);
            }
        }
    }

    /// This account's profile key, for a chat that lacks it: the one its own chat was started with,
    /// and its self chat.
    fn share_profile_key(&mut self, chat: Ulid, me: Ulid) {
        let Some(device) = self.device.as_ref() else {
            return;
        };
        let Ok((epoch, sealed)) = device.share_profile_key(chat, me) else {
            return;
        };
        if !self.profile_shared.insert((chat, epoch)) {
            return;
        }
        let reference = reference();
        let frame = ClientFrame::ProfileKeys {
            reference,
            chat,
            epoch,
            profile_key_list: vec![ProfileKeyShare {
                account: me,
                sealed: Bytes(sealed),
            }],
        };
        // Offline: the next list after reconnecting still lacks it, and it goes then.
        if !self.ask(frame, reference, Waiting::Done) {
            self.profile_shared.remove(&(chat, epoch));
        }
    }

    fn profile_got(&mut self, reply: ProfileReply) {
        let ProfileReply {
            account,
            username,
            display_name,
            version,
            profile,
            photo,
        } = reply;
        self.names_changed(account, username.clone(), display_name.clone());
        self.profiles.insert(
            account,
            ProfileState {
                account,
                version,
                username,
                display_name,
                signed: profile,
                photo,
                body: None,
            },
        );
        self.dirty = true;
        self.open_stored(account);
    }

    /// Opens a kept sealed profile, if its key and its account's master key are here, then shows
    /// it. One that does not open shows names only.
    fn open_stored(&mut self, account: Ulid) {
        let Some(signed) = self
            .profiles
            .get(&account)
            .and_then(|state| state.signed.clone())
        else {
            return self.show_profile(account);
        };
        // Signed by one of its own devices, or, for an agent, by one of its owner's, chosen by the
        // author, so its owner edits it without its words.
        let owner = self.accounts.get(&account).and_then(|info| info.owner);
        let (Some(device_public), Some(stamp), Some(signature)) = (
            fixed::<32>(&signed.device_public.0),
            fixed::<64>(&signed.stamp.0),
            fixed::<64>(&signed.signature.0),
        ) else {
            return self.show_profile(account);
        };
        let mut opened = None;
        let mut missing = Vec::new();
        for signer in std::iter::once(account).chain(owner) {
            let master = self
                .accounts
                .get(&signer)
                .and_then(|info| info.master_public.as_ref())
                .and_then(|master| fixed::<32>(&master.0));
            let Some(master_public) = master else {
                missing.push(signer);
                continue;
            };
            let record = DeviceRecord {
                account: signer,
                device: signed.device,
                device_public,
                master_public,
                stamp,
            };
            let Some(device) = self.device.as_mut() else {
                return;
            };
            if let Ok(plaintext) =
                device.open_profile(account, &record, &signed.sealed.0, &signature)
            {
                opened = Some(plaintext);
                break;
            }
        }
        if opened.is_none() && !missing.is_empty() {
            for signer in missing {
                self.ask_account(signer);
            }
            return self.show_profile(account);
        }
        // Without its key, it opens once a `profile_keys` frame or a later list brings one.
        if let Some(plaintext) = opened
            && let Ok(body) = serde_json::from_slice::<ProfileBody>(&plaintext)
            && let Some(state) = self.profiles.get_mut(&account)
        {
            state.body = Some(body);
            state.signed = None;
            self.dirty = true;
        }
        self.show_profile(account);
    }

    fn show_profile(&mut self, account: Ulid) {
        let owner = self.accounts.get(&account).and_then(|info| info.owner);
        let profile = match self.profiles.get(&account) {
            Some(state) => {
                let body = state.body.clone().unwrap_or_default();
                ShownProfile {
                    account,
                    username: state.username.clone(),
                    display_name: state.display_name.clone(),
                    version: state.version,
                    bio: body.bio,
                    link_list: body.link_list,
                    photo: body
                        .photo
                        .filter(|photo| state.photo == Some(photo.id))
                        .map(|photo| ShownPhoto {
                            id: photo.id,
                            blurhash: photo.blurhash,
                        }),
                    agent: owner.is_some(),
                    owner,
                }
            }
            None => match self.accounts.get(&account) {
                Some(info) => ShownProfile {
                    account,
                    username: info.username.clone(),
                    display_name: info.display_name.clone(),
                    version: 0,
                    bio: None,
                    link_list: Vec::new(),
                    photo: None,
                    agent: owner.is_some(),
                    owner,
                },
                None => return,
            },
        };
        self.event(Event::Profile { profile });
    }

    /// An account's names from the server, wherever this device keeps them.
    fn names_changed(&mut self, account: Ulid, username: String, display_name: Option<String>) {
        if self.account == Some(account) {
            self.username = Some(username.clone());
        }
        if let Some(state) = self.profiles.get_mut(&account) {
            state.username = username.clone();
            state.display_name = display_name.clone();
        }
        let Some(info) = self.accounts.get_mut(&account) else {
            return;
        };
        if info.username == username && info.display_name == display_name {
            return;
        }
        info.username = username.clone();
        info.display_name = display_name.clone();
        self.dirty = true;
        self.event(Event::Account {
            account: AccountBrief {
                id: account,
                username,
                display_name,
            },
        });
    }

    /// What this account's profile, or its agent's, holds now, to change one part of it. Its photo
    /// stays only if the server still has it.
    fn profile_body(&mut self, me: Ulid) -> Option<ProfileBody> {
        let Some(state) = self.profiles.get(&me) else {
            self.ask_profile(me);
            self.error("profile_loading", "Your profile has not loaded yet");
            return None;
        };
        if state.signed.is_some() {
            self.error("profile_loading", "Your profile could not be opened");
            return None;
        }
        let mut body = state.body.clone().unwrap_or_default();
        if body
            .photo
            .as_ref()
            .is_some_and(|photo| state.photo != Some(photo.id))
        {
            body.photo = None;
        }
        Some(body)
    }

    fn put_profile(&mut self, me: Ulid, body: ProfileBody) {
        let Some(device) = self.device.as_ref() else {
            return self.error("no_device", "This client has no device");
        };
        let plaintext = match serde_json::to_vec(&body) {
            Ok(plaintext) => zeroize::Zeroizing::new(plaintext),
            Err(_) => return self.error("internal", "Could not write the profile"),
        };
        let (sealed, signature) = match device.seal_profile(me, &plaintext) {
            Ok(sealed) => sealed,
            Err(error) => return self.error("seal_failed", &error.to_string()),
        };
        let put = PutProfileBody {
            sealed: Some(Bytes(sealed)),
            signature: Some(Bytes(signature.to_vec())),
            photo: body.photo.as_ref().map(|photo| photo.id),
        };
        self.request(
            "PUT",
            format!("/api/v1/accounts/{me}/profile"),
            Some(&put),
            Asked::PutProfile {
                account: me,
                body: Box::new(body),
            },
        );
    }

    fn profile_saved(&mut self, me: Ulid, version: u64, body: ProfileBody) {
        let (username, display_name) = match self.accounts.get(&me) {
            Some(info) => (info.username.clone(), info.display_name.clone()),
            None => (self.username.clone().unwrap_or_default(), None),
        };
        let state = self.profiles.entry(me).or_insert(ProfileState {
            account: me,
            version,
            username,
            display_name,
            signed: None,
            photo: None,
            body: None,
        });
        state.version = version;
        state.signed = None;
        state.photo = body.photo.as_ref().map(|photo| photo.id);
        state.body = Some(body);
        self.dirty = true;
        self.show_profile(me);
        self.event(Event::ProfileSaved);
    }

    /// Seals the JPEG as one chunk under its own key and sends it up. The profile names it once
    /// the server has it.
    fn set_photo(&mut self, me: Ulid, jpeg: &[u8], thumbnail: &Thumbnail) {
        if jpeg.is_empty() || jpeg.len() as u64 > file::CHUNK {
            return self.error("file_too_large", "That photo is too large");
        }
        let blurhash = match file::blurhash_of(thumbnail.width, thumbnail.height, &thumbnail.rgba) {
            Ok(blurhash) => blurhash,
            Err(error) => return self.error("bad_files", &error.to_string()),
        };
        let key = match file::new_key() {
            Ok(key) => key,
            Err(error) => return self.error("internal", &error.to_string()),
        };
        let sealed = match file::seal_chunk(key.as_ref(), 0, true, jpeg) {
            Ok(sealed) => sealed,
            Err(error) => return self.error("internal", &error.to_string()),
        };
        self.issue(Request {
            method: "POST".to_string(),
            path: format!("/api/v1/accounts/{me}/photos"),
            body: None,
            bytes: Some(sealed),
            raw: false,
            asked: Asked::AddPhoto {
                account: me,
                key,
                size: jpeg.len() as u64,
                blurhash,
            },
            attempt: 0,
        });
    }

    fn ask_photo(&mut self, account: Ulid) {
        let Some(photo) = self.profiles.get(&account).and_then(|state| {
            state
                .body
                .as_ref()
                .and_then(|body| body.photo.as_ref())
                .filter(|photo| state.photo == Some(photo.id))
                .map(|photo| photo.id)
        }) else {
            return;
        };
        if !self.photo_asked.insert(photo) {
            return;
        }
        self.issue(Request {
            method: "GET".to_string(),
            path: format!("/api/v1/photos/{photo}"),
            body: None,
            bytes: None,
            raw: true,
            asked: Asked::Photo { account, photo },
            attempt: 0,
        });
    }

    fn photo_got(&mut self, account: Ulid, photo: Ulid, sealed: &[u8]) {
        let Some(named) = self
            .profiles
            .get(&account)
            .and_then(|state| state.body.as_ref())
            .and_then(|body| body.photo.clone())
            .filter(|named| named.id == photo)
        else {
            return;
        };
        match file::open_chunk(&named.key.0, 0, true, sealed) {
            Ok(jpeg) => self.event(Event::Photo {
                account,
                photo,
                bytes: Bytes(jpeg.to_vec()),
            }),
            Err(error) => self.error("bad_photo", &error.to_string()),
        }
    }

    // ---------------------------------------------------------------------------------------------
    // The socket
    // ---------------------------------------------------------------------------------------------

    /// This device's `presence`, its text sealed under the account's profile key. A text that cannot
    /// be sealed is left out, and the state still goes.
    fn send_presence(&mut self) {
        let mut text = None;
        if let (Some(plain), Some(device)) = (&self.presence_text, self.device.as_ref()) {
            match device.seal_presence_text(plain) {
                Ok(sealed) => text = Some(Bytes(sealed)),
                Err(error) => self.error("seal_failed", &error.to_string()),
            }
        }
        self.send(&ClientFrame::Presence {
            in_app: self.in_app,
            trouble: self.trouble,
            text,
        });
    }

    /// Someone's presence, its text opened with their profile key. One that does not open is left
    /// out, as its key may not have come yet.
    fn presence_shown(&mut self, mut presence: PresenceItem) {
        let mut text = None;
        if let (Some(sealed), Some(device)) = (presence.text.take(), self.device.as_ref())
            && let Ok(plain) = device.open_presence_text(presence.account, &sealed.0)
            && let Ok(plain) = String::from_utf8(plain.to_vec())
        {
            text = Some(plain);
        }
        self.event(Event::Presence { presence, text });
    }

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
        // The first frame: until it, this device is not there to others.
        self.send_presence();
        self.ask_kv(self.kv_version);
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
        // Before the `sync`, so the list it brings already holds this arrangement.
        self.send_folders();
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
        // What tagged commands still waited for on this socket will never be answered: a message
        // waits on in the queue by its local id, and the rest is told it went offline.
        let lost: Vec<TagKey> = self
            .tag_keys
            .keys()
            .filter(|key| matches!(key, TagKey::Frame(_)))
            .copied()
            .collect();
        for key in lost {
            if let Some(tag) = self.untag(key)
                && let Some(state) = self.tagged.get_mut(&tag)
                && state.code.is_none()
                && !self.tag_keys.values().any(|other| *other == tag)
            {
                state.code = Some("offline".to_string());
            }
        }
        // A change whose answer never came goes again, under what was made since.
        self.kv_ready = false;
        if let Some(flight) = self.kv_flight.take() {
            self.kv_out = Some(match self.kv_out.take() {
                Some(out) => merge_change(Some(flight), out),
                None => flight,
            });
        }
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
            // An agent has nobody to type a password: it logs in again with its token.
            match self.agent_token.clone() {
                Some(token) => self.log_in_agent(token),
                None => self.show_login(),
            }
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
        for state in shelf.profile_list {
            let account = state.account;
            self.profiles.entry(account).or_insert(state);
            self.show_profile(account);
        }
        self.chat_cache = shelf.chat_list.clone();
        self.folders = shelf.folders.clone();
        self.folders_out = shelf.folders_out.clone();
        self.show_folders();
        self.kv = shelf.kv.clone();
        self.kv_version = shelf.kv_version;
        self.kv_out = shelf.kv_out.clone();
        self.show_kv();
        self.own_requests = shelf
            .asked_list
            .iter()
            .map(|item| (item.chat, item.clone()))
            .collect();
        self.invitations = shelf
            .invitation_list
            .iter()
            .map(|item| (item.chat, item.clone()))
            .collect();
        let mut shown = Vec::with_capacity(shelf.chat_list.len());
        let mut preview_list = Vec::new();
        // A group's or channel's opened profile is not kept, so the sealed one in the list is opened
        // again, and the platform gets its name after a restart as it did before.
        let mut profile_list = Vec::new();
        let mut setting_list = Vec::new();
        for mut item in shelf.chat_list {
            let state = self.chats.entry(item.chat).or_default();
            state.chat_type = Some(item.chat_type);
            if grouped(item.chat_type) {
                state.topics = Some(TopicsBody {
                    on: item.topics_on,
                    topic_list: item.topic_list.clone(),
                });
                if let Some(profile) = item.profile.clone() {
                    profile_list.push((item.chat, profile));
                }
                setting_list.push((item.chat, item.topic_setting_list.clone()));
            }
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
        for (chat, profile) in profile_list {
            self.chat_profile_in(chat, profile);
        }
        for (chat, list) in setting_list {
            self.topic_settings_in(chat, list);
        }
        let asked_list: Vec<RequestItem> = self.own_requests.values().cloned().collect();
        for request in asked_list {
            self.event(Event::Asked {
                chat: request.chat,
                name: request.name,
                state: request.state,
                at: request.at,
            });
        }
        let invitation_list: Vec<InvitationItem> = self.invitations.values().cloned().collect();
        for invitation in invitation_list {
            self.know([invitation.inviter]);
            let invitation_shown = self.invitation_shown(&invitation);
            self.event(Event::Invitation {
                chat: invitation.chat,
                invitation: Some(invitation_shown),
            });
        }
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
        let mut devices_wanted = Vec::new();
        for (chat, state) in &mut self.chats {
            if state.plan_list.is_empty() {
                continue;
            }
            let mut all_known = true;
            let mut needed = state.member_list.clone();
            for plan in &state.plan_list {
                if let Plan::Add { added, .. } = plan {
                    for (account, _) in added {
                        needed.push(*account);
                    }
                }
            }
            // A group's or channel's members come from the server's device list.
            if state.chat_type.is_some_and(grouped) && !state.members_known {
                all_known = false;
                if !state.fetching_devices {
                    devices_wanted.push(*chat);
                }
            }
            for account in &needed {
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
        for chat in devices_wanted {
            self.devices_of(chat);
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
        match self.chats.get(&chat) {
            Some(state) => self.account_keys(&state.member_list),
            None => Vec::new(),
        }
    }

    /// The keys of these accounts that are known.
    fn account_keys(&self, account_list: &[Ulid]) -> Vec<AccountKey> {
        let mut list = Vec::new();
        for account in account_list {
            if let Some(info) = self.accounts.get(account)
                && let Some(key) = account_key(info)
            {
                list.push(key);
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
                Item::Typing {
                    chat,
                    on,
                    topic,
                    thread_seq,
                } => {
                    self.send(&ClientFrame::Typing {
                        chat,
                        on,
                        topic,
                        thread_seq,
                    });
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
                    topic,
                    thread_seq,
                } => match self.can_seal(chat) {
                    Seal::Now => {
                        let content = Content {
                            content_kind,
                            text,
                            files,
                            remove,
                            call,
                            fields,
                            topic,
                            thread_seq,
                            ..Content::default()
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
        if state.joining || state.rotating || state.catching_up || !state.plan_list.is_empty() {
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
            topic,
            thread_seq,
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
            topic,
            thread_seq,
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
                // A reaction's target is `on`, any other content's `reply`.
                if let (Kind::Content, Some(seq)) = (kind, pointer_seq) {
                    let key = if content_kind == Some(ContentKind::Reaction) {
                        "on"
                    } else {
                        "reply"
                    };
                    plain.insert(key.to_string(), serde_json::Value::from(seq));
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
            profile_key_list: Vec::new(),
            topic,
            thread_seq,
            action: None,
            chat_key: None,
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
        self.fail_waiting(chat, local, code, None);
    }

    /// Fails a message and what waits on it, with the seconds a slow mode asks to wait.
    fn fail_waiting(&mut self, chat: Ulid, local: u64, code: &str, seconds: Option<u64>) {
        for dropped in self.queue.drop_with_dependents(local) {
            self.in_flight.remove(&dropped);
            self.event(Event::Failed {
                chat,
                local: dropped,
                code: code.to_string(),
                seconds,
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
                    seconds: None,
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
        // Wrapped to everyone in the chat afterwards: those added in, those taken out not.
        let mut after = self
            .chats
            .get(&chat)
            .map(|state| state.member_list.clone())
            .unwrap_or_default();
        match &plan {
            Plan::Add { added, .. } => {
                for (account, _) in added {
                    if !after.contains(account) {
                        after.push(*account);
                    }
                }
            }
            Plan::Remove { removed, .. } => after.retain(|account| !removed.contains(account)),
            _ => {}
        }
        let account_list = self.account_keys(&after);
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
            Plan::Add {
                package_list,
                added,
            } => {
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
                    added,
                }
            }
            // Or taken out some of these.
            Plan::Remove {
                device_list,
                removed,
            } => {
                let mut still_in = Vec::new();
                for gone in device_list {
                    if member_set.contains(&gone) {
                        still_in.push(gone);
                    }
                }
                if still_in.is_empty() {
                    self.queue.confirm(local, 0);
                    return;
                }
                Plan::Remove {
                    device_list: still_in,
                    removed,
                }
            }
            plan => plan,
        };
        let mut welcome_list = Vec::new();
        let mut action = None;
        let built = match &plan {
            Plan::Add {
                package_list,
                added,
            } => {
                let mut bodies = Vec::new();
                for package in package_list {
                    bodies.push(package.body.0.clone());
                }
                if !added.is_empty() {
                    action = Some(CommitAction {
                        kind: ActionKind::Add,
                        account_list: added.iter().map(|(account, _)| *account).collect(),
                    });
                }
                device.add_devices(chat, &bodies, &account_list)
            }
            Plan::Remove {
                device_list,
                removed,
            } => {
                if !removed.is_empty() {
                    action = Some(CommitAction {
                        kind: ActionKind::Remove,
                        account_list: removed.clone(),
                    });
                }
                device.remove_devices(chat, device_list, &account_list)
            }
            Plan::Rotate => device.rotate(chat, &account_list),
            Plan::Join { group_info, invite } => {
                device.join_external(chat, group_info, now, &account_list, invite.as_ref())
            }
            Plan::Create { .. } => return,
        };
        let commit = match built {
            Ok(commit) => commit,
            Err(error) => {
                self.commit_failed(chat, local, "commit_failed");
                return self.error("commit_failed", &error.to_string());
            }
        };
        if let (Plan::Add { package_list, .. }, Some(welcome)) = (&plan, &commit.welcome) {
            for package in package_list {
                welcome_list.push(WelcomeItem {
                    device: package.device,
                    body: Bytes(welcome.clone()),
                });
            }
        }
        // The people an add brings in, and a device joining by invitation, hold only the new key, so
        // the chat's profile key goes sealed under it, saved with the commit.
        let shares_key = matches!(plan, Plan::Join { .. })
            || action
                .as_ref()
                .is_some_and(|action| action.kind == ActionKind::Add);
        let chat_key = match commit.chat_key.clone() {
            Some(sealed) if shares_key => Some(Bytes(sealed)),
            _ => None,
        };
        let header = Header {
            chat,
            device: device.record().device,
            kind: Kind::Commit,
            content_kind: None,
            encrypted: false,
            epoch: Some(commit.epoch),
            pointer_seq: None,
            call_seq: None,
            topic: None,
            thread_seq: None,
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
            profile_key_list: commit
                .profile_key_list
                .into_iter()
                .map(|(account, sealed)| ProfileKeyShare {
                    account,
                    sealed: Bytes(sealed),
                })
                .collect(),
            topic: None,
            thread_seq: None,
            action,
            chat_key,
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
        // A group's or channel's first profile and options go with epoch 0's group info, saved in
        // the same step, so a crash can never leave it nameless. Its profile key is made here and
        // goes with them, sealed at epoch 0.
        if let Some(start) = self.chat(chat).start.as_ref() {
            let profile = start.profile.clone();
            let options = (start.options != ChatOptions::default()).then(|| start.options.clone());
            let shared = match self.device.as_mut() {
                Some(device) => device
                    .new_chat_key(chat)
                    .and_then(|()| device.share_chat_key(chat)),
                None => return,
            };
            let Ok((_, key)) = shared else {
                self.fail_chat(chat, "start_failed");
                return self.error("start_failed", "The chat's profile key could not be made");
            };
            let Some(body) = self.keep_chat_profile(chat, profile) else {
                self.fail_chat(chat, "start_failed");
                return;
            };
            self.chat(chat).start_profile = Some(StartProfile { body, key, options });
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
            let (chat_profile, chat_key, chat_options) = match &state.start_profile {
                Some(start) if epoch == 0 => (
                    Some(Box::new(start.body.clone())),
                    Some(Bytes(start.key.clone())),
                    start.options.clone().map(Box::new),
                ),
                _ => (None, None, None),
            };
            let reference = reference();
            let frame = ClientFrame::GroupInfo {
                reference,
                chat,
                epoch,
                body: Bytes(body),
                chat_profile,
                chat_key,
                chat_options,
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
                state.plan_list.push(Plan::Add {
                    package_list,
                    added: Vec::new(),
                });
            }
        }
    }

    fn ready(&mut self, chat: Ulid) {
        let state = self.chat(chat);
        state.joining = false;
        state.join_attempt = 0;
        state.join_at = None;
        self.event(Event::ChatReady { chat });
        if let (Some(me), Some(key_list)) = (self.account, self.profile_keys_waiting.remove(&chat))
        {
            self.take_profile_keys(chat, me, key_list);
        }
        self.profile_pass(chat);
        if let Some(item) = self.chat(chat).profile_waiting.take() {
            self.chat_profile_in(chat, item);
        }
        self.group_started(chat);
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
    // Groups and channels
    // ---------------------------------------------------------------------------------------------

    /// Sends a frame answered with `done`, or says it needs a connection.
    fn ask_done(&mut self, frame: ClientFrame, reference: Ulid, waiting: Waiting) {
        if !self.ask(frame, reference, waiting) {
            self.error("offline", "This needs a connection");
        }
    }

    /// Asks the server for the chat's id, then builds its group with this account its owner. Its
    /// profile, options and people follow once the group runs, in [`Client::ready`].
    fn create_group(
        &mut self,
        chat_type: ChatType,
        mut start: GroupStart,
        topics: Option<bool>,
        findable: bool,
    ) {
        if !grouped(chat_type) {
            return self.error("bad_chat_type", "CreateChat makes a group or a channel");
        }
        let profile = match checked_chat_profile(start.profile) {
            Ok(profile) => profile,
            Err(message) => return self.error("bad_profile", message),
        };
        let body = CreateChatBody {
            account: None,
            chat_type: Some(chat_type),
            topics,
            findable: findable.then(|| FindableItem {
                name: profile.name.clone(),
                bio: profile.bio.clone(),
            }),
        };
        start.profile = profile;
        self.request(
            "POST",
            "/api/v1/chats".to_string(),
            Some(&body),
            Asked::CreateGroup {
                chat_type,
                start: Box::new(start),
            },
        );
    }

    /// The server made the chat: this device starts its group, alone in it, as its owner.
    fn group_created(&mut self, chat: Ulid, chat_type: ChatType, mut start: GroupStart) {
        let Some(me) = self.account else {
            return;
        };
        self.event(Event::Started { chat, account: me });
        let state = self.chat(chat);
        state.chat_type = Some(chat_type);
        state.member_list = vec![me];
        state.members_known = true;
        let photo = start.photo.take();
        state.start = Some(start);
        state.joining = true;
        // A photo goes up first, so the first profile anyone gets already names it.
        match photo {
            Some(photo) => self.upload_chat_photo(chat, None, photo, true),
            None => self.start_group(chat),
        }
    }

    fn start_group(&mut self, chat: Ulid) {
        self.chat(chat).plan_list.push(Plan::Create {
            package_list: Vec::new(),
        });
    }

    /// A group's or channel's photo, or one of its topics', goes up as one of its files, of one
    /// chunk.
    fn upload_chat_photo(
        &mut self,
        chat: Ulid,
        topic: Option<Ulid>,
        photo: PreparedPhoto,
        starting: bool,
    ) {
        let body = CreateFileBody {
            kind: ContentKind::Image,
            size: photo.sealed.len() as u64,
            chunks: 1,
            encrypted: None,
        };
        self.request(
            "POST",
            format!("/api/v1/chats/{chat}/files"),
            Some(&body),
            Asked::ChatPhotoFile {
                chat,
                topic,
                photo: Box::new(photo),
                starting,
            },
        );
    }

    /// The photo is up: a new chat's first profile names it and its group starts, and an existing
    /// chat's or topic's profile is sealed again naming it.
    fn chat_photo_up(
        &mut self,
        chat: Ulid,
        topic: Option<Ulid>,
        file: Ulid,
        photo: PreparedPhoto,
        starting: bool,
    ) {
        let named = ProfilePhoto {
            id: file,
            key: Bytes(photo.key.to_vec()),
            size: photo.size,
            blurhash: photo.blurhash,
        };
        if let Some(topic) = topic {
            let mut profile = self
                .chat(chat)
                .topic_profiles
                .get(&topic)
                .map(|(profile, _)| profile.clone())
                .unwrap_or_default();
            profile.photo = Some(named);
            return self.send_topic_profile(chat, topic, profile);
        }
        if starting {
            if let Some(start) = self.chat(chat).start.as_mut() {
                start.profile.photo = Some(named);
            }
            return self.start_group(chat);
        }
        let Some((mut profile, _)) = self.chat(chat).profile.clone() else {
            return self.error("not_joined", "This chat's profile is not known yet");
        };
        profile.photo = Some(named);
        self.send_chat_profile(chat, profile);
    }

    /// A photo that did not go up: a new chat starts without it, and the person is told.
    fn chat_photo_failed(&mut self, chat: Ulid, starting: bool, code: &str, message: &str) {
        if starting {
            self.error("photo_failed", message);
            return self.start_group(chat);
        }
        self.error(code, message);
    }

    /// Fetches the one chunk of the photo a group's or channel's profile names, or one of its
    /// topics', once at a time.
    fn ask_chat_photo(&mut self, chat: Ulid, topic: Option<Ulid>) {
        let Some(photo) = self.named_photo(chat, topic).map(|photo| photo.id) else {
            return;
        };
        if !self.photo_asked.insert(photo) {
            return;
        }
        self.issue(Request {
            method: "GET".to_string(),
            path: format!("/api/v1/files/{photo}/chunks/0"),
            body: None,
            bytes: None,
            raw: true,
            asked: Asked::ChatPhotoGet { chat, topic, photo },
            attempt: 0,
        });
    }

    /// The photo a chat's profile names, or one of its topics'.
    fn named_photo(&self, chat: Ulid, topic: Option<Ulid>) -> Option<ProfilePhoto> {
        let state = self.chats.get(&chat)?;
        match topic {
            Some(topic) => state.topic_profiles.get(&topic)?.0.photo.clone(),
            None => state.profile.as_ref()?.0.photo.clone(),
        }
    }

    fn chat_photo_got(&mut self, chat: Ulid, topic: Option<Ulid>, photo: Ulid, sealed: &[u8]) {
        let Some(named) = self
            .named_photo(chat, topic)
            .filter(|named| named.id == photo)
        else {
            return;
        };
        let bytes = match file::open_chunk(&named.key.0, 0, true, sealed) {
            Ok(jpeg) => Bytes(jpeg.to_vec()),
            Err(error) => return self.error("bad_photo", &error.to_string()),
        };
        match topic {
            Some(topic) => self.event(Event::TopicPhoto {
                chat,
                topic,
                photo,
                bytes,
            }),
            None => self.event(Event::ChatPhoto { chat, photo, bytes }),
        }
    }

    /// A new group's people are added once it runs, its profile and options having gone up with
    /// epoch 0's group info.
    fn group_started(&mut self, chat: Ulid) {
        let Some(start) = self.chat(chat).start.take() else {
            return;
        };
        if !start.account_list.is_empty() {
            self.add_members(chat, start.account_list, start.note);
        }
    }

    /// Claims a package for each device of each new account, then adds them all in one commit.
    fn add_members(&mut self, chat: Ulid, account_list: Vec<Ulid>, note: Option<String>) {
        let me = self.account;
        let state = self.chat(chat);
        let mut adding = Vec::new();
        for account in account_list {
            if Some(account) != me
                && !state.member_list.contains(&account)
                && !adding.contains(&account)
            {
                adding.push(account);
            }
        }
        if adding.is_empty() {
            return self.error("already_member", "Everyone named is in this chat already");
        }
        for account in &adding {
            if !self.accounts.contains_key(account) {
                self.ask_account(*account);
            }
            match &note {
                Some(note) => self.invite_notes.insert((chat, *account), note.clone()),
                None => self.invite_notes.remove(&(chat, *account)),
            };
        }
        self.claim_add(
            chat,
            AddClaim {
                account_list: adding,
                ..AddClaim::default()
            },
        );
    }

    /// Claims the next account's packages, or plans the add once every account was asked.
    fn claim_add(&mut self, chat: Ulid, mut claim: AddClaim) {
        if claim.account_list.is_empty() {
            if !claim.package_list.is_empty() {
                self.chat(chat).plan_list.push(Plan::Add {
                    package_list: claim.package_list,
                    added: claim.added,
                });
            }
            return;
        }
        let account = claim.account_list.remove(0);
        // Naming the chat lets the server refuse someone added by invitation only before any of
        // their one-use packages goes.
        let body = ClaimBody { chat: Some(chat) };
        self.request(
            "POST",
            format!("/api/v1/accounts/{account}/key_packages/claim"),
            Some(&body),
            Asked::AddClaim {
                chat,
                account,
                claim,
            },
        );
    }

    /// One account's packages came. An account none of whose devices has one left cannot be added
    /// yet, and is left out.
    fn add_claimed(
        &mut self,
        chat: Ulid,
        account: Ulid,
        mut claim: AddClaim,
        package_list: Vec<KeyPackageItem>,
    ) {
        if package_list.is_empty() {
            self.error(
                "no_key_packages",
                "Someone named has no device that can be added yet",
            );
        } else {
            claim.added.push((
                account,
                package_list.iter().map(|item| item.device).collect(),
            ));
            claim.package_list.extend(package_list);
        }
        self.claim_add(chat, claim);
    }

    /// Takes these accounts out in one commit, every device of theirs in the group.
    fn remove_members(&mut self, chat: Ulid, account_list: Vec<Ulid>) {
        let Some(device) = self.device.as_ref() else {
            return self.error("no_device", "This client has no device");
        };
        let Ok(member_list) = device.member_list(chat) else {
            return self.error("not_joined", "This device is not in the chat yet");
        };
        let mut device_list = Vec::new();
        for member in member_list {
            if account_list.contains(&member.account) {
                device_list.push(member.device);
            }
        }
        if device_list.is_empty() {
            return self.error("not_member", "Nobody named is in this chat");
        }
        self.chat(chat).plan_list.push(Plan::Remove {
            device_list,
            removed: account_list,
        });
    }

    /// Some accounts named in an add take invitations only: the commit goes, each of them is
    /// invited, and the rest are added again with the packages already claimed.
    fn invite_needed(&mut self, chat: Ulid, local: u64, plan: Plan, account_list: Vec<Ulid>) {
        self.commit_busy = false;
        if let Some(device) = self.device.as_mut() {
            let _ = device.commit_rejected(chat);
        }
        self.dirty = true;
        self.in_flight.remove(&local);
        self.queue.drop_with_dependents(local);
        for account in &account_list {
            self.invite(chat, *account);
        }
        let Plan::Add {
            package_list,
            added,
        } = plan
        else {
            return;
        };
        let mut kept = Vec::new();
        let mut kept_devices = Vec::new();
        for (account, device_list) in added {
            if !account_list.contains(&account) {
                kept_devices.extend(device_list.iter().copied());
                kept.push((account, device_list));
            }
        }
        if kept.is_empty() {
            return;
        }
        let package_list = package_list
            .into_iter()
            .filter(|package| kept_devices.contains(&package.device))
            .collect();
        self.chat(chat).plan_list.push(Plan::Add {
            package_list,
            added: kept,
        });
    }

    /// Invites an account whose switch turns every add into an invitation, signed by this device
    /// so members accept the join it makes.
    /// The group's name and its photo's BlurHash are sealed to the account's invite key, once its
    /// stamp checks out against the pinned master key, so it waits for the account's keys.
    fn invite(&mut self, chat: Ulid, account: Ulid) {
        let Some(info) = self.accounts.get(&account) else {
            self.inviting.push((chat, account));
            return self.ask_account(account);
        };
        let (Some(master), Some(public), Some(stamp)) =
            (&info.master_public, &info.invite_public, &info.invite_stamp)
        else {
            return self.error("no_keys", "That account has not set up its words yet");
        };
        let (Some(master_public), Some(invite_public), Some(stamp)) =
            (fixed(&master.0), fixed(&public.0), fixed(&stamp.0))
        else {
            return self.error("bad_keys", "That account's keys are not the right size");
        };
        let invitee = InviteKey {
            account,
            invite_public,
            master_public,
            stamp,
        };
        let Some((profile, _)) = self
            .chats
            .get(&chat)
            .and_then(|state| state.profile.as_ref())
        else {
            return self.error("no_profile", "This chat's name is not known yet");
        };
        let Some(Ok(key)) = self.device.as_ref().map(|device| device.chat_key(chat)) else {
            return self.error("no_profile", "This chat's profile key is not known yet");
        };
        let plain = InvitationPlain {
            name: profile.name.clone(),
            blurhash: profile.photo.as_ref().map(|photo| photo.blurhash.clone()),
            key: Bytes(key.to_vec()),
            note: self.invite_notes.remove(&(chat, account)),
        };
        let Ok(plain) = serde_json::to_vec(&plain) else {
            return self.error("internal", "An invitation could not be written");
        };
        let Some(device) = self.device.as_mut() else {
            return;
        };
        let sealed = match device.seal_invitation(chat, &invitee, &plain) {
            Ok(sealed) => sealed,
            Err(error) => return self.error("seal_failed", &error.to_string()),
        };
        let signature = match device.sign_invite(&chat, &account) {
            Ok(signature) => signature,
            Err(error) => return self.error("sign_failed", &error.to_string()),
        };
        self.dirty = true;
        let reference = reference();
        let frame = ClientFrame::Invite {
            reference,
            chat,
            account,
            sealed: Bytes(sealed),
            signature: Bytes(signature.to_vec()),
        };
        self.ask_done(frame, reference, Waiting::Done);
    }

    /// Accepted, this account is a member on the server, and this device joins by itself with the
    /// invitation.
    fn invite_answered(&mut self, chat: Ulid, accept: bool) {
        if !accept {
            self.invitations.remove(&chat);
            self.dirty = true;
            return self.event(Event::Invitation {
                chat,
                invitation: None,
            });
        }
        let chat_type = self.invitations.get(&chat).map(|item| item.chat_type);
        let state = self.chat(chat);
        state.chat_type = chat_type;
        state.members_known = false;
        if state.joining {
            return;
        }
        state.joining = true;
        self.devices_of(chat);
        self.get(
            format!("/api/v1/chats/{chat}/group_info"),
            Asked::JoinInfo { chat },
        );
    }

    /// This account is out of the chat: its group, keys and state go, and the platform is told.
    fn drop_chat(&mut self, chat: Ulid) {
        if let Some(device) = self.device.as_mut()
            && device.epoch(chat).is_ok()
        {
            let _ = device.forget_chat(chat);
        }
        self.chats.remove(&chat);
        self.invitations.remove(&chat);
        self.chat_cache.retain(|item| item.chat != chat);
        self.dirty = true;
        self.event(Event::ChatLeft { chat });
    }

    /// An invitation with its sealed name and BlurHash opened, or without them if they do not open.
    fn invitation_shown(&self, item: &InvitationItem) -> InvitationShown {
        let plain = self
            .device
            .as_ref()
            .and_then(|device| device.open_invitation(item.chat, &item.sealed.0).ok())
            .and_then(|opened| serde_json::from_slice::<InvitationPlain>(&opened).ok());
        let (name, blurhash, note) = match plain {
            Some(plain) => (Some(plain.name), plain.blurhash, plain.note),
            None => (None, None, None),
        };
        InvitationShown {
            chat_type: item.chat_type,
            inviter: item.inviter,
            name,
            blurhash,
            note,
            at: item.at,
        }
    }

    /// This account's own join request changed, or was listed.
    fn asked(&mut self, request: RequestItem) {
        if request.state == RequestState::Gone {
            self.own_requests.remove(&request.chat);
        } else {
            self.own_requests.insert(request.chat, request.clone());
        }
        self.dirty = true;
        self.event(Event::Asked {
            chat: request.chat,
            name: request.name,
            state: request.state,
            at: request.at,
        });
    }

    /// Something the server wrote in a group or channel. Membership changes move what this device
    /// knows of the members: someone leaving or removed is taken out of the group by the next
    /// member online, and this account taken out drops the chat. From `sync`, this account's own
    /// leaving drops nothing while the chat list still has the chat, as it came back since.
    fn chat_event(&mut self, event: ChatEvent, live: bool) {
        self.know([event.actor].into_iter().chain(account_target(&event)));
        let chat = event.chat;
        let me = self.account;
        let listed = self.chat_cache.iter().any(|item| item.chat == chat);
        let about_me = event.target.is_some() && event.target == me && (live || !listed);
        match event.action {
            ChatAction::ChatRemoved => {
                self.show_chat_event(event);
                return self.drop_chat(chat);
            }
            ChatAction::Left | ChatAction::Removed if about_me => {
                self.show_chat_event(event);
                return self.drop_chat(chat);
            }
            ChatAction::Left | ChatAction::Removed => {
                if let Some(target) = event.target
                    && Some(target) != me
                {
                    self.chat(chat)
                        .member_list
                        .retain(|account| *account != target);
                }
                // The leaver's devices are still in the group until a member commits it.
                if event.action == ChatAction::Left {
                    self.chat(chat).members_known = false;
                    self.devices_of(chat);
                }
            }
            ChatAction::Added | ChatAction::LetIn | ChatAction::Joined => {
                if let Some(target) = event.target {
                    let state = self.chat(chat);
                    if !state.member_list.contains(&target) {
                        state.member_list.push(target);
                    }
                    if !self.accounts.contains_key(&target) {
                        self.ask_account(target);
                    }
                }
            }
            _ => {}
        }
        self.show_chat_event(event);
    }

    /// Hands a chat event to the platform with its copies opened, never their ciphertext.
    fn show_chat_event(&mut self, mut event: ChatEvent) {
        let copies = std::mem::take(&mut event.copy_list);
        let copy_list = copies
            .iter()
            .filter_map(|copy| self.open_copy(copy))
            .collect();
        self.event(Event::ChatEvent { event, copy_list });
    }

    /// One copy a history row kept: its signature checked against its device, then opened with the
    /// epoch key this device holds.
    fn open_copy(&self, message: &MessageFrame) -> Option<Shown> {
        let known = self.devices.get(&message.device?)?;
        let signature = fixed::<64>(&message.signature.as_ref()?.0)?;
        let header = message.header()?;
        let body = message
            .body
            .as_ref()
            .map(|body| body.0.clone())
            .unwrap_or_default();
        verify_message(&header, &body, &signature, &known.public).ok()?;
        let content = if header.encrypted {
            let plaintext = self
                .device
                .as_ref()?
                .open(&header, &body, &signature, &known.public)
                .ok()?;
            content_of(&plaintext, message.content_kind)
        } else {
            content_of(&body, message.content_kind)
        };
        let mine = self.account == Some(known.account);
        Some(shown(message, Some(known.account), mine, content, None))
    }

    /// Seals the profile under the chat's current key and sends it, keeping it as the one shown.
    fn send_chat_profile(&mut self, chat: Ulid, profile: ChatProfilePlain) {
        let Some(body) = self.keep_chat_profile(chat, profile) else {
            return;
        };
        let reference = reference();
        let frame = ClientFrame::ChatProfile {
            reference,
            chat,
            profile: body,
        };
        self.ask_done(frame, reference, Waiting::Done);
    }

    /// Seals the profile under the chat's profile key and keeps it as the one shown, handing back
    /// what goes up, which names its photo so the server keeps that file.
    fn keep_chat_profile(
        &mut self,
        chat: Ulid,
        profile: ChatProfilePlain,
    ) -> Option<ChatProfileBody> {
        let device = self.device.as_ref()?;
        let Ok(plain) = serde_json::to_vec(&profile) else {
            self.error("internal", "A profile could not be written");
            return None;
        };
        let (sealed, signature) = match device.seal_chat_profile(chat, None, &plain) {
            Ok(sealed) => sealed,
            Err(error) => {
                self.error("seal_failed", &error.to_string());
                return None;
            }
        };
        let photo = profile.photo.as_ref().map(|photo| photo.id);
        let state = self.chat(chat);
        let version = state.profile.as_ref().map_or(0, |(_, version)| *version);
        state.profile = Some((profile, version));
        self.dirty = true;
        Some(ChatProfileBody {
            sealed: Bytes(sealed),
            signature: Bytes(signature.to_vec()),
            photo,
        })
    }

    /// A profile from the server: checked against its device when that is known, opened, kept and
    /// shown. Its profile key is taken from the share beside it when this device has none, and one
    /// whose share is at an epoch this device cannot open yet waits until the chat is ready.
    fn chat_profile_in(&mut self, chat: Ulid, item: ChatProfileItem) {
        let Some(device) = self.device.as_mut() else {
            return;
        };
        if !device.has_chat_key(chat)
            && device
                .take_chat_key(chat, item.key_epoch, &item.key.0)
                .is_err()
        {
            self.chat(chat).profile_waiting = Some(item);
            return;
        }
        self.dirty = true;
        let Some(device) = self.device.as_ref() else {
            return;
        };
        if let (Some(known), Some(signature)) = (
            self.devices.get(&item.device),
            fixed::<64>(&item.signature.0),
        ) && crate::mls::verify_chat_profile(
            &known.public,
            &chat,
            None,
            &item.sealed.0,
            &signature,
        )
        .is_err()
        {
            return self.error(
                "bad_signature",
                "A chat's profile is not signed by its device",
            );
        }
        let opened = match device.open_chat_profile(chat, None, &item.sealed.0) {
            Ok(opened) => opened,
            Err(_) => {
                self.chat(chat).profile_waiting = Some(item);
                return;
            }
        };
        let Ok(profile) = serde_json::from_slice::<ChatProfilePlain>(&opened) else {
            return self.error("bad_profile", "A chat's profile could not be read");
        };
        let shown = ShownChatProfile {
            name: profile.name.clone(),
            bio: profile.bio.clone(),
            link_list: profile.link_list.clone(),
            rules: profile.rules.clone(),
            topic_list: profile.topic_list.clone(),
            photo: profile.photo.as_ref().map(|photo| ShownChatPhoto {
                id: photo.id,
                blurhash: photo.blurhash.clone(),
            }),
            version: item.version,
        };
        self.chat(chat).profile = Some((profile, item.version));
        self.dirty = true;
        self.event(Event::ChatProfile {
            chat,
            profile: shown,
        });
    }

    /// Changes a group's or channel's name, bio and links, keeping its topics' names and photo.
    fn edit_chat_profile(&mut self, chat: Ulid, profile: ChatProfilePlain) {
        let mut profile = match checked_chat_profile(profile) {
            Ok(profile) => profile,
            Err(message) => return self.error("bad_profile", message),
        };
        if let Some((current, _)) = &self.chat(chat).profile {
            profile.topic_list = current.topic_list.clone();
            profile.photo = current.photo.clone();
        }
        self.send_chat_profile(chat, profile);
    }

    /// Seals a topic's profile under the chat's profile key and sends it, keeping it as the one
    /// shown. It names its photo, so the server keeps that file.
    fn send_topic_profile(&mut self, chat: Ulid, topic: Ulid, profile: TopicProfilePlain) {
        let Some(device) = self.device.as_ref() else {
            return;
        };
        let Ok(plain) = serde_json::to_vec(&profile) else {
            return self.error("internal", "A profile could not be written");
        };
        let (sealed, signature) = match device.seal_chat_profile(chat, Some(topic), &plain) {
            Ok(sealed) => sealed,
            Err(error) => return self.error("seal_failed", &error.to_string()),
        };
        let photo = profile.photo.as_ref().map(|photo| photo.id);
        let state = self.chat(chat);
        let version = state
            .topic_profiles
            .get(&topic)
            .map_or(0, |(_, version)| *version);
        state.topic_profiles.insert(topic, (profile, version));
        let reference = reference();
        let frame = ClientFrame::TopicProfile {
            reference,
            chat,
            topic,
            profile: Box::new(ChatProfileBody {
                sealed: Bytes(sealed),
                signature: Bytes(signature.to_vec()),
                photo,
            }),
        };
        self.ask_done(frame, reference, Waiting::Done);
    }

    /// A topic's sealed profile from the server: checked against its device when that is known,
    /// opened under the chat's profile key, kept and shown. One this device cannot open yet stays
    /// unshown until the next list brings it again.
    fn topic_profile_in(&mut self, chat: Ulid, topic: Ulid, item: TopicProfileItem) {
        let known = self
            .chats
            .get(&chat)
            .and_then(|state| state.topic_profiles.get(&topic))
            .map(|(_, version)| *version);
        if known == Some(item.version) {
            return;
        }
        let Some(device) = self.device.as_ref() else {
            return;
        };
        if let (Some(known), Some(signature)) = (
            self.devices.get(&item.device),
            fixed::<64>(&item.signature.0),
        ) && crate::mls::verify_chat_profile(
            &known.public,
            &chat,
            Some(topic),
            &item.sealed.0,
            &signature,
        )
        .is_err()
        {
            return self.error(
                "bad_signature",
                "A topic's profile is not signed by its device",
            );
        }
        let Ok(opened) = device.open_chat_profile(chat, Some(topic), &item.sealed.0) else {
            return;
        };
        let Ok(profile) = serde_json::from_slice::<TopicProfilePlain>(&opened) else {
            return self.error("bad_profile", "A topic's profile could not be read");
        };
        let shown = ShownTopicProfile {
            bio: profile.bio.clone(),
            link_list: profile.link_list.clone(),
            rules: profile.rules.clone(),
            photo: profile.photo.as_ref().map(|photo| ShownChatPhoto {
                id: photo.id,
                blurhash: photo.blurhash.clone(),
            }),
            version: item.version,
        };
        self.chat(chat)
            .topic_profiles
            .insert(topic, (profile, item.version));
        self.event(Event::TopicProfile {
            chat,
            topic,
            profile: shown,
        });
    }

    /// Topics' settings as a `chats` entry lists them, every one this account may see; a live topic
    /// left out is hidden from it. One that was hidden and now shows again had its messages
    /// withheld, so the chat is synced from its start.
    fn topic_settings_in(&mut self, chat: Ulid, setting_list: Vec<TopicItem>) {
        let state = self.chat(chat);
        let listed: Vec<Ulid> = state
            .topics
            .as_ref()
            .map(|topics| topics.topic_list.clone())
            .unwrap_or_default();
        let unhidden: Vec<Ulid> = setting_list
            .iter()
            .map(|item| item.topic)
            .filter(|topic| state.hidden_topics.contains(topic))
            .collect();
        let mut profile_list = Vec::new();
        let mut settings = HashMap::with_capacity(setting_list.len());
        for mut item in setting_list {
            if let Some(profile) = item.profile.take() {
                profile_list.push((item.topic, profile));
            }
            settings.insert(item.topic, item);
        }
        state.hidden_topics = listed
            .into_iter()
            .filter(|topic| !settings.contains_key(topic))
            .collect();
        state.topic_settings = settings;
        let live: HashSet<Ulid> = state.topic_settings.keys().copied().collect();
        state.topic_profiles.retain(|topic, _| live.contains(topic));
        for (topic, profile) in profile_list {
            self.topic_profile_in(chat, topic, profile);
        }
        if !unhidden.is_empty() {
            self.chat(chat).refill.extend(unhidden);
            self.start_sync(Some(chat), None);
        }
    }

    /// One topic's settings, pushed live. One hidden from this account comes with nothing else and
    /// is dropped; one shown again after that syncs the chat from its start.
    fn topic_in(&mut self, chat: Ulid, mut item: TopicItem) {
        let hidden_from_me =
            item.options.hidden && item.mod_list.is_empty() && item.profile.is_none();
        let state = self.chat(chat);
        let profile = item.profile.take();
        let shown_again = !item.options.hidden && state.hidden_topics.remove(&item.topic);
        if hidden_from_me {
            state.hidden_topics.insert(item.topic);
            state.topic_settings.remove(&item.topic);
            state.topic_profiles.remove(&item.topic);
        } else {
            state.topic_settings.insert(item.topic, item.clone());
        }
        self.event(Event::TopicOptions {
            chat,
            topic: item.topic,
            options: item.options,
            mod_list: item.mod_list,
        });
        if let Some(profile) = profile {
            self.topic_profile_in(chat, item.topic, profile);
        }
        if shown_again {
            self.chat(chat).refill.insert(item.topic);
            self.start_sync(Some(chat), None);
        }
    }

    /// Topics on or off and the live ones in order, then the profile resealed with their names.
    fn set_topics(&mut self, chat: Ulid, on: bool, topic_list: Vec<TopicName>) {
        let mut named = Vec::with_capacity(topic_list.len());
        for (place, topic) in topic_list.into_iter().enumerate() {
            let name = topic.name.trim().to_string();
            let length = name.chars().count();
            if length == 0 || length > TOPIC_NAME_CHARS {
                return self.error("bad_topic", "A topic's name is 1 to 32 characters");
            }
            named.push(TopicName {
                id: topic.id,
                name,
                place: place as u64,
            });
        }
        let Some((mut profile, _)) = self.chat(chat).profile.clone() else {
            return self.error("not_joined", "This chat's profile is not known yet");
        };
        let topics = TopicsBody {
            on,
            topic_list: named.iter().map(|topic| topic.id).collect(),
        };
        let removed = self.chat(chat).topics.as_ref().is_some_and(|old| {
            old.topic_list
                .iter()
                .any(|topic| !topics.topic_list.contains(topic))
        });
        let reference = reference();
        let frame = ClientFrame::Topics {
            reference,
            chat,
            topics: topics.clone(),
        };
        self.ask_done(
            frame,
            reference,
            Waiting::TopicsSaved {
                chat,
                on,
                topic_list: named.clone(),
                removed,
            },
        );
        self.chat(chat).topics = Some(topics);
        profile.topic_list = named;
        self.send_chat_profile(chat, profile);
    }

    /// The live topics with the names the profile gives them.
    fn named_topics(&mut self, chat: Ulid, topic_list: &[Ulid]) -> Vec<TopicName> {
        let state = self.chat(chat);
        let mut named = Vec::with_capacity(topic_list.len());
        for (place, id) in topic_list.iter().enumerate() {
            let name = state
                .profile
                .as_ref()
                .and_then(|(profile, _)| profile.topic_list.iter().find(|topic| topic.id == *id))
                .map(|topic| topic.name.clone())
                .unwrap_or_default();
            named.push(TopicName {
                id: *id,
                name,
                place: place as u64,
            });
        }
        named
    }

    /// Seen up to a message inside a topic or a thread, sent at once with no retry, as the main
    /// list's position is the one counted for the chat.
    fn seen_in_place(
        &mut self,
        chat: Ulid,
        up_to_seq: u64,
        topic: Option<Ulid>,
        thread: Option<u64>,
    ) {
        let reference = reference();
        let frame = ClientFrame::Seen {
            reference,
            chat,
            up_to_seq,
            client_seen_at: self.now,
            topic,
            thread_seq: thread,
        };
        self.ask(frame, reference, Waiting::Done);
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
            Ok(plaintext) => Ok(content_of(&plaintext, None).text),
            Err(_) => Ok(None),
        }
    }

    // ---------------------------------------------------------------------------------------------
    // Frames
    // ---------------------------------------------------------------------------------------------

    fn frame(&mut self, frame: ServerFrame) {
        match frame {
            ServerFrame::ChatEvent(event) => {
                let last = SyncPosition {
                    at: event.server_at,
                    chat: event.chat,
                    seq: 0,
                    n: event.n,
                };
                let chat = event.chat;
                self.chat_event((*event).clone(), true);
                // A chat this event took away is wiped from the archive, not kept in it.
                if !self.chats.contains_key(&chat) {
                    return;
                }
                let rows = Rows {
                    message_list: Vec::new(),
                    call_list: Vec::new(),
                    chat_event_list: vec![*event],
                };
                self.save_rows(rows, Some(last), None);
            }
            ServerFrame::Members {
                reference,
                chat,
                member_list,
            } => {
                if self.waiting.remove(&reference).is_some() {
                    self.know(member_list.iter().map(|item| item.account));
                    self.event(Event::Members { chat, member_list });
                }
            }
            ServerFrame::TopicId {
                reference,
                chat,
                topic,
            } => {
                if self.waiting.remove(&reference).is_some() {
                    self.event(Event::TopicId { chat, topic });
                }
            }
            ServerFrame::Topics { chat, topics } => {
                let topic_list = self.named_topics(chat, &topics.topic_list);
                let on = topics.on;
                let state = self.chat(chat);
                // A removed topic's messages were wiped with no delete sent, so this chat is synced
                // to drop them from the archive at once.
                let removed = state.topics.as_ref().is_some_and(|old| {
                    old.topic_list
                        .iter()
                        .any(|topic| !topics.topic_list.contains(topic))
                });
                state.topics = Some(topics);
                self.dirty = true;
                if removed {
                    let since = self.chat(chat).known_at.or(self.since);
                    self.start_sync(Some(chat), since);
                }
                self.event(Event::Topics {
                    chat,
                    on,
                    topic_list,
                });
            }
            ServerFrame::ChatProfile { chat, profile } => self.chat_profile_in(chat, profile),
            ServerFrame::Topic { chat, item } => self.topic_in(chat, *item),
            ServerFrame::ChatOptions { chat, options } => {
                self.event(Event::ChatOptions { chat, options })
            }
            ServerFrame::Findable { chat, findable } => {
                let findable = match (findable.findable, findable.name) {
                    (true, Some(name)) => Some(FindableItem {
                        name,
                        bio: findable.bio,
                    }),
                    _ => None,
                };
                self.event(Event::Findable { chat, findable });
            }
            ServerFrame::JoinRequest(request) => {
                if Some(request.account) == self.account {
                    self.asked(*request);
                } else {
                    self.know([request.account]);
                    self.event(Event::JoinRequest { request: *request });
                }
            }
            ServerFrame::Invitation { chat, invitation } => {
                self.know(invitation.iter().map(|item| item.inviter));
                let shown = invitation.as_ref().map(|item| self.invitation_shown(item));
                match invitation {
                    Some(item) => self.invitations.insert(chat, item),
                    None => self.invitations.remove(&chat),
                };
                self.dirty = true;
                self.event(Event::Invitation {
                    chat,
                    invitation: shown,
                });
            }
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
                version,
                kv_list,
                account_list,
                seconds,
                ..
            } => match self.waiting.remove(&reference) {
                Some(Waiting::KvSet) => self.kv_refused(&code, &message, version, kv_list),
                Some(Waiting::Content { chat, local }) if seconds.is_some() => {
                    self.fail_waiting(chat, local, &code, seconds);
                }
                Some(Waiting::Commit { chat, local, plan })
                    if code == "invite_needed" && account_list.is_some() =>
                {
                    self.invite_needed(chat, local, plan, account_list.unwrap_or_default());
                }
                Some(waiting) => self.refused(waiting, &code, &message),
                None => {}
            },
            ServerFrame::Presence(presence) => self.presence_shown(*presence),
            ServerFrame::PresenceOf {
                reference,
                presence,
            } => {
                if self.waiting.remove(&reference).is_some() {
                    self.presence_shown(presence);
                }
            }
            ServerFrame::Kv {
                reference,
                version,
                kv_list,
                from,
                set,
                remove,
            } => match reference {
                Some(reference) => match self.waiting.remove(&reference) {
                    Some(Waiting::Kv) => self.kv_answered(version, kv_list),
                    Some(Waiting::KvSet) => {
                        self.kv_version = Some(version);
                        self.kv_flight = None;
                        self.dirty = true;
                        if self.kv_ready {
                            self.send_kv();
                        } else {
                            self.ask_kv(self.kv_version);
                        }
                    }
                    _ => {}
                },
                None => self.kv_pushed(
                    from,
                    version,
                    set.unwrap_or_default(),
                    remove.unwrap_or_default(),
                ),
            },
            ServerFrame::Message(message) => self.live(*message),
            ServerFrame::Chats {
                reference,
                chat_list,
                message_list,
                call_list,
                more,
                chat_folders,
                chat_event_list,
                asked_list,
                invitation_list,
            } => {
                if let Some(folders) = chat_folders {
                    self.folders_listed(folders);
                }
                // Pending requests and invitations come whole with the first page only, so one shown
                // before and no longer listed, expired or its chat removed meanwhile, is shown gone.
                // An invitation being accepted stays: the server dropped it, and the join still
                // carries its signature.
                let first = matches!(self.waiting.get(&reference), Some(Waiting::Sync { .. }))
                    && self.sync_run.as_ref().is_some_and(|run| run.first);
                if first {
                    let gone: Vec<RequestItem> = self
                        .own_requests
                        .values()
                        .filter(|item| !asked_list.iter().any(|listed| listed.chat == item.chat))
                        .cloned()
                        .collect();
                    for mut request in gone {
                        request.state = RequestState::Gone;
                        self.asked(request);
                    }
                    let gone: Vec<Ulid> = self
                        .invitations
                        .keys()
                        .filter(|chat| {
                            !invitation_list.iter().any(|listed| listed.chat == **chat)
                                && !self.chats.get(*chat).is_some_and(|state| state.joining)
                        })
                        .copied()
                        .collect();
                    for chat in gone {
                        self.invitations.remove(&chat);
                        self.dirty = true;
                        self.event(Event::Invitation {
                            chat,
                            invitation: None,
                        });
                    }
                }
                for request in asked_list {
                    self.asked(request);
                }
                for invitation in invitation_list {
                    let chat = invitation.chat;
                    self.know([invitation.inviter]);
                    let shown = self.invitation_shown(&invitation);
                    self.invitations.insert(chat, invitation);
                    self.dirty = true;
                    self.event(Event::Invitation {
                        chat,
                        invitation: Some(shown),
                    });
                }
                let rows = Rows {
                    message_list,
                    call_list,
                    chat_event_list,
                };
                self.synced(reference, chat_list, rows, more)
            }
            ServerFrame::Welcome { chat, body } => self.welcome(chat, &body.0),
            ServerFrame::Typing {
                chat,
                account,
                on,
                topic,
                thread_seq,
            } => self.event(Event::Typing {
                chat,
                account,
                on,
                topic,
                thread: thread_seq,
            }),
            ServerFrame::Marks { chat, mark } => self.event(Event::Marks { chat, mark }),
            ServerFrame::Draft { chat, draft } => self.show_draft(chat, draft),
            ServerFrame::ChatFolders(folders) => self.folders_listed(*folders),
            ServerFrame::FolderId { reference, folder } => match self.waiting.remove(&reference) {
                Some(Waiting::FolderId) => self.event(Event::FolderId { folder }),
                Some(Waiting::SuggestionFolder {
                    chat,
                    seq,
                    folder: suggested,
                }) => self.accept(chat, seq, suggested, Some(folder)),
                _ => {}
            },
            ServerFrame::Times {
                reference,
                chat,
                seq,
                member_list,
                more,
            } => {
                self.waiting.remove(&reference);
                self.event(Event::Times {
                    chat,
                    seq,
                    member_list,
                    more,
                });
            }
            ServerFrame::Revoked {
                account,
                device,
                statement,
            } => self.revoked(account, device, &statement.0),
            ServerFrame::CallEvent(event) => self.call_row(*event, true),
            ServerFrame::Profile {
                account,
                version,
                username,
                display_name,
            } => {
                self.names_changed(account, username, display_name);
                if self.profile_moved(account, version) {
                    self.ask_profile(account);
                }
            }
            ServerFrame::ProfileKeys {
                chat,
                profile_key_list,
            } => self.profile_keys_pushed(chat, profile_key_list),
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
            topic: frame.topic,
            thread_seq: frame.thread_seq,
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
        self.save_rows(Rows::messages(vec![row]), Some(last), None);
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
            Plan::Join { invite, .. } => {
                if invite.is_some() {
                    self.invitations.remove(&chat);
                }
                self.ready(chat);
                self.devices_of(chat);
                self.restore_chat(chat);
                if !self.restoring.contains(&chat) {
                    let waiting = std::mem::take(&mut self.chat(chat).waiting);
                    self.open_list(chat, waiting, true, true);
                }
            }
            Plan::Add { added, .. } if added.is_empty() => self.ready(chat),
            // The profile sealed under the new key went with the commit.
            Plan::Add { .. } => {}
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
                    state.start_profile = None;
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
            Waiting::Folders { folders } => {
                let (taken, kept) = std::mem::take(&mut self.acks_waiting)
                    .into_iter()
                    .partition::<Vec<_>, _>(|waiting| waiting.folders == folders);
                self.acks_waiting = kept;
                if self.folders_out.as_ref() == Some(&folders) {
                    self.folders = Some(folders);
                    self.folders_out = None;
                    self.dirty = true;
                }
                for waiting in taken {
                    self.send_ack(waiting.chat, waiting.ack);
                }
            }
            Waiting::Left { chat } => self.drop_chat(chat),
            Waiting::TopicsSaved {
                chat,
                on,
                topic_list,
                removed,
            } => {
                if removed {
                    let since = self.chat(chat).known_at.or(self.since);
                    self.start_sync(Some(chat), since);
                }
                self.event(Event::Topics {
                    chat,
                    on,
                    topic_list,
                });
            }
            Waiting::InviteAnswer { chat, accept } => self.invite_answered(chat, accept),
            Waiting::OptionsSaved { chat, options } => {
                self.event(Event::ChatOptions { chat, options })
            }
            Waiting::FindableSaved { chat, findable } => {
                self.event(Event::Findable { chat, findable })
            }
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
                    topic: None,
                    thread_seq: None,
                },
                Which::Seen => ClientFrame::Seen {
                    reference,
                    chat,
                    up_to_seq: up_to,
                    client_seen_at: at,
                    topic: None,
                    thread_seq: None,
                },
            };
            self.ask(frame, reference, Waiting::Position { chat, which, up_to });
        }
    }

    fn refused(&mut self, waiting: Waiting, code: &str, message: &str) {
        match waiting {
            // Sealed under a key older than the chat's: the commits it missed are fetched, then it
            // is sealed again under the newest.
            Waiting::Content { chat, local } if code == "stale_epoch" => {
                self.in_flight.remove(&local);
                self.queue.put_back(local);
                self.chat(chat).catching_up = true;
                let since = self.chat(chat).known_at.or(self.since);
                self.start_sync(Some(chat), since);
            }
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
                // A start's photo the server would not keep, such as one over its limit: the group
                // is still made, its first profile sealed again without it, and the person told.
                if code == "no_such_file" && epoch == 0 {
                    let unnamed = self
                        .chats
                        .get(&chat)
                        .filter(|state| state.start_profile.is_some())
                        .and_then(|state| state.profile.as_ref())
                        .filter(|(profile, _)| profile.photo.is_some())
                        .map(|(profile, _)| ChatProfilePlain {
                            photo: None,
                            ..profile.clone()
                        });
                    if let Some(profile) = unnamed
                        && let Some(body) = self.keep_chat_profile(chat, profile)
                    {
                        let state = self.chat(chat);
                        state.uploading = false;
                        if let Some(start) = state.start_profile.as_mut() {
                            start.body = body;
                        }
                        self.error("photo_failed", message);
                        return self.send_outs(chat);
                    }
                }
                let state = self.chat(chat);
                state.uploading = false;
                if matches!(state.group_info_out, Some((out, _)) if out == epoch) {
                    state.group_info_out = None;
                    state.start_profile = None;
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
            Waiting::FolderId | Waiting::SuggestionFolder { .. } => self.error(code, message),
            Waiting::TopicId
            | Waiting::Members
            | Waiting::Left { .. }
            | Waiting::InviteAnswer { .. } => self.error(code, message),
            // Refused, it would be refused again, so it goes, and the server's shows instead, with
            // no ack for a suggestion it accepted.
            Waiting::Folders { folders } => {
                self.error(code, message);
                self.acks_waiting
                    .retain(|waiting| waiting.folders != folders);
                if self.folders_out.as_ref() == Some(&folders) {
                    self.folders_out = None;
                    self.dirty = true;
                    self.show_folders();
                }
            }
            Waiting::Kv => self.error(code, message),
            Waiting::KvSet => self.kv_refused(code, message, None, None),
            Waiting::TopicsSaved { .. }
            | Waiting::OptionsSaved { .. }
            | Waiting::FindableSaved { .. }
            | Waiting::Done => self.error(code, message),
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

    /// Asks for this account's settings: the whole set unless `version` is the server's. Nothing
    /// pending goes out until the answer.
    fn ask_kv(&mut self, version: Option<u64>) {
        self.kv_ready = false;
        let reference = reference();
        self.ask(
            ClientFrame::Kv { reference, version },
            reference,
            Waiting::Kv,
        );
    }

    /// The answer to `kv`. A whole set replaces what this device holds, and a change of its own made
    /// on an older version gives way to it, as the server would refuse it. One made before this
    /// device ever had the settings, as a new account's first language, stays on top.
    fn kv_answered(&mut self, version: u64, kv_list: Option<Vec<KvItem>>) {
        if let Some(mut kv_list) = kv_list {
            kv_list.sort_by(|one, other| one.key.cmp(&other.key));
            if self.kv_version.is_some() {
                self.kv_out = None;
            }
            self.kv = kv_list;
            if let Some(out) = &self.kv_out {
                apply_change(&mut self.kv, &out.set, &out.remove);
            }
            self.dirty = true;
            self.show_kv();
        }
        self.kv_version = Some(version);
        self.kv_ready = true;
        self.send_kv();
    }

    /// Another device's change. On this device's version it applies, with this device's own still
    /// waiting on top; on another, the whole set is asked for. While an answer is awaited, it waits
    /// for that answer.
    fn kv_pushed(
        &mut self,
        from: Option<u64>,
        version: u64,
        set: Vec<KvItem>,
        remove: Vec<String>,
    ) {
        if self.kv_flight.is_some() || !self.kv_ready {
            self.kv_ready = false;
            return;
        }
        if from.is_none() || from != self.kv_version {
            return self.ask_kv(self.kv_version);
        }
        apply_change(&mut self.kv, &set, &remove);
        if let Some(out) = &self.kv_out {
            apply_change(&mut self.kv, &out.set, &out.remove);
        }
        self.kv_version = Some(version);
        self.dirty = true;
        self.show_kv();
        self.send_kv();
    }

    /// A refused change. On `kv_stale` the server's set wins, and comes with the refusal; any
    /// other refusal is reported, and the whole set is asked for again.
    fn kv_refused(
        &mut self,
        code: &str,
        message: &str,
        version: Option<u64>,
        kv_list: Option<Vec<KvItem>>,
    ) {
        self.kv_flight = None;
        self.dirty = true;
        if code == "kv_stale"
            && let (Some(version), Some(mut kv_list)) = (version, kv_list)
        {
            kv_list.sort_by(|one, other| one.key.cmp(&other.key));
            self.kv = kv_list;
            self.kv_version = Some(version);
            self.kv_out = None;
            self.kv_ready = true;
            return self.show_kv();
        }
        self.error(code, message);
        self.ask_kv(None);
    }

    /// Sends this device's own change still waiting, once the settings are known on this
    /// connection and nothing else is in flight.
    fn send_kv(&mut self) {
        if !self.kv_ready || self.kv_flight.is_some() {
            return;
        }
        let Some(mut change) = self.kv_out.take() else {
            return;
        };
        change.version = self.kv_version.unwrap_or(0);
        let reference = reference();
        let frame = ClientFrame::KvSet {
            reference,
            change: change.clone(),
        };
        if self.ask(frame, reference, Waiting::KvSet) {
            self.kv_flight = Some(change);
        } else {
            self.kv_out = Some(change);
        }
    }

    /// Shows this account's settings, opened. One that does not open or read is left out.
    fn show_kv(&mut self) {
        let Some(device) = self.device.as_ref() else {
            return;
        };
        let mut setting_list = Vec::with_capacity(self.kv.len());
        for item in &self.kv {
            let opened = if unsealed(&item.key) {
                zeroize::Zeroizing::new(item.value.0.clone())
            } else {
                let Ok(opened) = device.open_kv(&item.key, &item.value.0) else {
                    continue;
                };
                opened
            };
            if let Ok(value) = serde_json::from_slice(&opened) {
                setting_list.push(Setting {
                    key: item.key.clone(),
                    value,
                });
            }
        }
        self.event(Event::Kv { setting_list });
    }

    /// Shows and sends this account's whole arrangement, with plain names: only a changed name is
    /// sealed again. False when it could not be sealed, which is reported.
    fn arrange(&mut self, folder_list: Vec<Folder>, default: Ulid) -> bool {
        let Some(device) = self.device.as_ref() else {
            return false;
        };
        // A name the same as the one shown keeps its sealed bytes, so the server sees no
        // rename.
        let mut shown = HashMap::new();
        let current = match &self.folders_out {
            Some(folders) => Some(folders),
            None => self.folders.as_ref(),
        };
        if let Some(current) = current {
            for folder in &current.folder_list {
                if let Some(name) = &folder.name {
                    shown.insert(folder.id, name.clone());
                }
            }
        }
        let mut sealed_list = Vec::with_capacity(folder_list.len());
        for folder in folder_list {
            let name = match &folder.name {
                None => None,
                Some(name) => {
                    let kept = match shown.get(&folder.id) {
                        Some(sealed) => match device.open_folder_name(folder.id, &sealed.0) {
                            Ok(opened) if opened.as_slice() == name.0.as_bytes() => {
                                Some(sealed.clone())
                            }
                            _ => None,
                        },
                        None => None,
                    };
                    match kept {
                        Some(sealed) => Some(sealed),
                        None => match device.seal_folder_name(folder.id, &name.0) {
                            Ok(sealed) => Some(Bytes(sealed)),
                            Err(_) => {
                                self.error("seal", "A folder name could not be sealed");
                                return false;
                            }
                        },
                    }
                }
            };
            sealed_list.push(FolderItem {
                id: folder.id,
                name,
                chat_list: folder.chat_list.clone(),
                pin_list: folder.pin_list.clone(),
            });
        }
        let folders = ChatFolders {
            folder_list: sealed_list,
            default,
        };
        // An ack waiting on the arrangement this one replaces waits on this one, made from
        // what was shown, its change included.
        for waiting in &mut self.acks_waiting {
            if self.folders_out.as_ref() == Some(&waiting.folders) {
                waiting.folders = folders.clone();
            }
        }
        self.folders_out = Some(folders);
        self.dirty = true;
        self.show_folders();
        self.send_folders();
        true
    }

    /// Accepts a folder suggestion: its folder found by its folded name, or `made` once its id has
    /// come, and the ack sent once the server took the arrangement, or at once when nothing changes.
    fn accept(&mut self, chat: Ulid, seq: u64, folder: FolderSuggestion, made: Option<Ulid>) {
        let opened = match (
            self.device.as_ref(),
            self.folders_out.as_ref().or(self.folders.as_ref()),
        ) {
            (Some(device), Some(shown)) => {
                let mut folder_list = Vec::with_capacity(shown.folder_list.len() + 1);
                for item in &shown.folder_list {
                    let name = item.name.as_ref().map(|sealed| {
                        match device.open_folder_name(item.id, &sealed.0) {
                            Ok(opened) => Secret(String::from_utf8_lossy(&opened).into_owned()),
                            Err(_) => Secret(String::new()),
                        }
                    });
                    folder_list.push(Folder {
                        id: item.id,
                        name,
                        chat_list: item.chat_list.clone(),
                        pin_list: item.pin_list.clone(),
                    });
                }
                Some((folder_list, shown.default))
            }
            _ => None,
        };
        let Some((mut folder_list, default)) = opened else {
            return self.error("offline", "This account's folders have not arrived yet");
        };
        let name = folder.name.trim();
        let wanted = fold_name(name);
        let found = match made {
            Some(_) => None,
            None => folder_list.iter().position(
                |item| matches!(&item.name, Some(shown) if fold_name(&shown.0) == wanted),
            ),
        };
        let (target, created) = match (found, made) {
            (Some(target), _) => (target, false),
            (None, Some(id)) => {
                folder_list.push(Folder {
                    id,
                    name: Some(Secret(name.to_string())),
                    chat_list: Some(Vec::new()),
                    pin_list: Vec::new(),
                });
                (folder_list.len() - 1, true)
            }
            (None, None) => {
                let reference = reference();
                let frame = ClientFrame::FolderId { reference };
                if !self.ask(
                    frame,
                    reference,
                    Waiting::SuggestionFolder { chat, seq, folder },
                ) {
                    self.error("offline", "A new folder needs a connection");
                }
                return;
            }
        };
        // Pins follow the folders' rules: one in a folder left goes with it, and the target's stays.
        let mut changed = created;
        let mut left = 0;
        for (at, item) in folder_list.iter_mut().enumerate() {
            let Some(chat_list) = item.chat_list.as_mut() else {
                continue;
            };
            if at == target {
                if !chat_list.contains(&chat) {
                    chat_list.push(chat);
                    changed = true;
                }
            } else if folder.r#move && chat_list.contains(&chat) {
                chat_list.retain(|one| *one != chat);
                item.pin_list.retain(|one| *one != chat);
                left += 1;
                changed = true;
            }
        }
        let ack = Ack {
            suggestion: seq,
            accepted: true,
            created: Some(created),
            left: Some(left),
        };
        if !changed {
            return self.send_ack(chat, ack);
        }
        if self.arrange(folder_list, default)
            && let Some(folders) = self.folders_out.clone()
        {
            self.acks_waiting.push(WaitingAck { folders, chat, ack });
        }
    }

    fn send_ack(&mut self, chat: Ulid, ack: Ack) {
        let fields = match serde_json::to_string(&ack) {
            Ok(fields) => fields,
            Err(_) => return self.error("internal", "Could not write the ack"),
        };
        self.push_content(
            chat,
            Kind::Content,
            None,
            Content {
                content_kind: Some(ContentKind::Ack),
                fields: Some(Secret(fields)),
                ack: Some(ack),
                ..Content::default()
            },
        );
    }

    /// Sends the arrangement still waiting, if connected. Offline, the next connection sends it.
    fn send_folders(&mut self) {
        let Some(folders) = self.folders_out.clone() else {
            return;
        };
        let reference = reference();
        let frame = ClientFrame::ChatFolders {
            reference,
            folders: folders.clone(),
        };
        self.ask(frame, reference, Waiting::Folders { folders });
    }

    /// The server's arrangement, from a list or another device. One of this device's own still
    /// waiting stays shown, and goes when it can.
    fn folders_listed(&mut self, folders: ChatFolders) {
        self.folders = Some(folders);
        self.dirty = true;
        if self.folders_out.is_none() {
            self.show_folders();
        }
    }

    /// Shows this account's folders, its own unsent arrangement over the server's, names opened.
    fn show_folders(&mut self) {
        let shown = match &self.folders_out {
            Some(folders) => folders,
            None => match &self.folders {
                Some(folders) => folders,
                None => return,
            },
        };
        let mut folder_list = Vec::with_capacity(shown.folder_list.len());
        for folder in &shown.folder_list {
            let name = match &folder.name {
                None => None,
                Some(sealed) => {
                    let opened = match self.device.as_ref() {
                        Some(device) => device.open_folder_name(folder.id, &sealed.0).ok(),
                        None => None,
                    };
                    match opened {
                        Some(opened) => Some(Secret(String::from_utf8_lossy(&opened).into_owned())),
                        None => Some(Secret(String::new())),
                    }
                }
            };
            folder_list.push(Folder {
                id: folder.id,
                name,
                chat_list: folder.chat_list.clone(),
                pin_list: folder.pin_list.clone(),
            });
        }
        let default = shown.default;
        self.event(Event::Folders {
            folder_list,
            default,
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
        let mut profile_list = Vec::new();
        let mut topics_list = Vec::new();
        let mut setting_list = Vec::new();
        // A group or channel this device was in and the list no longer has went while it was away.
        let mut gone_list = Vec::new();
        for (chat, state) in &self.chats {
            if state.chat_type.is_some_and(grouped)
                && !chat_list.iter().any(|item| item.chat == *chat)
            {
                gone_list.push(*chat);
            }
        }
        for chat in gone_list {
            self.drop_chat(chat);
        }
        for mut item in chat_list {
            let own_epoch = match self.device.as_ref() {
                Some(device) => device.epoch(item.chat).ok(),
                None => None,
            };
            let state = self.chats.entry(item.chat).or_default();
            state.chat_type = Some(item.chat_type);
            let mut member_list = vec![me];
            if grouped(item.chat_type) {
                // Its members come from the server's device list, asked for before any commit,
                // as some may have left while this device was away.
                state.members_known = false;
                let topics = TopicsBody {
                    on: item.topics_on,
                    topic_list: item.topic_list.clone(),
                };
                // Topics this device has not shown yet, a change made while it was away too, are
                // shown as a live change is.
                if state.topics.as_ref() != Some(&topics) {
                    topics_list.push((item.chat, topics.on));
                }
                state.topics = Some(topics);
                setting_list.push((item.chat, item.topic_setting_list.clone()));
                let known = state.profile.as_ref().map(|(_, version)| *version);
                if let Some(profile) = item.profile.take()
                    && known != Some(profile.version)
                {
                    profile_list.push((item.chat, profile));
                }
            } else {
                for mark in &item.mark_list {
                    if !member_list.contains(&mark.account) {
                        member_list.push(mark.account);
                    }
                }
                if state.member_list.is_empty() || member_list.len() > 1 {
                    state.member_list = member_list.clone();
                }
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
        for (chat, profile) in profile_list {
            self.chat_profile_in(chat, profile);
        }
        for (chat, list) in setting_list {
            self.topic_settings_in(chat, list);
        }
        // After the profiles, so the topics carry their new names. The reconnect's sync brings any
        // removed topic's wiped rows.
        for (chat, on) in topics_list {
            let ids = self
                .chat(chat)
                .topics
                .as_ref()
                .map(|topics| topics.topic_list.clone())
                .unwrap_or_default();
            let topic_list = self.named_topics(chat, &ids);
            self.event(Event::Topics {
                chat,
                on,
                topic_list,
            });
        }
        let chat_list: Vec<Ulid> = self.chat_cache.iter().map(|item| item.chat).collect();
        for chat in chat_list {
            self.profile_pass(chat);
        }
        // Saving a bio or links keeps the photo, so this account's own is always known.
        if !self.profiles.contains_key(&me) {
            self.ask_profile(me);
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
    fn synced(&mut self, reference: Ulid, chat_list: Vec<ChatItem>, rows: Rows, more: bool) {
        let Rows {
            message_list,
            call_list,
            chat_event_list,
        } = rows;
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
        // A group's or channel's events sit at seq 0, numbered by their own n.
        for event in &chat_event_list {
            let position = SyncPosition {
                at: event.server_at,
                chat: event.chat,
                seq: 0,
                n: event.n,
            };
            last = last.max(Some(position));
        }
        for event in &call_list {
            self.call_row(event.clone(), false);
        }
        for event in &chat_event_list {
            self.chat_event(event.clone(), false);
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
        let rows = Rows {
            message_list,
            call_list,
            chat_event_list,
        };
        self.save_rows(rows, last, Some(page));
    }

    /// Hands rows to the platform to keep. A delete the server has saved, this device's own once
    /// answered or another's as it arrives, brings its target's removed row with it, as the server
    /// already wiped it, so the archive never holds a deleted message until the next `sync`.
    fn save_rows(&mut self, rows: Rows, last: Option<SyncPosition>, page: Option<PageSaved>) {
        let Rows {
            mut message_list,
            call_list,
            chat_event_list,
        } = rows;
        let wiped: Vec<MessageFrame> = message_list
            .iter()
            .filter(|message| message.kind == Some(Kind::Delete))
            .filter_map(|message| {
                Some(MessageFrame {
                    chat: message.chat,
                    seq: message.pointer_seq?,
                    device: None,
                    kind: None,
                    content_kind: None,
                    encrypted: None,
                    epoch: None,
                    pointer_seq: None,
                    topic: None,
                    thread_seq: None,
                    client_sent_at: None,
                    server_saved_at: message.server_saved_at,
                    body: None,
                    signature: None,
                })
            })
            .collect();
        message_list.extend(wiped);
        let id = self.next_id;
        self.next_id += 1;
        self.rows_saving.insert(id, RowsSave { last, page });
        self.out.push(Output::SaveRows {
            id,
            message_list,
            call_list,
            chat_event_list,
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
            let state = self.chat(*chat);
            state.busy = Busy::Idle;
            state.catching_up = false;
            if run.chat.is_some() {
                state.refill.clear();
            }
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
        self.save_rows(Rows::messages(vec![message]), Some(last), None);
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
            let rows = Rows {
                message_list: Vec::new(),
                call_list: vec![event],
                chat_event_list: Vec::new(),
            };
            self.save_rows(rows, Some(last), None);
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
                // A topic shown again: what was withheld from it opens now.
                let state = self.chat(chat);
                let withheld = message.kind == Some(Kind::Edit)
                    || message
                        .topic
                        .is_some_and(|topic| state.refill.contains(&topic));
                if withheld && !state.refill.is_empty() {
                    opened.push(message);
                }
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
            if kind == Kind::Line {
                let body = match &message.body {
                    Some(body) => serde_json::from_slice::<LineBody>(&body.0).ok(),
                    None => None,
                };
                let Some(body) = body else {
                    self.problem(&message, Some(known.account), Problem::Malformed);
                    continue;
                };
                for account in &body.account_list {
                    if !self.accounts.contains_key(account) {
                        self.ask_account(*account);
                    }
                }
                self.event(Event::Line {
                    chat,
                    seq: message.seq,
                    device,
                    account: known.account,
                    action: body.action,
                    account_list: body.account_list,
                    saved_at: message.server_saved_at.unwrap_or(0),
                    mine: self.account == Some(known.account),
                });
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
                content_of(&body, message.content_kind)
            } else {
                let Some(own) = self.device.as_ref() else {
                    return;
                };
                let current = own.epoch(chat).ok();
                match own.open(&header, &body, &signature, &known.public) {
                    Ok(plaintext) => content_of(&plaintext, message.content_kind),
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
                removed: Vec::new(),
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
            Asked::Challenge { credentials } => {
                let Some(ChallengeReply { challenge, account }) = self.data(body) else {
                    return;
                };
                if self.device.is_some() {
                    return self.prove_and_log_in(credentials, challenge);
                }
                // The platform loads this account's slots, if it keeps any, and answers.
                self.opening = Some(Opening {
                    account,
                    credentials,
                    challenge,
                });
                self.out.push(Output::Open { account });
            }
            Asked::Login { credentials, .. } => {
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
                self.stopped = false;
                self.session_lost = false;
                self.limited = false;
                // An agent learns its username from its own info, so its `SignedIn` waits for it.
                let signed_in = match credentials {
                    Credentials::Password { username, .. } => {
                        self.username = Some(username.clone());
                        self.event(Event::SignedIn {
                            account,
                            username,
                            device,
                        });
                        None
                    }
                    Credentials::Agent { .. } => Some(device),
                };
                if kept.is_some() {
                    self.event(Event::Token { token: kept });
                }
                self.asked_accounts.insert(account);
                self.get(
                    format!("/api/v1/accounts/{account}"),
                    Asked::Me { signed_in },
                );
            }
            Asked::Me { signed_in } => {
                let Some(info) = self.data::<AccountInfo>(body) else {
                    return;
                };
                if let Some(device) = signed_in {
                    self.username = Some(info.username.clone());
                    self.event(Event::SignedIn {
                        account: info.id,
                        username: info.username.clone(),
                        device,
                    });
                }
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
                // An invited account joins with its invitation.
                let mut invite = None;
                if let (Some(me), Some(invitation)) = (self.account, self.invitations.get(&chat)) {
                    invite = Some(InviteStatement {
                        account: me,
                        device: invitation.device,
                        signature: invitation.signature.clone(),
                    });
                }
                self.chat(chat).plan_list.push(Plan::Join {
                    group_info: group_info.0,
                    invite,
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
            Asked::CreateGroup { chat_type, start } => {
                if let Some(ChatReply { chat, .. }) = self.data(body) {
                    self.group_created(chat, chat_type, *start);
                }
            }
            Asked::ChatPhotoFile {
                chat,
                topic,
                photo,
                starting,
            } => {
                let Some(FileReply { file }) = self.data(body) else {
                    return self.chat_photo_failed(
                        chat,
                        starting,
                        "bad_reply",
                        "No file came back",
                    );
                };
                let sealed = photo.sealed.clone();
                self.issue(Request {
                    method: "PUT".to_string(),
                    path: format!("/api/v1/files/{file}/chunks/0"),
                    body: None,
                    bytes: Some(sealed),
                    raw: false,
                    asked: Asked::ChatPhotoChunk {
                        chat,
                        topic,
                        file,
                        photo,
                        starting,
                    },
                    attempt: 0,
                });
            }
            Asked::ChatPhotoChunk {
                chat,
                topic,
                file,
                photo,
                starting,
            } => self.chat_photo_up(chat, topic, file, *photo, starting),
            Asked::Requests { chat } => {
                if let Some(RequestListReply { request_list }) = self.data(body) {
                    self.know(request_list.iter().map(|item| item.account));
                    self.event(Event::Requests { chat, request_list });
                }
            }
            Asked::AddClaim {
                chat,
                account,
                claim,
            } => {
                let mut package_list = Vec::new();
                if let Some(KeyPackageListReply { key_package_list }) = self.data(body) {
                    package_list = key_package_list;
                }
                self.add_claimed(chat, account, claim, package_list);
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
            Asked::Photo { photo, .. } | Asked::ChatPhotoGet { photo, .. } => {
                self.photo_asked.remove(&photo);
                self.error("bad_reply", "The photo could not be read");
            }
            Asked::Profile { account } => {
                self.profile_asked.remove(&account);
                if let Some(reply) = self.data::<ProfileReply>(body) {
                    self.profile_got(reply);
                }
            }
            Asked::PutProfile {
                account,
                body: profile,
            } => {
                if let Some(VersionReply { version }) = self.data(body) {
                    self.profile_saved(account, version, *profile);
                }
            }
            Asked::AddPhoto {
                account,
                key,
                size,
                blurhash,
            } => {
                let Some(PhotoReply { photo }) = self.data(body) else {
                    return;
                };
                let Some(mut profile) = self.profile_body(account) else {
                    return;
                };
                profile.photo = Some(ProfilePhoto {
                    id: photo,
                    key: Bytes(key.to_vec()),
                    size,
                    blurhash,
                });
                self.put_profile(account, profile);
            }
            // The server's `profile` frame brings the names, and a moved version the rest.
            Asked::Names => self.event(Event::ProfileSaved),
            Asked::Search => {
                if let Some(AccountListReply {
                    account_list,
                    chat_list,
                }) = self.data(body)
                {
                    self.event(Event::SearchResults {
                        account_list,
                        chat_list,
                    });
                }
            }
            Asked::Devices => {
                if let Some(reply) = self.data::<DevicesReply>(body) {
                    self.event(Event::DeviceList { reply });
                }
            }
            Asked::CreateAgent { username, words } => {
                if let Some(CreatedAgentReply { account, token }) = self.data(body) {
                    self.agent_made(account, username, Secret(token), words);
                }
            }
            Asked::AgentKeys {
                account,
                username,
                token,
                words,
            } => {
                if self.data::<SelfChatReply>(body).is_some() {
                    self.event(Event::AgentCreated {
                        account,
                        username,
                        token,
                        words,
                    });
                }
            }
            Asked::Agents => {
                if let Some(AgentListReply { agent_list }) = self.data(body) {
                    for agent in &agent_list {
                        if !self.profiles.contains_key(&agent.id) {
                            self.ask_profile(agent.id);
                        }
                    }
                    self.event(Event::Agents { agent_list });
                }
            }
            Asked::AgentChanged => self.get("/api/v1/agents".to_string(), Asked::Agents),
            Asked::AgentDevice { agent, words, end } => {
                if let Some(AgentListReply { agent_list }) = self.data(body) {
                    self.agent_device_known(agent, words, end, agent_list);
                }
            }
            Asked::AgentEnded { agent, end } => match end {
                AgentEnd::Renew => {
                    if let Some(AgentTokenReply { token }) = self.data(body) {
                        self.event(Event::AgentToken {
                            agent,
                            token: Secret(token),
                        });
                    }
                }
                AgentEnd::Revoke | AgentEnd::Disable => {
                    if self.data::<EmptyReply>(body).is_some() {
                        self.get("/api/v1/agents".to_string(), Asked::Agents);
                    }
                }
            },
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
        let mut waiting = Vec::new();
        self.inviting.retain(|(chat, invited)| {
            if *invited == account {
                waiting.push(*chat);
            }
            *invited != account
        });
        for chat in waiting {
            self.invite(chat, account);
        }
        // A sealed profile waits for its account's master key.
        if self
            .profiles
            .get(&account)
            .is_some_and(|state| state.signed.is_some())
        {
            self.open_stored(account);
        }
    }

    fn rest_refused(&mut self, request: Request, status: u16, code: &str, message: &str) {
        let attempt = request.attempt;
        match request.asked {
            Asked::Login {
                credentials,
                with_device: true,
            } if code == "device_ended" => {
                self.device = None;
                self.out.push(Output::Wipe);
                let credentials = match credentials {
                    Credentials::Password {
                        username, password, ..
                    } => Credentials::Password {
                        username,
                        password,
                        captcha: None,
                    },
                    agent => agent,
                };
                self.post_login(credentials, None);
            }
            // Said as the login says it, so asking first shows the same as before.
            Asked::Challenge { .. } if code == "no_such_account" => {
                self.error("bad_login", "That username or password is wrong")
            }
            Asked::Register { .. }
            | Asked::Setup { .. }
            | Asked::Challenge { .. }
            | Asked::Login { .. } => self.error(code, message),
            // A wrong password is 401 here, and the session is still good.
            Asked::Names if code == "bad_password" => self.error(code, message),
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
                self.inviting.retain(|(_, invited)| *invited != account);
                self.error(code, message);
            }
            Asked::CreateChat { account } => {
                self.starting.remove(&account);
                self.error(code, message);
            }
            Asked::CreateFile { upload } | Asked::PutChunk { upload, .. } => {
                self.upload_stopped(upload, code)
            }
            Asked::ChatPhotoFile { chat, starting, .. }
            | Asked::ChatPhotoChunk { chat, starting, .. } => {
                self.chat_photo_failed(chat, starting, code, message)
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
            // That account cannot be added yet, or only by invitation, and the rest still can.
            Asked::AddClaim {
                chat,
                account,
                claim,
            } => {
                if code == "invite_needed" {
                    self.invite(chat, account);
                    self.claim_add(chat, claim);
                } else {
                    if code != "no_key_packages" {
                        self.error(code, message);
                    }
                    self.add_claimed(chat, account, claim, Vec::new());
                }
            }
            Asked::CreateGroup { .. } | Asked::Requests { .. } => self.error(code, message),
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
            // Someone who no longer shares a chat: what was known stays.
            Asked::Profile { account } => {
                self.profile_asked.remove(&account);
                if code != "not_shared" {
                    self.error(code, message);
                }
            }
            Asked::Photo { photo, .. } | Asked::ChatPhotoGet { photo, .. } => {
                self.photo_asked.remove(&photo);
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
        let state = self.chat(chat);
        if state.chat_type.is_some_and(grouped) {
            state.members_known = true;
            if let Some(me) = self.account
                && !member_list.contains(&me)
            {
                member_list.push(me);
            }
            // Devices of accounts the server no longer lists, which left or were taken out: the
            // first member online commits their removal, trusting the server.
            let mut stray = Vec::new();
            if let Some(device) = self.device.as_ref()
                && let Ok(in_group) = device.member_list(chat)
            {
                for member in in_group {
                    if !member_list.contains(&member.account) {
                        stray.push(member.device);
                    }
                }
            }
            let state = self.chat(chat);
            state.member_list = member_list;
            let planned = state.plan_list.iter().any(
                |plan| matches!(plan, Plan::Remove { device_list, .. } if *device_list == stray),
            );
            if !stray.is_empty() && !planned {
                state.plan_list.push(Plan::Remove {
                    device_list: stray,
                    removed: Vec::new(),
                });
            }
        } else if !member_list.is_empty() {
            state.member_list = member_list;
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
            // Profile keys shared at an epoch before this device joined open only now, and so does
            // the chat's own profile key.
            self.profile_pass(chat);
            if let Some(item) = self.chat(chat).profile_waiting.take() {
                self.chat_profile_in(chat, item);
            }
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

/// The account a chat event acts on. A topic's events name the topic there instead, which is no
/// account to ask the server about.
fn account_target(event: &ChatEvent) -> Option<Ulid> {
    match event.action {
        ChatAction::TopicAdded | ChatAction::TopicRemoved => None,
        _ => event.target,
    }
}
