//! Wire types: what WebSocket and REST carry.
//!
//! JSON, the same on both. Every frame has a `type`. A request's `ref` is a ULID the client makes,
//! which the server holds in memory only for that request and sends back in its answer. What is
//! saved goes by its sequence id, so the server never stores a client's ULID. Binary fields are standard base64, ULIDs are text, and kinds are the schema's numbers. Unknown
//! fields are ignored, because a new optional field does not bump the version.

use crate::ulid::Ulid;
use base64ct::{Base64, Encoding};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Bytes on the wire, as base64.
#[derive(Clone, PartialEq, Eq)]
pub struct Bytes(pub Vec<u8>);

/// Never the contents: these may be ciphertext or an unencrypted message, and neither is logged.
impl std::fmt::Debug for Bytes {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "Bytes({} bytes)", self.0.len())
    }
}

/// In TypeScript, the base64 text.
#[cfg(feature = "wasm")]
#[wasm_bindgen::prelude::wasm_bindgen(typescript_custom_section)]
const BYTES_TYPE: &str = "export type Bytes = string;";

impl Serialize for Bytes {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&Base64::encode_string(&self.0))
    }
}

impl<'de> Deserialize<'de> for Bytes {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        match Base64::decode_vec(&text) {
            Ok(bytes) => Ok(Self(bytes)),
            Err(_) => Err(serde::de::Error::custom("this is not valid base64")),
        }
    }
}

/// An enum whose wire form is its schema number, refusing any number it doesn't know.
macro_rules! numbered {
    ($(#[$meta:meta])* $name:ident { $($variant:ident = $number:literal,)+ }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(try_from = "u8", into = "u8")]
        pub enum $name {
            $($variant = $number,)+
        }

        /// In TypeScript, the numbers it may be.
        #[cfg(feature = "wasm")]
        const _: () = {
            #[wasm_bindgen::prelude::wasm_bindgen(typescript_custom_section)]
            const TYPE: &str = concat!("export type ", stringify!($name), " = never", $(" | ", stringify!($number),)+ ";");
        };

        impl From<$name> for u8 {
            fn from(value: $name) -> u8 {
                value as u8
            }
        }

        impl TryFrom<u8> for $name {
            type Error = String;

            fn try_from(number: u8) -> Result<Self, String> {
                match number {
                    $($number => Ok(Self::$variant),)+
                    _ => Err(format!("{number} is not a {}", stringify!($name))),
                }
            }
        }
    };
}

numbered!(
    /// The numbers are the `message.kind` column's.
    Kind {
        Content = 0,
        Edit = 1,
        Delete = 2,
        Commit = 3,
        DeviceNotice = 4,
        Line = 5,
    }
);

numbered!(
    /// The numbers are the `message.content_kind` column's. `Call` is one call, with everything
    /// inside it kept beside it. `MediaBundle` is several images and videos in one message. `Music` is
    /// an MP3, apart from `Voice`, which is a recorded note. `Question` is a tree of questions and
    /// `Answer` one account's answer to it. `Sticker` names one of the server's stickers.
    /// `Suggestion` suggests what to do with its chat, and `Ack` is one account's accepting or
    /// declining it.
    ContentKind {
        Text = 0,
        Voice = 1,
        Reaction = 2,
        Image = 3,
        Music = 4,
        Video = 5,
        Doc = 6,
        File = 7,
        Call = 8,
        MediaBundle = 9,
        Question = 10,
        Answer = 11,
        Sticker = 12,
        Suggestion = 13,
        Ack = 14,
    }
);

numbered!(
    /// The numbers are the `call.outcome` column's, on a call's `end` row only. Readable, so a
    /// client can show a missed call and an answered one differently. A call does not count as unread.
    CallOutcome {
        Missed = 0,
        Answered = 1,
    }
);

numbered!(
    /// The numbers are the `call.kind` column's. The server writes join, leave and end, and
    /// clients send decline and text.
    CallEventKind {
        Join = 0,
        Leave = 1,
        Decline = 2,
        Text = 3,
        End = 4,
    }
);

numbered!(
    /// The numbers are the `conversation.type` column's.
    ChatType {
        SelfChat = 0,
        Direct = 1,
        Group = 2,
        Channel = 3,
    }
);

/// Everything about a message that its device vouches for, apart from its body. The server
/// assigns the sequence id afterwards, so it cannot be here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub chat: Ulid,
    pub device: Ulid,
    pub kind: Kind,
    pub content_kind: Option<ContentKind>,
    pub encrypted: bool,
    pub epoch: Option<u64>,
    pub pointer_seq: Option<u64>,
    pub call_seq: Option<u64>,
    /// The topic a group's or channel's message is in. None for General.
    pub topic: Option<Ulid>,
    /// The main-list message whose thread this is in.
    pub thread_seq: Option<u64>,
    pub client_sent_at: i64,
}

impl Header {
    /// The fixed binary layout: AES's associated data, and with the body after it, what the
    /// device signs. **Permanent:** stored signatures depend on it, and `tests/protocol.rs`
    /// pins the exact bytes. A leading byte says whether an optional field is present, so two
    /// different headers can never give the same bytes.
    pub fn bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(128);
        bytes.extend_from_slice(b"keryx v1 message");
        bytes.extend_from_slice(self.chat.bytes());
        bytes.extend_from_slice(self.device.bytes());
        bytes.push(self.kind as u8);
        match self.content_kind {
            Some(content_kind) => bytes.extend_from_slice(&[1, content_kind as u8]),
            None => bytes.extend_from_slice(&[0, 0]),
        }
        bytes.push(u8::from(self.encrypted));
        for field in [self.epoch, self.pointer_seq, self.call_seq] {
            match field {
                Some(value) => {
                    bytes.push(1);
                    bytes.extend_from_slice(&value.to_be_bytes());
                }
                None => bytes.extend_from_slice(&[0; 9]),
            }
        }
        match self.topic {
            Some(topic) => {
                bytes.push(1);
                bytes.extend_from_slice(topic.bytes());
            }
            None => bytes.extend_from_slice(&[0; 17]),
        }
        match self.thread_seq {
            Some(value) => {
                bytes.push(1);
                bytes.extend_from_slice(&value.to_be_bytes());
            }
            None => bytes.extend_from_slice(&[0; 9]),
        }
        bytes.extend_from_slice(&self.client_sent_at.to_be_bytes());
        bytes
    }
}

/// One epoch's key, wrapped to one account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WrapItem {
    pub account: Ulid,
    pub wrapped: Bytes,
}

/// An MLS Welcome for one device a commit added.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WelcomeItem {
    pub device: Ulid,
    pub body: Bytes,
}

/// A message or action from a client. The server knows the device from the session. A commit is
/// one of these with kind 3, carrying its wraps and welcomes, so there is one path into the task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendFrame {
    #[serde(rename = "ref")]
    pub reference: Ulid,
    pub chat: Ulid,
    pub kind: Kind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_kind: Option<ContentKind>,
    pub encrypted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epoch: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pointer_seq: Option<u64>,
    /// A content message's topic in a group or channel, signed in its header. None for General.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<Ulid>,
    /// A content message's thread: the main-list message it answers under, signed in its header.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_seq: Option<u64>,
    pub client_sent_at: i64,
    pub body: Bytes,
    pub signature: Bytes,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wrap_list: Vec<WrapItem>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub welcome_list: Vec<WelcomeItem>,
    /// A commit's: the chat's group info at the epoch it moves to, which the server keeps with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_info: Option<Bytes>,
    /// A content message's files, which the server keeps from now on. See the root AGENTS.md.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub persist: Vec<Ulid>,
    /// An edit's or delete's files, which the server removes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remove: Vec<Ulid>,
    /// A commit's: every member's profile key it knows, sealed at the epoch it moves to.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub profile_key_list: Vec<ProfileKeyShare>,
    /// A group's or channel's commit that changes who is in it or their role, which the server
    /// checks and applies in the same step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<CommitAction>,
    /// An add commit's, or the commit of a device joining by invitation: the group's or channel's
    /// profile key sealed at the epoch it moves to, which everyone it leaves in the chat holds.
    /// Saved in the same step, replacing the one kept, and pushed after the commit with the profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_key: Option<Bytes>,
}

/// What a commit does to a group's or channel's people, readable so the server checks it.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitAction {
    pub kind: ActionKind,
    pub account_list: Vec<Ulid>,
}

#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    /// New accounts, by someone who may add, or let in by request.
    Add,
    /// Accounts taken out by someone who may remove them. Finishing in MLS what a leave or an ended
    /// device already did carries no action.
    Remove,
}

/// A group's or channel's built-in roles.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatRole {
    Owner,
    Moderator,
    Member,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientFrame {
    Send(Box<SendFrame>),
    Seen {
        #[serde(rename = "ref")]
        reference: Ulid,
        chat: Ulid,
        up_to_seq: u64,
        client_seen_at: i64,
        /// Where it was read: a topic, a thread, or neither for the main list.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        topic: Option<Ulid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread_seq: Option<u64>,
    },
    /// "Received up to N": this device now holds those messages. Kept like `seen`, and answered
    /// with `done`.
    Received {
        #[serde(rename = "ref")]
        reference: Ulid,
        chat: Ulid,
        up_to_seq: u64,
        client_received_at: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        topic: Option<Ulid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread_seq: Option<u64>,
    },
    /// Keeps this account's draft in a chat, or clears it with none. Answered with `done`, and the
    /// account's other online devices get it as `draft`.
    Draft {
        #[serde(rename = "ref")]
        reference: Ulid,
        chat: Ulid,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        draft: Option<DraftBody>,
    },
    /// Asks for a new folder's id, so its name can be sealed with it. Answered with `folder_id`.
    FolderId {
        #[serde(rename = "ref")]
        reference: Ulid,
    },
    /// This account's whole arrangement of folders at once. Answered with `done`, and the account's
    /// other online devices get it as `chat_folders`.
    ChatFolders {
        #[serde(rename = "ref")]
        reference: Ulid,
        #[serde(flatten)]
        folders: ChatFolders,
    },
    /// When each other member's device received and saw one message, answered with `times` in
    /// pages: `after` is the last account of the page before.
    Times {
        #[serde(rename = "ref")]
        reference: Ulid,
        chat: Ulid,
        seq: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after: Option<Ulid>,
    },
    /// Not saved and not answered, so it has no `ref` and no sequence id. In a channel it goes out
    /// only inside a thread.
    Typing {
        chat: Ulid,
        on: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        topic: Option<Ulid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread_seq: Option<u64>,
    },
    /// The chats list, and the messages and call rows saved after a time, in pages. The server
    /// works out which chats, and the client never sends a list. `since` is the server time of the
    /// last row the client saved, and the server starts 1 second before it, or from the beginning
    /// without one. `after` continues a page that said `more`, exactly after its last row, with no
    /// step back. `chat` asks for that one chat only.
    Sync {
        #[serde(rename = "ref")]
        reference: Ulid,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        since: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after: Option<SyncPosition>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        chat: Option<Ulid>,
    },
    GroupInfo {
        #[serde(rename = "ref")]
        reference: Ulid,
        chat: Ulid,
        epoch: u64,
        body: Bytes,
        /// A new group's or channel's first profile, sealed under its profile key, saved in the
        /// same step.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        chat_profile: Option<Box<ChatProfileBody>>,
        /// That profile key, sealed at epoch 0, which goes with the first profile.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        chat_key: Option<Bytes>,
        /// A new group's or channel's first options, saved in the same step.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        chat_options: Option<Box<ChatOptions>>,
    },
    /// Asks whether the server is alive, for a browser, which cannot send a WebSocket ping. Answered
    /// with `done`.
    Ping {
        #[serde(rename = "ref")]
        reference: Ulid,
    },
    /// The right wraps, from a device that found stored ones wrong or missing.
    Wraps {
        #[serde(rename = "ref")]
        reference: Ulid,
        chat: Ulid,
        epoch: u64,
        wrap_list: Vec<WrapItem>,
    },
    /// Profile keys missing from a chat, such as this account's own, sealed at `epoch`. Answered with
    /// `done`. The server keeps the newest per chat and account.
    ProfileKeys {
        #[serde(rename = "ref")]
        reference: Ulid,
        chat: Ulid,
        epoch: u64,
        profile_key_list: Vec<ProfileKeyShare>,
    },
    /// Declining a call, from a device not in its MoQ. Saved inside the call and answered with
    /// `done`. Text in a call goes up on MoQ instead.
    CallEvent {
        #[serde(rename = "ref")]
        reference: Ulid,
        chat: Ulid,
        call_seq: u64,
        kind: CallEventKind,
    },
    /// This device's presence, XMPP's way: a connection counts for nothing to others until its
    /// first one. Sent on connecting and on every app open and close. An event, like `typing`, so
    /// it has no `ref` and no answer. The first on a connection is followed by one `presence` per
    /// person sharing a chat.
    Presence {
        in_app: bool,
    },
    /// Someone's presence, asked once, for a person sharing no chat. Answered with `presence_of`.
    PresenceOf {
        #[serde(rename = "ref")]
        reference: Ulid,
        account: Ulid,
    },
    /// This account's settings. Answered with `kv`: the version alone when it is `version`, the
    /// whole set with it otherwise.
    Kv {
        #[serde(rename = "ref")]
        reference: Ulid,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<u64>,
    },
    /// A change to this account's settings, based on `version`. Answered with `kv` and the new
    /// version, or refused with `kv_stale` and the server's whole set.
    KvSet {
        #[serde(rename = "ref")]
        reference: Ulid,
        #[serde(flatten)]
        change: KvChange,
    },
    /// An owner removes a whole group or channel, for everyone. Answered with `done`.
    ChatRemove {
        #[serde(rename = "ref")]
        reference: Ulid,
        chat: Ulid,
    },
    /// Leaves a group or channel, at once on the server, and the next member online commits the
    /// removal. Answered with `done`.
    Leave {
        #[serde(rename = "ref")]
        reference: Ulid,
        chat: Ulid,
    },
    /// Makes a member of a group or channel an owner, a moderator or a plain member, on the server
    /// alone, which clients trust for roles. Answered with `done`.
    ChatRole {
        #[serde(rename = "ref")]
        reference: Ulid,
        chat: Ulid,
        account: Ulid,
        role: ChatRole,
    },
    /// Asks to join a findable group or channel, with a note the server can read. Answered with
    /// `done`.
    JoinRequest {
        #[serde(rename = "ref")]
        reference: Ulid,
        chat: Ulid,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    /// Takes back this account's request. Answered with `done`.
    JoinCancel {
        #[serde(rename = "ref")]
        reference: Ulid,
        chat: Ulid,
    },
    /// Declines someone's request, by one who may add people. Answered with `done`.
    JoinDecline {
        #[serde(rename = "ref")]
        reference: Ulid,
        chat: Ulid,
        account: Ulid,
    },
    /// Invites someone whose switch turns every add into an invitation. `sealed` holds the inviter
    /// and the chat's name and photo, sealed to the invited account, and `signature` is this
    /// device's over `keryx v1 invite`, the chat and that account. Answered with `done`.
    Invite {
        #[serde(rename = "ref")]
        reference: Ulid,
        chat: Ulid,
        account: Ulid,
        sealed: Bytes,
        signature: Bytes,
    },
    /// Accepts or declines this account's invitation. Accepting makes it a member at once, and its
    /// device then joins by external commit. Answered with `done`.
    InviteAnswer {
        #[serde(rename = "ref")]
        reference: Ulid,
        chat: Ulid,
        accept: bool,
    },
    /// Asks for a new topic's id, so its name can be sealed with it. Answered with `topic_id`.
    TopicId {
        #[serde(rename = "ref")]
        reference: Ulid,
        chat: Ulid,
    },
    /// Asks for a group's or channel's members and their roles, by any member. Answered with
    /// `members`.
    Members {
        #[serde(rename = "ref")]
        reference: Ulid,
        chat: Ulid,
    },
    /// Whether topics show, and the live topics, in order. A topic left out is removed with every
    /// message in it. Answered with `done`.
    Topics {
        #[serde(rename = "ref")]
        reference: Ulid,
        chat: Ulid,
        #[serde(flatten)]
        topics: TopicsBody,
    },
    /// A group's or channel's sealed profile, with its topics' names. Answered with `done`.
    ChatProfile {
        #[serde(rename = "ref")]
        reference: Ulid,
        chat: Ulid,
        #[serde(flatten)]
        profile: ChatProfileBody,
    },
    /// What a group's or channel's members may do, by owners. Answered with `done`.
    ChatOptions {
        #[serde(rename = "ref")]
        reference: Ulid,
        chat: Ulid,
        #[serde(flatten)]
        options: ChatOptions,
    },
    /// Whether the directory lists this group or channel, with the name and bio it shows, which
    /// the server reads. Answered with `done`.
    Findable {
        #[serde(rename = "ref")]
        reference: Ulid,
        chat: Ulid,
        #[serde(flatten)]
        findable: FindableBody,
    },
    /// A topic's sealed profile, General's under the all-zero id, by its moderators and the group's
    /// owners and moderators. Answered with `done`.
    TopicProfile {
        #[serde(rename = "ref")]
        reference: Ulid,
        chat: Ulid,
        topic: Ulid,
        #[serde(flatten)]
        profile: Box<ChatProfileBody>,
    },
    /// A topic's own options, by its moderators and the group's owners and moderators. Answered
    /// with `done`.
    TopicOptions {
        #[serde(rename = "ref")]
        reference: Ulid,
        chat: Ulid,
        topic: Ulid,
        #[serde(flatten)]
        options: Box<TopicOptionsBody>,
    },
    /// Makes a member a topic's moderator, or a plain member there again, by the group's owners.
    /// Answered with `done`.
    TopicRole {
        #[serde(rename = "ref")]
        reference: Ulid,
        chat: Ulid,
        topic: Ulid,
        account: Ulid,
        role: ChatRole,
    },
}

/// Where someone is: on a call, in the app, connected with the app in the background, or not there.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresenceState {
    Offline,
    Away,
    Online,
    Call,
}

/// One account's presence, its devices merged: the state from `presence.in_app`, and each time only
/// to whoever holds its permission. The newest of the account's devices.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresenceItem {
    pub account: Ulid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<PresenceState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_app_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connected_at: Option<i64>,
}

/// `PUT /api/v1/devices/{id}/presence`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresenceBody {
    pub in_app: bool,
}

/// `PUT /api/v1/devices/{id}/presence`'s answer: everyone sharing a chat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresenceListReply {
    pub presence_list: Vec<PresenceItem>,
}

/// One setting: its name, and its value as bytes the server never reads.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KvItem {
    pub key: String,
    pub value: Bytes,
}

/// A change to an account's settings, based on `version`. Also the body of `PATCH /api/v1/kv`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KvChange {
    pub version: u64,
    #[serde(default)]
    pub set: Vec<KvItem>,
    #[serde(default)]
    pub remove: Vec<String>,
}

/// `GET /api/v1/kv?version=N`: the version alone when it is `N`, the whole set with it otherwise.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KvReply {
    pub version: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kv_list: Option<Vec<KvItem>>,
}

/// What happened inside a call, numbered from 1 within it: one row of the `call` table. The same JSON
/// on the `events` track, in the WebSocket's `call_event` and in `sync`'s `call_list`. Only text has a
/// body, encrypted and signed like a message, with `call_seq` in its header so it can never pass for a
/// chat message. Only `end` has an `outcome`.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallEvent {
    pub chat: Ulid,
    pub call_seq: u64,
    pub n: u64,
    pub kind: CallEventKind,
    pub device: Ulid,
    pub server_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epoch: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<Bytes>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Bytes>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<CallOutcome>,
}

impl CallEvent {
    /// Text's signed header. `None` for any other kind, or text missing a field it always has.
    pub fn header(&self) -> Option<Header> {
        if self.kind != CallEventKind::Text {
            return None;
        }
        Some(call_text_header(
            self.chat,
            self.device,
            self.call_seq,
            self.epoch?,
            self.client_at?,
        ))
    }
}

/// The header text in a call is signed and sealed under.
pub fn call_text_header(
    chat: Ulid,
    device: Ulid,
    call_seq: u64,
    epoch: u64,
    client_at: i64,
) -> Header {
    Header {
        chat,
        device,
        kind: Kind::Content,
        content_kind: Some(ContentKind::Text),
        encrypted: true,
        epoch: Some(epoch),
        pointer_seq: None,
        call_seq: Some(call_seq),
        topic: None,
        thread_seq: None,
        client_sent_at: client_at,
    }
}

/// Text a device sends on its own `text` track in a call. The relay hands it to the chat's task.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallText {
    pub client_at: i64,
    pub epoch: u64,
    pub signature: Bytes,
    pub body: Bytes,
}

/// A call's start message's body, inside its encryption: what the caller has on.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallBody {
    pub video: bool,
    pub voice: bool,
}

/// The longest a question's or an option's text may be, in characters, fixed by the author.
pub const QUESTION_TEXT: usize = 500;
/// The longest written-in text may be, in characters, fixed by the author.
pub const OWN_TEXT: usize = 2000;
/// The most nodes one question message holds, fixed by the author.
pub const QUESTION_NODES: usize = 32;
/// The longest path through a question message, in nodes, fixed by the author.
pub const QUESTION_PATH: usize = 16;
/// The most options one choice offers, beside the automatic "write your own", fixed by the author.
pub const CHOICE_OPTIONS: usize = 8;

/// The longest a suggestion's text may be, in characters, fixed by the author.
pub const SUGGESTION_TEXT: usize = 500;
/// The longest a folder's name may be, in characters: the clients' limit, which a suggestion is
/// checked against.
pub const FOLDER_NAME: usize = 32;

/// A suggestion's action, inside its encryption, beside its optional `text`: exactly one key, so a
/// later action is a new key and not a new kind. Written as `{"folder": {"name", "move"}}`.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Suggestion {
    /// Put the chat it was sent in into a folder.
    Folder(FolderSuggestion),
    /// An action this client does not know, from a newer keryx. Shown with no buttons.
    Unknown,
}

/// The folder a chat goes in, found by its folded name or made. `move` also takes the chat out of
/// every other folder but `all`.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderSuggestion {
    pub name: String,
    pub r#move: bool,
}

/// One account's accepting or declining a suggestion, inside its encryption. `suggestion` is its
/// sequence id, kept inside like a reply's. Accepted, it says what changed: `created` when the
/// folder was made for it, and `left` how many folders the chat was taken out of. Declined, it
/// holds neither.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ack {
    pub suggestion: u64,
    pub accepted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left: Option<u32>,
}

impl Suggestion {
    /// A suggestion's plaintext, `{"text"?, <action>}`, or `None` when it is not one: no action or
    /// two, or a known action that does not hold together. Its text is the message's own.
    pub fn from_plaintext(plaintext: &[u8]) -> Option<Self> {
        let mut object =
            serde_json::from_slice::<serde_json::Map<String, serde_json::Value>>(plaintext).ok()?;
        if let Some(text) = object.remove("text")
            && !matches!(&text, serde_json::Value::String(text) if !too_long(text, SUGGESTION_TEXT))
        {
            return None;
        }
        // The reply, which any content may carry, is not an action.
        object.remove("reply");
        let mut keys = object.into_iter();
        let (key, value) = keys.next()?;
        if keys.next().is_some() {
            return None;
        }
        match key.as_str() {
            "folder" => {
                let suggestion = Suggestion::Folder(serde_json::from_value(value).ok()?);
                suggestion.check().ok()?;
                Some(suggestion)
            }
            _ => Some(Suggestion::Unknown),
        }
    }

    /// Checks an action against the limits fixed in core: a folder's name 1 to [`FOLDER_NAME`]
    /// characters once trimmed. An unknown action is never sent.
    pub fn check(&self) -> Result<(), FormError> {
        match self {
            Suggestion::Folder(folder) => {
                let name = folder.name.trim();
                if name.is_empty() || too_long(name, FOLDER_NAME) {
                    return Err(FormError::Suggestion);
                }
                Ok(())
            }
            Suggestion::Unknown => Err(FormError::Suggestion),
        }
    }
}

impl Ack {
    /// `created` and `left` are there exactly when it was accepted.
    pub fn check(&self) -> Result<(), FormError> {
        if self.accepted == self.created.is_some() && self.accepted == self.left.is_some() {
            Ok(())
        } else {
            Err(FormError::Ack)
        }
    }
}

/// A question message's tree, inside its encryption, beside its optional intro in `text`. Every
/// question also takes written-in text, and any question can be skipped.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Question {
    pub first: String,
    pub nodes: Vec<QuestionNode>,
}

/// One question: a choice or a number, and where each kind of answer leads.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionNode {
    pub id: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choice: Option<Choice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<NumberRange>,
    /// Tried in order, and the first match wins. No match ends the form.
    #[serde(default)]
    pub next: Vec<NextRule>,
}

#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choice {
    #[serde(default)]
    pub multi: bool,
    pub options: Vec<ChoiceOption>,
}

#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChoiceOption {
    pub id: String,
    pub text: String,
}

/// A number question's range, as decimals. `step` is 1 when unset, and 0.5 allows halves.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NumberRange {
    #[cfg_attr(feature = "wasm", tsify(type = "number"))]
    pub min: serde_json::Number,
    #[cfg_attr(feature = "wasm", tsify(type = "number"))]
    pub max: serde_json::Number,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "wasm", tsify(type = "number | undefined"))]
    pub step: Option<serde_json::Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

/// Where an answer leads. At most one condition is set, and none matches anything. `to` of `None`
/// ends the form.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NextRule {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub picked: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "wasm", tsify(type = "[number, number] | undefined"))]
    pub range: Option<(serde_json::Number, serde_json::Number)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub own: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped: Option<bool>,
    pub to: Option<String>,
}

/// One account's answer, inside its encryption. `question` is the question's sequence id, kept
/// inside like a reply's. `skipped` alone, with no answers, skips the whole form.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answer {
    pub question: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub answers: Vec<AnswerItem>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub skipped: bool,
}

/// The answer to one question on the path actually taken, or its skip.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnswerItem {
    pub node: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub picked: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub own: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "wasm", tsify(type = "number | undefined"))]
    pub number: Option<serde_json::Number>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub skipped: bool,
}

/// Why a question, an answer, a suggestion or an ack does not hold together.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum FormError {
    #[error("A question message holds 1 to {QUESTION_NODES} questions")]
    Nodes,
    #[error("Two questions or two options share an id")]
    Duplicate,
    #[error("A question names a question that does not exist")]
    Unknown,
    #[error("A question is a choice or a number, never both or neither")]
    Shape,
    #[error("A choice has 1 to {CHOICE_OPTIONS} options")]
    Options,
    #[error("A text is too long")]
    TooLong,
    #[error("A number's range is not min <= max with a step above 0")]
    Range,
    #[error("A rule's condition does not fit its question")]
    Rule,
    #[error("The questions loop, or one is never reached")]
    Path,
    #[error("A path is longer than {QUESTION_PATH} questions")]
    Deep,
    #[error("An answer does not follow its question")]
    Answer,
    #[error(
        "A suggestion holds one known action, and a folder's name is 1 to {FOLDER_NAME} characters"
    )]
    Suggestion,
    #[error("An ack says what changed exactly when it accepts")]
    Ack,
}

fn decimal(number: &serde_json::Number) -> f64 {
    number.as_f64().unwrap_or(f64::NAN)
}

fn has_option(choice: &Choice, id: &str) -> bool {
    for option in &choice.options {
        if option.id == id {
            return true;
        }
    }
    false
}

fn too_long(text: &str, limit: usize) -> bool {
    text.chars().count() > limit
}

impl Question {
    fn node(&self, id: &str) -> Option<&QuestionNode> {
        self.nodes.iter().find(|node| node.id == id)
    }

    /// Checks the tree against the limits fixed in core: ids, shapes, ranges, rules, no loop, no
    /// question nothing reaches, and no path longer than [`QUESTION_PATH`].
    pub fn check(&self) -> Result<(), FormError> {
        if self.nodes.is_empty() || self.nodes.len() > QUESTION_NODES {
            return Err(FormError::Nodes);
        }
        let mut ids = std::collections::HashSet::new();
        for node in &self.nodes {
            if !ids.insert(node.id.as_str()) {
                return Err(FormError::Duplicate);
            }
        }
        if !ids.contains(self.first.as_str()) {
            return Err(FormError::Unknown);
        }
        for node in &self.nodes {
            if too_long(&node.text, QUESTION_TEXT) {
                return Err(FormError::TooLong);
            }
            match (&node.choice, &node.number) {
                (Some(choice), None) => {
                    if choice.options.is_empty() || choice.options.len() > CHOICE_OPTIONS {
                        return Err(FormError::Options);
                    }
                    let mut option_ids = std::collections::HashSet::new();
                    for option in &choice.options {
                        if !option_ids.insert(option.id.as_str()) {
                            return Err(FormError::Duplicate);
                        }
                        if too_long(&option.text, QUESTION_TEXT) {
                            return Err(FormError::TooLong);
                        }
                    }
                }
                (None, Some(range)) => {
                    let step = match &range.step {
                        Some(step) => decimal(step),
                        None => 1.0,
                    };
                    let (min, max) = (decimal(&range.min), decimal(&range.max));
                    if !(min <= max && step > 0.0 && step.is_finite()) {
                        return Err(FormError::Range);
                    }
                    if let Some(unit) = &range.unit
                        && too_long(unit, QUESTION_TEXT)
                    {
                        return Err(FormError::TooLong);
                    }
                }
                _ => return Err(FormError::Shape),
            }
            for rule in &node.next {
                let conditions = usize::from(rule.picked.is_some())
                    + usize::from(rule.range.is_some())
                    + usize::from(rule.own.is_some())
                    + usize::from(rule.skipped.is_some());
                if conditions > 1 {
                    return Err(FormError::Rule);
                }
                if let Some(picked) = &rule.picked {
                    let Some(choice) = &node.choice else {
                        return Err(FormError::Rule);
                    };
                    for id in picked {
                        if !has_option(choice, id) {
                            return Err(FormError::Rule);
                        }
                    }
                }
                if let Some((low, high)) = &rule.range
                    && (node.number.is_none() || decimal(low) > decimal(high))
                {
                    return Err(FormError::Rule);
                }
                if let Some(to) = &rule.to
                    && !ids.contains(to.as_str())
                {
                    return Err(FormError::Unknown);
                }
            }
        }
        // Depth-first from the start: a node met again on the current path is a loop, and the
        // longest path is measured on the way.
        let mut reached = std::collections::HashSet::new();
        let mut longest = std::collections::HashMap::new();
        if self.walk(&self.first, &mut Vec::new(), &mut reached, &mut longest)? > QUESTION_PATH {
            return Err(FormError::Deep);
        }
        if reached.len() != self.nodes.len() {
            return Err(FormError::Path);
        }
        Ok(())
    }

    /// The longest path from `id` in nodes, or a loop.
    fn walk<'a>(
        &'a self,
        id: &'a str,
        path: &mut Vec<&'a str>,
        reached: &mut std::collections::HashSet<&'a str>,
        longest: &mut std::collections::HashMap<&'a str, usize>,
    ) -> Result<usize, FormError> {
        if path.contains(&id) {
            return Err(FormError::Path);
        }
        if let Some(length) = longest.get(id) {
            return Ok(*length);
        }
        let Some(node) = self.node(id) else {
            return Err(FormError::Unknown);
        };
        reached.insert(id);
        path.push(id);
        let mut deepest = 0;
        for rule in &node.next {
            if let Some(to) = &rule.to {
                deepest = deepest.max(self.walk(to, path, reached, longest)?);
            }
        }
        path.pop();
        longest.insert(id, deepest + 1);
        Ok(deepest + 1)
    }

    /// Where an answer to `node` leads: the first rule it matches, or the end.
    pub fn next_of(&self, node: &QuestionNode, item: &AnswerItem) -> Option<String> {
        for rule in &node.next {
            let matched = if let Some(picked) = &rule.picked {
                let mut any = false;
                for id in picked {
                    if item.picked.contains(id) {
                        any = true;
                    }
                }
                !item.skipped && any
            } else if let Some((low, high)) = &rule.range {
                match &item.number {
                    Some(number) if !item.skipped => {
                        let number = decimal(number);
                        decimal(low) <= number && number <= decimal(high)
                    }
                    _ => false,
                }
            } else if rule.own.is_some() {
                !item.skipped && item.own.is_some()
            } else if rule.skipped.is_some() {
                item.skipped
            } else {
                true
            };
            if matched {
                return rule.to.clone();
            }
        }
        None
    }

    /// Checks an answer follows this question: each item answers the question the path reached, in
    /// order, within its options, range and limits, and the path ends with the last one.
    pub fn check_answer(&self, answer: &Answer) -> Result<(), FormError> {
        if answer.skipped {
            if answer.answers.is_empty() {
                return Ok(());
            }
            return Err(FormError::Answer);
        }
        let mut at = Some(self.first.clone());
        for item in &answer.answers {
            let Some(id) = at else {
                return Err(FormError::Answer);
            };
            let Some(node) = self.node(&id) else {
                return Err(FormError::Answer);
            };
            if item.node != node.id {
                return Err(FormError::Answer);
            }
            if let Some(own) = &item.own
                && too_long(own, OWN_TEXT)
            {
                return Err(FormError::TooLong);
            }
            if item.skipped {
                if !item.picked.is_empty() || item.own.is_some() || item.number.is_some() {
                    return Err(FormError::Answer);
                }
            } else {
                match (&node.choice, &node.number) {
                    (Some(choice), _) => {
                        if item.number.is_some()
                            || (item.picked.is_empty() && item.own.is_none())
                            || (!choice.multi && item.picked.len() > 1)
                        {
                            return Err(FormError::Answer);
                        }
                        for id in &item.picked {
                            if !has_option(choice, id) {
                                return Err(FormError::Answer);
                            }
                        }
                    }
                    (None, Some(range)) => {
                        if !item.picked.is_empty() {
                            return Err(FormError::Answer);
                        }
                        if let Some(number) = &item.number {
                            let number = decimal(number);
                            let min = decimal(&range.min);
                            let step = match &range.step {
                                Some(step) => decimal(step),
                                None => 1.0,
                            };
                            let steps = (number - min) / step;
                            if number < min
                                || number > decimal(&range.max)
                                || (steps - steps.round()).abs() > 1e-9
                            {
                                return Err(FormError::Answer);
                            }
                        } else if item.own.is_none() {
                            return Err(FormError::Answer);
                        }
                    }
                    (None, None) => return Err(FormError::Answer),
                }
            }
            at = self.next_of(node, item);
        }
        match at {
            None => Ok(()),
            Some(_) => Err(FormError::Answer),
        }
    }
}

/// One sticker the server keeps: a WebP of at most 512x512, in a pack.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StickerItem {
    pub id: Ulid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emoji: Option<String>,
    pub width: u32,
    pub height: u32,
    pub size: u64,
}

#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StickerPack {
    pub id: Ulid,
    pub name: String,
    pub sticker_list: Vec<StickerItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StickerListReply {
    pub pack_list: Vec<StickerPack>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreatePackBody {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackReply {
    pub pack: Ulid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StickerReply {
    pub sticker: StickerItem,
}

/// One account's profile key, sealed under a chat's key at the epoch the commit or frame names. See
/// the root AGENTS.md.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileKeyShare {
    pub account: Ulid,
    pub sealed: Bytes,
}

/// One member of a chat, in its `chats` entry: their profile's version, which moves on every change
/// of their names, photo, bio or links, and their profile key sealed at `epoch` once someone shared it.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileKeyItem {
    pub account: Ulid,
    pub version: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epoch: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sealed: Option<Bytes>,
}

/// An account's sealed photo, bio and links, signed by the device that saved them, with that device's
/// key and the master key's stamp on it, so a client checks it against the pinned master key alone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedProfile {
    pub sealed: Bytes,
    pub signature: Bytes,
    pub device: Ulid,
    pub device_public: Bytes,
    pub stamp: Bytes,
}

/// `GET /api/v1/accounts/{id}/profile`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileReply {
    pub account: Ulid,
    pub username: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub version: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<SignedProfile>,
    /// The photo the server holds, which the sealed profile names with its key. Gone when admin
    /// removed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub photo: Option<Ulid>,
}

/// `PUT /api/v1/accounts/{id}/profile`: the sealed profile and its signature, or none to clear it,
/// and the photo it names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PutProfileBody {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sealed: Option<Bytes>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<Bytes>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub photo: Option<Ulid>,
}

/// `PUT /api/v1/accounts/{id}/names`. Changing your own username needs your password; admin changing
/// someone else's needs none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamesBody {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionReply {
    pub version: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhotoReply {
    pub photo: Ulid,
}

/// What an account's sealed profile holds. Only its own client writes it and only clients sharing a
/// chat open it, so the server never reads it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileBody {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bio: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub link_list: Vec<ProfileLink>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub photo: Option<ProfilePhoto>,
}

/// One of a profile's links, opened in a browser.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileLink {
    pub name: String,
    pub url: String,
}

/// The photo a sealed profile names: its id on the server, the key its one chunk is sealed under,
/// its JPEG's size and its BlurHash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfilePhoto {
    pub id: Ulid,
    pub key: Bytes,
    pub size: u64,
    pub blurhash: String,
}

/// A stored message. **A removed one has only `chat`, `seq` and `server_saved_at`**, the time it was
/// removed, so `sync` hands it out and every device drops its copy, and a client knows something was
/// there, not what. A call's message holds nothing about the call: that is in its `call` rows.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageFrame {
    pub chat: Ulid,
    pub seq: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<Ulid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<Kind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_kind: Option<ContentKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encrypted: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epoch: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pointer_seq: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<Ulid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_seq: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_sent_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_saved_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Bytes>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<Bytes>,
}

impl MessageFrame {
    /// The signed header, to check this message against. `None` for a removed message, or one
    /// missing a field a stored message always has.
    pub fn header(&self) -> Option<Header> {
        Some(Header {
            chat: self.chat,
            device: self.device?,
            kind: self.kind?,
            content_kind: self.content_kind,
            encrypted: self.encrypted?,
            epoch: self.epoch,
            pointer_seq: self.pointer_seq,
            call_seq: None,
            topic: self.topic,
            thread_seq: self.thread_seq,
            client_sent_at: self.client_sent_at?,
        })
    }
}

/// One of a person's chats and its last state, for the list a client shows first.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatItem {
    pub chat: Ulid,
    #[serde(rename = "type")]
    pub chat_type: ChatType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seq: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_kind: Option<Kind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_content_seq: Option<u64>,
    /// The epoch of the last accepted commit, so a device back from being away knows whether it is
    /// behind. None until the group is started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epoch: Option<u64>,
    /// The newest content message, still as stored, so one request gives every preview.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_content: Option<Box<MessageFrame>>,
    /// The other members' positions, so a reload draws the marks at once. Empty in a channel.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mark_list: Vec<MarkItem>,
    /// This member's seen position. After a reload, updating fetches from here. The unread count
    /// is the content messages opened after it, and a reaction does not count. The server does
    /// not send the count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seen_seq: Option<u64>,
    /// This account's draft in the chat, still sealed, so a reload and a new device show it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<DraftItem>,
    /// Every member's profile version and sealed profile key, this account's too.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub profile_key_list: Vec<ProfileKeyItem>,
    /// A group's or channel's: this account's role there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<ChatRole>,
    /// Whether its messages show in topics.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub topics_on: bool,
    /// Its live topics, in order. General is not listed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub topic_list: Vec<Ulid>,
    /// Its topics' own settings, moderators and profiles, General's too under the all-zero id. A
    /// hidden topic is left out for those who may not see it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub topic_setting_list: Vec<TopicItem>,
    /// This account's seen positions in topics and threads it has read in. The main list's is
    /// `seen_seq`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub place_seen_list: Vec<PlaceSeen>,
    /// Its sealed profile, with its topics' names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<ChatProfileItem>,
    /// What its members may do, when it differs from the built-in member role.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<ChatOptions>,
    /// The name and bio the directory shows, while it is findable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub findable: Option<FindableItem>,
    /// How many accounts are in it, for a channel's subscribers, who do not see the list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member_count: Option<u64>,
    /// How many join requests wait, set only for an account that may let people in, 0 included, so
    /// a device back from being away knows whether any wait.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_count: Option<u64>,
}

/// One topic's or thread's seen position for this account.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaceSeen {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<Ulid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_seq: Option<u64>,
    pub seen_seq: u64,
}

numbered!(
    /// The numbers are the `chat_event.action` column's. `ChatRemoved` is sent live only, since the
    /// chat's rows go with it, and a device back later finds the chat missing from its list.
    ChatAction {
        Created = 0,
        Added = 1,
        Removed = 2,
        Left = 3,
        LetIn = 4,
        Joined = 5,
        MadeOwner = 6,
        MadeModerator = 7,
        MadeMember = 8,
        Deleted = 9,
        TopicAdded = 10,
        TopicRemoved = 11,
        TopicsOn = 12,
        TopicsOff = 13,
        InvitationDeclined = 14,
        ChatRemoved = 15,
        TopicEdited = 16,
        TopicOptions = 17,
        MadeTopicModerator = 18,
        MadeTopicMember = 19,
        Edited = 20,
    }
);

/// What a membership line says happened, as a word in its readable body.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineAction {
    Created,
    Added,
    LetIn,
    Joined,
    Removed,
    Left,
    MadeOwner,
    MadeModerator,
    MadeMember,
}

/// A membership line's body, readable JSON the server writes. The actor is its device's account,
/// and `account_list` the people acted on, empty when the actor is the one it is about.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineBody {
    pub action: LineAction,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub account_list: Vec<Ulid>,
}

numbered!(
    /// Who sees a chat event: the numbers are the `chat_event.audience` column's, except
    /// `Everyone`, which is only ever sent live and never saved: a whole chat removed, and each
    /// membership change, so every device keeps its member list current.
    Audience {
        Everyone = 0,
        Moderators = 1,
        One = 2,
    }
);

/// What the server wrote happened in a group or channel: a membership line every member sees, or
/// history for its owners and moderators, or a line for one account.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatEvent {
    pub chat: Ulid,
    pub n: u64,
    pub actor: Ulid,
    pub action: ChatAction,
    /// The account acted on, or the topic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<Ulid>,
    /// The message a delete removed or an edit changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
    /// The topic a topic moderator's row is about, its account being the target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<Ulid>,
    pub audience: Audience,
    pub server_at: i64,
    /// A delete's or edit's copy of the message and its edits as they were, ciphertext, header and
    /// signature, kept for the history's readers to open.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub copy_list: Vec<MessageFrame>,
}

/// A commit's authenticated data, as JSON. Only an external join by an account not yet in the chat
/// carries anything: the invitation that lets it in.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invite: Option<InviteStatement>,
}

/// An invitation's signature, `keryx v1 invite`, the chat and the invited account, by a device in the
/// chat. Members accept an outsider's external join only with one from a device already in the group.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InviteStatement {
    /// The invited account.
    pub account: Ulid,
    /// The inviting device.
    pub device: Ulid,
    pub signature: Bytes,
}

/// A join request, as its asker and those who may let it in see it.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestItem {
    pub chat: Ulid,
    pub account: Ulid,
    /// The chat's findable name, for the asker's row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub state: RequestState,
    pub at: i64,
}

#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestState {
    Waiting,
    Declined,
    /// Taken back by its asker, or expired.
    Gone,
    /// Let in: the chat follows.
    LetIn,
}

/// An invitation's sealed part, HPKE-sealed to the invited account's invite key, so the server sees
/// none of it: the group's name and its photo's BlurHash, shown before joining, and the chat's
/// profile key, which opens its whole profile once joined. The photo itself shows once they join.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvitationPlain {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blurhash: Option<String>,
    pub key: Bytes,
    /// The inviter's note, at most 500 characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// An invitation as core shows it, its sealed part opened. `name` is `None` when it does not open.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvitationShown {
    #[serde(rename = "type")]
    pub chat_type: ChatType,
    pub inviter: Ulid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blurhash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub at: i64,
}

/// An invitation, as the invited account sees it.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvitationItem {
    pub chat: Ulid,
    #[serde(rename = "type")]
    pub chat_type: ChatType,
    pub inviter: Ulid,
    pub device: Ulid,
    pub sealed: Bytes,
    pub signature: Bytes,
    pub at: i64,
}

/// Whether topics show, and the live topics in order. Also the body of `PUT
/// /api/v1/chats/{id}/topics`.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopicsBody {
    pub on: bool,
    pub topic_list: Vec<Ulid>,
}

/// A group's or channel's sealed profile, as a device sends it. Also the body of `PUT
/// /api/v1/chats/{id}/profile`. `photo` is the chat file it names, kept from now on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatProfileBody {
    pub sealed: Bytes,
    pub signature: Bytes,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub photo: Option<Ulid>,
}

/// What a group's or channel's sealed profile holds, as JSON: its name, bio, links and photo, and its
/// topics' names. Sealed under the chat's own profile key, which never changes, so anyone given the
/// key reads it, and every later change.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatProfilePlain {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bio: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub link_list: Vec<ProfileLink>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub photo: Option<ProfilePhoto>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub topic_list: Vec<TopicName>,
    /// At most 2,000 characters, counted as Unicode characters, checked by core.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules: Option<String>,
}

/// One topic's name, inside the chat's sealed profile. `place` is its order among the topics.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopicName {
    pub id: Ulid,
    pub name: String,
    #[serde(default)]
    pub place: u64,
}

/// What a group's or channel's text message holds besides its text: the accounts it mentions, which
/// the server never sees.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mentions {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mentions: Vec<Ulid>,
}

/// A group's or channel's sealed profile, as the server keeps it, with the profile key it is sealed
/// under, itself sealed at `key_epoch`: the newest epoch the key was shared at, which everyone in the
/// chat holds.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatProfileItem {
    pub sealed: Bytes,
    pub signature: Bytes,
    pub device: Ulid,
    pub version: u64,
    pub key_epoch: u64,
    pub key: Bytes,
}

/// What a chat's members may do beyond or short of the built-in member role: permission names
/// without their prefix, and slow mode in seconds. Also the body of `PUT
/// /api/v1/chats/{id}/options`.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatOptions {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slow_mode: Option<u64>,
}

/// Who may write in a topic's main list. Members still react and reply in threads either way.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TopicWrite {
    #[default]
    Everyone,
    Moderators,
}

impl TopicWrite {
    pub fn is_everyone(&self) -> bool {
        *self == TopicWrite::Everyone
    }
}

/// A topic's own options, starting from the group's: permission names without their prefix, from
/// the reaction, sticker and file kinds and the four times permissions, slow mode in seconds (none
/// follows the group's, 0 is off), who may write, and whether it is closed or hidden. Also the body of `PUT
/// /api/v1/chats/{id}/topics/{topic}/options`.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopicOptionsBody {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slow_mode: Option<u64>,
    #[serde(default, skip_serializing_if = "TopicWrite::is_everyone")]
    pub write: TopicWrite,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub closed: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
}

/// A topic's sealed profile, as the server keeps it. Sealed under the group's own profile key.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopicProfileItem {
    pub sealed: Bytes,
    pub signature: Bytes,
    pub device: Ulid,
    pub version: u64,
}

/// What a topic's sealed profile holds, as JSON: its bio, links, photo and rules. Its name stays in
/// the group's topic list.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopicProfilePlain {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bio: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub link_list: Vec<ProfileLink>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub photo: Option<ProfilePhoto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules: Option<String>,
}

/// One topic's own settings, moderators and sealed profile, General's under the all-zero id.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopicItem {
    pub topic: Ulid,
    #[serde(flatten)]
    pub options: TopicOptionsBody,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mod_list: Vec<Ulid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<TopicProfileItem>,
}

/// General's id where a topic's own settings are kept: all zeros. Its messages carry no topic.
pub const GENERAL_TOPIC: Ulid = Ulid::ZERO;

/// Whether a group or channel is findable, with the name and bio the directory shows. Also the
/// body of `PUT /api/v1/chats/{id}/findable`.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindableBody {
    pub findable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bio: Option<String>,
}

/// A findable group's or channel's readable name and bio.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindableItem {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bio: Option<String>,
}

/// A findable group or channel, as the directory lists it.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FoundChat {
    pub chat: Ulid,
    #[serde(rename = "type")]
    pub chat_type: ChatType,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bio: Option<String>,
    pub member_count: u64,
}

/// One folder of an account. `all` is the one with no name and no `chat_list`, and holds every chat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderItem {
    pub id: Ulid,
    /// Sealed under the account's profile key, with the folder's id in its associated data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<Bytes>,
    /// Its chats, in no order a client has to keep.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_list: Option<Vec<Ulid>>,
    /// Its pinned chats, first on top.
    pub pin_list: Vec<Ulid>,
}

/// An account's whole arrangement: its folders in tab order, and the one a device opens on. Also the
/// body of `PUT /api/v1/chat_folders`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatFolders {
    pub folder_list: Vec<FolderItem>,
    pub default: Ulid,
}

/// `POST /api/v1/chat_folders/id`: a new folder's id, kept for this account for 5 minutes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderReply {
    pub folder: Ulid,
}

/// An account's unsent text in one chat, sealed under the chat's key at `epoch`. The server holds
/// the blob and never reads it. `saved_at` is the server's time, so a device keeps the newer one.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DraftItem {
    pub epoch: u64,
    pub body: Bytes,
    #[serde(default)]
    pub saved_at: i64,
}

/// A draft as a device sends it: sealed under the chat's key at `epoch`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DraftBody {
    pub epoch: u64,
    pub body: Bytes,
}

/// `PUT /api/v1/chats/{id}/draft`: the draft to keep, or none to clear it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DraftRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<DraftBody>,
}

/// When one member's devices first received and first saw a message, by the server's clock. Each is
/// missing until it has happened, or when the asker may not see it.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimesItem {
    pub account: Ulid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub received_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seen_at: Option<i64>,
}

/// `GET /api/v1/chats/{id}/messages/{seq}/times`: the same page `times` answers with, of every
/// member but the message's sender.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimesReply {
    pub chat: Ulid,
    pub seq: u64,
    pub member_list: Vec<TimesItem>,
    /// Another page follows: ask again with `after` at this one's last account.
    pub more: bool,
}

/// How far one member's devices have received and seen a chat.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarkItem {
    pub account: Ulid,
    pub received_seq: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seen_seq: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerFrame {
    /// The first frame on every connection: what this server does, under `server.toml`'s names. The
    /// ping settings are in seconds.
    Features {
        ping_after: u64,
        ping_every: u64,
        ping_timeout: u64,
    },
    /// Stored, under this sequence id.
    Sent {
        #[serde(rename = "ref")]
        reference: Ulid,
        seq: u64,
        server_saved_at: i64,
    },
    /// Done, for what takes no sequence id.
    Done {
        #[serde(rename = "ref")]
        reference: Ulid,
    },
    /// `code` is for programs, and `message` for people.
    Refused {
        #[serde(rename = "ref")]
        reference: Ulid,
        code: String,
        message: String,
        /// Only for `call_open`: the open call's sequence id, so the client joins that one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        call_seq: Option<u64>,
        /// Only for `kv_stale`: the server's version of the settings, and its whole set.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kv_list: Option<Vec<KvItem>>,
        /// Only for `slow_mode`: how many seconds until this member may write there again.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        seconds: Option<u64>,
        /// Only for `invite_needed`: the accounts added by invitation only, whom the client invites.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        account_list: Option<Vec<Ulid>>,
    },
    Message(Box<MessageFrame>),
    /// A `times` request's answer: who got and saw the message, and when, one page of them.
    Times {
        #[serde(rename = "ref")]
        reference: Ulid,
        chat: Ulid,
        seq: u64,
        member_list: Vec<TimesItem>,
        more: bool,
    },
    /// A `sync`'s answer: the chats list on its first page only, then up to `[limits] history_page`
    /// messages and call rows, ordered by server time, chat, sequence id and `n`, where a message's
    /// `n` is 0. `more` says another page follows: ask again with `after` at this one's last row.
    Chats {
        #[serde(rename = "ref")]
        reference: Ulid,
        chat_list: Vec<ChatItem>,
        message_list: Vec<MessageFrame>,
        call_list: Vec<CallEvent>,
        /// Groups' and channels' events this account may see, ordered with the rest, at sequence
        /// id 0 and their own `n`.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        chat_event_list: Vec<ChatEvent>,
        more: bool,
        /// This account's folders, on the first page only.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        chat_folders: Option<ChatFolders>,
        /// This account's own join requests, waiting or declined, on the first page only.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        asked_list: Vec<RequestItem>,
        /// This account's invitations, on the first page only.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        invitation_list: Vec<InvitationItem>,
    },
    Welcome {
        chat: Ulid,
        body: Bytes,
    },
    Typing {
        chat: Ulid,
        account: Ulid,
        on: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        topic: Option<Ulid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread_seq: Option<u64>,
    },
    /// This account's draft changed on another of its devices. None means it was cleared.
    Draft {
        chat: Ulid,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        draft: Option<DraftItem>,
    },
    /// A new folder's id, answering `folder_id`.
    FolderId {
        #[serde(rename = "ref")]
        reference: Ulid,
        folder: Ulid,
    },
    /// This account's folders changed on another of its devices: the whole arrangement.
    ChatFolders(Box<ChatFolders>),
    /// A member's received or seen position rose, sent to the other members' online devices.
    Marks {
        chat: Ulid,
        #[serde(flatten)]
        mark: MarkItem,
    },
    /// Something happened inside a call, sent to the chat's online devices.
    CallEvent(Box<CallEvent>),
    /// Something happened in a group or channel, sent to the online devices of those it is for.
    ChatEvent(Box<ChatEvent>),
    /// A new topic's id, answering `topic_id`.
    TopicId {
        #[serde(rename = "ref")]
        reference: Ulid,
        chat: Ulid,
        topic: Ulid,
    },
    /// A group's or channel's members and their roles, answering `members`.
    Members {
        #[serde(rename = "ref")]
        reference: Ulid,
        chat: Ulid,
        member_list: Vec<MemberItem>,
    },
    /// A group's or channel's topics changed.
    Topics {
        chat: Ulid,
        #[serde(flatten)]
        topics: TopicsBody,
    },
    /// A group's or channel's sealed profile changed.
    ChatProfile {
        chat: Ulid,
        profile: ChatProfileItem,
    },
    /// What a group's or channel's members may do changed.
    ChatOptions {
        chat: Ulid,
        #[serde(flatten)]
        options: ChatOptions,
    },
    /// Whether the directory lists a group or channel changed, with the name and bio it shows.
    Findable {
        chat: Ulid,
        #[serde(flatten)]
        findable: FindableBody,
    },
    /// A topic's settings, moderators or profile changed: the whole item, to those who may see it.
    Topic {
        chat: Ulid,
        #[serde(flatten)]
        item: Box<TopicItem>,
    },
    /// A join request changed: to the asker's devices, its own, and to those who may let it in.
    JoinRequest(Box<RequestItem>),
    /// An invitation arrived, or with none, went: answered on another device, or expired.
    Invitation {
        chat: Ulid,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        invitation: Option<InvitationItem>,
    },
    /// A device of someone sharing a chat ended. `statement` is its signed revocation or logout,
    /// which a client verifies before removing the device from each chat.
    Revoked {
        account: Ulid,
        device: Ulid,
        statement: Bytes,
    },
    /// Someone sharing a chat, or this account, changed their names, photo, bio or links. A client
    /// fetches the profile when `version` moved.
    Profile {
        account: Ulid,
        version: u64,
        username: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        display_name: Option<String>,
    },
    /// Profile keys just shared in a chat, by a `profile_keys` frame or a commit, as its `chats`
    /// entry carries them. A device that was offline gets them in its next `chats` list instead.
    ProfileKeys {
        chat: Ulid,
        profile_key_list: Vec<ProfileKeyItem>,
    },
    /// Someone sharing a chat: after this device's first `presence`, once per person, and again on
    /// each change.
    Presence(Box<PresenceItem>),
    /// A `presence_of`'s answer.
    PresenceOf {
        #[serde(rename = "ref")]
        reference: Ulid,
        presence: PresenceItem,
    },
    /// This account's settings. Answering `kv`: `version`, and `kv_list` when it differed. Answering
    /// `kv_set`: the new `version`. Pushed to the account's other devices after a change, with no
    /// `ref`: the change from `from` to `version`, which a device on another version asks for whole.
    Kv {
        #[serde(rename = "ref", default, skip_serializing_if = "Option::is_none")]
        reference: Option<Ulid>,
        version: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kv_list: Option<Vec<KvItem>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        set: Option<Vec<KvItem>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        remove: Option<Vec<String>>,
    },
}

// -------------------------------------------------------------------------------------------------
// REST: each route's body and reply, read and written by the server and clients alike, so a renamed
// field breaks both sides at compile time. Every reply is wrapped as `{"ok": true, "data": ...}`.
// -------------------------------------------------------------------------------------------------

/// A reply with nothing in it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmptyReply {}

/// What anyone can read before an account exists. `registration` is `[registration] allow`.
/// `captcha` is whether a `[captcha]` section is set. `name` is the optional server name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeaturesReply {
    pub registration: bool,
    pub captcha: bool,
    /// Absent when the server has no name. A blank name is stored as absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptchaReply {
    pub id: Ulid,
    /// A `data:image/png;base64,` URL.
    pub image: String,
}

#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptchaAnswer {
    pub id: Ulid,
    pub answer: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterBody {
    pub username: String,
    pub password: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captcha: Option<CaptchaAnswer>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountReply {
    pub account: Ulid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateAccountBody {
    pub username: String,
    #[serde(default)]
    pub admin: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreatedAccountReply {
    pub account: Ulid,
    pub link: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupBody {
    pub token: String,
    pub password: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupReply {
    pub account: Ulid,
    pub username: String,
}

/// A returning device's signature on a `device_login` challenge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceProof {
    pub id: Ulid,
    pub challenge: Bytes,
    pub signature: Bytes,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginBody {
    pub username: String,
    pub password: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captcha: Option<CaptchaAnswer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceProof>,
}

/// `token` only from `/api/v1/login/token`, which sets no cookie.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginReply {
    pub account: Ulid,
    pub device: Option<Ulid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChallengeBody {
    pub username: String,
    /// `password_reset` or `device_login`.
    pub purpose: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChallengeReply {
    pub challenge: Bytes,
    /// The account the username names, so a phone finds its folder before logging in.
    pub account: Ulid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResetBody {
    pub username: String,
    pub challenge: Bytes,
    pub new_password: String,
    pub signature: Bytes,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionItem {
    pub id: Ulid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<Ulid>,
    pub created_at: i64,
    pub last_used_at: i64,
    pub expires_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionListReply {
    pub session_list: Vec<SessionItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountKeysBody {
    pub master_public: Bytes,
    pub account_public: Bytes,
    pub account_stamp: Bytes,
    /// The X25519 key invitations are sealed to, and the master key's stamp on it.
    pub invite_public: Bytes,
    pub invite_stamp: Bytes,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelfChatReply {
    pub self_chat: Ulid,
}

/// An account, with its keys once its words are set up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountInfo {
    pub id: Ulid,
    pub username: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub master_public: Option<Bytes>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_public: Option<Bytes>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_stamp: Option<Bytes>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invite_public: Option<Bytes>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invite_stamp: Option<Bytes>,
}

/// What anyone logged in can see of an account.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountBrief {
    pub id: Ulid,
    pub username: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountListReply {
    pub account_list: Vec<AccountBrief>,
    /// Findable groups and channels whose name holds the search.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chat_list: Vec<FoundChat>,
}

/// `POST /api/v1/chats/{id}/requests`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestBody {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// `GET /api/v1/chats/{id}/requests`: the waiting ones.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestListReply {
    pub request_list: Vec<RequestItem>,
}

/// `POST /api/v1/chats/{id}/invitations`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InviteBody {
    pub account: Ulid,
    pub sealed: Bytes,
    pub signature: Bytes,
}

/// `POST /api/v1/chats/{id}/invitation`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InviteAnswerBody {
    pub accept: bool,
}

/// `PUT /api/v1/chats/{id}/members/{account}/role`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleBody {
    pub role: ChatRole,
}

/// One member of a group or channel, with its role.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberItem {
    pub account: Ulid,
    pub role: ChatRole,
}

/// `GET /api/v1/chats/{id}/members`: every member, channel subscribers too, with its role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberListReply {
    pub member_list: Vec<MemberItem>,
}

/// `POST /api/v1/chats/{id}/topics/id`: a new topic's id, kept for this account for 5 minutes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopicReply {
    pub topic: Ulid,
}

#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceLogItem {
    pub online_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offline_at: Option<i64>,
    pub ip: String,
}

/// All of a device's record, for its owner on a registered device.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceItem {
    pub id: Ulid,
    /// 0 desktop, 1 mobile.
    pub platform: i64,
    pub os: String,
    pub os_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_platform: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_platform_version: Option<String>,
    pub first_ip: String,
    pub last_ip: String,
    pub created_at: i64,
    pub last_active_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminated_at: Option<i64>,
    pub log_list: Vec<DeviceLogItem>,
}

#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceListReply {
    pub device_list: Vec<DeviceItem>,
}

/// What a password-only session sees of a device: enough to choose what to terminate.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReducedDeviceItem {
    pub id: Ulid,
    pub platform: i64,
    pub last_active_day: i64,
}

#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReducedDeviceListReply {
    pub device_list: Vec<ReducedDeviceItem>,
}

/// `GET /api/v1/devices`: the whole list on a registered device, the reduced one otherwise.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DevicesReply {
    Full(DeviceListReply),
    Reduced(ReducedDeviceListReply),
}

/// A device id: the one handed out to stamp, or the one registered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceReply {
    pub device: Ulid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterDeviceBody {
    pub device: Ulid,
    pub public_key: Bytes,
    pub stamp: Bytes,
    /// 0 desktop, 1 mobile.
    pub platform: i64,
    pub os: String,
    pub os_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_platform: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_platform_version: Option<String>,
}

/// A terminate or logout statement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedBody {
    pub signature: Bytes,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyPackagesBody {
    pub body_list: Vec<Bytes>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CountReply {
    pub count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyPackageItem {
    pub device: Ulid,
    pub body: Bytes,
}

/// The body of `POST /api/v1/accounts/{id}/key_packages/claim`. Claiming to add someone to a group
/// or channel names it, so an account added by invitation only is refused before any of its
/// one-use packages is handed out. An empty body names none.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimBody {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat: Option<Ulid>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyPackageListReply {
    pub key_package_list: Vec<KeyPackageItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateChatBody {
    /// A direct chat's other person, or your own account for your self chat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<Ulid>,
    /// A group or channel, with this account its owner, alone in it until it adds people.
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub chat_type: Option<ChatType>,
    /// Whether it starts with topics on. Unset takes the server's `[chat] topics`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topics: Option<bool>,
    /// Listed in the directory from the start, under this name and bio.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub findable: Option<FindableItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatReply {
    pub chat: Ulid,
    pub created: bool,
}

/// A new upload into a chat. `kind` is the declared type that picks the limit: voice, image,
/// video, doc or file. `size` is the encrypted bytes, and `chunks` how many.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateFileBody {
    pub kind: ContentKind,
    pub size: u64,
    pub chunks: u64,
    /// False for an unencrypted message's file, which needs the permission to send one. Unset
    /// means encrypted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encrypted: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileReply {
    pub file: Ulid,
}

/// How far an upload got: `received` chunks of `chunks`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileStatusReply {
    pub size: u64,
    pub chunks: u64,
    pub received: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupInfoReply {
    pub epoch: u64,
    pub group_info: Bytes,
}

/// A device in a chat, as members need it to compare with their MLS members. `statement` is set
/// once it has ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatDevice {
    pub account: Ulid,
    pub device: Ulid,
    pub public_key: Bytes,
    pub stamp: Bytes,
    pub master_public: Bytes,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub statement: Option<Bytes>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatDeviceListReply {
    pub device_list: Vec<ChatDevice>,
}

/// One stored wrap: an epoch's key, wrapped to one account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WrapEntry {
    pub epoch: u64,
    pub account: Ulid,
    pub wrapped: Bytes,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WrapListReply {
    pub wrap_list: Vec<WrapEntry>,
}

/// A refusal, as REST carries it: the same `code` and `message` as the WebSocket's `refused`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefusedReply {
    pub code: String,
    pub message: String,
    /// Only for `call_open`: the open call's sequence id, so the client joins that one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_seq: Option<u64>,
    /// Only for `kv_stale`: the server's version of the settings, and its whole set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kv_list: Option<Vec<KvItem>>,
}

/// Where a `sync` page ended: its last row's server time, chat, sequence id, and `n`, which is 0 for a
/// message and the call row's own for a call row.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SyncPosition {
    pub at: i64,
    pub chat: Ulid,
    pub seq: u64,
    pub n: u64,
}
