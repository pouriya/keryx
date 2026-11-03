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
    }
);

numbered!(
    /// The numbers are the `message.content_kind` column's. 10 is reserved for `media_bundle`,
    /// which is not built.
    ContentKind {
        Text = 0,
        Voice = 1,
        Reaction = 2,
        Image = 3,
        Video = 4,
        Doc = 5,
        File = 6,
        VideoCall = 7,
        VoiceCall = 8,
        TextCall = 9,
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

numbered!(
    /// The numbers are the `conversation.mode` column's.
    ChatMode {
        Private = 0,
        Mixed = 1,
        Open = 2,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_seq: Option<u64>,
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
    },
    /// "Received up to N": this device now holds those messages. Kept like `seen`, and answered
    /// with `done`.
    Received {
        #[serde(rename = "ref")]
        reference: Ulid,
        chat: Ulid,
        up_to_seq: u64,
        client_received_at: i64,
    },
    /// Not saved and not answered, so it has no `ref` and no sequence id.
    Typing {
        chat: Ulid,
        on: bool,
    },
    /// The server works out which chats, and the client never sends a list.
    Sync {
        #[serde(rename = "ref")]
        reference: Ulid,
    },
    /// Up to `limit` messages after `after_seq`, oldest first. With `before_seq`, the newest `limit`
    /// of those below it instead, still oldest first, so one past the last sequence id gives the
    /// newest page and scrolling up asks below the oldest one shown.
    History {
        #[serde(rename = "ref")]
        reference: Ulid,
        chat: Ulid,
        after_seq: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before_seq: Option<u64>,
        limit: u32,
    },
    GroupInfo {
        #[serde(rename = "ref")]
        reference: Ulid,
        chat: Ulid,
        epoch: u64,
        body: Bytes,
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
}

/// A stored message. **A removed one has only `chat` and `seq`**, so pages have no gaps, and a
/// client knows something was there, not what.
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
    pub call_seq: Option<u64>,
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
            call_seq: self.call_seq,
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
    pub mode: ChatMode,
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
}

/// How far one member's devices have received and seen a chat. No `seen_seq` for someone with
/// receipts off.
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
    },
    Message(Box<MessageFrame>),
    Chats {
        #[serde(rename = "ref")]
        reference: Ulid,
        chat_list: Vec<ChatItem>,
    },
    Page {
        #[serde(rename = "ref")]
        reference: Ulid,
        message_list: Vec<MessageFrame>,
    },
    Welcome {
        chat: Ulid,
        body: Bytes,
    },
    Typing {
        chat: Ulid,
        account: Ulid,
        on: bool,
    },
    /// A member's received or seen position rose, sent to the other members' online devices.
    Marks {
        chat: Ulid,
        #[serde(flatten)]
        mark: MarkItem,
    },
    /// A device of someone sharing a chat ended. `statement` is its signed revocation or logout,
    /// which a client verifies before removing the device from each chat.
    Revoked {
        account: Ulid,
        device: Ulid,
        statement: Bytes,
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
/// `captcha` is whether a `[captcha]` section is set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeaturesReply {
    pub registration: bool,
    pub captcha: bool,
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelfChatReply {
    pub self_chat: Ulid,
}

/// An account, with its three keys once its words are set up.
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyPackageListReply {
    pub key_package_list: Vec<KeyPackageItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateChatBody {
    pub account: Ulid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatReply {
    pub chat: Ulid,
    pub created: bool,
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
}
