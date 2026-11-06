//! The storage trait on SQLite, and the only place SQL lives.
//!
//! One `.db` file with no `.wal`: the classic `DELETE` journal, so a `-journal` file exists only
//! during a write. One writer and one reader connection, each behind a lock, taking turns on the
//! blocking pool. If that becomes too slow, the answer is Postgres.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use keryx_core::protocol::{
    AccountBrief, Bytes, CallEvent, ChatDevice, ChatItem, ChatMode, ChatType, DeviceItem,
    DeviceLogItem, KeyPackageItem, Kind, MarkItem, MessageFrame, SessionItem, WrapEntry,
};
use keryx_core::ulid::Ulid;
use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior, params};

use super::{
    Account, Appended, BuiltInRole, CallChange, ChatAccess, DeviceKey, DeviceUsage, Error,
    GroupInfoPut, MessageBrief, NewAccount, NewDevice, NewFile, NewMessage, NewSession, NewWrap,
    Position, PositionReport, Registered, Result, Session, SetupLink, Storage, StoredFile,
};

/// How long a connection waits for a lock another process holds, such as a backup.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

const MESSAGE_COLUMNS: &str = "conversation_id, seq, device_id, kind, content_kind, encrypted, \
     epoch, pointer_seq, call_outcome, client_sent_at, server_saved_at, body, signature, \
     call_ended_at";

/// A message's kind, content kind, sender's account, sender's device's age, and save time.
type BriefRow = (i64, Option<i64>, [u8; 16], i64, i64);

pub struct Sqlite {
    writer: Arc<Mutex<Connection>>,
    reader: Arc<Mutex<Connection>>,
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
        Ok(Self {
            writer: Arc::new(Mutex::new(writer)),
            reader: Arc::new(Mutex::new(reader)),
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

/// A sequence id or epoch as SQLite holds it. A larger number than any stored can match nothing.
fn int(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
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
    let body: Option<Vec<u8>> = row.get(11)?;
    let signature: Option<Vec<u8>> = row.get(12)?;
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
        client_sent_at: row.get(9)?,
        server_saved_at: row.get(10)?,
        body: body.map(Bytes),
        signature: signature.map(Bytes),
        call_outcome: opt_numbered(row.get(8)?)?,
        call_ended_at: row.get(13)?,
    })
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
    })
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
    })
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
                     account_public_key, account_key_stamp FROM account WHERE username = ?1",
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
                     account_public_key, account_key_stamp FROM account WHERE id = ?1",
                    params![id.bytes()],
                    account_row,
                )
                .optional()
        })
        .await
    }

    async fn search_accounts(&self, term: String, limit: u32) -> Result<Vec<AccountBrief>> {
        self.read(move |connection| {
            let mut escaped = String::with_capacity(term.len() + 2);
            escaped.push('%');
            for character in term.chars() {
                if matches!(character, '%' | '_' | '\\') {
                    escaped.push('\\');
                }
                escaped.push(character);
            }
            escaped.push('%');
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
        master_public: Vec<u8>,
        account_public: Vec<u8>,
        stamp: Vec<u8>,
        chat: Ulid,
        now: i64,
    ) -> Result<bool> {
        self.write(move |connection| {
            let changed = connection.execute(
                "UPDATE account SET master_public_key = ?2, account_public_key = ?3,
                 account_key_stamp = ?4, words_set_at = ?5
                 WHERE id = ?1 AND master_public_key IS NULL",
                params![account.bytes(), master_public, account_public, stamp, now],
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

    async fn server_permitted(&self, account: Ulid, permission: &'static str) -> Result<bool> {
        self.read(move |connection| {
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
        })
        .await
    }

    async fn chat_permitted(
        &self,
        chat: Ulid,
        account: Ulid,
        permission: &'static str,
    ) -> Result<bool> {
        self.read(move |connection| {
            connection.query_row(
                "SELECT EXISTS (
                     SELECT 1 FROM account_role a JOIN role r ON r.id = a.role_id
                     WHERE a.account_id = ?2 AND r.scope = 0 AND r.built_in = 1 AND r.name = 'admin'
                 ) OR EXISTS (
                     SELECT 1 FROM conversation c
                     JOIN role r ON r.scope = 1 AND r.built_in = 1 AND r.name = 'member'
                     JOIN role_permission p ON p.role_id = r.id AND p.permission = ?3
                     WHERE c.id = ?1 AND c.type IN (0, 1)
                 ) OR EXISTS (
                     SELECT 1 FROM member_role m
                     JOIN role_permission p ON p.role_id = m.role_id AND p.permission = ?3
                     WHERE m.conversation_id = ?1 AND m.account_id = ?2
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
                "UPDATE conversation SET last_seq = last_seq + 1, last_kind = 4, last_at = ?2
                 WHERE type = 0 AND direct_low = ?1
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
                client_sent_at: Some(device.now),
                server_saved_at: Some(device.now),
                body: Some(Bytes(Vec::new())),
                signature: Some(Bytes(device.stamp)),
                call_outcome: None,
                call_ended_at: None,
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

    async fn chat_access(&self, chat: Ulid, account: Ulid) -> Result<Option<ChatAccess>> {
        self.read(move |connection| {
            let row: Option<(i64, i64, i64)> = connection
                .query_row(
                    "SELECT c.type, c.mode, c.last_seq FROM conversation c
                     JOIN member m ON m.conversation_id = c.id
                     WHERE c.id = ?1 AND m.account_id = ?2",
                    params![chat.bytes(), account.bytes()],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()?;
            let Some((chat_type, mode, last_seq)) = row else {
                return Ok(None);
            };
            let chat_type: ChatType = numbered(chat_type)?;
            let mode: ChatMode = numbered(mode)?;
            Ok(Some(ChatAccess {
                chat_type,
                mode,
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

    async fn chat_list(&self, account: Ulid) -> Result<Vec<ChatItem>> {
        self.read(move |connection| {
            let mut statement = connection.prepare(
                "SELECT c.id, c.type, c.mode, c.last_seq, c.last_kind, c.last_at, c.last_content_seq,
                 g.epoch, m.seen_seq
                 FROM member m JOIN conversation c ON c.id = m.conversation_id
                 LEFT JOIN mls_group g ON g.conversation_id = c.id
                 WHERE m.account_id = ?1
                 ORDER BY COALESCE(c.last_at, c.created_at) DESC",
            )?;
            let rows = statement.query_map(params![account.bytes()], |row| {
                let last_seq: i64 = row.get(3)?;
                Ok(ChatItem {
                    chat: Ulid::from_bytes(row.get(0)?),
                    chat_type: numbered(row.get(1)?)?,
                    mode: numbered(row.get(2)?)?,
                    last_seq: if last_seq == 0 {
                        None
                    } else {
                        Some(unsigned(last_seq))
                    },
                    last_kind: opt_numbered(row.get(4)?)?,
                    last_at: row.get(5)?,
                    last_content_seq: opt_unsigned(row.get(6)?),
                    epoch: opt_unsigned(row.get(7)?),
                    last_content: None,
                    mark_list: Vec::new(),
                    seen_seq: Some(unsigned(row.get(8)?)),
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
            for item in &mut chat_list {
                if let Some(seq) = item.last_content_seq {
                    let message = content
                        .query_row(params![item.chat.bytes(), int(seq)], message_frame)
                        .optional()?;
                    if let Some(message) = message {
                        item.last_content = Some(Box::new(message));
                    }
                }
                if item.chat_type == ChatType::Channel {
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
            }
            Ok(chat_list)
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

    async fn message_brief(&self, chat: Ulid, seq: u64) -> Result<Option<MessageBrief>> {
        self.read(move |connection| {
            let row: Option<BriefRow> = connection
                .query_row(
                    "SELECT m.kind, m.content_kind, d.account_id, d.created_at, m.server_saved_at
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
                        ))
                    },
                )
                .optional()?;
            let Some((kind, content_kind, account, device_created_at, saved_at)) = row else {
                return Ok(None);
            };
            Ok(Some(MessageBrief {
                kind: numbered(kind)?,
                content_kind: opt_numbered(content_kind)?,
                account: Ulid::from_bytes(account),
                device_created_at,
                saved_at,
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
            let seq: i64 = connection.query_row(
                "UPDATE conversation SET last_seq = last_seq + 1, last_kind = ?2 WHERE id = ?1
                 RETURNING last_seq",
                params![chat.bytes(), message.kind as u8],
                |row| row.get(0),
            )?;
            if message.kind == Kind::Delete {
                connection.execute(
                    "UPDATE message SET device_id = NULL, kind = NULL, content_kind = NULL,
                     encrypted = NULL, epoch = NULL, pointer_seq = NULL,
                     call_outcome = NULL, call_ended_at = NULL, client_sent_at = NULL,
                     server_saved_at = NULL,
                     expires_at = NULL, signature = NULL, body = NULL
                     WHERE conversation_id = ?1 AND (seq = ?2 OR (kind = 1 AND pointer_seq = ?2))",
                    params![chat.bytes(), opt_int(message.pointer_seq)],
                )?;
                // A deleted call takes what happened inside it along.
                connection.execute(
                    "DELETE FROM call_event WHERE conversation_id = ?1 AND call_seq = ?2",
                    params![chat.bytes(), opt_int(message.pointer_seq)],
                )?;
            }
            let content_kind = message.content_kind.map(|content_kind| content_kind as u8);
            connection.execute(
                "INSERT INTO message (conversation_id, seq, device_id, kind, content_kind,
                 encrypted, epoch, pointer_seq, client_sent_at, server_saved_at,
                 signature, body) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
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
                    message.body
                ],
            )?;
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
                    connection.execute(
                        "UPDATE conversation SET last_at = ?2, last_content_seq = ?3 WHERE id = ?1",
                        params![chat.bytes(), message.now, seq],
                    )?;
                }
                Kind::Delete => {
                    connection.execute(
                        "UPDATE conversation SET
                         last_content_seq = (
                             SELECT seq FROM message WHERE conversation_id = ?1 AND kind = 0
                             ORDER BY seq DESC LIMIT 1
                         ),
                         last_at = (
                             SELECT server_saved_at FROM message WHERE conversation_id = ?1
                             AND kind IN (0, 4)
                             ORDER BY seq DESC LIMIT 1
                         )
                         WHERE id = ?1",
                        params![chat.bytes()],
                    )?;
                }
                Kind::Commit => {
                    connection.execute(
                        "UPDATE mls_group SET epoch = ?2, group_info = ?3, updated_at = ?4
                         WHERE conversation_id = ?1",
                        params![
                            chat.bytes(),
                            opt_int(message.epoch),
                            message.group_info,
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
            Ok(Appended::Saved(unsigned(seq)))
        })
        .await
    }

    async fn history(
        &self,
        chat: Ulid,
        after_seq: u64,
        before_seq: Option<u64>,
        limit: u32,
    ) -> Result<Vec<MessageFrame>> {
        self.read(move |connection| {
            let mut message_list = Vec::new();
            if let Some(before_seq) = before_seq {
                let mut statement = connection.prepare(&format!(
                    "SELECT {MESSAGE_COLUMNS} FROM message
                     WHERE conversation_id = ?1 AND seq > ?2 AND seq < ?3
                     ORDER BY seq DESC LIMIT ?4"
                ))?;
                let rows = statement.query_map(
                    params![chat.bytes(), int(after_seq), int(before_seq), limit],
                    message_frame,
                )?;
                for message in rows {
                    message_list.push(message?);
                }
                message_list.reverse();
            } else {
                let mut statement = connection.prepare(&format!(
                    "SELECT {MESSAGE_COLUMNS} FROM message WHERE conversation_id = ?1 AND seq > ?2
                     ORDER BY seq LIMIT ?3"
                ))?;
                let rows = statement
                    .query_map(params![chat.bytes(), int(after_seq), limit], message_frame)?;
                for message in rows {
                    message_list.push(message?);
                }
            }
            Ok(message_list)
        })
        .await
    }

    async fn record_position(&self, report: PositionReport) -> Result<Option<MarkItem>> {
        let (update, insert) = match report.position {
            Position::Received => (
                "UPDATE member SET received_seq = ?3
                 WHERE conversation_id = ?1 AND account_id = ?2 AND received_seq < ?3",
                "INSERT INTO received (conversation_id, account_id, up_to_seq, device_id,
                 client_received_at, server_received_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            ),
            Position::Seen => (
                "UPDATE member SET seen_seq = ?3
                 WHERE conversation_id = ?1 AND account_id = ?2 AND seen_seq < ?3",
                "INSERT INTO seen (conversation_id, account_id, up_to_seq, device_id,
                 client_seen_at, server_seen_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            ),
        };
        self.write(move |connection| {
            let PositionReport {
                chat,
                account,
                device,
                up_to_seq,
                client_at,
                now,
                ..
            } = report;
            let changed = connection.execute(
                update,
                params![chat.bytes(), account.bytes(), int(up_to_seq)],
            )?;
            if changed == 0 {
                return Ok(None);
            }
            connection.execute(
                insert,
                params![
                    chat.bytes(),
                    account.bytes(),
                    int(up_to_seq),
                    device.bytes(),
                    client_at,
                    now
                ],
            )?;
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

    async fn put_group_info(
        &self,
        chat: Ulid,
        epoch: u64,
        body: Vec<u8>,
        now: i64,
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
                    connection.execute(
                        "INSERT INTO mls_group (conversation_id, epoch, group_info, updated_at)
                         VALUES (?1, 0, ?2, ?3)",
                        params![chat.bytes(), body, now],
                    )?;
                    Ok(GroupInfoPut::Saved)
                }
                None => Ok(GroupInfoPut::Stale),
                Some(0) if epoch == 0 => {
                    let stored: Vec<u8> = connection.query_row(
                        "SELECT group_info FROM mls_group WHERE conversation_id = ?1",
                        params![chat.bytes()],
                        |row| row.get(0),
                    )?;
                    if stored == body {
                        Ok(GroupInfoPut::Saved)
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
                    Ok(GroupInfoPut::Saved)
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

    async fn save_call(&self, change: CallChange) -> Result<()> {
        self.write(move |connection| {
            for event in &change.event_list {
                connection.execute(
                    "INSERT INTO call_event (conversation_id, call_seq, n, kind, device_id,
                     server_at, client_at, epoch, signature, body)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                    params![
                        change.chat.bytes(),
                        int(change.call_seq),
                        int(event.n),
                        event.kind as u8,
                        event.device.bytes(),
                        event.server_at,
                        event.client_at,
                        opt_int(event.epoch),
                        event.signature.as_ref().map(|signature| &signature.0),
                        event.body.as_ref().map(|body| &body.0)
                    ],
                )?;
            }
            if change.outcome.is_some() || change.ended_at.is_some() {
                connection.execute(
                    "UPDATE message SET call_outcome = COALESCE(call_outcome, ?3),
                     call_ended_at = COALESCE(call_ended_at, ?4)
                     WHERE conversation_id = ?1 AND seq = ?2 AND content_kind = 8",
                    params![
                        change.chat.bytes(),
                        int(change.call_seq),
                        change.outcome.map(|outcome| outcome as u8),
                        change.ended_at
                    ],
                )?;
            }
            Ok(())
        })
        .await
    }

    async fn call_events(&self, chat: Ulid, call_seq: u64) -> Result<Vec<CallEvent>> {
        self.read(move |connection| {
            let mut statement = connection.prepare(
                "SELECT conversation_id, call_seq, n, kind, device_id, server_at, client_at,
                 epoch, signature, body FROM call_event
                 WHERE conversation_id = ?1 AND call_seq = ?2 ORDER BY n",
            )?;
            let rows = statement.query_map(params![chat.bytes(), int(call_seq)], call_event_row)?;
            let mut event_list = Vec::new();
            for event in rows {
                event_list.push(event?);
            }
            Ok(event_list)
        })
        .await
    }

    async fn end_open_calls(&self, now: i64) -> Result<()> {
        self.write(move |connection| {
            connection.execute(
                "INSERT INTO call_event (conversation_id, call_seq, n, kind, device_id, server_at)
                 SELECT m.conversation_id, m.seq,
                 COALESCE((SELECT MAX(e.n) FROM call_event e
                           WHERE e.conversation_id = m.conversation_id AND e.call_seq = m.seq), 0)
                 + 1, 4, m.device_id, ?1
                 FROM message m WHERE m.content_kind = 8 AND m.call_ended_at IS NULL",
                params![now],
            )?;
            connection.execute(
                "UPDATE message SET call_outcome = COALESCE(call_outcome, 0), call_ended_at = ?1
                 WHERE content_kind = 8 AND call_ended_at IS NULL",
                params![now],
            )?;
            Ok(())
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
