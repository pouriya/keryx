//! The storage trait on SQLite, and the only place SQL lives.
//!
//! One `.db` file with no `.wal`: the classic `DELETE` journal, so a `-journal` file exists only
//! during a write. One writer and one reader connection, each behind a lock, taking turns on the
//! blocking pool. If that becomes too slow, the answer is Postgres.

use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use keryx_core::protocol::{
    AccountBrief, ActionKind, AgentItem, Audience, Bytes, CallEvent, CallEventKind, ChatAction,
    ChatDevice, ChatEvent, ChatFolders, ChatItem, ChatOptions, ChatProfileItem, ChatRole, ChatType,
    ContentKind, DeviceItem, DeviceLogItem, DraftItem, FindableItem, FolderItem, FoundChat,
    GENERAL_TOPIC, InvitationItem, KeyPackageItem, Kind, KvChange, KvItem, KvReply, LineAction,
    LineBody, MarkItem, MessageFrame, PlaceSeen, ProfileKeyItem, ProfileReply, RequestItem,
    RequestState, SessionItem, SignedProfile, StickerItem, StickerPack, SyncPosition, TimesItem,
    TopicItem, TopicOptionsBody, TopicProfileItem, TopicWrite, WrapEntry,
};
use keryx_core::ulid::Ulid;
use rusqlite::types::Type;
use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior, params};

use super::{
    Account, AgentChange, AgentCreated, Answered, Appended, Asked, BuiltInRole, ChatAccess,
    ChatProfileSaved, DeviceKey, DeviceUsage, Error, GroupInfoPut, KvChanged, Left, MessageBrief,
    NamesSaved, NewAccount, NewAccountKeys, NewAgent, NewChatOptions, NewChatProfile, NewDevice,
    NewFile, NewGroup, NewGroupStart, NewInvitation, NewMessage, NewNames, NewPhoto, NewProfile,
    NewSession, NewSticker, NewTopicOptions, NewTopicProfile, NewWrap, Place, Position,
    PositionReport, PresenceTimes, ProfileSaved, Registered, Removed, Result, RoleSet, Session,
    SetupLink, Storage, StoredFile, SyncFrom, SyncPage, TimesShown, TopicAccess, TopicMods,
    TopicSaved, TopicsPut,
};

/// How long a connection waits for a lock another process holds, such as a backup.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

const MESSAGE_COLUMNS: &str = "conversation_id, seq, device_id, kind, content_kind, encrypted, \
     epoch, pointer_seq, client_sent_at, server_saved_at, body, signature, topic_id, thread_seq";

const CHAT_EVENT_COLUMNS: &str =
    "conversation_id, n, actor, action, target, seq, audience, server_at, topic_id, copy";

const CALL_COLUMNS: &str = "conversation_id, call_seq, n, kind, device_id, server_at, client_at, \
     epoch, signature, body, outcome";

/// A message's kind, content kind, sender's account, sender's device's age, save time, topic and
/// thread.
type BriefRow = (
    i64,
    Option<i64>,
    [u8; 16],
    i64,
    i64,
    Option<[u8; 16]>,
    Option<i64>,
);

pub struct Sqlite {
    writer: Arc<Mutex<Connection>>,
    reader: Arc<Mutex<Connection>>,
    /// The last server time handed out for a message or call row. See `saved_now`.
    last_saved: AtomicI64,
}

impl Sqlite {
    /// Opens the database, creating it with its tables the first time.
    pub fn open(path: &Path) -> Result<Self> {
        let mut connections = Vec::with_capacity(2);
        for _ in 0..2 {
            let connection = Connection::open(path)?;
            connection.busy_timeout(BUSY_TIMEOUT)?;
            connection.pragma_update(None, "journal_mode", "DELETE")?;
            connection.pragma_update(None, "foreign_keys", "ON")?;
            connections.push(connection);
        }
        let Some(reader) = connections.pop() else {
            return Err(Error::Database(rusqlite::Error::InvalidQuery));
        };
        let Some(writer) = connections.pop() else {
            return Err(Error::Database(rusqlite::Error::InvalidQuery));
        };
        // No migrations until there is a production server, chosen by the author: a schema change
        // means deleting the database.
        let version: i64 = writer.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version == 0 {
            writer.execute_batch(&format!(
                "BEGIN IMMEDIATE;\n{}\nPRAGMA user_version = 1;\nCOMMIT;",
                include_str!("sqlite.sql")
            ))?;
        }
        let last_saved: Option<i64> = writer.query_row(
            "SELECT MAX(at) FROM (SELECT MAX(server_saved_at) AS at FROM message
             UNION ALL SELECT MAX(server_at) FROM call)",
            [],
            |row| row.get(0),
        )?;
        Ok(Self {
            writer: Arc::new(Mutex::new(writer)),
            reader: Arc::new(Mutex::new(reader)),
            last_saved: AtomicI64::new(last_saved.unwrap_or(0)),
        })
    }

    async fn read<T: Send + 'static>(
        &self,
        work: impl FnOnce(&Connection) -> rusqlite::Result<T> + Send + 'static,
    ) -> Result<T> {
        let reader = Arc::clone(&self.reader);
        let done = tokio::task::spawn_blocking(move || {
            let connection = match reader.lock() {
                Ok(connection) => connection,
                Err(poisoned) => poisoned.into_inner(),
            };
            work(&connection)
        })
        .await;
        match done {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) => Err(Error::Database(error)),
            Err(error) => Err(Error::Task(error)),
        }
    }

    /// One transaction. An error rolls all of it back.
    async fn write<T: Send + 'static>(
        &self,
        work: impl FnOnce(&Connection) -> rusqlite::Result<T> + Send + 'static,
    ) -> Result<T> {
        let writer = Arc::clone(&self.writer);
        let done = tokio::task::spawn_blocking(move || {
            let mut connection = match writer.lock() {
                Ok(connection) => connection,
                Err(poisoned) => poisoned.into_inner(),
            };
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let value = work(&transaction)?;
            transaction.commit()?;
            Ok(value)
        })
        .await;
        match done {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) => Err(Error::Database(error)),
            Err(error) => Err(Error::Task(error)),
        }
    }
}

/// Whether a group's or channel's profile names a photo it may keep: a complete image of this chat
/// uploaded by its account, or the photo it already has. One naming none fits.
fn photo_fits(connection: &Connection, profile: &NewChatProfile) -> rusqlite::Result<bool> {
    let Some(photo) = profile.photo else {
        return Ok(true);
    };
    connection.query_row(
        "SELECT EXISTS (SELECT 1 FROM file f JOIN device d ON d.id = f.uploaded_by
         WHERE f.id = ?1 AND f.conversation_id = ?2 AND f.kind = 3 AND f.size <= ?4
         AND f.received = f.chunks AND f.removed_at IS NULL
         AND (d.account_id = ?3 OR f.id = (SELECT photo_id FROM conversation_profile
                                           WHERE conversation_id = ?2)))",
        params![
            photo.bytes(),
            profile.chat.bytes(),
            profile.account.bytes(),
            int(profile.photo_limit)
        ],
        |row| row.get(0),
    )
}

/// Keeps a group's or channel's profile, whose photo [`photo_fits`] checked, with the file it
/// names kept from now on and the one it replaced marked removed. Its first, at epoch 0, comes with
/// the chat's profile key, and later ones replace it beside the key already kept.
fn save_chat_profile(
    connection: &Connection,
    profile: NewChatProfile,
    first_key: Option<Vec<u8>>,
) -> rusqlite::Result<ChatProfileSaved> {
    if let Some(photo) = profile.photo {
        connection.execute(
            "UPDATE file SET used_at = ?2 WHERE id = ?1 AND used_at IS NULL",
            params![photo.bytes(), profile.now],
        )?;
    }
    let old_photo: Option<Option<[u8; 16]>> = connection
        .query_row(
            "SELECT photo_id FROM conversation_profile WHERE conversation_id = ?1",
            params![profile.chat.bytes()],
            |row| row.get(0),
        )
        .optional()?;
    let mut removed = Vec::new();
    if let Some(Some(old)) = old_photo {
        let old = Ulid::from_bytes(old);
        if Some(old) != profile.photo {
            connection.execute(
                "UPDATE file SET removed_at = ?2 WHERE id = ?1 AND removed_at IS NULL",
                params![old.bytes(), profile.now],
            )?;
            removed.push(old);
        }
    }
    let photo = profile.photo.as_ref().map(|photo| photo.bytes().to_vec());
    let kept = match first_key {
        Some(key) => connection
            .query_row(
                "INSERT INTO conversation_profile (conversation_id, sealed, key, key_epoch,
                 photo_id, signature, updated_by, version, updated_at)
                 VALUES (?1, ?2, ?3, 0, ?4, ?5, ?6, 1, ?7)
                 RETURNING version, key, key_epoch",
                params![
                    profile.chat.bytes(),
                    profile.sealed,
                    key,
                    photo,
                    profile.signature,
                    profile.device.bytes(),
                    profile.now
                ],
                kept_key,
            )
            .optional()?,
        None => connection
            .query_row(
                "UPDATE conversation_profile SET sealed = ?2, photo_id = ?3, signature = ?4,
                 updated_by = ?5, version = version + 1, updated_at = ?6
                 WHERE conversation_id = ?1
                 RETURNING version, key, key_epoch",
                params![
                    profile.chat.bytes(),
                    profile.sealed,
                    photo,
                    profile.signature,
                    profile.device.bytes(),
                    profile.now
                ],
                kept_key,
            )
            .optional()?,
    };
    let Some((version, key, key_epoch)) = kept else {
        return Ok(ChatProfileSaved::NotStarted);
    };
    Ok(ChatProfileSaved::Saved {
        item: ChatProfileItem {
            sealed: Bytes(profile.sealed),
            signature: Bytes(profile.signature),
            device: profile.device,
            version,
            key_epoch,
            key: Bytes(key),
        },
        removed,
    })
}

fn kept_key(row: &Row<'_>) -> rusqlite::Result<(u64, Vec<u8>, u64)> {
    Ok((unsigned(row.get(0)?), row.get(1)?, unsigned(row.get(2)?)))
}

/// A chat's profile row, read back with its key.
fn chat_profile_row(row: &Row<'_>) -> rusqlite::Result<ChatProfileItem> {
    Ok(ChatProfileItem {
        sealed: Bytes(row.get(0)?),
        signature: Bytes(row.get(1)?),
        device: Ulid::from_bytes(row.get(2)?),
        version: unsigned(row.get(3)?),
        key: Bytes(row.get(4)?),
        key_epoch: unsigned(row.get(5)?),
    })
}

/// What a chat's members may do beyond or short of the member role, replacing what was kept.
fn save_chat_options(
    connection: &Connection,
    chat: Ulid,
    options: &NewChatOptions,
) -> rusqlite::Result<()> {
    connection.execute(
        "DELETE FROM chat_permission WHERE conversation_id = ?1",
        params![chat.bytes()],
    )?;
    for (list, allowed) in [(&options.allow, true), (&options.deny, false)] {
        for permission in list {
            connection.execute(
                "INSERT OR REPLACE INTO chat_permission (conversation_id, permission,
                 allowed) VALUES (?1, ?2, ?3)",
                params![chat.bytes(), permission, allowed],
            )?;
        }
    }
    connection.execute(
        "UPDATE conversation SET slow_mode = ?2 WHERE id = ?1",
        params![chat.bytes(), opt_int(options.slow_mode)],
    )?;
    Ok(())
}

/// A sequence id or epoch as SQLite holds it. A larger number than any stored can match nothing.
fn int(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// An account's settings: their version, and the whole set unless the version is `known`.
fn kv_of(connection: &Connection, account: Ulid, known: Option<u64>) -> rusqlite::Result<KvReply> {
    let version: i64 = connection.query_row(
        "SELECT kv_version FROM account WHERE id = ?1",
        params![account.bytes()],
        |row| row.get(0),
    )?;
    let version = u64::try_from(version).unwrap_or(0);
    if known == Some(version) {
        return Ok(KvReply {
            version,
            kv_list: None,
        });
    }
    let mut statement =
        connection.prepare("SELECT key, value FROM kv WHERE account_id = ?1 ORDER BY key")?;
    let mut kv_list = Vec::new();
    for item in statement.query_map(params![account.bytes()], |row| {
        Ok(KvItem {
            key: row.get(0)?,
            value: Bytes(row.get(1)?),
        })
    })? {
        kv_list.push(item?);
    }
    Ok(KvReply {
        version,
        kv_list: Some(kv_list),
    })
}

/// Ids stored side by side, 16 bytes each, as a folder's chats and pins are.
fn id_bytes(id_list: &[Ulid]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(id_list.len() * 16);
    for id in id_list {
        bytes.extend_from_slice(id.bytes());
    }
    bytes
}

fn id_list(bytes: &[u8]) -> Vec<Ulid> {
    let mut id_list = Vec::with_capacity(bytes.len() / 16);
    for chunk in bytes.chunks_exact(16) {
        let mut id = [0u8; 16];
        id.copy_from_slice(chunk);
        id_list.push(Ulid::from_bytes(id));
    }
    id_list
}

fn opt_int(value: Option<u64>) -> Option<i64> {
    if let Some(value) = value {
        return Some(int(value));
    }
    None
}

fn unsigned(value: i64) -> u64 {
    u64::try_from(value).unwrap_or_default()
}

fn opt_unsigned(value: Option<i64>) -> Option<u64> {
    if let Some(value) = value {
        return Some(unsigned(value));
    }
    None
}

fn opt_id(value: Option<[u8; 16]>) -> Option<Ulid> {
    if let Some(value) = value {
        return Some(Ulid::from_bytes(value));
    }
    None
}

/// One of the schema's numbered columns.
fn numbered<T: TryFrom<u8>>(value: i64) -> rusqlite::Result<T> {
    if let Ok(number) = u8::try_from(value)
        && let Ok(parsed) = T::try_from(number)
    {
        return Ok(parsed);
    }
    Err(rusqlite::Error::IntegralValueOutOfRange(0, value))
}

fn opt_numbered<T: TryFrom<u8>>(value: Option<i64>) -> rusqlite::Result<Option<T>> {
    match value {
        Some(value) => Ok(Some(numbered(value)?)),
        None => Ok(None),
    }
}

fn message_frame(row: &Row<'_>) -> rusqlite::Result<MessageFrame> {
    let body: Option<Vec<u8>> = row.get(10)?;
    let signature: Option<Vec<u8>> = row.get(11)?;
    let encrypted: Option<i64> = row.get(5)?;
    Ok(MessageFrame {
        chat: Ulid::from_bytes(row.get(0)?),
        seq: unsigned(row.get(1)?),
        device: opt_id(row.get(2)?),
        kind: opt_numbered(row.get(3)?)?,
        content_kind: opt_numbered(row.get(4)?)?,
        encrypted: encrypted.map(|encrypted| encrypted == 1),
        epoch: opt_unsigned(row.get(6)?),
        pointer_seq: opt_unsigned(row.get(7)?),
        topic: opt_id(row.get(12)?),
        thread_seq: opt_unsigned(row.get(13)?),
        client_sent_at: row.get(8)?,
        server_saved_at: row.get(9)?,
        body: body.map(Bytes),
        signature: signature.map(Bytes),
    })
}

fn chat_event_row(row: &Row<'_>) -> rusqlite::Result<ChatEvent> {
    Ok(ChatEvent {
        chat: Ulid::from_bytes(row.get(0)?),
        n: unsigned(row.get(1)?),
        actor: Ulid::from_bytes(row.get(2)?),
        action: numbered(row.get(3)?)?,
        target: opt_id(row.get(4)?),
        seq: opt_unsigned(row.get(5)?),
        topic: opt_id(row.get(8)?),
        audience: numbered(row.get(6)?)?,
        server_at: row.get(7)?,
        copy_list: match row.get::<_, Option<Vec<u8>>>(9)? {
            Some(copy) => serde_json::from_slice(&copy).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(9, Type::Blob, Box::new(error))
            })?,
            None => Vec::new(),
        },
    })
}

/// Writes a delete's or edit's history row, in its message's topic, with a copy of the message and
/// its edits as they are now, so the row keeps what the delete wipes.
fn write_copied(
    connection: &Connection,
    message: &NewMessage,
    author: Ulid,
) -> rusqlite::Result<ChatEvent> {
    let chat = message.chat;
    let target = opt_int(message.pointer_seq);
    let topic: Option<Option<[u8; 16]>> = connection
        .query_row(
            "SELECT topic_id FROM message WHERE conversation_id = ?1 AND seq = ?2",
            params![chat.bytes(), target],
            |row| row.get(0),
        )
        .optional()?;
    let action = if message.kind == Kind::Edit {
        ChatAction::Edited
    } else {
        ChatAction::Deleted
    };
    let mut changed = event(
        chat,
        message.account,
        action,
        Some(author),
        Audience::Moderators,
        message.now,
    );
    changed.seq = message.pointer_seq;
    changed.topic = topic.flatten().map(Ulid::from_bytes);
    let mut statement = connection.prepare(&format!(
        "SELECT {MESSAGE_COLUMNS} FROM message WHERE conversation_id = ?1 AND kind IS NOT NULL
         AND (seq = ?2 OR (kind = 1 AND pointer_seq = ?2)) ORDER BY seq"
    ))?;
    let rows = statement.query_map(params![chat.bytes(), target], message_frame)?;
    changed.copy_list = rows.collect::<rusqlite::Result<_>>()?;
    write_event(connection, changed, None)
}

fn chat_event_position(event: &ChatEvent) -> SyncPosition {
    SyncPosition {
        at: event.server_at,
        chat: event.chat,
        seq: 0,
        n: event.n,
    }
}

/// Writes a chat event under the chat's next `n`. `viewer` is the one account an audience-2 event
/// is for.
fn write_event(
    connection: &Connection,
    mut event: ChatEvent,
    viewer: Option<Ulid>,
) -> rusqlite::Result<ChatEvent> {
    let n: i64 = connection.query_row(
        "SELECT COALESCE(MAX(n), 0) + 1 FROM chat_event WHERE conversation_id = ?1",
        params![event.chat.bytes()],
        |row| row.get(0),
    )?;
    let copy = if event.copy_list.is_empty() {
        None
    } else {
        Some(
            serde_json::to_vec(&event.copy_list)
                .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?,
        )
    };
    connection.execute(
        "INSERT INTO chat_event (conversation_id, n, actor, action, target, seq, topic_id, audience,
         viewer, server_at, copy) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            event.chat.bytes(),
            n,
            event.actor.bytes(),
            u8::from(event.action),
            event.target.as_ref().map(|target| target.bytes().to_vec()),
            opt_int(event.seq),
            event.topic.as_ref().map(|topic| topic.bytes().to_vec()),
            u8::from(event.audience),
            viewer.as_ref().map(|viewer| viewer.bytes().to_vec()),
            event.server_at,
            copy
        ],
    )?;
    event.n = unsigned(n);
    Ok(event)
}

/// A new event, before its `n` is known.
fn event(
    chat: Ulid,
    actor: Ulid,
    action: ChatAction,
    target: Option<Ulid>,
    audience: Audience,
    now: i64,
) -> ChatEvent {
    ChatEvent {
        chat,
        n: 0,
        actor,
        action,
        target,
        seq: None,
        topic: None,
        audience,
        server_at: now,
        copy_list: Vec::new(),
    }
}

/// Writes a membership line as the chat's next message, in a group only, since a channel's main
/// list holds posts alone. Unencrypted and unsigned, its device the actor's, it moves the chat up
/// and is its preview, as text is.
fn write_line(
    connection: &Connection,
    chat: Ulid,
    device: Ulid,
    action: LineAction,
    account_list: Vec<Ulid>,
    now: i64,
) -> rusqlite::Result<Option<MessageFrame>> {
    let chat_type: u8 = connection.query_row(
        "SELECT type FROM conversation WHERE id = ?1",
        params![chat.bytes()],
        |row| row.get(0),
    )?;
    if chat_type != u8::from(ChatType::Group) {
        return Ok(None);
    }
    let body = serde_json::to_vec(&LineBody {
        action,
        account_list,
    })
    .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    let seq: i64 = connection.query_row(
        "UPDATE conversation SET last_seq = last_seq + 1, last_kind = 5, last_at = ?2,
         last_content_seq = last_seq + 1 WHERE id = ?1 RETURNING last_seq",
        params![chat.bytes(), now],
        |row| row.get(0),
    )?;
    connection.execute(
        "INSERT INTO message (conversation_id, seq, device_id, kind, encrypted, client_sent_at,
         server_saved_at, body) VALUES (?1, ?2, ?3, 5, 0, ?4, ?4, ?5)",
        params![chat.bytes(), seq, device.bytes(), now, body],
    )?;
    Ok(Some(MessageFrame {
        chat,
        seq: unsigned(seq),
        device: Some(device),
        kind: Some(Kind::Line),
        content_kind: None,
        encrypted: Some(false),
        epoch: None,
        pointer_seq: None,
        topic: None,
        thread_seq: None,
        client_sent_at: Some(now),
        server_saved_at: Some(now),
        body: Some(Bytes(body)),
        signature: None,
    }))
}

/// Takes a chat out of an account's folders and pins, as it stops being in the chat.
fn drop_from_folders(connection: &Connection, chat: Ulid, account: Ulid) -> rusqlite::Result<()> {
    let mut statement =
        connection.prepare("SELECT id, chats, pins FROM folder WHERE account_id = ?1")?;
    let mut changed = Vec::new();
    let rows = statement.query_map(params![account.bytes()], |row| {
        Ok((
            Ulid::from_bytes(row.get(0)?),
            row.get::<_, Option<Vec<u8>>>(1)?,
            row.get::<_, Vec<u8>>(2)?,
        ))
    })?;
    for row in rows {
        let (folder, chats, pins) = row?;
        let mut chat_list = None;
        if let Some(chats) = &chats {
            chat_list = Some(id_list(chats));
        }
        let mut pin_list = id_list(&pins);
        let had = pin_list.contains(&chat)
            || chat_list
                .as_ref()
                .is_some_and(|chat_list| chat_list.contains(&chat));
        if !had {
            continue;
        }
        pin_list.retain(|pinned| *pinned != chat);
        if let Some(chat_list) = &mut chat_list {
            chat_list.retain(|held| *held != chat);
        }
        changed.push((folder, chat_list, pin_list));
    }
    for (folder, chat_list, pin_list) in changed {
        connection.execute(
            "UPDATE folder SET chats = ?2, pins = ?3 WHERE id = ?1",
            params![
                folder.bytes(),
                chat_list.as_deref().map(id_bytes),
                id_bytes(&pin_list)
            ],
        )?;
    }
    Ok(())
}

/// A chat's options: its member role's changes, without their prefix, and its slow mode.
fn chat_options(connection: &Connection, chat: Ulid) -> rusqlite::Result<ChatOptions> {
    let mut options = ChatOptions::default();
    let mut statement = connection.prepare(
        "SELECT permission, allowed FROM chat_permission WHERE conversation_id = ?1
         ORDER BY permission",
    )?;
    let rows = statement.query_map(params![chat.bytes()], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?))
    })?;
    for row in rows {
        let (permission, allowed) = row?;
        let name = match permission.split_once('.') {
            Some((_, name)) => name.to_string(),
            None => permission,
        };
        if allowed {
            options.allow.push(name);
        } else {
            options.deny.push(name);
        }
    }
    let slow_mode: Option<i64> = connection.query_row(
        "SELECT slow_mode FROM conversation WHERE id = ?1",
        params![chat.bytes()],
        |row| row.get(0),
    )?;
    options.slow_mode = opt_unsigned(slow_mode);
    Ok(options)
}

/// General's id where its own settings are kept, as SQL: all zeros. Lists of live topics leave it out.
const GENERAL_SQL: &str = "X'00000000000000000000000000000000'";

/// Leaves out a message in a hidden topic, or an edit of one, unless the account in `?9` sees it:
/// the chat's owners and moderators, and the topic's own moderators.
const HIDDEN_FROM: &str = "NOT EXISTS (SELECT 1 FROM topic t
     WHERE t.conversation_id = message.conversation_id AND t.hidden = 1 AND t.removed = 0
     AND t.id = COALESCE(message.topic_id, (SELECT p.topic_id FROM message p
                WHERE p.conversation_id = message.conversation_id AND p.seq = message.pointer_seq
                AND message.kind = 1))
     AND NOT EXISTS (SELECT 1 FROM member m WHERE m.conversation_id = t.conversation_id
                     AND m.account_id = ?9 AND m.is_owner = 1)
     AND NOT EXISTS (SELECT 1 FROM member_role r WHERE r.conversation_id = t.conversation_id
                     AND r.account_id = ?9)
     AND NOT EXISTS (SELECT 1 FROM topic_moderator d WHERE d.conversation_id = t.conversation_id
                     AND d.topic_id = t.id AND d.account_id = ?9))";

/// One topic's settings, moderators and sealed profile, General's under the all-zero id. `None` when
/// it is not a live topic of the chat.
fn topic_item(
    connection: &Connection,
    chat: Ulid,
    topic: Ulid,
) -> rusqlite::Result<Option<TopicItem>> {
    type TopicRow = (
        bool,
        bool,
        i64,
        Option<i64>,
        Option<Vec<u8>>,
        Option<Vec<u8>>,
        Option<[u8; 16]>,
        i64,
    );
    let row: Option<TopicRow> = connection
        .query_row(
            "SELECT closed, hidden, write, slow_mode, sealed, signature, updated_by, version
             FROM topic WHERE conversation_id = ?1 AND id = ?2 AND removed = 0",
            params![chat.bytes(), topic.bytes()],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                ))
            },
        )
        .optional()?;
    let Some((closed, hidden, write, slow_mode, sealed, signature, updated_by, version)) = row
    else {
        return Ok(None);
    };
    let mut options = TopicOptionsBody {
        slow_mode: opt_unsigned(slow_mode),
        write: if write == 1 {
            TopicWrite::Moderators
        } else {
            TopicWrite::Everyone
        },
        closed,
        hidden,
        ..TopicOptionsBody::default()
    };
    let mut statement = connection.prepare(
        "SELECT permission, allowed FROM topic_permission WHERE conversation_id = ?1
         AND topic_id = ?2 ORDER BY permission",
    )?;
    let rows = statement.query_map(params![chat.bytes(), topic.bytes()], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?))
    })?;
    for row in rows {
        let (permission, allowed) = row?;
        let name = match permission.split_once('.') {
            Some((_, name)) => name.to_string(),
            None => permission,
        };
        if allowed {
            options.allow.push(name);
        } else {
            options.deny.push(name);
        }
    }
    let mut statement = connection.prepare(
        "SELECT account_id FROM topic_moderator WHERE conversation_id = ?1 AND topic_id = ?2
         ORDER BY account_id",
    )?;
    let mut mod_list = Vec::new();
    for account in statement.query_map(params![chat.bytes(), topic.bytes()], |row| row.get(0))? {
        mod_list.push(Ulid::from_bytes(account?));
    }
    let profile = match (sealed, signature, updated_by) {
        (Some(sealed), Some(signature), Some(device)) => Some(TopicProfileItem {
            sealed: Bytes(sealed),
            signature: Bytes(signature),
            device: Ulid::from_bytes(device),
            version: unsigned(version),
        }),
        _ => None,
    };
    Ok(Some(TopicItem {
        topic,
        options,
        mod_list,
        profile,
    }))
}

/// Whether a topic is hidden from this account: hidden, and it is none of the chat's owners and
/// moderators nor the topic's own.
fn hidden_from(
    connection: &Connection,
    chat: Ulid,
    topic: Ulid,
    account: Ulid,
) -> rusqlite::Result<bool> {
    connection.query_row(
        "SELECT EXISTS (SELECT 1 FROM topic WHERE conversation_id = ?1 AND id = ?2 AND hidden = 1)
         AND NOT EXISTS (SELECT 1 FROM member WHERE conversation_id = ?1 AND account_id = ?3
                         AND is_owner = 1)
         AND NOT EXISTS (SELECT 1 FROM member_role WHERE conversation_id = ?1 AND account_id = ?3)
         AND NOT EXISTS (SELECT 1 FROM topic_moderator WHERE conversation_id = ?1 AND topic_id = ?2
                         AND account_id = ?3)",
        params![chat.bytes(), topic.bytes(), account.bytes()],
        |row| row.get(0),
    )
}

/// A topic's history row and the whole item after a change, or `NoSuchTopic`.
fn topic_saved(
    connection: &Connection,
    chat: Ulid,
    topic: Ulid,
    changed: Option<ChatEvent>,
    was_hidden: bool,
    removed: Vec<Ulid>,
) -> rusqlite::Result<TopicSaved> {
    let Some(item) = topic_item(connection, chat, topic)? else {
        return Ok(TopicSaved::NoSuchTopic);
    };
    let event = match changed {
        Some(mut changed) => {
            changed.topic = Some(topic);
            Some(write_event(connection, changed, None)?)
        }
        None => None,
    };
    Ok(TopicSaved::Saved {
        event,
        item: Box::new(item),
        was_hidden,
        removed,
    })
}

/// Whether the live topic exists and was hidden.
fn topic_hidden(
    connection: &Connection,
    chat: Ulid,
    topic: Ulid,
) -> rusqlite::Result<Option<bool>> {
    connection
        .query_row(
            "SELECT hidden FROM topic WHERE conversation_id = ?1 AND id = ?2 AND removed = 0",
            params![chat.bytes(), topic.bytes()],
            |row| row.get(0),
        )
        .optional()
}

/// A LIKE pattern finding `term` anywhere, its own `%`, `_` and `\\` escaped.
fn like_pattern(term: &str) -> String {
    let mut escaped = String::with_capacity(term.len() + 2);
    escaped.push('%');
    for character in term.chars() {
        if matches!(character, '%' | '_' | '\\') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped.push('%');
    escaped
}

/// Removes a whole chat for everyone, out of every member's folders, giving who was in it, who had
/// asked to join or was invited, and its files, whose bytes can go.
fn remove_chat_rows(connection: &Connection, chat: Ulid) -> rusqlite::Result<Option<Removed>> {
    let mut lists = [Vec::new(), Vec::new(), Vec::new()];
    for (table, list) in ["member", "join_request", "invitation"]
        .into_iter()
        .zip(lists.iter_mut())
    {
        let mut statement = connection.prepare(&format!(
            "SELECT account_id FROM {table} WHERE conversation_id = ?1"
        ))?;
        for account in statement.query_map(params![chat.bytes()], |row| row.get(0))? {
            list.push(Ulid::from_bytes(account?));
        }
    }
    let [member_list, asker_list, invitee_list] = lists;
    let mut statement = connection
        .prepare("SELECT id FROM file WHERE conversation_id = ?1 AND removed_at IS NULL")?;
    let mut file_list = Vec::new();
    for file in statement.query_map(params![chat.bytes()], |row| row.get(0))? {
        file_list.push(Ulid::from_bytes(file?));
    }
    for account in &member_list {
        drop_from_folders(connection, chat, *account)?;
    }
    let removed = connection.execute(
        "DELETE FROM conversation WHERE id = ?1",
        params![chat.bytes()],
    )?;
    if removed == 0 {
        return Ok(None);
    }
    Ok(Some(Removed {
        member_list,
        asker_list,
        invitee_list,
        file_list,
    }))
}

fn request_row(row: &Row<'_>) -> rusqlite::Result<RequestItem> {
    let state: i64 = row.get(4)?;
    Ok(RequestItem {
        chat: Ulid::from_bytes(row.get(0)?),
        account: Ulid::from_bytes(row.get(1)?),
        note: row.get(2)?,
        name: row.get(3)?,
        state: if state == 1 {
            RequestState::Declined
        } else {
            RequestState::Waiting
        },
        at: row.get(5)?,
    })
}

const REQUEST_COLUMNS: &str = "r.conversation_id, r.account_id, r.note, c.find_name, r.state, r.at";

fn invitation_row(row: &Row<'_>) -> rusqlite::Result<InvitationItem> {
    Ok(InvitationItem {
        chat: Ulid::from_bytes(row.get(0)?),
        chat_type: numbered(row.get(1)?)?,
        inviter: Ulid::from_bytes(row.get(2)?),
        device: Ulid::from_bytes(row.get(3)?),
        sealed: Bytes(row.get(4)?),
        signature: Bytes(row.get(5)?),
        at: row.get(6)?,
    })
}

const INVITATION_COLUMNS: &str =
    "i.conversation_id, c.type, d.account_id, i.inviter, i.sealed, i.signature, i.at";

/// A place's two columns in `seen` and `received`.
fn place_columns(place: Place) -> (i64, Vec<u8>) {
    match place {
        Place::Main => (0, Vec::new()),
        Place::Topic(topic) => (1, topic.bytes().to_vec()),
        Place::Thread(seq) => (2, seq.to_be_bytes().to_vec()),
    }
}

fn call_event_row(row: &Row<'_>) -> rusqlite::Result<CallEvent> {
    let signature: Option<Vec<u8>> = row.get(8)?;
    let body: Option<Vec<u8>> = row.get(9)?;
    Ok(CallEvent {
        chat: Ulid::from_bytes(row.get(0)?),
        call_seq: unsigned(row.get(1)?),
        n: unsigned(row.get(2)?),
        kind: numbered(row.get(3)?)?,
        device: Ulid::from_bytes(row.get(4)?),
        server_at: row.get(5)?,
        client_at: row.get(6)?,
        epoch: opt_unsigned(row.get(7)?),
        signature: signature.map(Bytes),
        body: body.map(Bytes),
        outcome: opt_numbered(row.get(10)?)?,
    })
}

/// Where a row stands in a `sync`: server time, chat, sequence id, and `n`, 0 for a message.
fn message_position(message: &MessageFrame) -> SyncPosition {
    SyncPosition {
        at: message.server_saved_at.unwrap_or(0),
        chat: message.chat,
        seq: message.seq,
        n: 0,
    }
}

fn call_position(event: &CallEvent) -> SyncPosition {
    SyncPosition {
        at: event.server_at,
        chat: event.chat,
        seq: event.call_seq,
        n: event.n,
    }
}

/// What one `sync` asks for: from where, in the account's chats or only `chat`, and how many rows.
#[derive(Clone, Copy)]
struct SyncAsk {
    account: Ulid,
    chat: Option<Ulid>,
    from: SyncFrom,
    limit: u32,
}

/// Rows of one table for `ask`. `order` names the time and sequence id columns this table orders
/// by, and its `n` column. A message has none, and stands at `n` 0 before its call's rows.
/// `seen` adds a condition naming the asking account as `?9`, for rows only some members see.
fn sync_rows<T>(
    connection: &Connection,
    query: &str,
    order: (&str, &str, Option<&str>),
    seen: Option<&str>,
    ask: SyncAsk,
    row_of: fn(&Row<'_>) -> rusqlite::Result<T>,
) -> rusqlite::Result<Vec<T>> {
    let SyncAsk {
        account,
        chat,
        from,
        limit,
    } = ask;
    let (at, seq, n) = order;
    // SQLite reads a bare number in ORDER BY as a column's place, so a message orders without one.
    // A chat event's sequence id is the bare 0, so it orders by its `n` alone.
    let order_by = match (n, seq) {
        (Some(n), "0") => format!("{at}, conversation_id, {n}"),
        (Some(n), _) => format!("{at}, conversation_id, {seq}, {n}"),
        (None, _) => format!("{at}, conversation_id, {seq}"),
    };
    let n = n.unwrap_or("0");
    let scope = match chat {
        Some(_) => "conversation_id = ?1",
        None => "conversation_id IN (SELECT conversation_id FROM member WHERE account_id = ?1)",
    };
    let scope_id = match chat {
        Some(chat) => *chat.bytes(),
        None => *account.bytes(),
    };
    let seen = match seen {
        Some(seen) => format!(" AND {seen}"),
        None => String::new(),
    };
    let mut value_list: Vec<rusqlite::types::Value> = vec![scope_id.to_vec().into()];
    let query = match from {
        SyncFrom::Since(since) => {
            value_list.push(since.into());
            value_list.push(i64::from(limit).into());
            format!(
                "{query} WHERE {scope} AND {at} >= ?2{seen}
                 ORDER BY {order_by} LIMIT ?3"
            )
        }
        SyncFrom::After(position) => {
            value_list.push(position.at.into());
            value_list.push(position.chat.bytes().to_vec().into());
            value_list.push(int(position.seq).into());
            value_list.push(int(position.n).into());
            value_list.push(i64::from(limit).into());
            format!(
                "{query} WHERE {scope} AND ({at}, conversation_id, {seq}, {n}) > (?2, ?3, ?4, ?5)
                 {seen} ORDER BY {order_by} LIMIT ?6"
            )
        }
    };
    if !seen.is_empty() {
        while value_list.len() < 8 {
            value_list.push(rusqlite::types::Value::Null);
        }
        value_list.push(account.bytes().to_vec().into());
    }
    let mut statement = connection.prepare(&query)?;
    let mut row_list = Vec::new();
    for row in statement.query_map(rusqlite::params_from_iter(value_list), row_of)? {
        row_list.push(row?);
    }
    Ok(row_list)
}

fn account_row(row: &Row<'_>) -> rusqlite::Result<Account> {
    Ok(Account {
        id: Ulid::from_bytes(row.get(0)?),
        username: row.get(1)?,
        display_name: row.get(2)?,
        password_hash: row.get(3)?,
        master_public: row.get(4)?,
        account_public: row.get(5)?,
        account_stamp: row.get(6)?,
        profile_version: unsigned(row.get(7)?),
        invite_public: row.get(8)?,
        invite_stamp: row.get(9)?,
        owner: row.get::<_, Option<[u8; 16]>>(10)?.map(Ulid::from_bytes),
        disabled: row.get::<_, Option<i64>>(11)?.is_some(),
    })
}

fn put_profile_keys(
    connection: &Connection,
    chat: Ulid,
    epoch: u64,
    key_list: &[(Ulid, Vec<u8>)],
) -> rusqlite::Result<()> {
    for (account, sealed) in key_list {
        connection.execute(
            "INSERT INTO account_profile_key (account_id, conversation_id, epoch, sealed_key)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (account_id, conversation_id) DO UPDATE SET epoch = excluded.epoch,
             sealed_key = excluded.sealed_key WHERE excluded.epoch >= account_profile_key.epoch",
            params![account.bytes(), chat.bytes(), int(epoch), sealed],
        )?;
    }
    Ok(())
}

/// Deletes the rows of the account's photos except `keep`, giving them so their bytes can go.
fn take_photos(
    connection: &Connection,
    account: Ulid,
    keep: Option<Ulid>,
) -> rusqlite::Result<Vec<Ulid>> {
    let keep = keep.map(|photo| photo.bytes().to_vec());
    let mut statement = connection.prepare(
        "DELETE FROM file WHERE account_id = ?1 AND (?2 IS NULL OR id != ?2) RETURNING id",
    )?;
    let mut removed = Vec::new();
    for id in statement.query_map(params![account.bytes(), keep], |row| row.get(0))? {
        removed.push(Ulid::from_bytes(id?));
    }
    Ok(removed)
}

fn next_profile_version(connection: &Connection, account: Ulid) -> rusqlite::Result<u64> {
    let version: i64 = connection.query_row(
        "UPDATE account SET profile_version = profile_version + 1 WHERE id = ?1
         RETURNING profile_version",
        params![account.bytes()],
        |row| row.get(0),
    )?;
    Ok(unsigned(version))
}

fn usage(
    connection: &Connection,
    account: &Ulid,
    configured_limit: Option<u64>,
) -> rusqlite::Result<DeviceUsage> {
    let count: i64 = connection.query_row(
        "SELECT COUNT(*) FROM device WHERE account_id = ?1 AND terminated_at IS NULL",
        params![account.bytes()],
        |row| row.get(0),
    )?;
    if agent_of(connection, *account)?.is_some() {
        return Ok(DeviceUsage {
            count: unsigned(count),
            limit: Some(1),
        });
    }
    let by_role: Option<i64> = connection.query_row(
        "SELECT MAX(r.max_devices) FROM account_role a JOIN role r ON r.id = a.role_id
         WHERE a.account_id = ?1",
        params![account.bytes()],
        |row| row.get(0),
    )?;
    Ok(DeviceUsage {
        count: unsigned(count),
        limit: match by_role {
            Some(limit) => Some(unsigned(limit)),
            None => configured_limit,
        },
    })
}

/// An agent's ticked server permissions, written fresh.
fn put_ticks(
    connection: &Connection,
    agent: Ulid,
    permission_list: &[String],
) -> rusqlite::Result<()> {
    for permission in permission_list {
        connection.execute(
            "INSERT OR IGNORE INTO agent_permission (account_id, permission) VALUES (?1, ?2)",
            params![agent.bytes(), permission],
        )?;
    }
    Ok(())
}

/// Whether an account holds the built-in server admin role. An agent holds it only while it runs and
/// its owner holds it too.
fn admin_now(connection: &Connection, account: Ulid) -> rusqlite::Result<bool> {
    if !admin_held(connection, account)? {
        return Ok(false);
    }
    match agent_of(connection, account)? {
        None => Ok(true),
        Some((_, true)) => Ok(false),
        Some((owner, false)) => admin_held(connection, owner),
    }
}

fn admin_held(connection: &Connection, account: Ulid) -> rusqlite::Result<bool> {
    connection.query_row(
        "SELECT EXISTS (
             SELECT 1 FROM account_role a JOIN role r ON r.id = a.role_id
             WHERE a.account_id = ?1 AND r.scope = 0 AND r.built_in = 1 AND r.name = 'admin'
         )",
        params![account.bytes()],
        |row| row.get(0),
    )
}

/// An agent's owner and whether it is disabled, or `None` for a person.
fn agent_of(connection: &Connection, account: Ulid) -> rusqlite::Result<Option<(Ulid, bool)>> {
    let row: Option<(Option<[u8; 16]>, Option<i64>)> = connection
        .query_row(
            "SELECT owner_id, disabled_at FROM account WHERE id = ?1",
            params![account.bytes()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    Ok(match row {
        Some((Some(owner), disabled_at)) => Some((Ulid::from_bytes(owner), disabled_at.is_some())),
        _ => None,
    })
}

/// A person's server permission through their roles. The admin role holds them all.
fn person_permitted(
    connection: &Connection,
    account: Ulid,
    permission: &str,
) -> rusqlite::Result<bool> {
    connection.query_row(
        "SELECT EXISTS (
             SELECT 1 FROM account_role a JOIN role r ON r.id = a.role_id
             LEFT JOIN role_permission p ON p.role_id = r.id AND p.permission = ?2
             WHERE a.account_id = ?1
             AND ((r.built_in = 1 AND r.name = 'admin') OR p.permission IS NOT NULL)
         )",
        params![account.bytes(), permission],
        |row| row.get(0),
    )
}

impl Storage for Sqlite {
    async fn seed_roles(&self, role_list: Vec<BuiltInRole>) -> Result<()> {
        self.write(move |connection| {
            for role in &role_list {
                let exists: bool = connection.query_row(
                    "SELECT EXISTS (SELECT 1 FROM role WHERE scope = ?1 AND name = ?2
                     AND conversation_id IS NULL)",
                    params![role.scope, role.name],
                    |row| row.get(0),
                )?;
                if exists {
                    continue;
                }
                connection.execute(
                    "INSERT INTO role (id, scope, name, built_in) VALUES (?1, ?2, ?3, 1)",
                    params![role.id.bytes(), role.scope, role.name],
                )?;
                for permission in &role.permission_list {
                    connection.execute(
                        "INSERT INTO role_permission (role_id, permission) VALUES (?1, ?2)",
                        params![role.id.bytes(), permission],
                    )?;
                }
            }
            Ok(())
        })
        .await
    }

    async fn permission_names(&self) -> Result<Vec<String>> {
        self.read(|connection| {
            let mut statement =
                connection.prepare("SELECT DISTINCT permission FROM role_permission")?;
            let mut names = Vec::new();
            for name in statement.query_map([], |row| row.get(0))? {
                names.push(name?);
            }
            Ok(names)
        })
        .await
    }

    async fn create_account(&self, account: NewAccount) -> Result<bool> {
        self.write(move |connection| {
            let taken: bool = connection.query_row(
                "SELECT EXISTS (SELECT 1 FROM account WHERE username = ?1)",
                params![account.username],
                |row| row.get(0),
            )?;
            if taken {
                return Ok(false);
            }
            connection.execute(
                "INSERT INTO account (id, username, display_name, password_hash, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    account.id.bytes(),
                    account.username,
                    account.display_name,
                    account.password_hash,
                    account.now
                ],
            )?;
            connection.execute(
                "INSERT INTO account_role (account_id, role_id)
                 SELECT ?1, id FROM role WHERE scope = 0 AND built_in = 1 AND name = ?2",
                params![
                    account.id.bytes(),
                    if account.admin { "admin" } else { "member" }
                ],
            )?;
            connection.execute(
                "INSERT INTO folder (id, account_id, place, name, chats, pins, is_default)
                 VALUES (?1, ?2, 0, NULL, NULL, x'', 1)",
                params![account.all_folder.bytes(), account.id.bytes()],
            )?;
            if let Some((token_hash, expires_at)) = account.setup {
                connection.execute(
                    "INSERT INTO account_setup (token_hash, account_id, expires_at)
                     VALUES (?1, ?2, ?3)",
                    params![token_hash, account.id.bytes(), expires_at],
                )?;
            }
            Ok(true)
        })
        .await
    }

    async fn account_by_username(&self, username: String) -> Result<Option<Account>> {
        self.read(move |connection| {
            connection
                .query_row(
                    "SELECT id, username, display_name, password_hash, master_public_key,
                     account_public_key, account_key_stamp, profile_version, invite_public_key,
                     invite_key_stamp, owner_id, disabled_at
                     FROM account WHERE username = ?1",
                    params![username],
                    account_row,
                )
                .optional()
        })
        .await
    }

    async fn account_by_id(&self, id: Ulid) -> Result<Option<Account>> {
        self.read(move |connection| {
            connection
                .query_row(
                    "SELECT id, username, display_name, password_hash, master_public_key,
                     account_public_key, account_key_stamp, profile_version, invite_public_key,
                     invite_key_stamp, owner_id, disabled_at
                     FROM account WHERE id = ?1",
                    params![id.bytes()],
                    account_row,
                )
                .optional()
        })
        .await
    }

    async fn create_agent(&self, agent: NewAgent) -> Result<AgentCreated> {
        self.write(move |connection| {
            let taken: bool = connection.query_row(
                "SELECT EXISTS (SELECT 1 FROM account WHERE username = ?1)",
                params![agent.username],
                |row| row.get(0),
            )?;
            if taken {
                return Ok(AgentCreated::UsernameTaken);
            }
            let running: i64 = connection.query_row(
                "SELECT COUNT(*) FROM account WHERE owner_id = ?1 AND disabled_at IS NULL",
                params![agent.owner.bytes()],
                |row| row.get(0),
            )?;
            if unsigned(running) >= agent.limit {
                return Ok(AgentCreated::TooMany);
            }
            connection.execute(
                "INSERT INTO account (id, username, display_name, owner_id, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    agent.id.bytes(),
                    agent.username,
                    agent.display_name,
                    agent.owner.bytes(),
                    agent.now
                ],
            )?;
            if agent.admin {
                connection.execute(
                    "INSERT INTO account_role (account_id, role_id)
                     SELECT ?1, id FROM role WHERE scope = 0 AND built_in = 1 AND name = 'admin'",
                    params![agent.id.bytes()],
                )?;
            }
            put_ticks(connection, agent.id, &agent.permission_list)?;
            connection.execute(
                "INSERT INTO agent_token (account_id, jti, created_at) VALUES (?1, ?2, ?3)",
                params![agent.id.bytes(), agent.jti, agent.now],
            )?;
            connection.execute(
                "INSERT INTO folder (id, account_id, place, name, chats, pins, is_default)
                 VALUES (?1, ?2, 0, NULL, NULL, x'', 1)",
                params![agent.all_folder.bytes(), agent.id.bytes()],
            )?;
            Ok(AgentCreated::Saved)
        })
        .await
    }

    async fn agents(&self, owner: Ulid) -> Result<Vec<AgentItem>> {
        self.read(move |connection| {
            let mut statement = connection.prepare(
                "SELECT a.id, a.username, a.display_name, a.disabled_at, t.created_at,
                 (SELECT MAX(last_connected_at) FROM device WHERE account_id = a.id),
                 (SELECT id FROM device WHERE account_id = a.id AND terminated_at IS NULL)
                 FROM account a LEFT JOIN agent_token t ON t.account_id = a.id
                 WHERE a.owner_id = ?1 ORDER BY a.username",
            )?;
            let rows = statement.query_map(params![owner.bytes()], |row| {
                Ok(AgentItem {
                    id: Ulid::from_bytes(row.get(0)?),
                    username: row.get(1)?,
                    display_name: row.get(2)?,
                    disabled: row.get::<_, Option<i64>>(3)?.is_some(),
                    admin: false,
                    permission_list: Vec::new(),
                    token_at: row.get(4)?,
                    last_connected_at: row.get(5)?,
                    device: row.get::<_, Option<[u8; 16]>>(6)?.map(Ulid::from_bytes),
                })
            })?;
            let mut agent_list = Vec::new();
            for row in rows {
                agent_list.push(row?);
            }
            let mut ticks = connection.prepare(
                "SELECT permission FROM agent_permission WHERE account_id = ?1 ORDER BY permission",
            )?;
            for agent in &mut agent_list {
                agent.admin = admin_held(connection, agent.id)?;
                for name in ticks.query_map(params![agent.id.bytes()], |row| row.get(0))? {
                    agent.permission_list.push(name?);
                }
            }
            Ok(agent_list)
        })
        .await
    }

    async fn change_agent(&self, agent: Ulid, change: AgentChange) -> Result<()> {
        self.write(move |connection| {
            if let Some(permission_list) = &change.permission_list {
                connection.execute(
                    "DELETE FROM agent_permission WHERE account_id = ?1",
                    params![agent.bytes()],
                )?;
                put_ticks(connection, agent, permission_list)?;
            }
            if let Some(admin) = change.admin {
                connection.execute(
                    "DELETE FROM account_role WHERE account_id = ?1",
                    params![agent.bytes()],
                )?;
                if admin {
                    connection.execute(
                        "INSERT INTO account_role (account_id, role_id)
                         SELECT ?1, id FROM role
                         WHERE scope = 0 AND built_in = 1 AND name = 'admin'",
                        params![agent.bytes()],
                    )?;
                }
            }
            Ok(())
        })
        .await
    }

    async fn agent_jti(&self, agent: Ulid) -> Result<Option<[u8; 16]>> {
        self.read(move |connection| {
            connection
                .query_row(
                    "SELECT t.jti FROM agent_token t JOIN account a ON a.id = t.account_id
                     WHERE t.account_id = ?1 AND a.disabled_at IS NULL",
                    params![agent.bytes()],
                    |row| row.get(0),
                )
                .optional()
        })
        .await
    }

    async fn end_agent_token(
        &self,
        agent: Ulid,
        jti: Option<[u8; 16]>,
        disable: bool,
        now: i64,
    ) -> Result<Vec<Ulid>> {
        self.write(move |connection| {
            connection.execute(
                "DELETE FROM agent_token WHERE account_id = ?1",
                params![agent.bytes()],
            )?;
            if let Some(jti) = jti {
                connection.execute(
                    "INSERT INTO agent_token (account_id, jti, created_at) VALUES (?1, ?2, ?3)",
                    params![agent.bytes(), jti, now],
                )?;
            }
            connection.execute(
                "DELETE FROM login_session WHERE account_id = ?1",
                params![agent.bytes()],
            )?;
            if !disable {
                return Ok(Vec::new());
            }
            connection.execute(
                "UPDATE account SET disabled_at = ?2 WHERE id = ?1 AND disabled_at IS NULL",
                params![agent.bytes(), now],
            )?;
            let mut statement = connection.prepare(
                "SELECT m.conversation_id FROM member m
                 JOIN conversation c ON c.id = m.conversation_id
                 WHERE m.account_id = ?1 AND m.removed_seq IS NULL AND c.type IN (2, 3)",
            )?;
            let mut chat_list = Vec::new();
            for chat in statement.query_map(params![agent.bytes()], |row| row.get(0))? {
                chat_list.push(Ulid::from_bytes(chat?));
            }
            Ok(chat_list)
        })
        .await
    }

    async fn search_accounts(&self, term: String, limit: u32) -> Result<Vec<AccountBrief>> {
        self.read(move |connection| {
            let escaped = like_pattern(&term);
            let mut statement = connection.prepare(
                "SELECT id, username, display_name FROM account
                 WHERE username LIKE ?1 ESCAPE '\\' OR display_name LIKE ?1 ESCAPE '\\'
                 ORDER BY username LIMIT ?2",
            )?;
            let rows = statement.query_map(params![escaped, limit], |row| {
                Ok(AccountBrief {
                    id: Ulid::from_bytes(row.get(0)?),
                    username: row.get(1)?,
                    display_name: row.get(2)?,
                })
            })?;
            let mut account_list = Vec::new();
            for account in rows {
                account_list.push(account?);
            }
            Ok(account_list)
        })
        .await
    }

    async fn setup_link(&self, token_hash: [u8; 32], now: i64) -> Result<SetupLink> {
        self.read(move |connection| {
            let found: Option<([u8; 16], i64, Option<i64>)> = connection
                .query_row(
                    "SELECT account_id, expires_at, used_at FROM account_setup
                     WHERE token_hash = ?1",
                    params![token_hash],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()?;
            let Some((account, expires_at, used_at)) = found else {
                return Ok(SetupLink::Unknown);
            };
            if used_at.is_some() {
                return Ok(SetupLink::Used);
            }
            if expires_at < now {
                return Ok(SetupLink::Expired);
            }
            connection.query_row(
                "SELECT id, username, display_name FROM account WHERE id = ?1",
                params![account],
                |row| {
                    Ok(SetupLink::Ready(AccountBrief {
                        id: Ulid::from_bytes(row.get(0)?),
                        username: row.get(1)?,
                        display_name: row.get(2)?,
                    }))
                },
            )
        })
        .await
    }

    async fn finish_setup(
        &self,
        account: Ulid,
        token_hash: [u8; 32],
        password_hash: String,
        now: i64,
    ) -> Result<bool> {
        self.write(move |connection| {
            let changed = connection.execute(
                "UPDATE account_setup SET used_at = ?2
                 WHERE token_hash = ?1 AND used_at IS NULL AND expires_at >= ?2",
                params![token_hash, now],
            )?;
            if changed == 0 {
                return Ok(false);
            }
            connection.execute(
                "UPDATE account SET password_hash = ?2 WHERE id = ?1",
                params![account.bytes(), password_hash],
            )?;
            connection.execute(
                "DELETE FROM login_session WHERE account_id = ?1",
                params![account.bytes()],
            )?;
            Ok(true)
        })
        .await
    }

    async fn set_password(&self, account: Ulid, hash: String) -> Result<()> {
        self.write(move |connection| {
            connection.execute(
                "UPDATE account SET password_hash = ?2 WHERE id = ?1",
                params![account.bytes(), hash],
            )?;
            connection.execute(
                "DELETE FROM login_session WHERE account_id = ?1",
                params![account.bytes()],
            )?;
            Ok(())
        })
        .await
    }

    async fn set_account_keys(
        &self,
        account: Ulid,
        keys: NewAccountKeys,
        chat: Ulid,
        now: i64,
    ) -> Result<bool> {
        self.write(move |connection| {
            let changed = connection.execute(
                "UPDATE account SET master_public_key = ?2, account_public_key = ?3,
                 account_key_stamp = ?4, invite_public_key = ?5, invite_key_stamp = ?6,
                 words_set_at = ?7
                 WHERE id = ?1 AND master_public_key IS NULL",
                params![
                    account.bytes(),
                    keys.master_public,
                    keys.account_public,
                    keys.account_stamp,
                    keys.invite_public,
                    keys.invite_stamp,
                    now
                ],
            )?;
            if changed == 0 {
                return Ok(false);
            }
            connection.execute(
                "INSERT INTO conversation (id, type, direct_low, direct_high, created_at)
                 VALUES (?1, 0, ?2, ?2, ?3)",
                params![chat.bytes(), account.bytes(), now],
            )?;
            connection.execute(
                "INSERT INTO member (conversation_id, account_id, joined_seq) VALUES (?1, ?2, 0)",
                params![chat.bytes(), account.bytes()],
            )?;
            Ok(true)
        })
        .await
    }

    async fn create_challenge(
        &self,
        value: Vec<u8>,
        account: Ulid,
        purpose: i64,
        expires_at: i64,
        now: i64,
    ) -> Result<()> {
        self.write(move |connection| {
            connection.execute("DELETE FROM challenge WHERE expires_at < ?1", params![now])?;
            connection.execute(
                "INSERT INTO challenge (value, account_id, purpose, expires_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![value, account.bytes(), purpose, expires_at],
            )?;
            Ok(())
        })
        .await
    }

    async fn take_challenge(
        &self,
        value: Vec<u8>,
        account: Ulid,
        purpose: i64,
        now: i64,
    ) -> Result<bool> {
        self.write(move |connection| {
            let found: Option<i64> = connection
                .query_row(
                    "DELETE FROM challenge WHERE value = ?1 AND account_id = ?2 AND purpose = ?3
                     RETURNING expires_at",
                    params![value, account.bytes(), purpose],
                    |row| row.get(0),
                )
                .optional()?;
            match found {
                Some(expires_at) => Ok(expires_at >= now),
                None => Ok(false),
            }
        })
        .await
    }

    async fn create_captcha(&self, id: Ulid, answer: String, expires_at: i64) -> Result<()> {
        self.write(move |connection| {
            connection.execute(
                "INSERT INTO captcha (id, answer, expires_at) VALUES (?1, ?2, ?3)",
                params![id.bytes(), answer, expires_at],
            )?;
            Ok(())
        })
        .await
    }

    async fn take_captcha(&self, id: Ulid) -> Result<Option<(String, i64)>> {
        self.write(move |connection| {
            connection
                .query_row(
                    "DELETE FROM captcha WHERE id = ?1 RETURNING answer, expires_at",
                    params![id.bytes()],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
        })
        .await
    }

    async fn sweep_captchas(&self, expired_before: i64) -> Result<()> {
        self.write(move |connection| {
            connection.execute(
                "DELETE FROM captcha WHERE expires_at < ?1",
                params![expired_before],
            )?;
            Ok(())
        })
        .await
    }

    async fn create_session(&self, session: NewSession) -> Result<()> {
        self.write(move |connection| {
            connection.execute(
                "DELETE FROM login_session WHERE expires_at < ?1",
                params![session.now],
            )?;
            let device = session.device.map(|device| *device.bytes());
            connection.execute(
                "INSERT INTO login_session (id, token_hash, account_id, device_id, created_at,
                 last_used_at, expires_at) VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6)",
                params![
                    session.id.bytes(),
                    session.token_hash,
                    session.account.bytes(),
                    device,
                    session.now,
                    session.expires_at
                ],
            )?;
            Ok(())
        })
        .await
    }

    async fn session_by_token(
        &self,
        token_hash: [u8; 32],
        now: i64,
        expires_at: i64,
    ) -> Result<Option<Session>> {
        self.write(move |connection| {
            connection
                .query_row(
                    "UPDATE login_session SET last_used_at = ?2, expires_at = ?3
                     WHERE token_hash = ?1 AND expires_at >= ?2
                     RETURNING id, account_id, device_id",
                    params![token_hash, now, expires_at],
                    |row| {
                        Ok(Session {
                            id: Ulid::from_bytes(row.get(0)?),
                            account: Ulid::from_bytes(row.get(1)?),
                            device: opt_id(row.get(2)?),
                        })
                    },
                )
                .optional()
        })
        .await
    }

    async fn list_sessions(&self, account: Ulid) -> Result<Vec<SessionItem>> {
        self.read(move |connection| {
            let mut statement = connection.prepare(
                "SELECT id, device_id, created_at, last_used_at, expires_at FROM login_session
                 WHERE account_id = ?1 ORDER BY id",
            )?;
            let rows = statement.query_map(params![account.bytes()], |row| {
                Ok(SessionItem {
                    id: Ulid::from_bytes(row.get(0)?),
                    device: opt_id(row.get(1)?),
                    created_at: row.get(2)?,
                    last_used_at: row.get(3)?,
                    expires_at: row.get(4)?,
                })
            })?;
            let mut session_list = Vec::new();
            for session in rows {
                session_list.push(session?);
            }
            Ok(session_list)
        })
        .await
    }

    async fn delete_session(&self, account: Ulid, id: Ulid) -> Result<bool> {
        self.write(move |connection| {
            let changed = connection.execute(
                "DELETE FROM login_session WHERE id = ?1 AND account_id = ?2",
                params![id.bytes(), account.bytes()],
            )?;
            Ok(changed > 0)
        })
        .await
    }

    async fn is_admin(&self, account: Ulid) -> Result<bool> {
        self.read(move |connection| admin_now(connection, account))
            .await
    }

    async fn server_permitted(&self, account: Ulid, permission: &'static str) -> Result<bool> {
        self.read(move |connection| {
            let Some((owner, disabled)) = agent_of(connection, account)? else {
                return person_permitted(connection, account, permission);
            };
            if disabled || permission == "agent.create" {
                return Ok(false);
            }
            if admin_now(connection, account)? {
                return Ok(true);
            }
            let ticked: bool = connection.query_row(
                "SELECT EXISTS (SELECT 1 FROM agent_permission
                 WHERE account_id = ?1 AND permission = ?2)",
                params![account.bytes(), permission],
                |row| row.get(0),
            )?;
            Ok(ticked && person_permitted(connection, owner, permission)?)
        })
        .await
    }

    async fn chat_permitted(&self, chat: Ulid, account: Ulid, permission: String) -> Result<bool> {
        self.read(move |connection| {
            if admin_now(connection, account)? {
                return Ok(true);
            }
            connection.query_row(
                "SELECT EXISTS (
                     SELECT 1 FROM member m
                     JOIN role r ON r.scope = 1 AND r.built_in = 1 AND r.name = 'owner'
                     JOIN role_permission p ON p.role_id = r.id AND p.permission = ?3
                     WHERE m.conversation_id = ?1 AND m.account_id = ?2 AND m.is_owner = 1
                 ) OR EXISTS (
                     SELECT 1 FROM member_role m
                     JOIN role_permission p ON p.role_id = m.role_id AND p.permission = ?3
                     WHERE m.conversation_id = ?1 AND m.account_id = ?2
                 ) OR (
                     EXISTS (
                         SELECT 1 FROM member
                         WHERE conversation_id = ?1 AND account_id = ?2
                     ) AND COALESCE(
                         (SELECT allowed FROM chat_permission
                          WHERE conversation_id = ?1 AND permission = ?3),
                         EXISTS (
                             SELECT 1 FROM role r
                             JOIN role_permission p ON p.role_id = r.id AND p.permission = ?3
                             WHERE r.scope = 1 AND r.built_in = 1 AND r.name = 'member'
                         )
                     )
                 )",
                params![chat.bytes(), account.bytes(), permission],
                |row| row.get(0),
            )
        })
        .await
    }

    async fn device_usage(
        &self,
        account: Ulid,
        configured_limit: Option<u64>,
    ) -> Result<DeviceUsage> {
        self.read(move |connection| usage(connection, &account, configured_limit))
            .await
    }

    async fn register_device(
        &self,
        device: NewDevice,
        session: Ulid,
        configured_limit: Option<u64>,
    ) -> Result<Registered> {
        self.write(move |connection| {
            let found: Option<i64> = connection
                .query_row(
                    "DELETE FROM challenge WHERE value = ?1 AND account_id = ?2 AND purpose = 2
                     RETURNING expires_at",
                    params![device.id.bytes(), device.account.bytes()],
                    |row| row.get(0),
                )
                .optional()?;
            match found {
                Some(expires_at) if expires_at >= device.now => {}
                _ => return Ok(Registered::UnknownId),
            }
            let current = usage(connection, &device.account, configured_limit)?;
            if let Some(limit) = current.limit
                && current.count >= limit
            {
                return Ok(Registered::AtLimit(limit));
            }
            connection.execute(
                "INSERT INTO device (id, account_id, signing_public_key, master_stamp, platform,
                 os, os_version, web_platform, web_platform_version, first_ip, last_ip, created_at,
                 last_active_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10, ?11, ?11)",
                params![
                    device.id.bytes(),
                    device.account.bytes(),
                    device.public_key,
                    device.stamp,
                    device.platform,
                    device.os,
                    device.os_version,
                    device.web_platform,
                    device.web_platform_version,
                    device.ip,
                    device.now
                ],
            )?;
            connection.execute(
                "UPDATE login_session SET device_id = ?2 WHERE id = ?1",
                params![session.bytes(), device.id.bytes()],
            )?;
            let (chat, seq): ([u8; 16], i64) = connection.query_row(
                // The notice is the self chat's last content, so a later commit never hides it.
                "UPDATE conversation SET last_seq = last_seq + 1, last_kind = 4, last_at = ?2,
                 last_content_seq = last_seq + 1 WHERE type = 0 AND direct_low = ?1
                 RETURNING id, last_seq",
                params![device.account.bytes(), device.now],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            connection.execute(
                "INSERT INTO message (conversation_id, seq, device_id, kind, encrypted,
                 client_sent_at, server_saved_at, signature, body)
                 VALUES (?1, ?2, ?3, 4, 0, ?4, ?4, ?5, x'')",
                params![chat, seq, device.id.bytes(), device.now, device.stamp],
            )?;
            Ok(Registered::Saved(Box::new(MessageFrame {
                chat: Ulid::from_bytes(chat),
                seq: unsigned(seq),
                device: Some(device.id),
                kind: Some(Kind::DeviceNotice),
                content_kind: None,
                encrypted: Some(false),
                epoch: None,
                pointer_seq: None,
                topic: None,
                thread_seq: None,
                client_sent_at: Some(device.now),
                server_saved_at: Some(device.now),
                body: Some(Bytes(Vec::new())),
                signature: Some(Bytes(device.stamp)),
            })))
        })
        .await
    }

    async fn device_key(&self, id: Ulid) -> Result<Option<DeviceKey>> {
        self.read(move |connection| {
            connection
                .query_row(
                    "SELECT account_id, signing_public_key, terminated_at IS NOT NULL
                     FROM device WHERE id = ?1",
                    params![id.bytes()],
                    |row| {
                        Ok(DeviceKey {
                            account: Ulid::from_bytes(row.get(0)?),
                            public_key: row.get(1)?,
                            ended: row.get(2)?,
                        })
                    },
                )
                .optional()
        })
        .await
    }

    async fn list_devices(&self, account: Ulid) -> Result<Vec<DeviceItem>> {
        self.read(move |connection| {
            let mut devices = connection.prepare(
                "SELECT id, platform, os, os_version, web_platform, web_platform_version, first_ip,
                 last_ip, created_at, last_active_at, terminated_at FROM device
                 WHERE account_id = ?1 ORDER BY id",
            )?;
            let mut logs = connection.prepare(
                "SELECT online_at, offline_at, ip FROM device_session WHERE device_id = ?1
                 ORDER BY online_at",
            )?;
            let rows = devices.query_map(params![account.bytes()], |row| {
                Ok(DeviceItem {
                    id: Ulid::from_bytes(row.get(0)?),
                    platform: row.get(1)?,
                    os: row.get(2)?,
                    os_version: row.get(3)?,
                    web_platform: row.get(4)?,
                    web_platform_version: row.get(5)?,
                    first_ip: row.get(6)?,
                    last_ip: row.get(7)?,
                    created_at: row.get(8)?,
                    last_active_at: row.get(9)?,
                    terminated_at: row.get(10)?,
                    log_list: Vec::new(),
                })
            })?;
            let mut device_list = Vec::new();
            for device in rows {
                let mut device = device?;
                let log_rows = logs.query_map(params![device.id.bytes()], |row| {
                    Ok(DeviceLogItem {
                        online_at: row.get(0)?,
                        offline_at: row.get(1)?,
                        ip: row.get(2)?,
                    })
                })?;
                for item in log_rows {
                    device.log_list.push(item?);
                }
                device_list.push(device);
            }
            Ok(device_list)
        })
        .await
    }

    async fn end_device(&self, id: Ulid, statement: Vec<u8>, now: i64) -> Result<bool> {
        self.write(move |connection| {
            let changed = connection.execute(
                "UPDATE device SET terminated_at = ?2, revocation = ?3
                 WHERE id = ?1 AND terminated_at IS NULL",
                params![id.bytes(), now, statement],
            )?;
            if changed == 0 {
                return Ok(false);
            }
            connection.execute(
                "DELETE FROM login_session WHERE device_id = ?1",
                params![id.bytes()],
            )?;
            connection.execute(
                "DELETE FROM key_package WHERE device_id = ?1",
                params![id.bytes()],
            )?;
            Ok(true)
        })
        .await
    }

    async fn accounts_sharing(&self, account: Ulid) -> Result<Vec<Ulid>> {
        self.read(move |connection| {
            let mut statement = connection.prepare(
                "SELECT DISTINCT other.account_id FROM member mine
                 JOIN member other ON other.conversation_id = mine.conversation_id
                 WHERE mine.account_id = ?1",
            )?;
            let mut account_list = Vec::new();
            for id in statement.query_map(params![account.bytes()], |row| row.get(0))? {
                account_list.push(Ulid::from_bytes(id?));
            }
            Ok(account_list)
        })
        .await
    }

    async fn device_online(&self, id: Ulid, ip: String, now: i64, grace_ms: i64) -> Result<()> {
        self.write(move |connection| {
            connection.execute(
                "UPDATE device SET last_ip = ?2, last_active_at = ?3 WHERE id = ?1",
                params![id.bytes(), ip, now],
            )?;
            let last: Option<(i64, Option<i64>)> = connection
                .query_row(
                    "SELECT online_at, offline_at FROM device_session WHERE device_id = ?1
                     ORDER BY online_at DESC LIMIT 1",
                    params![id.bytes()],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            if let Some((online_at, offline_at)) = last {
                let recent = match offline_at {
                    Some(offline_at) => now - offline_at <= grace_ms,
                    None => true,
                };
                if recent {
                    connection.execute(
                        "UPDATE device_session SET offline_at = NULL
                         WHERE device_id = ?1 AND online_at = ?2",
                        params![id.bytes(), online_at],
                    )?;
                    return Ok(());
                }
            }
            connection.execute(
                "INSERT INTO device_session (device_id, online_at, ip) VALUES (?1, ?2, ?3)",
                params![id.bytes(), now, ip],
            )?;
            Ok(())
        })
        .await
    }

    async fn device_offline(&self, id: Ulid, now: i64) -> Result<()> {
        self.write(move |connection| {
            connection.execute(
                "UPDATE device SET last_active_at = ?2 WHERE id = ?1",
                params![id.bytes(), now],
            )?;
            connection.execute(
                "UPDATE device_session SET offline_at = ?2
                 WHERE device_id = ?1 AND offline_at IS NULL",
                params![id.bytes(), now],
            )?;
            Ok(())
        })
        .await
    }

    async fn device_presence(
        &self,
        id: Ulid,
        connected_at: Option<i64>,
        in_app_at: Option<i64>,
    ) -> Result<()> {
        self.write(move |connection| {
            connection.execute(
                "UPDATE device SET last_connected_at = coalesce(?2, last_connected_at),
                 last_in_app_at = coalesce(?3, last_in_app_at) WHERE id = ?1",
                params![id.bytes(), connected_at, in_app_at],
            )?;
            Ok(())
        })
        .await
    }

    async fn presence_times(&self, account_list: Vec<Ulid>) -> Result<Vec<PresenceTimes>> {
        self.read(move |connection| {
            let mut statement = connection.prepare(
                "SELECT max(last_connected_at), max(last_in_app_at) FROM device
                 WHERE account_id = ?1",
            )?;
            let mut times = Vec::with_capacity(account_list.len());
            for account in account_list {
                let (connected_at, in_app_at) = statement
                    .query_row(params![account.bytes()], |row| {
                        Ok((row.get(0)?, row.get(1)?))
                    })?;
                times.push(PresenceTimes {
                    account,
                    connected_at,
                    in_app_at,
                });
            }
            Ok(times)
        })
        .await
    }

    async fn kv(&self, account: Ulid, known: Option<u64>) -> Result<KvReply> {
        self.read(move |connection| kv_of(connection, account, known))
            .await
    }

    async fn change_kv(&self, account: Ulid, change: KvChange, max_keys: u64) -> Result<KvChanged> {
        self.write(move |connection| {
            let version: i64 = connection.query_row(
                "SELECT kv_version FROM account WHERE id = ?1",
                params![account.bytes()],
                |row| row.get(0),
            )?;
            let version = u64::try_from(version).unwrap_or(0);
            if version != change.version {
                return Ok(KvChanged::Stale(kv_of(connection, account, None)?));
            }
            let mut key_set: HashSet<String> = HashSet::new();
            let mut statement = connection.prepare("SELECT key FROM kv WHERE account_id = ?1")?;
            for key in statement.query_map(params![account.bytes()], |row| row.get(0))? {
                key_set.insert(key?);
            }
            for key in &change.remove {
                key_set.remove(key);
            }
            for item in &change.set {
                key_set.insert(item.key.clone());
            }
            if u64::try_from(key_set.len()).unwrap_or(u64::MAX) > max_keys {
                return Ok(KvChanged::TooMany);
            }
            for key in &change.remove {
                connection.execute(
                    "DELETE FROM kv WHERE account_id = ?1 AND key = ?2",
                    params![account.bytes(), key],
                )?;
            }
            for item in &change.set {
                connection.execute(
                    "INSERT INTO kv (account_id, key, value) VALUES (?1, ?2, ?3)
                     ON CONFLICT (account_id, key) DO UPDATE SET value = excluded.value",
                    params![account.bytes(), item.key, item.value.0],
                )?;
            }
            let version = version + 1;
            connection.execute(
                "UPDATE account SET kv_version = ?2 WHERE id = ?1",
                params![account.bytes(), int(version)],
            )?;
            Ok(KvChanged::Saved(version))
        })
        .await
    }

    async fn close_device_logs(&self, now: i64) -> Result<()> {
        self.write(move |connection| {
            connection.execute(
                "UPDATE device_session SET offline_at = ?1 WHERE offline_at IS NULL",
                params![now],
            )?;
            Ok(())
        })
        .await
    }

    async fn add_key_packages(
        &self,
        device: Ulid,
        package_list: Vec<(Ulid, Vec<u8>)>,
        limit: u64,
        now: i64,
    ) -> Result<Option<u64>> {
        self.write(move |connection| {
            let stored: i64 = connection.query_row(
                "SELECT COUNT(*) FROM key_package WHERE device_id = ?1",
                params![device.bytes()],
                |row| row.get(0),
            )?;
            let total = unsigned(stored) + package_list.len() as u64;
            if total > limit {
                return Ok(None);
            }
            for (id, body) in &package_list {
                connection.execute(
                    "INSERT INTO key_package (id, device_id, body, created_at)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![id.bytes(), device.bytes(), body, now],
                )?;
            }
            Ok(Some(total))
        })
        .await
    }

    async fn claim_key_packages(&self, account: Ulid) -> Result<Vec<KeyPackageItem>> {
        self.write(move |connection| {
            let mut devices = connection
                .prepare("SELECT id FROM device WHERE account_id = ?1 AND terminated_at IS NULL")?;
            let mut device_list: Vec<[u8; 16]> = Vec::new();
            for device in devices.query_map(params![account.bytes()], |row| row.get(0))? {
                device_list.push(device?);
            }
            let mut package_list = Vec::with_capacity(device_list.len());
            for device in device_list {
                let body: Option<Vec<u8>> = connection
                    .query_row(
                        "DELETE FROM key_package WHERE id = (
                             SELECT id FROM key_package WHERE device_id = ?1 ORDER BY id LIMIT 1
                         ) RETURNING body",
                        params![device],
                        |row| row.get(0),
                    )
                    .optional()?;
                if let Some(body) = body {
                    package_list.push(KeyPackageItem {
                        device: Ulid::from_bytes(device),
                        body: Bytes(body),
                    });
                }
            }
            Ok(package_list)
        })
        .await
    }

    async fn direct_chat(&self, id: Ulid, low: Ulid, high: Ulid, now: i64) -> Result<(Ulid, bool)> {
        self.write(move |connection| {
            let existing: Option<[u8; 16]> = connection
                .query_row(
                    "SELECT id FROM conversation WHERE direct_low = ?1 AND direct_high = ?2",
                    params![low.bytes(), high.bytes()],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(existing) = existing {
                return Ok((Ulid::from_bytes(existing), false));
            }
            connection.execute(
                "INSERT INTO conversation (id, type, direct_low, direct_high, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    id.bytes(),
                    if low == high { 0 } else { 1 },
                    low.bytes(),
                    high.bytes(),
                    now
                ],
            )?;
            connection.execute(
                "INSERT OR IGNORE INTO member (conversation_id, account_id, joined_seq)
                 VALUES (?1, ?2, 0), (?1, ?3, 0)",
                params![id.bytes(), low.bytes(), high.bytes()],
            )?;
            Ok((id, true))
        })
        .await
    }

    async fn create_group(&self, group: NewGroup) -> Result<(ChatEvent, Option<MessageFrame>)> {
        self.write(move |connection| {
            let (findable, find_name, find_bio) = match group.findable {
                Some(findable) => (true, Some(findable.name), findable.bio),
                None => (false, None, None),
            };
            connection.execute(
                "INSERT INTO conversation (id, type, topics_on, findable, find_name, find_bio,
                 created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    group.id.bytes(),
                    u8::from(group.chat_type),
                    group.topics_on,
                    findable,
                    find_name,
                    find_bio,
                    group.now
                ],
            )?;
            connection.execute(
                "INSERT INTO member (conversation_id, account_id, joined_seq, is_owner)
                 VALUES (?1, ?2, 0, 1)",
                params![group.id.bytes(), group.owner.bytes()],
            )?;
            // General's own settings live in a topic row under the all-zero id.
            connection.execute(
                &format!(
                    "INSERT INTO topic (conversation_id, id, issued_at) VALUES (?1, {GENERAL_SQL}, ?2)"
                ),
                params![group.id.bytes(), group.now],
            )?;
            let created = event(
                group.id,
                group.owner,
                ChatAction::Created,
                None,
                Audience::Moderators,
                group.now,
            );
            let created = write_event(connection, created, None)?;
            let line = write_line(
                connection,
                group.id,
                group.device,
                LineAction::Created,
                Vec::new(),
                group.now,
            )?;
            Ok((created, line))
        })
        .await
    }

    async fn member_roles(&self, chat: Ulid) -> Result<Vec<(Ulid, ChatRole)>> {
        self.read(move |connection| {
            let mut statement = connection.prepare(
                "SELECT m.account_id, m.is_owner,
                 EXISTS (SELECT 1 FROM member_role r WHERE r.conversation_id = m.conversation_id
                         AND r.account_id = m.account_id)
                 FROM member m WHERE m.conversation_id = ?1",
            )?;
            let rows = statement.query_map(params![chat.bytes()], |row| {
                let owner: bool = row.get(1)?;
                let moderator: bool = row.get(2)?;
                Ok((
                    Ulid::from_bytes(row.get(0)?),
                    if owner {
                        ChatRole::Owner
                    } else if moderator {
                        ChatRole::Moderator
                    } else {
                        ChatRole::Member
                    },
                ))
            })?;
            let mut role_list = Vec::new();
            for row in rows {
                role_list.push(row?);
            }
            Ok(role_list)
        })
        .await
    }

    async fn leave_chat(&self, chat: Ulid, account: Ulid, device: Ulid, now: i64) -> Result<Left> {
        self.write(move |connection| {
            let owner: Option<bool> = connection
                .query_row(
                    "SELECT is_owner FROM member WHERE conversation_id = ?1 AND account_id = ?2",
                    params![chat.bytes(), account.bytes()],
                    |row| row.get(0),
                )
                .optional()?;
            let Some(owner) = owner else {
                return Ok(Left::NotMember);
            };
            let owners: i64 = connection.query_row(
                "SELECT COUNT(*) FROM member WHERE conversation_id = ?1 AND is_owner = 1",
                params![chat.bytes()],
                |row| row.get(0),
            )?;
            // The last owner leaving removes the chat for everyone, so a group always has an owner.
            if owner && owners == 1 {
                return match remove_chat_rows(connection, chat)? {
                    Some(removed) => Ok(Left::Removed(removed)),
                    None => Ok(Left::NotMember),
                };
            }
            connection.execute(
                "DELETE FROM member WHERE conversation_id = ?1 AND account_id = ?2",
                params![chat.bytes(), account.bytes()],
            )?;
            drop_from_folders(connection, chat, account)?;
            let left = event(
                chat,
                account,
                ChatAction::Left,
                Some(account),
                Audience::Moderators,
                now,
            );
            let left = write_event(connection, left, None)?;
            let line = write_line(connection, chat, device, LineAction::Left, Vec::new(), now)?;
            let mut statement =
                connection.prepare("SELECT account_id FROM member WHERE conversation_id = ?1")?;
            let mut member_list = Vec::new();
            for member in statement.query_map(params![chat.bytes()], |row| row.get(0))? {
                member_list.push(Ulid::from_bytes(member?));
            }
            Ok(Left::Left {
                event: Box::new(left),
                line: line.map(Box::new),
                member_list,
            })
        })
        .await
    }

    async fn set_role(
        &self,
        chat: Ulid,
        actor: Ulid,
        device: Ulid,
        account: Ulid,
        role: ChatRole,
        now: i64,
    ) -> Result<RoleSet> {
        self.write(move |connection| {
            let owner: Option<bool> = connection
                .query_row(
                    "SELECT is_owner FROM member WHERE conversation_id = ?1 AND account_id = ?2",
                    params![chat.bytes(), account.bytes()],
                    |row| row.get(0),
                )
                .optional()?;
            let Some(owner) = owner else {
                return Ok(RoleSet::NotMember);
            };
            let owners: i64 = connection.query_row(
                "SELECT COUNT(*) FROM member WHERE conversation_id = ?1 AND is_owner = 1",
                params![chat.bytes()],
                |row| row.get(0),
            )?;
            if owner && owners == 1 && role != ChatRole::Owner {
                return Ok(RoleSet::LastOwner);
            }
            connection.execute(
                "UPDATE member SET is_owner = ?3 WHERE conversation_id = ?1 AND account_id = ?2",
                params![chat.bytes(), account.bytes(), role == ChatRole::Owner],
            )?;
            connection.execute(
                "DELETE FROM member_role WHERE conversation_id = ?1 AND account_id = ?2",
                params![chat.bytes(), account.bytes()],
            )?;
            if role == ChatRole::Moderator {
                connection.execute(
                    "INSERT INTO member_role (conversation_id, account_id, role_id)
                     SELECT ?1, ?2, id FROM role WHERE scope = 1 AND built_in = 1
                     AND name = 'moderator' AND conversation_id IS NULL",
                    params![chat.bytes(), account.bytes()],
                )?;
            }
            let (action, said) = match role {
                ChatRole::Owner => (ChatAction::MadeOwner, LineAction::MadeOwner),
                ChatRole::Moderator => (ChatAction::MadeModerator, LineAction::MadeModerator),
                ChatRole::Member => (ChatAction::MadeMember, LineAction::MadeMember),
            };
            let history = event(
                chat,
                actor,
                action,
                Some(account),
                Audience::Moderators,
                now,
            );
            let history = write_event(connection, history, None)?;
            let line = write_line(connection, chat, device, said, vec![account], now)?;
            let mut statement =
                connection.prepare("SELECT account_id FROM member WHERE conversation_id = ?1")?;
            let mut member_list = Vec::new();
            for member in statement.query_map(params![chat.bytes()], |row| row.get(0))? {
                member_list.push(Ulid::from_bytes(member?));
            }
            Ok(RoleSet::Set {
                event: Box::new(history),
                line: line.map(Box::new),
                member_list,
            })
        })
        .await
    }

    async fn remove_chat(&self, chat: Ulid) -> Result<Option<Removed>> {
        self.write(move |connection| remove_chat_rows(connection, chat))
            .await
    }

    async fn ask_to_join(
        &self,
        chat: Ulid,
        account: Ulid,
        note: Option<String>,
        now: i64,
        expire_before: i64,
    ) -> Result<Asked> {
        self.write(move |connection| {
            let name: Option<Option<String>> = connection
                .query_row(
                    "SELECT find_name FROM conversation WHERE id = ?1 AND findable = 1
                     AND type IN (2, 3)",
                    params![chat.bytes()],
                    |row| row.get(0),
                )
                .optional()?;
            let Some(name) = name else {
                return Ok(Asked::NotFindable);
            };
            let member: bool = connection.query_row(
                "SELECT EXISTS (SELECT 1 FROM member WHERE conversation_id = ?1
                 AND account_id = ?2)",
                params![chat.bytes(), account.bytes()],
                |row| row.get(0),
            )?;
            if member {
                return Ok(Asked::AlreadyMember);
            }
            // A waiting request stays as it was asked; a declined or expired one is asked anew.
            connection.execute(
                "DELETE FROM join_request WHERE conversation_id = ?1 AND account_id = ?2
                 AND (state = 1 OR at < ?3)",
                params![chat.bytes(), account.bytes(), expire_before],
            )?;
            connection.execute(
                "INSERT OR IGNORE INTO join_request (conversation_id, account_id, note, state, at)
                 VALUES (?1, ?2, ?3, 0, ?4)",
                params![chat.bytes(), account.bytes(), note, now],
            )?;
            let (note, at): (Option<String>, i64) = connection.query_row(
                "SELECT note, at FROM join_request WHERE conversation_id = ?1 AND account_id = ?2",
                params![chat.bytes(), account.bytes()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            Ok(Asked::Saved(Box::new(RequestItem {
                chat,
                account,
                name,
                note,
                state: RequestState::Waiting,
                at,
            })))
        })
        .await
    }

    async fn cancel_request(&self, chat: Ulid, account: Ulid) -> Result<bool> {
        self.write(move |connection| {
            let removed = connection.execute(
                "DELETE FROM join_request WHERE conversation_id = ?1 AND account_id = ?2",
                params![chat.bytes(), account.bytes()],
            )?;
            Ok(removed > 0)
        })
        .await
    }

    async fn decline_request(
        &self,
        chat: Ulid,
        account: Ulid,
        now: i64,
        expire_before: i64,
    ) -> Result<Option<RequestItem>> {
        self.write(move |connection| {
            let changed = connection.execute(
                "UPDATE join_request SET state = 1, at = ?3 WHERE conversation_id = ?1
                 AND account_id = ?2 AND state = 0 AND at >= ?4",
                params![chat.bytes(), account.bytes(), now, expire_before],
            )?;
            if changed == 0 {
                return Ok(None);
            }
            connection
                .query_row(
                    &format!(
                        "SELECT {REQUEST_COLUMNS} FROM join_request r
                         JOIN conversation c ON c.id = r.conversation_id
                         WHERE r.conversation_id = ?1 AND r.account_id = ?2"
                    ),
                    params![chat.bytes(), account.bytes()],
                    request_row,
                )
                .optional()
        })
        .await
    }

    async fn chat_requests(&self, chat: Ulid, expire_before: i64) -> Result<Vec<RequestItem>> {
        self.read(move |connection| {
            let mut statement = connection.prepare(&format!(
                "SELECT {REQUEST_COLUMNS} FROM join_request r
                 JOIN conversation c ON c.id = r.conversation_id
                 WHERE r.conversation_id = ?1 AND r.state = 0 AND r.at >= ?2 ORDER BY r.at"
            ))?;
            let mut request_list = Vec::new();
            for row in statement.query_map(params![chat.bytes(), expire_before], request_row)? {
                request_list.push(row?);
            }
            Ok(request_list)
        })
        .await
    }

    async fn own_requests(&self, account: Ulid, expire_before: i64) -> Result<Vec<RequestItem>> {
        self.read(move |connection| {
            let mut statement = connection.prepare(&format!(
                "SELECT {REQUEST_COLUMNS} FROM join_request r
                 JOIN conversation c ON c.id = r.conversation_id
                 WHERE r.account_id = ?1 AND r.at >= ?2 ORDER BY r.at"
            ))?;
            let mut request_list = Vec::new();
            for row in statement.query_map(params![account.bytes(), expire_before], request_row)? {
                request_list.push(row?);
            }
            Ok(request_list)
        })
        .await
    }

    async fn invite(&self, invitation: NewInvitation) -> Result<bool> {
        self.write(move |connection| {
            let member: bool = connection.query_row(
                "SELECT EXISTS (SELECT 1 FROM member WHERE conversation_id = ?1
                 AND account_id = ?2)",
                params![invitation.chat.bytes(), invitation.account.bytes()],
                |row| row.get(0),
            )?;
            if member {
                return Ok(false);
            }
            connection.execute(
                "INSERT OR REPLACE INTO invitation (conversation_id, account_id, inviter, sealed,
                 signature, at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    invitation.chat.bytes(),
                    invitation.account.bytes(),
                    invitation.inviter.bytes(),
                    invitation.sealed,
                    invitation.signature,
                    invitation.now
                ],
            )?;
            Ok(true)
        })
        .await
    }

    async fn answer_invitation(
        &self,
        chat: Ulid,
        account: Ulid,
        accept: Option<Ulid>,
        limit: u64,
        now: i64,
        expire_before: i64,
    ) -> Result<Answered> {
        self.write(move |connection| {
            let invitation = connection
                .query_row(
                    &format!(
                        "SELECT {INVITATION_COLUMNS} FROM invitation i
                         JOIN conversation c ON c.id = i.conversation_id
                         JOIN device d ON d.id = i.inviter
                         WHERE i.conversation_id = ?1 AND i.account_id = ?2 AND i.at >= ?3"
                    ),
                    params![chat.bytes(), account.bytes(), expire_before],
                    invitation_row,
                )
                .optional()?;
            let Some(invitation) = invitation else {
                return Ok(Answered::NoInvitation);
            };
            if accept.is_some() {
                let count: i64 = connection.query_row(
                    "SELECT COUNT(*) FROM member WHERE conversation_id = ?1",
                    params![chat.bytes()],
                    |row| row.get(0),
                )?;
                if unsigned(count) >= limit {
                    return Ok(Answered::Full);
                }
            }
            connection.execute(
                "DELETE FROM invitation WHERE conversation_id = ?1 AND account_id = ?2",
                params![chat.bytes(), account.bytes()],
            )?;
            let (history, line) = if let Some(device) = accept {
                let last_seq: i64 = connection.query_row(
                    "SELECT last_seq FROM conversation WHERE id = ?1",
                    params![chat.bytes()],
                    |row| row.get(0),
                )?;
                connection.execute(
                    "INSERT OR IGNORE INTO member (conversation_id, account_id, joined_seq)
                     VALUES (?1, ?2, ?3)",
                    params![chat.bytes(), account.bytes(), last_seq + 1],
                )?;
                connection.execute(
                    "DELETE FROM join_request WHERE conversation_id = ?1 AND account_id = ?2",
                    params![chat.bytes(), account.bytes()],
                )?;
                let joined = event(
                    chat,
                    account,
                    ChatAction::Joined,
                    Some(invitation.inviter),
                    Audience::Moderators,
                    now,
                );
                (
                    write_event(connection, joined, None)?,
                    write_line(
                        connection,
                        chat,
                        device,
                        LineAction::Joined,
                        Vec::new(),
                        now,
                    )?,
                )
            } else {
                // Only the inviter sees a decline.
                let declined = event(
                    chat,
                    account,
                    ChatAction::InvitationDeclined,
                    Some(account),
                    Audience::One,
                    now,
                );
                (
                    write_event(connection, declined, Some(invitation.inviter))?,
                    None,
                )
            };
            Ok(Answered::Saved {
                invitation: Box::new(invitation),
                event: Box::new(history),
                line: line.map(Box::new),
            })
        })
        .await
    }

    async fn own_invitations(
        &self,
        account: Ulid,
        expire_before: i64,
    ) -> Result<Vec<InvitationItem>> {
        self.read(move |connection| {
            let mut statement = connection.prepare(&format!(
                "SELECT {INVITATION_COLUMNS} FROM invitation i
                 JOIN conversation c ON c.id = i.conversation_id
                 JOIN device d ON d.id = i.inviter
                 WHERE i.account_id = ?1 AND i.at >= ?2 ORDER BY i.at"
            ))?;
            let mut invitation_list = Vec::new();
            let rows =
                statement.query_map(params![account.bytes(), expire_before], invitation_row)?;
            for row in rows {
                invitation_list.push(row?);
            }
            Ok(invitation_list)
        })
        .await
    }

    async fn issue_topic(
        &self,
        chat: Ulid,
        account: Ulid,
        topic: Ulid,
        expires_at: i64,
    ) -> Result<()> {
        self.write(move |connection| {
            connection.execute(
                "DELETE FROM challenge WHERE expires_at < ?1",
                params![Ulid::now()],
            )?;
            connection.execute(
                "INSERT INTO challenge (value, account_id, purpose, conversation_id, expires_at)
                 VALUES (?1, ?2, 4, ?3, ?4)",
                params![topic.bytes(), account.bytes(), chat.bytes(), expires_at],
            )?;
            Ok(())
        })
        .await
    }

    async fn put_topics(
        &self,
        chat: Ulid,
        account: Ulid,
        on: bool,
        topic_list: Vec<Ulid>,
        max: u64,
        now: i64,
    ) -> Result<TopicsPut> {
        self.write(move |connection| {
            if topic_list.len() as u64 > max {
                return Ok(TopicsPut::TooMany);
            }
            let mut statement = connection.prepare(&format!(
                "SELECT id FROM topic WHERE conversation_id = ?1 AND removed = 0
                     AND id != {GENERAL_SQL}"
            ))?;
            let mut live = Vec::new();
            for topic in statement.query_map(params![chat.bytes()], |row| row.get(0))? {
                live.push(Ulid::from_bytes(topic?));
            }
            let mut issued = Vec::new();
            for topic in &topic_list {
                if live.contains(topic) {
                    continue;
                }
                let expires_at: Option<i64> = connection
                    .query_row(
                        "SELECT expires_at FROM challenge WHERE value = ?1 AND account_id = ?2
                         AND purpose = 4 AND conversation_id = ?3",
                        params![topic.bytes(), account.bytes(), chat.bytes()],
                        |row| row.get(0),
                    )
                    .optional()?;
                match expires_at {
                    Some(expires_at) if expires_at >= now => issued.push(*topic),
                    _ => return Ok(TopicsPut::UnknownTopic),
                }
            }
            let mut event_list = Vec::new();
            let was_on: bool = connection.query_row(
                "SELECT topics_on FROM conversation WHERE id = ?1",
                params![chat.bytes()],
                |row| row.get(0),
            )?;
            if was_on != on {
                connection.execute(
                    "UPDATE conversation SET topics_on = ?2 WHERE id = ?1",
                    params![chat.bytes(), on],
                )?;
                let action = if on {
                    ChatAction::TopicsOn
                } else {
                    ChatAction::TopicsOff
                };
                let changed = event(chat, account, action, None, Audience::Moderators, now);
                event_list.push(write_event(connection, changed, None)?);
            }
            for topic in issued {
                connection.execute(
                    "DELETE FROM challenge WHERE value = ?1",
                    params![topic.bytes()],
                )?;
                connection.execute(
                    "INSERT INTO topic (conversation_id, id, issued_at) VALUES (?1, ?2, ?3)",
                    params![chat.bytes(), topic.bytes(), now],
                )?;
                let added = event(
                    chat,
                    account,
                    ChatAction::TopicAdded,
                    Some(topic),
                    Audience::Moderators,
                    now,
                );
                event_list.push(write_event(connection, added, None)?);
            }
            let mut wiped = false;
            for topic in live {
                if topic_list.contains(&topic) {
                    continue;
                }
                connection.execute(
                    "UPDATE topic SET removed = 1 WHERE conversation_id = ?1 AND id = ?2",
                    params![chat.bytes(), topic.bytes()],
                )?;
                // Wiped as a delete wipes: the topic's messages, their threads, and their edits.
                connection.execute(
                    "UPDATE message SET device_id = NULL, kind = NULL, content_kind = NULL,
                     encrypted = NULL, epoch = NULL, pointer_seq = NULL, client_sent_at = NULL,
                     server_saved_at = ?3, topic_id = NULL, thread_seq = NULL,
                     expires_at = NULL, signature = NULL, body = NULL
                     WHERE conversation_id = ?1 AND kind = 1 AND pointer_seq IN (
                         SELECT seq FROM message WHERE conversation_id = ?1 AND topic_id = ?2
                     )",
                    params![chat.bytes(), topic.bytes(), now],
                )?;
                connection.execute(
                    "UPDATE message SET device_id = NULL, kind = NULL, content_kind = NULL,
                     encrypted = NULL, epoch = NULL, pointer_seq = NULL, client_sent_at = NULL,
                     server_saved_at = ?3, topic_id = NULL, thread_seq = NULL,
                     expires_at = NULL, signature = NULL, body = NULL
                     WHERE conversation_id = ?1 AND topic_id = ?2",
                    params![chat.bytes(), topic.bytes(), now],
                )?;
                wiped = true;
                let removed = event(
                    chat,
                    account,
                    ChatAction::TopicRemoved,
                    Some(topic),
                    Audience::Moderators,
                    now,
                );
                event_list.push(write_event(connection, removed, None)?);
            }
            if wiped {
                connection.execute(
                    "UPDATE conversation SET
                     last_content_seq = (
                         SELECT seq FROM message WHERE conversation_id = ?1
                         AND (kind IN (4, 5) OR (kind = 0 AND content_kind IS NOT 14))
                         AND thread_seq IS NULL
                         ORDER BY seq DESC LIMIT 1
                     ),
                     last_at = (
                         SELECT server_saved_at FROM message WHERE conversation_id = ?1
                         AND kind IN (0, 4, 5) AND content_kind IS NOT 14 AND thread_seq IS NULL
                         ORDER BY seq DESC LIMIT 1
                     )
                     WHERE id = ?1",
                    params![chat.bytes()],
                )?;
            }
            Ok(TopicsPut::Saved { event_list, wiped })
        })
        .await
    }

    async fn put_chat_profile(&self, profile: NewChatProfile) -> Result<ChatProfileSaved> {
        self.write(move |connection| {
            if !photo_fits(connection, &profile)? {
                return Ok(ChatProfileSaved::NoSuchPhoto);
            }
            save_chat_profile(connection, profile, None)
        })
        .await
    }

    async fn put_chat_options(&self, chat: Ulid, options: NewChatOptions) -> Result<()> {
        self.write(move |connection| save_chat_options(connection, chat, &options))
            .await
    }

    async fn put_findable(&self, chat: Ulid, findable: Option<FindableItem>) -> Result<()> {
        self.write(move |connection| {
            let (on, name, bio) = match findable {
                Some(findable) => (true, Some(findable.name), findable.bio),
                None => (false, None, None),
            };
            connection.execute(
                "UPDATE conversation SET findable = ?2, find_name = ?3, find_bio = ?4
                 WHERE id = ?1",
                params![chat.bytes(), on, name, bio],
            )?;
            Ok(())
        })
        .await
    }

    async fn search_chats(&self, term: String, limit: u32) -> Result<Vec<FoundChat>> {
        self.read(move |connection| {
            let mut statement = connection.prepare(
                "SELECT c.id, c.type, c.find_name, c.find_bio,
                 (SELECT COUNT(*) FROM member m WHERE m.conversation_id = c.id)
                 FROM conversation c
                 WHERE c.findable = 1 AND c.find_name LIKE ?1 ESCAPE '\\'
                 ORDER BY c.find_name LIMIT ?2",
            )?;
            let rows = statement.query_map(params![like_pattern(&term), limit], |row| {
                Ok(FoundChat {
                    chat: Ulid::from_bytes(row.get(0)?),
                    chat_type: numbered(row.get(1)?)?,
                    name: row.get(2)?,
                    bio: row.get(3)?,
                    member_count: unsigned(row.get(4)?),
                })
            })?;
            let mut chat_list = Vec::new();
            for row in rows {
                chat_list.push(row?);
            }
            Ok(chat_list)
        })
        .await
    }

    async fn chat_type(&self, chat: Ulid) -> Result<Option<ChatType>> {
        self.read(move |connection| {
            let chat_type: Option<i64> = connection
                .query_row(
                    "SELECT type FROM conversation WHERE id = ?1",
                    params![chat.bytes()],
                    |row| row.get(0),
                )
                .optional()?;
            match chat_type {
                Some(number) => Ok(Some(numbered(number)?)),
                None => Ok(None),
            }
        })
        .await
    }

    async fn chat_options(&self, chat: Ulid) -> Result<ChatOptions> {
        self.read(move |connection| chat_options(connection, chat))
            .await
    }

    async fn topic_access(
        &self,
        chat: Ulid,
        topic: Ulid,
        account: Ulid,
    ) -> Result<Option<TopicAccess>> {
        self.read(move |connection| {
            let Some(item) = topic_item(connection, chat, topic)? else {
                return Ok(None);
            };
            let chat_type: i64 = connection.query_row(
                "SELECT type FROM conversation WHERE id = ?1",
                params![chat.bytes()],
                |row| row.get(0),
            )?;
            let prefix = if numbered::<ChatType>(chat_type)? == ChatType::Channel {
                "channel"
            } else {
                "group"
            };
            let last_sent_at = connection
                .query_row(
                    "SELECT at FROM slow_sent WHERE conversation_id = ?1 AND topic_id = ?2
                     AND account_id = ?3",
                    params![chat.bytes(), topic.bytes(), account.bytes()],
                    |row| row.get(0),
                )
                .optional()?;
            Ok(Some(TopicAccess {
                closed: item.options.closed,
                hidden: item.options.hidden,
                write: item.options.write,
                slow_mode: item.options.slow_mode,
                last_sent_at,
                moderator: item.mod_list.contains(&account),
                allow: item
                    .options
                    .allow
                    .iter()
                    .map(|name| format!("{prefix}.{name}"))
                    .collect(),
                deny: item
                    .options
                    .deny
                    .iter()
                    .map(|name| format!("{prefix}.{name}"))
                    .collect(),
            }))
        })
        .await
    }

    async fn put_topic_options(
        &self,
        chat: Ulid,
        topic: Ulid,
        actor: Ulid,
        options: NewTopicOptions,
        now: i64,
    ) -> Result<TopicSaved> {
        self.write(move |connection| {
            let Some(was_hidden) = topic_hidden(connection, chat, topic)? else {
                return Ok(TopicSaved::NoSuchTopic);
            };
            connection.execute(
                "UPDATE topic SET closed = ?3, hidden = ?4, write = ?5, slow_mode = ?6
                 WHERE conversation_id = ?1 AND id = ?2",
                params![
                    chat.bytes(),
                    topic.bytes(),
                    options.closed,
                    options.hidden,
                    i64::from(options.write == TopicWrite::Moderators),
                    opt_int(options.slow_mode)
                ],
            )?;
            connection.execute(
                "DELETE FROM topic_permission WHERE conversation_id = ?1 AND topic_id = ?2",
                params![chat.bytes(), topic.bytes()],
            )?;
            for (list, allowed) in [(&options.allow, true), (&options.deny, false)] {
                for permission in list {
                    connection.execute(
                        "INSERT OR REPLACE INTO topic_permission (conversation_id, topic_id,
                         permission, allowed) VALUES (?1, ?2, ?3, ?4)",
                        params![chat.bytes(), topic.bytes(), permission, allowed],
                    )?;
                }
            }
            let changed = event(
                chat,
                actor,
                ChatAction::TopicOptions,
                None,
                Audience::Moderators,
                now,
            );
            topic_saved(
                connection,
                chat,
                topic,
                Some(changed),
                was_hidden,
                Vec::new(),
            )
        })
        .await
    }

    async fn put_topic_profile(&self, profile: NewTopicProfile) -> Result<TopicSaved> {
        self.write(move |connection| {
            let chat = profile.chat;
            let topic = profile.topic;
            let Some(was_hidden) = topic_hidden(connection, chat, topic)? else {
                return Ok(TopicSaved::NoSuchTopic);
            };
            let old_photo: Option<[u8; 16]> = connection.query_row(
                "SELECT photo_id FROM topic WHERE conversation_id = ?1 AND id = ?2",
                params![chat.bytes(), topic.bytes()],
                |row| row.get(0),
            )?;
            let old_photo = old_photo.map(Ulid::from_bytes);
            // The first profile, set by whoever added the topic, is part of creating it, which its
            // "added" row already says, so it writes no "edited" row.
            let creating: bool = connection.query_row(
                "SELECT EXISTS (SELECT 1 FROM topic t JOIN chat_event e
                 ON e.conversation_id = t.conversation_id AND e.action = 10 AND e.target = t.id
                 WHERE t.conversation_id = ?1 AND t.id = ?2 AND t.version = 0 AND e.actor = ?3)",
                params![chat.bytes(), topic.bytes(), profile.account.bytes()],
                |row| row.get(0),
            )?;
            if let Some(photo) = profile.photo {
                let fits: bool = connection.query_row(
                    "SELECT EXISTS (SELECT 1 FROM file f JOIN device d ON d.id = f.uploaded_by
                     WHERE f.id = ?1 AND f.conversation_id = ?2 AND f.kind = 3 AND f.size <= ?4
                     AND f.received = f.chunks AND f.removed_at IS NULL
                     AND (d.account_id = ?3 OR f.id = ?5))",
                    params![
                        photo.bytes(),
                        chat.bytes(),
                        profile.account.bytes(),
                        int(profile.photo_limit),
                        old_photo.map(|old| old.bytes().to_vec())
                    ],
                    |row| row.get(0),
                )?;
                if !fits {
                    return Ok(TopicSaved::NoSuchPhoto);
                }
                connection.execute(
                    "UPDATE file SET used_at = ?2 WHERE id = ?1 AND used_at IS NULL",
                    params![photo.bytes(), profile.now],
                )?;
            }
            let mut removed = Vec::new();
            if let Some(old) = old_photo
                && Some(old) != profile.photo
            {
                connection.execute(
                    "UPDATE file SET removed_at = ?2 WHERE id = ?1 AND removed_at IS NULL",
                    params![old.bytes(), profile.now],
                )?;
                removed.push(old);
            }
            connection.execute(
                "UPDATE topic SET sealed = ?3, signature = ?4, photo_id = ?5, updated_by = ?6,
                 version = version + 1, updated_at = ?7 WHERE conversation_id = ?1 AND id = ?2",
                params![
                    chat.bytes(),
                    topic.bytes(),
                    profile.sealed,
                    profile.signature,
                    profile.photo.map(|photo| photo.bytes().to_vec()),
                    profile.device.bytes(),
                    profile.now
                ],
            )?;
            let changed = (!creating).then(|| {
                event(
                    chat,
                    profile.account,
                    ChatAction::TopicEdited,
                    None,
                    Audience::Moderators,
                    profile.now,
                )
            });
            topic_saved(connection, chat, topic, changed, was_hidden, removed)
        })
        .await
    }

    async fn put_topic_role(
        &self,
        chat: Ulid,
        topic: Ulid,
        actor: Ulid,
        account: Ulid,
        moderator: bool,
        now: i64,
    ) -> Result<TopicSaved> {
        self.write(move |connection| {
            let Some(was_hidden) = topic_hidden(connection, chat, topic)? else {
                return Ok(TopicSaved::NoSuchTopic);
            };
            let member: bool = connection.query_row(
                "SELECT EXISTS (SELECT 1 FROM member WHERE conversation_id = ?1 AND account_id = ?2)",
                params![chat.bytes(), account.bytes()],
                |row| row.get(0),
            )?;
            if !member {
                return Ok(TopicSaved::NotMember);
            }
            let action = if moderator {
                connection.execute(
                    "INSERT OR IGNORE INTO topic_moderator (conversation_id, topic_id, account_id)
                     VALUES (?1, ?2, ?3)",
                    params![chat.bytes(), topic.bytes(), account.bytes()],
                )?;
                ChatAction::MadeTopicModerator
            } else {
                connection.execute(
                    "DELETE FROM topic_moderator WHERE conversation_id = ?1 AND topic_id = ?2
                     AND account_id = ?3",
                    params![chat.bytes(), topic.bytes(), account.bytes()],
                )?;
                ChatAction::MadeTopicMember
            };
            let changed = event(chat, actor, action, Some(account), Audience::Moderators, now);
            topic_saved(connection, chat, topic, Some(changed), was_hidden, Vec::new())
        })
        .await
    }

    async fn topic_moderators(&self, chat: Ulid) -> Result<Vec<TopicMods>> {
        self.read(move |connection| {
            let mut statement = connection.prepare(
                "SELECT id, hidden FROM topic WHERE conversation_id = ?1 AND removed = 0",
            )?;
            let rows = statement.query_map(params![chat.bytes()], |row| {
                Ok((Ulid::from_bytes(row.get(0)?), row.get::<_, bool>(1)?))
            })?;
            let mut list = Vec::new();
            for row in rows {
                let (topic, hidden) = row?;
                let mut mods = connection.prepare(
                    "SELECT account_id FROM topic_moderator WHERE conversation_id = ?1
                     AND topic_id = ?2",
                )?;
                let mut mod_list = Vec::new();
                for account in
                    mods.query_map(params![chat.bytes(), topic.bytes()], |row| row.get(0))?
                {
                    mod_list.push(Ulid::from_bytes(account?));
                }
                list.push(TopicMods {
                    topic,
                    hidden,
                    mod_list,
                });
            }
            Ok(list)
        })
        .await
    }

    async fn chat_access(&self, chat: Ulid, account: Ulid) -> Result<Option<ChatAccess>> {
        self.read(move |connection| {
            type AccessRow = (i64, i64, bool, bool, Option<i64>, bool);
            let row: Option<AccessRow> = connection
                .query_row(
                    "SELECT c.type, c.last_seq, m.is_owner,
                     EXISTS (SELECT 1 FROM member_role r WHERE r.conversation_id = c.id
                             AND r.account_id = m.account_id),
                     c.slow_mode, c.topics_on
                     FROM conversation c
                     JOIN member m ON m.conversation_id = c.id
                     WHERE c.id = ?1 AND m.account_id = ?2",
                    params![chat.bytes(), account.bytes()],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                            row.get(5)?,
                        ))
                    },
                )
                .optional()?;
            let Some((chat_type, last_seq, owner, moderator, slow_mode, topics_on)) = row else {
                return Ok(None);
            };
            let chat_type: ChatType = numbered(chat_type)?;
            Ok(Some(ChatAccess {
                chat_type,
                role: if owner {
                    ChatRole::Owner
                } else if moderator {
                    ChatRole::Moderator
                } else {
                    ChatRole::Member
                },
                slow_mode: opt_unsigned(slow_mode),
                topics_on,
                last_seq: unsigned(last_seq),
            }))
        })
        .await
    }

    async fn member_accounts(&self, chat: Ulid) -> Result<Vec<Ulid>> {
        self.read(move |connection| {
            let mut statement =
                connection.prepare("SELECT account_id FROM member WHERE conversation_id = ?1")?;
            let mut account_list = Vec::new();
            for id in statement.query_map(params![chat.bytes()], |row| row.get(0))? {
                account_list.push(Ulid::from_bytes(id?));
            }
            Ok(account_list)
        })
        .await
    }

    async fn chat_profile_keys(
        &self,
        chat: Ulid,
        account_list: Vec<Ulid>,
    ) -> Result<Vec<ProfileKeyItem>> {
        self.read(move |connection| {
            let mut statement = connection.prepare(
                "SELECT a.profile_version, k.epoch, k.sealed_key FROM account_profile_key k
                 JOIN account a ON a.id = k.account_id
                 WHERE k.conversation_id = ?1 AND k.account_id = ?2",
            )?;
            let mut key_list = Vec::with_capacity(account_list.len());
            for account in account_list {
                let item = statement
                    .query_row(params![chat.bytes(), account.bytes()], |row| {
                        Ok(ProfileKeyItem {
                            account,
                            version: unsigned(row.get(0)?),
                            epoch: Some(unsigned(row.get(1)?)),
                            sealed: Some(Bytes(row.get(2)?)),
                        })
                    })
                    .optional()?;
                key_list.extend(item);
            }
            Ok(key_list)
        })
        .await
    }

    async fn chat_list(&self, account: Ulid, expire_before: i64) -> Result<Vec<ChatItem>> {
        self.read(move |connection| {
            let mut statement = connection.prepare(
                "SELECT c.id, c.type, c.last_seq, c.last_kind, c.last_at, c.last_content_seq,
                 g.epoch, m.seen_seq, m.draft, m.draft_epoch, m.draft_at, m.is_owner,
                 EXISTS (SELECT 1 FROM member_role r WHERE r.conversation_id = c.id
                         AND r.account_id = m.account_id),
                 c.topics_on, c.find_name, c.find_bio
                 FROM member m JOIN conversation c ON c.id = m.conversation_id
                 LEFT JOIN mls_group g ON g.conversation_id = c.id
                 WHERE m.account_id = ?1
                 ORDER BY COALESCE(c.last_at, c.created_at) DESC",
            )?;
            let rows = statement.query_map(params![account.bytes()], |row| {
                let last_seq: i64 = row.get(2)?;
                let chat_type: ChatType = numbered(row.get(1)?)?;
                let owner: bool = row.get(11)?;
                let moderator: bool = row.get(12)?;
                let find_name: Option<String> = row.get(14)?;
                Ok(ChatItem {
                    chat: Ulid::from_bytes(row.get(0)?),
                    chat_type,
                    role: if !matches!(chat_type, ChatType::Group | ChatType::Channel) {
                        None
                    } else if owner {
                        Some(ChatRole::Owner)
                    } else if moderator {
                        Some(ChatRole::Moderator)
                    } else {
                        Some(ChatRole::Member)
                    },
                    topics_on: row.get(13)?,
                    topic_list: Vec::new(),
                    topic_setting_list: Vec::new(),
                    place_seen_list: Vec::new(),
                    profile: None,
                    options: None,
                    findable: match find_name {
                        Some(name) => Some(FindableItem {
                            name,
                            bio: row.get(15)?,
                        }),
                        None => None,
                    },
                    member_count: None,
                    request_count: None,
                    last_seq: if last_seq == 0 {
                        None
                    } else {
                        Some(unsigned(last_seq))
                    },
                    last_kind: opt_numbered(row.get(3)?)?,
                    last_at: row.get(4)?,
                    last_content_seq: opt_unsigned(row.get(5)?),
                    epoch: opt_unsigned(row.get(6)?),
                    last_content: None,
                    mark_list: Vec::new(),
                    seen_seq: Some(unsigned(row.get(7)?)),
                    draft: {
                        let body: Option<Vec<u8>> = row.get(8)?;
                        let epoch: Option<i64> = row.get(9)?;
                        let saved_at: Option<i64> = row.get(10)?;
                        match (body, epoch, saved_at) {
                            (Some(body), Some(epoch), Some(saved_at)) => Some(DraftItem {
                                epoch: unsigned(epoch),
                                body: Bytes(body),
                                saved_at,
                            }),
                            _ => None,
                        }
                    },
                    profile_key_list: Vec::new(),
                })
            })?;
            let mut chat_list = Vec::new();
            for chat in rows {
                chat_list.push(chat?);
            }
            let mut content = connection.prepare(&format!(
                "SELECT {MESSAGE_COLUMNS} FROM message WHERE conversation_id = ?1 AND seq = ?2"
            ))?;
            let mut marks = connection.prepare(
                "SELECT account_id, received_seq, seen_seq FROM member
                 WHERE conversation_id = ?1 AND account_id != ?2 ORDER BY account_id",
            )?;
            let mut profiles = connection.prepare(
                "SELECT m.account_id, a.profile_version, k.epoch, k.sealed_key FROM member m
                 JOIN account a ON a.id = m.account_id
                 LEFT JOIN account_profile_key k
                     ON k.conversation_id = m.conversation_id AND k.account_id = m.account_id
                 WHERE m.conversation_id = ?1 ORDER BY m.account_id",
            )?;
            let mut topics = connection.prepare(&format!(
                "SELECT id FROM topic WHERE conversation_id = ?1 AND removed = 0
                 AND id != {GENERAL_SQL} ORDER BY id"
            ))?;
            let mut places = connection.prepare(
                "SELECT kind, place, MAX(up_to_seq) FROM seen
                 WHERE conversation_id = ?1 AND account_id = ?2 AND kind > 0
                 GROUP BY kind, place",
            )?;
            let mut profile = connection.prepare(
                "SELECT sealed, signature, updated_by, version, key, key_epoch
                 FROM conversation_profile WHERE conversation_id = ?1",
            )?;
            let mut count =
                connection.prepare("SELECT COUNT(*) FROM member WHERE conversation_id = ?1")?;
            let mut requests = connection.prepare(
                "SELECT COUNT(*) FROM join_request WHERE conversation_id = ?1 AND state = 0
                 AND at >= ?2",
            )?;
            for item in &mut chat_list {
                if let Some(seq) = item.last_content_seq {
                    let message = content
                        .query_row(params![item.chat.bytes(), int(seq)], message_frame)
                        .optional()?;
                    // A hidden topic's message is no preview for whoever may not see it.
                    if let Some(message) = message
                        && match message.topic {
                            Some(topic) => !hidden_from(connection, item.chat, topic, account)?,
                            None => true,
                        }
                    {
                        item.last_content = Some(Box::new(message));
                    }
                }
                if matches!(item.chat_type, ChatType::Group | ChatType::Channel) {
                    for topic in topics.query_map(params![item.chat.bytes()], |row| row.get(0))? {
                        item.topic_list.push(Ulid::from_bytes(topic?));
                    }
                    // Each topic's own settings, General's first, leaving out a hidden one for
                    // whoever may not see it.
                    let manager = item.role != Some(ChatRole::Member);
                    let ids = std::iter::once(GENERAL_TOPIC).chain(item.topic_list.iter().copied());
                    for topic in ids.collect::<Vec<_>>() {
                        if let Some(setting) = topic_item(connection, item.chat, topic)?
                            && (!setting.options.hidden
                                || manager
                                || setting.mod_list.contains(&account))
                        {
                            item.topic_setting_list.push(setting);
                        }
                    }
                    let rows =
                        places.query_map(params![item.chat.bytes(), account.bytes()], |row| {
                            let kind: i64 = row.get(0)?;
                            let place: Vec<u8> = row.get(1)?;
                            let seen_seq: i64 = row.get(2)?;
                            Ok((kind, place, seen_seq))
                        })?;
                    for row in rows {
                        let (kind, place, seen_seq) = row?;
                        let mut seen = PlaceSeen {
                            topic: None,
                            thread_seq: None,
                            seen_seq: unsigned(seen_seq),
                        };
                        if kind == 1
                            && let Ok(topic) = <[u8; 16]>::try_from(place.as_slice())
                        {
                            seen.topic = Some(Ulid::from_bytes(topic));
                        } else if let Ok(thread) = <[u8; 8]>::try_from(place.as_slice()) {
                            seen.thread_seq = Some(u64::from_be_bytes(thread));
                        }
                        item.place_seen_list.push(seen);
                    }
                    item.profile = profile
                        .query_row(params![item.chat.bytes()], chat_profile_row)
                        .optional()?;
                    let options = chat_options(connection, item.chat)?;
                    // Owners and moderators let people in, and members too where the chat's
                    // options say so.
                    let may_add = item.role != Some(ChatRole::Member)
                        || (options.allow.iter().any(|name| name == "member.add")
                            && !options.deny.iter().any(|name| name == "member.add"));
                    if may_add {
                        let waiting: i64 = requests
                            .query_row(params![item.chat.bytes(), expire_before], |row| {
                                row.get(0)
                            })?;
                        item.request_count = Some(unsigned(waiting));
                    }
                    if options != ChatOptions::default() {
                        item.options = Some(options);
                    }
                }
                if item.chat_type == ChatType::Channel {
                    let members: i64 =
                        count.query_row(params![item.chat.bytes()], |row| row.get(0))?;
                    item.member_count = Some(unsigned(members));
                    continue;
                }
                let rows = marks.query_map(params![item.chat.bytes(), account.bytes()], |row| {
                    let received_seq: i64 = row.get(1)?;
                    let seen_seq: i64 = row.get(2)?;
                    Ok(MarkItem {
                        account: Ulid::from_bytes(row.get(0)?),
                        received_seq: unsigned(received_seq),
                        seen_seq: Some(unsigned(seen_seq)),
                    })
                })?;
                for mark in rows {
                    item.mark_list.push(mark?);
                }
                let rows = profiles.query_map(params![item.chat.bytes()], |row| {
                    Ok(ProfileKeyItem {
                        account: Ulid::from_bytes(row.get(0)?),
                        version: unsigned(row.get(1)?),
                        epoch: opt_unsigned(row.get(2)?),
                        sealed: row.get::<_, Option<Vec<u8>>>(3)?.map(Bytes),
                    })
                })?;
                for profile in rows {
                    item.profile_key_list.push(profile?);
                }
            }
            Ok(chat_list)
        })
        .await
    }

    async fn put_draft(
        &self,
        chat: Ulid,
        account: Ulid,
        draft: Option<(u64, Vec<u8>)>,
        now: i64,
    ) -> Result<Option<DraftItem>> {
        self.write(move |connection| {
            let (body, epoch, saved_at) = match &draft {
                Some((epoch, body)) => (Some(body.as_slice()), Some(int(*epoch)), Some(now)),
                None => (None, None, None),
            };
            connection.execute(
                "UPDATE member SET draft = ?3, draft_epoch = ?4, draft_at = ?5
                 WHERE conversation_id = ?1 AND account_id = ?2",
                params![chat.bytes(), account.bytes(), body, epoch, saved_at],
            )?;
            Ok(draft.map(|(epoch, body)| DraftItem {
                epoch,
                body: Bytes(body),
                saved_at: now,
            }))
        })
        .await
    }

    async fn chat_folders(&self, account: Ulid) -> Result<(ChatFolders, Vec<Ulid>)> {
        self.read(move |connection| {
            let mut statement = connection.prepare(
                "SELECT id, name, chats, pins, is_default FROM folder WHERE account_id = ?1
                 ORDER BY place",
            )?;
            let rows = statement.query_map(params![account.bytes()], |row| {
                let chats: Option<Vec<u8>> = row.get(2)?;
                let pins: Vec<u8> = row.get(3)?;
                let is_default: bool = row.get(4)?;
                Ok((
                    FolderItem {
                        id: Ulid::from_bytes(row.get(0)?),
                        name: row.get::<_, Option<Vec<u8>>>(1)?.map(Bytes),
                        chat_list: chats.map(|chats| id_list(&chats)),
                        pin_list: id_list(&pins),
                    },
                    is_default,
                ))
            })?;
            let mut folder_list = Vec::new();
            let mut default = None;
            for row in rows {
                let (folder, is_default) = row?;
                if is_default {
                    default = Some(folder.id);
                }
                folder_list.push(folder);
            }
            // Every account has its `all`, which starts as the default.
            let Some(default) = default else {
                return Err(rusqlite::Error::QueryReturnedNoRows);
            };
            let mut statement =
                connection.prepare("SELECT conversation_id FROM member WHERE account_id = ?1")?;
            let rows = statement.query_map(params![account.bytes()], |row| {
                Ok(Ulid::from_bytes(row.get(0)?))
            })?;
            let mut chat_list = Vec::new();
            for chat in rows {
                chat_list.push(chat?);
            }
            Ok((
                ChatFolders {
                    folder_list,
                    default,
                },
                chat_list,
            ))
        })
        .await
    }

    async fn put_chat_folders(
        &self,
        account: Ulid,
        folders: ChatFolders,
        purpose: i64,
        now: i64,
    ) -> Result<bool> {
        self.write(move |connection| {
            let mut held = HashSet::new();
            let mut statement =
                connection.prepare("SELECT id FROM folder WHERE account_id = ?1")?;
            let rows = statement.query_map(params![account.bytes()], |row| {
                Ok(Ulid::from_bytes(row.get(0)?))
            })?;
            for id in rows {
                held.insert(id?);
            }
            // Every new id is checked before any is used up, so a refusal changes nothing.
            let mut issued = Vec::new();
            for folder in &folders.folder_list {
                if held.contains(&folder.id) {
                    continue;
                }
                let expires_at: Option<i64> = connection
                    .query_row(
                        "SELECT expires_at FROM challenge
                         WHERE value = ?1 AND account_id = ?2 AND purpose = ?3",
                        params![folder.id.bytes(), account.bytes(), purpose],
                        |row| row.get(0),
                    )
                    .optional()?;
                match expires_at {
                    Some(expires_at) if expires_at >= now => issued.push(folder.id),
                    _ => return Ok(false),
                }
            }
            for id in issued {
                connection.execute(
                    "DELETE FROM challenge WHERE value = ?1",
                    params![id.bytes()],
                )?;
            }
            connection.execute(
                "DELETE FROM folder WHERE account_id = ?1",
                params![account.bytes()],
            )?;
            for (place, folder) in (0u64..).zip(&folders.folder_list) {
                let chats = folder.chat_list.as_deref().map(id_bytes);
                connection.execute(
                    "INSERT INTO folder (id, account_id, place, name, chats, pins, is_default)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        folder.id.bytes(),
                        account.bytes(),
                        int(place),
                        folder.name.as_ref().map(|name| name.0.as_slice()),
                        chats,
                        id_bytes(&folder.pin_list),
                        folder.id == folders.default
                    ],
                )?;
            }
            Ok(true)
        })
        .await
    }

    async fn chat_devices(&self, chat: Ulid) -> Result<Vec<ChatDevice>> {
        self.read(move |connection| {
            let mut statement = connection.prepare(
                "SELECT d.account_id, d.id, d.signing_public_key, d.master_stamp,
                 a.master_public_key, d.revocation
                 FROM member m JOIN device d ON d.account_id = m.account_id
                 JOIN account a ON a.id = m.account_id
                 WHERE m.conversation_id = ?1 ORDER BY d.id",
            )?;
            let rows = statement.query_map(params![chat.bytes()], |row| {
                let statement: Option<Vec<u8>> = row.get(5)?;
                Ok(ChatDevice {
                    account: Ulid::from_bytes(row.get(0)?),
                    device: Ulid::from_bytes(row.get(1)?),
                    public_key: Bytes(row.get(2)?),
                    stamp: Bytes(row.get(3)?),
                    master_public: Bytes(row.get(4)?),
                    statement: statement.map(Bytes),
                })
            })?;
            let mut device_list = Vec::new();
            for device in rows {
                device_list.push(device?);
            }
            Ok(device_list)
        })
        .await
    }

    async fn message_times(
        &self,
        chat: Ulid,
        seq: u64,
        after: Option<Ulid>,
        shown: TimesShown,
        limit: usize,
    ) -> Result<Option<(Vec<TimesItem>, bool)>> {
        self.read(move |connection| {
            type Found = ([u8; 16], Option<[u8; 16]>, Option<i64>, bool);
            let found: Option<Found> = connection
                .query_row(
                    "SELECT d.account_id, m.topic_id, m.thread_seq, c.topics_on
                     FROM message m JOIN device d ON d.id = m.device_id
                     JOIN conversation c ON c.id = m.conversation_id
                     WHERE m.conversation_id = ?1 AND m.seq = ?2",
                    params![chat.bytes(), int(seq)],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()?;
            let Some((sender, topic, thread, topics_on)) = found else {
                return Ok(None);
            };
            // Where reading it was reported. A thread's own place; a topic's while topics are on;
            // with them off the one list, or the topic from while they were on. Received is only
            // ever reported for the whole chat.
            let (first, second) = match (thread, topic) {
                (Some(thread), _) => {
                    let place = place_columns(Place::Thread(unsigned(thread)));
                    (place.clone(), place)
                }
                (None, Some(topic)) if topics_on => {
                    let place = place_columns(Place::Topic(Ulid::from_bytes(topic)));
                    (place.clone(), place)
                }
                (None, Some(topic)) => (
                    place_columns(Place::Main),
                    place_columns(Place::Topic(Ulid::from_bytes(topic))),
                ),
                (None, None) => (place_columns(Place::Main), place_columns(Place::Main)),
            };
            // One query, so a page is ordered as a whole: seen first, then received only, then
            // neither, the newest first, then by account. `after`'s own place is worked out from
            // its times, a member or not, so one who left between pages does not end the list.
            let mut page = connection.prepare(
                "WITH people (account) AS (
                     SELECT account_id FROM member WHERE conversation_id = ?1 AND account_id != ?3
                     UNION SELECT ?4 WHERE ?4 IS NOT NULL
                 ),
                 times AS (
                     SELECT account,
                     CASE WHEN ?5 THEN (SELECT MIN(server_received_at) FROM received
                         WHERE conversation_id = ?1 AND account_id = account AND kind = 0
                         AND place = x'' AND up_to_seq >= ?2) END AS received_at,
                     CASE WHEN ?6 THEN (SELECT MIN(server_seen_at) FROM seen
                         WHERE conversation_id = ?1 AND account_id = account AND up_to_seq >= ?2
                         AND ((kind = ?7 AND place = ?8) OR (kind = ?9 AND place = ?10))) END
                         AS seen_at
                     FROM people
                 ),
                 keyed AS (
                     SELECT account, received_at, seen_at,
                     CASE WHEN seen_at IS NOT NULL THEN 0 WHEN received_at IS NOT NULL THEN 1
                         ELSE 2 END AS bucket,
                     -COALESCE(seen_at, received_at, 0) AS late
                     FROM times
                 )
                 SELECT account, received_at, seen_at FROM keyed
                 WHERE ?4 IS NULL
                 OR (bucket, late, account) > (SELECT bucket, late, account FROM keyed WHERE account = ?4)
                 ORDER BY bucket, late, account LIMIT ?11",
            )?;
            let rows = page.query_map(
                params![
                    chat.bytes(),
                    int(seq),
                    sender,
                    after.map(|account| account.bytes().to_vec()),
                    shown.received,
                    shown.seen,
                    first.0,
                    first.1,
                    second.0,
                    second.1,
                    int(limit as u64 + 1),
                ],
                |row| {
                    Ok(TimesItem {
                        account: Ulid::from_bytes(row.get(0)?),
                        received_at: row.get(1)?,
                        seen_at: row.get(2)?,
                    })
                },
            )?;
            let mut member_list = Vec::new();
            for row in rows {
                member_list.push(row?);
            }
            let more = member_list.len() > limit;
            member_list.truncate(limit);
            Ok(Some((member_list, more)))
        })
        .await
    }

    async fn message_brief(&self, chat: Ulid, seq: u64) -> Result<Option<MessageBrief>> {
        self.read(move |connection| {
            let row: Option<BriefRow> = connection
                .query_row(
                    "SELECT m.kind, m.content_kind, d.account_id, d.created_at, m.server_saved_at,
                     m.topic_id, m.thread_seq
                     FROM message m JOIN device d ON d.id = m.device_id
                     WHERE m.conversation_id = ?1 AND m.seq = ?2",
                    params![chat.bytes(), int(seq)],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                            row.get(5)?,
                            row.get(6)?,
                        ))
                    },
                )
                .optional()?;
            let Some((kind, content_kind, account, device_created_at, saved_at, topic, thread)) =
                row
            else {
                return Ok(None);
            };
            Ok(Some(MessageBrief {
                kind: numbered(kind)?,
                content_kind: opt_numbered(content_kind)?,
                account: Ulid::from_bytes(account),
                device_created_at,
                saved_at,
                topic: opt_id(topic),
                thread_seq: opt_unsigned(thread),
            }))
        })
        .await
    }

    async fn append_message(&self, message: NewMessage) -> Result<Appended> {
        self.write(move |connection| {
            let chat = message.chat;
            if message.kind == Kind::Commit {
                let current: Option<i64> = connection
                    .query_row(
                        "SELECT epoch FROM mls_group WHERE conversation_id = ?1",
                        params![chat.bytes()],
                        |row| row.get(0),
                    )
                    .optional()?;
                match (current, message.epoch) {
                    (Some(current), Some(epoch)) if int(epoch) == current + 1 => {}
                    _ => return Ok(Appended::CommitLost),
                }
            }
            // Nothing new is sealed under a key someone taken out still holds.
            if matches!(message.kind, Kind::Content | Kind::Edit)
                && message.encrypted
                && let Some(epoch) = message.epoch
            {
                let current: Option<i64> = connection
                    .query_row(
                        "SELECT epoch FROM mls_group WHERE conversation_id = ?1",
                        params![chat.bytes()],
                        |row| row.get(0),
                    )
                    .optional()?;
                if current.is_some_and(|current| int(epoch) < current) {
                    return Ok(Appended::StaleEpoch);
                }
            }
            if let Some(topic) = message.topic {
                let live: bool = connection.query_row(
                    "SELECT EXISTS (SELECT 1 FROM topic WHERE conversation_id = ?1 AND id = ?2
                     AND removed = 0)",
                    params![chat.bytes(), topic.bytes()],
                    |row| row.get(0),
                )?;
                if !live {
                    return Ok(Appended::TopicGone);
                }
            }
            if let Some(action) = &message.action
                && action.kind == ActionKind::Add
            {
                let mut count: i64 = connection.query_row(
                    "SELECT COUNT(*) FROM member WHERE conversation_id = ?1",
                    params![chat.bytes()],
                    |row| row.get(0),
                )?;
                for account in &action.account_list {
                    let member: bool = connection.query_row(
                        "SELECT EXISTS (SELECT 1 FROM member WHERE conversation_id = ?1
                         AND account_id = ?2)",
                        params![chat.bytes(), account.bytes()],
                        |row| row.get(0),
                    )?;
                    if !member {
                        count += 1;
                    }
                }
                if unsigned(count) > action.limit {
                    return Ok(Appended::Full);
                }
            }
            let seq: i64 = connection.query_row(
                "UPDATE conversation SET last_seq = last_seq + 1, last_kind = ?2 WHERE id = ?1
                 RETURNING last_seq",
                params![chat.bytes(), message.kind as u8],
                |row| row.get(0),
            )?;
            let mut event_list = Vec::new();
            // A delete's history row copies what it is about to wipe.
            if message.kind == Kind::Delete
                && let Some(author) = message.history
            {
                event_list.push(write_copied(connection, &message, author)?);
            }
            if message.kind == Kind::Delete {
                connection.execute(
                    "UPDATE message SET device_id = NULL, kind = NULL, content_kind = NULL,
                     encrypted = NULL, epoch = NULL, pointer_seq = NULL, client_sent_at = NULL,
                     server_saved_at = ?3, topic_id = NULL, thread_seq = NULL,
                     expires_at = NULL, signature = NULL, body = NULL
                     WHERE conversation_id = ?1 AND (seq = ?2 OR (kind = 1 AND pointer_seq = ?2))",
                    params![chat.bytes(), opt_int(message.pointer_seq), message.now],
                )?;
                // A deleted call takes everything about it along.
                connection.execute(
                    "DELETE FROM call WHERE conversation_id = ?1 AND call_seq = ?2",
                    params![chat.bytes(), opt_int(message.pointer_seq)],
                )?;
            }
            let content_kind = message.content_kind.map(|content_kind| content_kind as u8);
            connection.execute(
                "INSERT INTO message (conversation_id, seq, device_id, kind, content_kind,
                 encrypted, epoch, pointer_seq, client_sent_at, server_saved_at,
                 signature, body, topic_id, thread_seq)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                params![
                    chat.bytes(),
                    seq,
                    message.device.bytes(),
                    message.kind as u8,
                    content_kind,
                    message.encrypted,
                    opt_int(message.epoch),
                    opt_int(message.pointer_seq),
                    message.client_sent_at,
                    message.now,
                    message.signature,
                    message.body,
                    message.topic.as_ref().map(|topic| topic.bytes().to_vec()),
                    opt_int(message.thread_seq)
                ],
            )?;
            if message.slow {
                let topic = message.topic.unwrap_or(GENERAL_TOPIC);
                connection.execute(
                    "INSERT INTO slow_sent (conversation_id, topic_id, account_id, at)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT (conversation_id, topic_id, account_id) DO UPDATE SET at = ?4",
                    params![
                        chat.bytes(),
                        topic.bytes(),
                        message.account.bytes(),
                        message.now
                    ],
                )?;
            }
            let mut gone_list = Vec::new();
            // An edit's history row holds the message and every edit so far, this one included.
            if message.kind == Kind::Edit
                && let Some(author) = message.history
            {
                event_list.push(write_copied(connection, &message, author)?);
            }
            for file in &message.persist {
                connection.execute(
                    "UPDATE file SET used_at = ?2 WHERE id = ?1",
                    params![file.bytes(), message.now],
                )?;
            }
            for file in &message.remove {
                connection.execute(
                    "UPDATE file SET removed_at = ?2 WHERE id = ?1 AND removed_at IS NULL",
                    params![file.bytes(), message.now],
                )?;
            }
            match message.kind {
                Kind::Content => {
                    if message.content_kind == Some(ContentKind::Ack) {
                        // An ack is quiet: it moves neither the chat's time nor its preview.
                    } else if message.thread_seq.is_some() {
                        // A thread's reply counts under the message it answers, not in the list.
                    } else if message.content_kind == Some(ContentKind::Call) {
                        // A call is the preview, and it leaves the chat's time where it was.
                        connection.execute(
                            "UPDATE conversation SET last_content_seq = ?2 WHERE id = ?1",
                            params![chat.bytes(), seq],
                        )?;
                    } else {
                        connection.execute(
                            "UPDATE conversation SET last_at = ?2, last_content_seq = ?3
                             WHERE id = ?1",
                            params![chat.bytes(), message.now, seq],
                        )?;
                    }
                }
                Kind::Delete => {
                    connection.execute(
                        "UPDATE conversation SET
                         last_content_seq = (
                             SELECT seq FROM message WHERE conversation_id = ?1
                             AND (kind IN (4, 5) OR (kind = 0 AND content_kind IS NOT 14))
                             AND thread_seq IS NULL
                             ORDER BY seq DESC LIMIT 1
                         ),
                         last_at = (
                             SELECT server_saved_at FROM message WHERE conversation_id = ?1
                             AND kind IN (0, 4, 5) AND content_kind IS NOT 14 AND thread_seq IS NULL
                             ORDER BY seq DESC LIMIT 1
                         )
                         WHERE id = ?1",
                        params![chat.bytes()],
                    )?;
                }
                Kind::Commit => {
                    connection.execute(
                        "UPDATE mls_group SET epoch = ?2, group_info = ?3, public_group = ?4,
                         updated_at = ?5 WHERE conversation_id = ?1",
                        params![
                            chat.bytes(),
                            opt_int(message.epoch),
                            message.group_info,
                            message.public_group,
                            message.now
                        ],
                    )?;
                    for wrap in &message.wrap_list {
                        connection.execute(
                            "INSERT OR REPLACE INTO epoch_key (conversation_id, epoch, account_id,
                             wrapped_key, wrapped_by) VALUES (?1, ?2, ?3, ?4, ?5)",
                            params![
                                chat.bytes(),
                                opt_int(message.epoch),
                                wrap.account.bytes(),
                                wrap.wrapped,
                                message.device.bytes()
                            ],
                        )?;
                    }
                    if let Some(epoch) = message.epoch {
                        put_profile_keys(connection, chat, epoch, &message.profile_key_list)?;
                    }
                    for welcome in &message.welcome_list {
                        connection.execute(
                            "INSERT INTO mls_welcome (id, device_id, conversation_id, body,
                             created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                            params![
                                welcome.id.bytes(),
                                welcome.device.bytes(),
                                chat.bytes(),
                                welcome.body,
                                message.now
                            ],
                        )?;
                    }
                }
                _ => {}
            }
            let mut line_list = Vec::new();
            if let Some(action) = &message.action {
                let mut added_list = Vec::new();
                let mut let_in_list = Vec::new();
                let mut removed_list = Vec::new();
                for account in &action.account_list {
                    let member: bool = connection.query_row(
                        "SELECT EXISTS (SELECT 1 FROM member WHERE conversation_id = ?1
                         AND account_id = ?2)",
                        params![chat.bytes(), account.bytes()],
                        |row| row.get(0),
                    )?;
                    let line = match action.kind {
                        ActionKind::Add => {
                            if member {
                                continue;
                            }
                            connection.execute(
                                "INSERT INTO member (conversation_id, account_id, joined_seq)
                                 VALUES (?1, ?2, ?3)",
                                params![chat.bytes(), account.bytes(), seq],
                            )?;
                            connection.execute(
                                "DELETE FROM invitation WHERE conversation_id = ?1
                                 AND account_id = ?2",
                                params![chat.bytes(), account.bytes()],
                            )?;
                            let asked = connection.execute(
                                "DELETE FROM join_request WHERE conversation_id = ?1
                                 AND account_id = ?2 AND state = 0",
                                params![chat.bytes(), account.bytes()],
                            )?;
                            if asked > 0 {
                                let_in_list.push(*account);
                                ChatAction::LetIn
                            } else {
                                added_list.push(*account);
                                ChatAction::Added
                            }
                        }
                        // Someone who left is gone already, and a commit only finishes it in MLS.
                        ActionKind::Remove => {
                            if !member {
                                continue;
                            }
                            connection.execute(
                                "DELETE FROM member WHERE conversation_id = ?1 AND account_id = ?2",
                                params![chat.bytes(), account.bytes()],
                            )?;
                            drop_from_folders(connection, chat, *account)?;
                            gone_list.push(*account);
                            removed_list.push(*account);
                            ChatAction::Removed
                        }
                    };
                    let history = event(
                        chat,
                        action.actor,
                        line,
                        Some(*account),
                        Audience::Moderators,
                        message.now,
                    );
                    event_list.push(write_event(connection, history, None)?);
                }
                // One line per action, however many people it names.
                for (said, account_list) in [
                    (LineAction::Added, added_list),
                    (LineAction::LetIn, let_in_list),
                    (LineAction::Removed, removed_list),
                ] {
                    if account_list.is_empty() {
                        continue;
                    }
                    line_list.extend(write_line(
                        connection,
                        chat,
                        message.device,
                        said,
                        account_list,
                        message.now,
                    )?);
                }
            }
            // The profile key shared at this commit's epoch replaces the one kept, so every member,
            // the people it brought in too, holds the key the newest share is sealed under.
            let mut profile = None;
            if let (Some(key), Some(epoch)) = (message.chat_key, message.epoch) {
                profile = connection
                    .query_row(
                        "UPDATE conversation_profile SET key = ?2, key_epoch = ?3
                         WHERE conversation_id = ?1
                         RETURNING sealed, signature, updated_by, version, key, key_epoch",
                        params![chat.bytes(), key, int(epoch)],
                        chat_profile_row,
                    )
                    .optional()?;
            }
            Ok(Appended::Saved {
                seq: unsigned(seq),
                event_list,
                line_list,
                gone_list,
                profile,
            })
        })
        .await
    }

    async fn sync(
        &self,
        account: Ulid,
        chat: Option<Ulid>,
        from: SyncFrom,
        limit: u32,
    ) -> Result<SyncPage> {
        self.read(move |connection| {
            // One more than a page from each table tells whether more follow.
            let ask = SyncAsk {
                account,
                chat,
                from,
                limit: limit.saturating_add(1),
            };
            // A hidden topic's messages, and edits of them, reach only its moderators and the chat's
            // owners and moderators.
            let message_list = sync_rows(
                connection,
                &format!("SELECT {MESSAGE_COLUMNS} FROM message"),
                ("server_saved_at", "seq", None),
                Some(HIDDEN_FROM),
                ask,
                message_frame,
            )?;
            let call_list = sync_rows(
                connection,
                &format!("SELECT {CALL_COLUMNS} FROM call"),
                ("server_at", "call_seq", Some("n")),
                None,
                ask,
                call_event_row,
            )?;
            // A chat event stands at sequence id 0, before every message of its time. Owners and
            // moderators see history, and a line for one account only that account.
            let chat_event_list = sync_rows(
                connection,
                &format!("SELECT {CHAT_EVENT_COLUMNS} FROM chat_event"),
                ("server_at", "0", Some("n")),
                Some(
                    "((audience = 2 AND viewer = ?9) OR (audience = 1 AND (
                         EXISTS (SELECT 1 FROM member m WHERE m.conversation_id =
                                 chat_event.conversation_id AND m.account_id = ?9
                                 AND m.is_owner = 1)
                         OR EXISTS (SELECT 1 FROM member_role r WHERE r.conversation_id =
                                    chat_event.conversation_id AND r.account_id = ?9)
                         OR EXISTS (SELECT 1 FROM topic_moderator t WHERE t.conversation_id =
                                    chat_event.conversation_id AND t.topic_id = chat_event.topic_id
                                    AND t.account_id = ?9))))",
                ),
                ask,
                chat_event_row,
            )?;
            // The three merged in order, cut at the page's size.
            let limit = usize::try_from(limit).unwrap_or(usize::MAX);
            let mut page = SyncPage {
                message_list: Vec::new(),
                call_list: Vec::new(),
                chat_event_list: Vec::new(),
                more: false,
            };
            let mut messages = message_list.into_iter().peekable();
            let mut calls = call_list.into_iter().peekable();
            let mut events = chat_event_list.into_iter().peekable();
            loop {
                let mut next = None;
                if let Some(message) = messages.peek() {
                    next = Some((message_position(message), 0));
                }
                if let Some(call) = calls.peek() {
                    let position = call_position(call);
                    if next.is_none_or(|(least, _)| position < least) {
                        next = Some((position, 1));
                    }
                }
                if let Some(chat_event) = events.peek() {
                    let position = chat_event_position(chat_event);
                    if next.is_none_or(|(least, _)| position < least) {
                        next = Some((position, 2));
                    }
                }
                let Some((_, which)) = next else {
                    break;
                };
                if page.message_list.len() + page.call_list.len() + page.chat_event_list.len()
                    == limit
                {
                    page.more = true;
                    break;
                }
                match which {
                    0 => page.message_list.extend(messages.next()),
                    1 => page.call_list.extend(calls.next()),
                    _ => page.chat_event_list.extend(events.next()),
                }
            }
            Ok(page)
        })
        .await
    }

    fn saved_now(&self) -> i64 {
        let now = Ulid::now();
        self.last_saved.fetch_max(now, Ordering::Relaxed).max(now)
    }

    async fn record_position(&self, report: PositionReport) -> Result<Option<MarkItem>> {
        let (update, insert, highest) = match report.position {
            Position::Received => (
                "UPDATE member SET received_seq = ?3
                 WHERE conversation_id = ?1 AND account_id = ?2 AND received_seq < ?3",
                "INSERT INTO received (conversation_id, account_id, kind, place, up_to_seq,
                 device_id, client_received_at, server_received_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                "SELECT MAX(up_to_seq) FROM received
                 WHERE conversation_id = ?1 AND account_id = ?2 AND kind = ?3 AND place = ?4",
            ),
            Position::Seen => (
                "UPDATE member SET seen_seq = ?3
                 WHERE conversation_id = ?1 AND account_id = ?2 AND seen_seq < ?3",
                "INSERT INTO seen (conversation_id, account_id, kind, place, up_to_seq,
                 device_id, client_seen_at, server_seen_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                "SELECT MAX(up_to_seq) FROM seen
                 WHERE conversation_id = ?1 AND account_id = ?2 AND kind = ?3 AND place = ?4",
            ),
        };
        self.write(move |connection| {
            let PositionReport {
                chat,
                account,
                device,
                place,
                up_to_seq,
                client_at,
                now,
                ..
            } = report;
            let (kind, place_bytes) = place_columns(place);
            // A topic's or thread's position is the highest of its rows, and only the main list's
            // sits on `member`.
            if place == Place::Main {
                let changed = connection.execute(
                    update,
                    params![chat.bytes(), account.bytes(), int(up_to_seq)],
                )?;
                if changed == 0 {
                    return Ok(None);
                }
            } else {
                let highest: Option<i64> = connection.query_row(
                    highest,
                    params![chat.bytes(), account.bytes(), kind, place_bytes],
                    |row| row.get(0),
                )?;
                if highest.is_some_and(|highest| highest >= int(up_to_seq)) {
                    return Ok(None);
                }
            }
            connection.execute(
                insert,
                params![
                    chat.bytes(),
                    account.bytes(),
                    kind,
                    place_bytes,
                    int(up_to_seq),
                    device.bytes(),
                    client_at,
                    now
                ],
            )?;
            if place != Place::Main {
                return Ok(None);
            }
            let (received_seq, seen_seq): (i64, i64) = connection.query_row(
                "SELECT received_seq, seen_seq FROM member
                 WHERE conversation_id = ?1 AND account_id = ?2",
                params![chat.bytes(), account.bytes()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            Ok(Some(MarkItem {
                account,
                received_seq: unsigned(received_seq),
                seen_seq: Some(unsigned(seen_seq)),
            }))
        })
        .await
    }

    async fn group_info(&self, chat: Ulid) -> Result<Option<(u64, Vec<u8>)>> {
        self.read(move |connection| {
            let row: Option<(i64, Vec<u8>)> = connection
                .query_row(
                    "SELECT epoch, group_info FROM mls_group WHERE conversation_id = ?1",
                    params![chat.bytes()],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            match row {
                Some((epoch, body)) => Ok(Some((unsigned(epoch), body))),
                None => Ok(None),
            }
        })
        .await
    }

    async fn public_group(&self, chat: Ulid) -> Result<Option<Vec<u8>>> {
        self.read(move |connection| {
            connection
                .query_row(
                    "SELECT public_group FROM mls_group WHERE conversation_id = ?1",
                    params![chat.bytes()],
                    |row| row.get(0),
                )
                .optional()
        })
        .await
    }

    async fn put_group_info(
        &self,
        chat: Ulid,
        epoch: u64,
        body: Vec<u8>,
        now: i64,
        start: Option<NewGroupStart>,
        public_group: Option<Vec<u8>>,
    ) -> Result<GroupInfoPut> {
        self.write(move |connection| {
            let current: Option<i64> = connection
                .query_row(
                    "SELECT epoch FROM mls_group WHERE conversation_id = ?1",
                    params![chat.bytes()],
                    |row| row.get(0),
                )
                .optional()?;
            match current {
                None if epoch == 0 => {
                    let Some(public_group) = public_group else {
                        return Ok(GroupInfoPut::Stale);
                    };
                    if let Some(start) = &start
                        && !photo_fits(connection, &start.profile)?
                    {
                        return Ok(GroupInfoPut::NoSuchPhoto);
                    }
                    connection.execute(
                        "INSERT INTO mls_group (conversation_id, epoch, group_info, public_group,
                         updated_at) VALUES (?1, 0, ?2, ?3, ?4)",
                        params![chat.bytes(), body, public_group, now],
                    )?;
                    let mut profile = None;
                    if let Some(start) = start {
                        if let Some(options) = &start.options {
                            save_chat_options(connection, chat, options)?;
                        }
                        if let ChatProfileSaved::Saved { item, .. } =
                            save_chat_profile(connection, start.profile, Some(start.key))?
                        {
                            profile = Some(item);
                        }
                    }
                    Ok(GroupInfoPut::Saved { profile })
                }
                None => Ok(GroupInfoPut::Stale),
                Some(0) if epoch == 0 => {
                    let stored: Vec<u8> = connection.query_row(
                        "SELECT group_info FROM mls_group WHERE conversation_id = ?1",
                        params![chat.bytes()],
                        |row| row.get(0),
                    )?;
                    if stored == body {
                        Ok(GroupInfoPut::Saved { profile: None })
                    } else {
                        Ok(GroupInfoPut::AlreadyStarted)
                    }
                }
                Some(_) if epoch == 0 => Ok(GroupInfoPut::AlreadyStarted),
                Some(current) if current == int(epoch) => {
                    connection.execute(
                        "UPDATE mls_group SET group_info = ?2, updated_at = ?3
                         WHERE conversation_id = ?1",
                        params![chat.bytes(), body, now],
                    )?;
                    Ok(GroupInfoPut::Saved { profile: None })
                }
                Some(_) => Ok(GroupInfoPut::Stale),
            }
        })
        .await
    }

    async fn wraps(&self, chat: Ulid) -> Result<Vec<WrapEntry>> {
        self.read(move |connection| {
            let mut statement = connection.prepare(
                "SELECT epoch, account_id, wrapped_key FROM epoch_key WHERE conversation_id = ?1
                 ORDER BY epoch, account_id",
            )?;
            let rows = statement.query_map(params![chat.bytes()], |row| {
                Ok(WrapEntry {
                    epoch: unsigned(row.get(0)?),
                    account: Ulid::from_bytes(row.get(1)?),
                    wrapped: Bytes(row.get(2)?),
                })
            })?;
            let mut wrap_list = Vec::new();
            for wrap in rows {
                wrap_list.push(wrap?);
            }
            Ok(wrap_list)
        })
        .await
    }

    async fn put_wraps(
        &self,
        chat: Ulid,
        epoch: u64,
        wrap_list: Vec<NewWrap>,
        device: Ulid,
    ) -> Result<()> {
        self.write(move |connection| {
            for wrap in &wrap_list {
                connection.execute(
                    "INSERT OR REPLACE INTO epoch_key (conversation_id, epoch, account_id,
                     wrapped_key, wrapped_by) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        chat.bytes(),
                        int(epoch),
                        wrap.account.bytes(),
                        wrap.wrapped,
                        device.bytes()
                    ],
                )?;
            }
            Ok(())
        })
        .await
    }

    async fn put_profile_keys(
        &self,
        chat: Ulid,
        epoch: u64,
        key_list: Vec<(Ulid, Vec<u8>)>,
    ) -> Result<()> {
        self.write(move |connection| put_profile_keys(connection, chat, epoch, &key_list))
            .await
    }

    async fn take_welcomes(&self, device: Ulid) -> Result<Vec<(Ulid, Vec<u8>)>> {
        self.write(move |connection| {
            let mut statement = connection.prepare(
                "DELETE FROM mls_welcome WHERE device_id = ?1 RETURNING conversation_id, body",
            )?;
            let rows = statement.query_map(params![device.bytes()], |row| {
                Ok((Ulid::from_bytes(row.get(0)?), row.get(1)?))
            })?;
            let mut welcome_list = Vec::new();
            for welcome in rows {
                welcome_list.push(welcome?);
            }
            Ok(welcome_list)
        })
        .await
    }

    async fn save_call(&self, event_list: Vec<CallEvent>) -> Result<()> {
        self.write(move |connection| {
            for event in &event_list {
                connection.execute(
                    &format!(
                        "INSERT INTO call ({CALL_COLUMNS})
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)"
                    ),
                    params![
                        event.chat.bytes(),
                        int(event.call_seq),
                        int(event.n),
                        event.kind as u8,
                        event.device.bytes(),
                        event.server_at,
                        event.client_at,
                        opt_int(event.epoch),
                        event.signature.as_ref().map(|signature| &signature.0),
                        event.body.as_ref().map(|body| &body.0),
                        event.outcome.map(|outcome| outcome as u8)
                    ],
                )?;
            }
            Ok(())
        })
        .await
    }

    async fn end_open_calls(&self, now: i64) -> Result<()> {
        self.write(move |connection| {
            // Answered once a device of an account other than the caller's joined.
            connection.execute(
                "INSERT INTO call (conversation_id, call_seq, n, kind, device_id, server_at, outcome)
                 SELECT m.conversation_id, m.seq,
                 COALESCE((SELECT MAX(c.n) FROM call c
                           WHERE c.conversation_id = m.conversation_id AND c.call_seq = m.seq), 0)
                 + 1, ?2, m.device_id, ?1,
                 EXISTS (SELECT 1 FROM call c
                         JOIN device joined ON joined.id = c.device_id
                         JOIN device caller ON caller.id = m.device_id
                         WHERE c.conversation_id = m.conversation_id AND c.call_seq = m.seq
                         AND c.kind = ?3 AND joined.account_id <> caller.account_id)
                 FROM message m WHERE m.content_kind = 8
                 AND NOT EXISTS (SELECT 1 FROM call c WHERE c.conversation_id = m.conversation_id
                                 AND c.call_seq = m.seq AND c.kind = ?2)",
                params![
                    now,
                    CallEventKind::End as u8,
                    CallEventKind::Join as u8
                ],
            )?;
            Ok(())
        })
        .await
    }

    async fn create_pack(&self, id: Ulid, name: String, now: i64) -> Result<bool> {
        self.write(move |connection| {
            let added = connection.execute(
                "INSERT INTO sticker_pack (id, name, created_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT (name) DO NOTHING",
                params![id.bytes(), name, now],
            )?;
            Ok(added == 1)
        })
        .await
    }

    async fn remove_pack(&self, id: Ulid) -> Result<Option<Vec<Ulid>>> {
        self.write(move |connection| {
            let mut sticker_list = Vec::new();
            {
                let mut statement =
                    connection.prepare("SELECT id FROM sticker WHERE pack_id = ?1")?;
                for row in statement.query_map(params![id.bytes()], |row| row.get(0))? {
                    sticker_list.push(Ulid::from_bytes(row?));
                }
            }
            let removed = connection.execute(
                "DELETE FROM sticker_pack WHERE id = ?1",
                params![id.bytes()],
            )?;
            if removed == 0 {
                return Ok(None);
            }
            Ok(Some(sticker_list))
        })
        .await
    }

    async fn add_sticker(&self, sticker: NewSticker) -> Result<bool> {
        self.write(move |connection| {
            let added = connection.execute(
                "INSERT INTO sticker (id, pack_id, emoji, width, height, size, created_at)
                 SELECT ?1, id, ?3, ?4, ?5, ?6, ?7 FROM sticker_pack WHERE id = ?2",
                params![
                    sticker.id.bytes(),
                    sticker.pack.bytes(),
                    sticker.emoji,
                    sticker.width,
                    sticker.height,
                    int(sticker.size),
                    sticker.now
                ],
            )?;
            Ok(added == 1)
        })
        .await
    }

    async fn remove_sticker(&self, id: Ulid) -> Result<bool> {
        self.write(move |connection| {
            let removed =
                connection.execute("DELETE FROM sticker WHERE id = ?1", params![id.bytes()])?;
            Ok(removed == 1)
        })
        .await
    }

    async fn sticker_list(&self) -> Result<Vec<StickerPack>> {
        self.read(move |connection| {
            let mut pack_list: Vec<StickerPack> = Vec::new();
            let mut statement =
                connection.prepare("SELECT id, name FROM sticker_pack ORDER BY created_at, id")?;
            let rows = statement.query_map([], |row| {
                Ok(StickerPack {
                    id: Ulid::from_bytes(row.get(0)?),
                    name: row.get(1)?,
                    sticker_list: Vec::new(),
                })
            })?;
            for row in rows {
                pack_list.push(row?);
            }
            let mut statement = connection.prepare(
                "SELECT pack_id, id, emoji, width, height, size FROM sticker
                 ORDER BY pack_id, created_at, id",
            )?;
            let rows = statement.query_map([], |row| {
                let pack = Ulid::from_bytes(row.get(0)?);
                let sticker = StickerItem {
                    id: Ulid::from_bytes(row.get(1)?),
                    emoji: row.get(2)?,
                    width: row.get(3)?,
                    height: row.get(4)?,
                    size: unsigned(row.get(5)?),
                };
                Ok((pack, sticker))
            })?;
            for row in rows {
                let (pack, sticker) = row?;
                for item in &mut pack_list {
                    if item.id == pack {
                        item.sticker_list.push(sticker);
                        break;
                    }
                }
            }
            Ok(pack_list)
        })
        .await
    }

    async fn sticker(&self, id: Ulid) -> Result<Option<StickerItem>> {
        self.read(move |connection| {
            connection
                .query_row(
                    "SELECT id, emoji, width, height, size FROM sticker WHERE id = ?1",
                    params![id.bytes()],
                    |row| {
                        Ok(StickerItem {
                            id: Ulid::from_bytes(row.get(0)?),
                            emoji: row.get(1)?,
                            width: row.get(2)?,
                            height: row.get(3)?,
                            size: unsigned(row.get(4)?),
                        })
                    },
                )
                .optional()
        })
        .await
    }

    async fn profile(&self, account: Ulid) -> Result<Option<ProfileReply>> {
        self.read(move |connection| {
            connection
                .query_row(
                    "SELECT a.id, a.username, a.display_name, a.profile_version, p.sealed,
                     p.signature, p.updated_by, d.signing_public_key, d.master_stamp, p.photo_id
                     FROM account a LEFT JOIN account_profile p ON p.account_id = a.id
                     LEFT JOIN device d ON d.id = p.updated_by WHERE a.id = ?1",
                    params![account.bytes()],
                    |row| {
                        let sealed: Option<Vec<u8>> = row.get(4)?;
                        let signature: Option<Vec<u8>> = row.get(5)?;
                        let device: Option<[u8; 16]> = row.get(6)?;
                        let device_public: Option<Vec<u8>> = row.get(7)?;
                        let stamp: Option<Vec<u8>> = row.get(8)?;
                        let photo: Option<[u8; 16]> = row.get(9)?;
                        let profile = match (sealed, signature, device, device_public, stamp) {
                            (
                                Some(sealed),
                                Some(signature),
                                Some(device),
                                Some(device_public),
                                Some(stamp),
                            ) => Some(SignedProfile {
                                sealed: Bytes(sealed),
                                signature: Bytes(signature),
                                device: Ulid::from_bytes(device),
                                device_public: Bytes(device_public),
                                stamp: Bytes(stamp),
                            }),
                            _ => None,
                        };
                        Ok(ProfileReply {
                            account: Ulid::from_bytes(row.get(0)?),
                            username: row.get(1)?,
                            display_name: row.get(2)?,
                            version: unsigned(row.get(3)?),
                            profile,
                            photo: photo.map(Ulid::from_bytes),
                        })
                    },
                )
                .optional()
        })
        .await
    }

    async fn put_profile(&self, profile: NewProfile) -> Result<ProfileSaved> {
        self.write(move |connection| {
            if let Some(photo) = profile.photo {
                let owned: bool = connection.query_row(
                    "SELECT EXISTS (SELECT 1 FROM file WHERE id = ?1 AND account_id = ?2)",
                    params![photo.bytes(), profile.account.bytes()],
                    |row| row.get(0),
                )?;
                if !owned {
                    return Ok(ProfileSaved::NoSuchPhoto);
                }
                connection.execute(
                    "UPDATE file SET used_at = ?2 WHERE id = ?1 AND used_at IS NULL",
                    params![photo.bytes(), profile.now],
                )?;
            }
            let removed = take_photos(connection, profile.account, profile.photo)?;
            match profile.sealed {
                Some((sealed, signature)) => {
                    connection.execute(
                        "INSERT INTO account_profile (account_id, sealed, signature, updated_by,
                         photo_id, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                         ON CONFLICT (account_id) DO UPDATE SET sealed = excluded.sealed,
                         signature = excluded.signature, updated_by = excluded.updated_by,
                         photo_id = excluded.photo_id, updated_at = excluded.updated_at",
                        params![
                            profile.account.bytes(),
                            sealed,
                            signature,
                            profile.device.bytes(),
                            profile.photo.map(|photo| photo.bytes().to_vec()),
                            profile.now
                        ],
                    )?;
                }
                None => {
                    connection.execute(
                        "DELETE FROM account_profile WHERE account_id = ?1",
                        params![profile.account.bytes()],
                    )?;
                }
            }
            let version = next_profile_version(connection, profile.account)?;
            Ok(ProfileSaved::Saved { version, removed })
        })
        .await
    }

    async fn set_names(&self, names: NewNames) -> Result<NamesSaved> {
        self.write(move |connection| {
            if let Some(username) = &names.username {
                let taken: bool = connection.query_row(
                    "SELECT EXISTS (SELECT 1 FROM account WHERE username = ?1 AND id != ?2)",
                    params![username, names.account.bytes()],
                    |row| row.get(0),
                )?;
                if taken {
                    return Ok(NamesSaved::UsernameTaken);
                }
                connection.execute(
                    "UPDATE account SET username = ?2 WHERE id = ?1",
                    params![names.account.bytes(), username],
                )?;
            }
            if let Some(display_name) = &names.display_name {
                connection.execute(
                    "UPDATE account SET display_name = ?2 WHERE id = ?1",
                    params![names.account.bytes(), display_name],
                )?;
            }
            let found = connection
                .query_row(
                    "SELECT username, display_name FROM account WHERE id = ?1",
                    params![names.account.bytes()],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
                )
                .optional()?;
            let Some((username, display_name)) = found else {
                return Ok(NamesSaved::NoSuchAccount);
            };
            let version = next_profile_version(connection, names.account)?;
            Ok(NamesSaved::Saved {
                version,
                username,
                display_name,
            })
        })
        .await
    }

    async fn add_photo(&self, photo: NewPhoto) -> Result<Vec<Ulid>> {
        self.write(move |connection| {
            let mut statement = connection.prepare(
                "DELETE FROM file WHERE account_id = ?1 AND used_at IS NULL RETURNING id",
            )?;
            let mut removed = Vec::new();
            for id in statement.query_map(params![photo.account.bytes()], |row| row.get(0))? {
                removed.push(Ulid::from_bytes(id?));
            }
            connection.execute(
                "INSERT INTO file (id, account_id, encrypted, kind, size, chunks, received,
                 uploaded_by, created_at) VALUES (?1, ?2, 1, 3, ?3, 1, 1, ?4, ?5)",
                params![
                    photo.id.bytes(),
                    photo.account.bytes(),
                    int(photo.size),
                    photo.device.bytes(),
                    photo.now
                ],
            )?;
            Ok(removed)
        })
        .await
    }

    async fn photo(&self, id: Ulid) -> Result<Option<Ulid>> {
        self.read(move |connection| {
            let account: Option<[u8; 16]> = connection
                .query_row(
                    "SELECT account_id FROM file WHERE id = ?1 AND account_id IS NOT NULL
                     AND used_at IS NOT NULL",
                    params![id.bytes()],
                    |row| row.get(0),
                )
                .optional()?;
            Ok(account.map(Ulid::from_bytes))
        })
        .await
    }

    async fn remove_photo(&self, account: Ulid) -> Result<Option<(Vec<Ulid>, u64)>> {
        self.write(move |connection| {
            let removed = take_photos(connection, account, None)?;
            if removed.is_empty() {
                return Ok(None);
            }
            let version = next_profile_version(connection, account)?;
            Ok(Some((removed, version)))
        })
        .await
    }

    async fn create_file(&self, file: NewFile) -> Result<()> {
        self.write(move |connection| {
            connection.execute(
                "INSERT INTO file (id, conversation_id, encrypted, kind, size, chunks,
                 uploaded_by, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    file.id.bytes(),
                    file.chat.bytes(),
                    file.encrypted,
                    file.kind as u8,
                    int(file.size),
                    int(file.chunks),
                    file.device.bytes(),
                    file.now
                ],
            )?;
            Ok(())
        })
        .await
    }

    async fn file(&self, id: Ulid) -> Result<Option<StoredFile>> {
        self.read(move |connection| {
            connection
                .query_row(
                    "SELECT f.conversation_id, d.account_id, f.size, f.chunks, f.received,
                     f.used_at IS NOT NULL, f.removed_at IS NOT NULL
                     FROM file f JOIN device d ON d.id = f.uploaded_by
                     WHERE f.id = ?1 AND f.conversation_id IS NOT NULL",
                    params![id.bytes()],
                    |row| {
                        Ok(StoredFile {
                            chat: Ulid::from_bytes(row.get(0)?),
                            account: Ulid::from_bytes(row.get(1)?),
                            size: unsigned(row.get(2)?),
                            chunks: unsigned(row.get(3)?),
                            received: unsigned(row.get(4)?),
                            used: row.get(5)?,
                            removed: row.get(6)?,
                        })
                    },
                )
                .optional()
        })
        .await
    }

    async fn file_received(&self, id: Ulid, n: u64) -> Result<()> {
        self.write(move |connection| {
            connection.execute(
                "UPDATE file SET received = ?2 + 1 WHERE id = ?1 AND received = ?2",
                params![id.bytes(), int(n)],
            )?;
            Ok(())
        })
        .await
    }

    async fn take_unused_files(
        &self,
        chat: Option<Ulid>,
        before: i64,
    ) -> Result<Vec<(Ulid, Ulid)>> {
        self.write(move |connection| {
            let chat = chat.map(|chat| chat.bytes().to_vec());
            let mut gone = Vec::new();
            let mut statement = connection.prepare(
                "DELETE FROM file WHERE used_at IS NULL AND created_at < ?2
                 AND conversation_id IS NOT NULL AND (?1 IS NULL OR conversation_id = ?1)
                 RETURNING conversation_id, id",
            )?;
            let rows = statement.query_map(params![chat, before], |row| {
                Ok((Ulid::from_bytes(row.get(0)?), Ulid::from_bytes(row.get(1)?)))
            })?;
            for row in rows {
                gone.push(row?);
            }
            let mut statement = connection.prepare(
                "SELECT conversation_id, id FROM file WHERE removed_at IS NOT NULL
                 AND conversation_id IS NOT NULL AND (?1 IS NULL OR conversation_id = ?1)",
            )?;
            let rows = statement.query_map(params![chat], |row| {
                Ok((Ulid::from_bytes(row.get(0)?), Ulid::from_bytes(row.get(1)?)))
            })?;
            for row in rows {
                gone.push(row?);
            }
            Ok(gone)
        })
        .await
    }
}
