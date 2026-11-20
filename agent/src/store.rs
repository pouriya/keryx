//! `agent.db`: its schema and every query. One SQLite file with the server's `DELETE` journal, so a
//! crash mid-write rolls back, holding core's snapshot slots, the archive of raw frames, both queues,
//! the run log, schedules and memory notes, so one transaction covers a loop step and its queue
//! changes. See Storage in agent/AGENTS.md.

use std::path::Path;

use keryx_core::client::ScanAt;
use keryx_core::protocol::{CallEvent, ChatEvent, Kind, MessageFrame};
use keryx_core::ulid::Ulid;
use rusqlite::{Connection, OptionalExtension, Row, params};

/// The schema, run once on a new file.
const SCHEMA: &str = include_str!("agent.sql");

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("agent.db: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("agent.db holds a row it cannot read: {0}")]
    Json(#[from] serde_json::Error),
    #[error("agent.db is version {0}, and this keryx-agent reads version 1")]
    Version(i64),
}

type Result<T> = std::result::Result<T, Error>;

/// What may wake the loop, as the `event.kind` column numbers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    Message = 0,
    Reaction = 1,
    Answer = 2,
    Ack = 3,
    Added = 4,
    Removed = 5,
    Invitation = 6,
    JoinRequest = 7,
    Edit = 8,
    Delete = 9,
    Schedule = 10,
    Storage = 11,
    Sent = 12,
}

/// Every kind, in the column's order, so a number finds its kind.
const EVENT_KINDS: [EventKind; 13] = [
    EventKind::Message,
    EventKind::Reaction,
    EventKind::Answer,
    EventKind::Ack,
    EventKind::Added,
    EventKind::Removed,
    EventKind::Invitation,
    EventKind::JoinRequest,
    EventKind::Edit,
    EventKind::Delete,
    EventKind::Schedule,
    EventKind::Storage,
    EventKind::Sent,
];

impl EventKind {
    /// The name `[[wake]]`'s `event` and the loop's JSON use.
    pub fn name(self) -> &'static str {
        match self {
            Self::Message => "message",
            Self::Reaction => "reaction",
            Self::Answer => "answer",
            Self::Ack => "ack",
            Self::Added => "added",
            Self::Removed => "removed",
            Self::Invitation => "invitation",
            Self::JoinRequest => "join_request",
            Self::Edit => "edit",
            Self::Delete => "delete",
            Self::Schedule => "schedule",
            Self::Storage => "storage",
            Self::Sent => "sent",
        }
    }

    fn of(number: i64) -> rusqlite::Result<Self> {
        match usize::try_from(number)
            .ok()
            .and_then(|n| EVENT_KINDS.get(n))
        {
            Some(kind) => Ok(*kind),
            None => Err(rusqlite::Error::IntegralValueOutOfRange(1, number)),
        }
    }
}

/// An event before it is kept.
pub struct NewEvent {
    pub kind: EventKind,
    pub chat: Option<Ulid>,
    pub topic: Option<Ulid>,
    pub thread_seq: Option<u64>,
    pub seq: Option<u64>,
    pub body: serde_json::Value,
    pub until: Option<i64>,
}

/// One kept event.
#[derive(Debug, Clone)]
pub struct EventRow {
    pub id: Ulid,
    pub kind: EventKind,
    pub chat: Option<Ulid>,
    pub topic: Option<Ulid>,
    pub thread_seq: Option<u64>,
    pub seq: Option<u64>,
    pub body: serde_json::Value,
    pub received_at: i64,
    pub run: Option<Ulid>,
    pub step: Option<u64>,
    /// Put back by `inbox.defer`: with no `until` it rides along with the next wake rather than
    /// waking the loop itself.
    pub deferred: bool,
    pub until: Option<i64>,
}

/// What a write's row in the outgoing queue says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutgoingState {
    Queued = 0,
    Sent = 1,
    Done = 2,
    Refused = 3,
}

/// One write in the outgoing queue.
#[derive(Debug, Clone)]
pub struct OutgoingRow {
    pub id: Ulid,
    pub run: Option<Ulid>,
    pub step: Option<u64>,
    pub n: Option<u64>,
    pub chat: Ulid,
    pub command: serde_json::Value,
    pub client_at: i64,
    pub state: OutgoingState,
    pub seq: Option<u64>,
    pub code: Option<String>,
}

/// How a run ended, as the `run.state` column numbers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunState {
    Running = 0,
    Done = 1,
    Limit = 2,
    Failed = 3,
}

impl RunState {
    pub fn name(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Done => "done",
            Self::Limit => "limit",
            Self::Failed => "failed",
        }
    }

    fn of(number: i64) -> rusqlite::Result<Self> {
        match number {
            0 => Ok(Self::Running),
            1 => Ok(Self::Done),
            2 => Ok(Self::Limit),
            3 => Ok(Self::Failed),
            _ => Err(rusqlite::Error::IntegralValueOutOfRange(3, number)),
        }
    }
}

/// One run of the loop.
#[derive(Debug, Clone)]
pub struct RunRow {
    pub id: Ulid,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub state: RunState,
}

/// One step of a run, as far as its phases went.
#[derive(Debug, Clone, Default)]
pub struct StepRow {
    pub n: u64,
    pub phase: u8,
    pub context: String,
    pub intention_list: Option<String>,
    pub answer: Option<String>,
    pub script: Option<String>,
    pub sent_at: Option<i64>,
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
    pub cut: bool,
    pub tokens_in: Option<u64>,
    pub tokens_out: Option<u64>,
}

/// One print or action of a step's script.
#[derive(Debug, Clone)]
pub struct OutputRow {
    pub n: u64,
    pub action: bool,
    pub body: String,
    pub at: i64,
}

/// One schedule.
#[derive(Debug, Clone)]
pub struct ScheduleRow {
    pub id: Ulid,
    /// 0 at, 1 every, 2 cron.
    pub kind: u8,
    pub spec: String,
    pub tz: String,
    pub start: Option<i64>,
    pub note: String,
    pub chat_list: Vec<Ulid>,
    pub join: bool,
    pub next_at: i64,
    pub fired_at: Option<i64>,
    pub run: Option<Ulid>,
    pub made_at: i64,
}

/// One memory note.
#[derive(Debug, Clone)]
pub struct NoteRow {
    pub id: Ulid,
    pub chat: Option<Ulid>,
    pub text: String,
    pub run: Ulid,
    pub step: u64,
    pub source_chat: Option<Ulid>,
    pub seq: Option<u64>,
    pub made_at: i64,
    pub updated_at: i64,
}

/// The bytes each kind of row holds, for `storage.usage`.
#[derive(Debug, Clone, Default)]
pub struct Usage {
    pub file: u64,
    pub runs: u64,
    pub memory: u64,
    pub archive: u64,
}

/// A page of the archive with what lies among it: its call rows and the chat's history rows.
pub struct Rows {
    pub message_list: Vec<MessageFrame>,
    pub call_list: Vec<CallEvent>,
    pub chat_event_list: Vec<ChatEvent>,
}

fn blob(id: Ulid) -> Vec<u8> {
    id.bytes().to_vec()
}

fn opt_blob(id: Option<Ulid>) -> Option<Vec<u8>> {
    id.map(blob)
}

fn ulid_at(row: &Row<'_>, index: usize) -> rusqlite::Result<Ulid> {
    let bytes: Vec<u8> = row.get(index)?;
    match <[u8; 16]>::try_from(bytes.as_slice()) {
        Ok(bytes) => Ok(Ulid::from_bytes(bytes)),
        Err(_) => Err(rusqlite::Error::InvalidColumnType(
            index,
            "a ULID".to_string(),
            rusqlite::types::Type::Blob,
        )),
    }
}

fn opt_ulid_at(row: &Row<'_>, index: usize) -> rusqlite::Result<Option<Ulid>> {
    let bytes: Option<Vec<u8>> = row.get(index)?;
    match bytes {
        Some(bytes) => match <[u8; 16]>::try_from(bytes.as_slice()) {
            Ok(bytes) => Ok(Some(Ulid::from_bytes(bytes))),
            Err(_) => Err(rusqlite::Error::InvalidColumnType(
                index,
                "a ULID".to_string(),
                rusqlite::types::Type::Blob,
            )),
        },
        None => Ok(None),
    }
}

fn opt_u64_at(row: &Row<'_>, index: usize) -> rusqlite::Result<Option<u64>> {
    let value: Option<i64> = row.get(index)?;
    Ok(value.and_then(|value| u64::try_from(value).ok()))
}

fn u64_at(row: &Row<'_>, index: usize) -> rusqlite::Result<u64> {
    let value: i64 = row.get(index)?;
    Ok(u64::try_from(value).unwrap_or(0))
}

fn int(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn opt_int(value: Option<u64>) -> Option<i64> {
    value.map(int)
}

fn json_at(row: &Row<'_>, index: usize) -> rusqlite::Result<serde_json::Value> {
    let text: String = row.get(index)?;
    serde_json::from_str(&text).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}

fn chats_blob(chat_list: &[Ulid]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(chat_list.len() * 16);
    for chat in chat_list {
        bytes.extend_from_slice(chat.bytes());
    }
    bytes
}

fn chats_of(bytes: &[u8]) -> Vec<Ulid> {
    let mut chat_list = Vec::new();
    for chunk in bytes.chunks_exact(16) {
        if let Ok(id) = <[u8; 16]>::try_from(chunk) {
            chat_list.push(Ulid::from_bytes(id));
        }
    }
    chat_list
}

fn new_id() -> Ulid {
    match Ulid::new() {
        Ok(id) => id,
        Err(_) => Ulid::from_bytes([0; 16]),
    }
}

const EVENT_COLUMNS: &str =
    "id, kind, chat, topic, thread_seq, seq, body, received_at, run, step, state, until";

fn event_row(row: &Row<'_>) -> rusqlite::Result<EventRow> {
    Ok(EventRow {
        id: ulid_at(row, 0)?,
        kind: EventKind::of(row.get(1)?)?,
        chat: opt_ulid_at(row, 2)?,
        topic: opt_ulid_at(row, 3)?,
        thread_seq: opt_u64_at(row, 4)?,
        seq: opt_u64_at(row, 5)?,
        body: json_at(row, 6)?,
        received_at: row.get(7)?,
        run: opt_ulid_at(row, 8)?,
        step: opt_u64_at(row, 9)?,
        deferred: row.get::<_, i64>(10)? == 2,
        until: row.get(11)?,
    })
}

const OUTGOING_COLUMNS: &str = "id, run, step, n, chat, command, client_at, state, seq, code";

fn outgoing_row(row: &Row<'_>) -> rusqlite::Result<OutgoingRow> {
    let state = match row.get::<_, i64>(7)? {
        0 => OutgoingState::Queued,
        1 => OutgoingState::Sent,
        2 => OutgoingState::Done,
        _ => OutgoingState::Refused,
    };
    Ok(OutgoingRow {
        id: ulid_at(row, 0)?,
        run: opt_ulid_at(row, 1)?,
        step: opt_u64_at(row, 2)?,
        n: opt_u64_at(row, 3)?,
        chat: ulid_at(row, 4)?,
        command: json_at(row, 5)?,
        client_at: row.get(6)?,
        state,
        seq: opt_u64_at(row, 8)?,
        code: row.get(9)?,
    })
}

const STEP_COLUMNS: &str = "n, phase, context, intention_list, answer, script, sent_at, started_at, \
     ended_at, cut, tokens_in, tokens_out";

fn step_row(row: &Row<'_>) -> rusqlite::Result<StepRow> {
    let phase: i64 = row.get(1)?;
    Ok(StepRow {
        n: u64_at(row, 0)?,
        phase: u8::try_from(phase).unwrap_or(1),
        context: row.get(2)?,
        intention_list: row.get(3)?,
        answer: row.get(4)?,
        script: row.get(5)?,
        sent_at: row.get(6)?,
        started_at: row.get(7)?,
        ended_at: row.get(8)?,
        cut: row.get::<_, i64>(9)? == 1,
        tokens_in: opt_u64_at(row, 10)?,
        tokens_out: opt_u64_at(row, 11)?,
    })
}

const RUN_COLUMNS: &str = "id, started_at, ended_at, state";

fn run_row(row: &Row<'_>) -> rusqlite::Result<RunRow> {
    Ok(RunRow {
        id: ulid_at(row, 0)?,
        started_at: row.get(1)?,
        ended_at: row.get(2)?,
        state: RunState::of(row.get(3)?)?,
    })
}

const SCHEDULE_COLUMNS: &str =
    "id, kind, spec, tz, start, note, chats, join_run, next_at, fired_at, run, made_at";

fn schedule_row(row: &Row<'_>) -> rusqlite::Result<ScheduleRow> {
    let kind: i64 = row.get(1)?;
    let chats: Vec<u8> = row.get(6)?;
    Ok(ScheduleRow {
        id: ulid_at(row, 0)?,
        kind: u8::try_from(kind).unwrap_or(0),
        spec: row.get(2)?,
        tz: row.get(3)?,
        start: row.get(4)?,
        note: row.get(5)?,
        chat_list: chats_of(&chats),
        join: row.get::<_, i64>(7)? == 1,
        next_at: row.get(8)?,
        fired_at: row.get(9)?,
        run: opt_ulid_at(row, 10)?,
        made_at: row.get(11)?,
    })
}

const NOTE_COLUMNS: &str = "id, chat, text, run, step, source_chat, seq, made_at, updated_at";

fn note_row(row: &Row<'_>) -> rusqlite::Result<NoteRow> {
    Ok(NoteRow {
        id: ulid_at(row, 0)?,
        chat: opt_ulid_at(row, 1)?,
        text: row.get(2)?,
        run: ulid_at(row, 3)?,
        step: u64_at(row, 4)?,
        source_chat: opt_ulid_at(row, 5)?,
        seq: opt_u64_at(row, 6)?,
        made_at: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

fn frames<T: serde::de::DeserializeOwned>(texts: Vec<String>) -> Result<Vec<T>> {
    let mut list = Vec::with_capacity(texts.len());
    for text in texts {
        list.push(serde_json::from_str(&text)?);
    }
    Ok(list)
}

/// One connection to `agent.db`. Callers share it behind a lock, as the loop and the socket both
/// write, and take turns.
pub struct Store {
    connection: Connection,
}

impl Store {
    /// Opens `agent.db` in `directory`, making it with its tables the first time.
    pub fn open(directory: &Path) -> Result<Self> {
        let connection = Connection::open(directory.join("agent.db"))?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        connection.pragma_update(None, "journal_mode", "DELETE")?;
        let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        match version {
            0 => connection.execute_batch(&format!(
                "BEGIN IMMEDIATE;\n{SCHEMA}\nPRAGMA user_version = 1;\nCOMMIT;"
            ))?,
            1 => {}
            other => return Err(Error::Version(other)),
        }
        Ok(Self { connection })
    }

    // ---------------------------------------------------------------------------------------------
    // What core keeps
    // ---------------------------------------------------------------------------------------------

    /// Both snapshot slots, for core's `Loaded`.
    pub fn slots(&self) -> Result<[Option<Vec<u8>>; 2]> {
        let mut slots = [None, None];
        let mut statement = self
            .connection
            .prepare("SELECT slot, bytes FROM snapshot")?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let slot: i64 = row.get(0)?;
            if let Some(place) = usize::try_from(slot).ok().and_then(|n| slots.get_mut(n)) {
                *place = Some(row.get(1)?);
            }
        }
        Ok(slots)
    }

    pub fn save_slot(&self, slot: u8, bytes: &[u8], now: i64) -> Result<()> {
        self.connection.execute(
            "INSERT OR REPLACE INTO snapshot (slot, bytes, saved_at) VALUES (?1, ?2, ?3)",
            params![slot, bytes, now],
        )?;
        Ok(())
    }

    /// This device ended: both slots and the session go.
    pub fn wipe_slots(&self) -> Result<()> {
        self.connection
            .execute_batch("DELETE FROM snapshot; DELETE FROM session;")?;
        Ok(())
    }

    pub fn session(&self) -> Result<Option<String>> {
        Ok(self
            .connection
            .query_row("SELECT token FROM session WHERE id = 0", [], |row| {
                row.get(0)
            })
            .optional()?)
    }

    pub fn set_session(&self, token: Option<&str>) -> Result<()> {
        match token {
            Some(token) => self.connection.execute(
                "INSERT OR REPLACE INTO session (id, token) VALUES (0, ?1)",
                params![token],
            )?,
            None => self.connection.execute("DELETE FROM session", [])?,
        };
        Ok(())
    }

    // ---------------------------------------------------------------------------------------------
    // The archive
    // ---------------------------------------------------------------------------------------------

    /// Keeps rows exactly as they are, in one transaction, as core's `SaveRows` asks. A row already
    /// kept stays, except that a removed message replaces its copy and takes the edits pointing at
    /// it, and a removed call message that call's rows, as Android's archive does.
    pub fn save_rows(
        &mut self,
        message_list: &[MessageFrame],
        call_list: &[CallEvent],
        chat_event_list: &[ChatEvent],
    ) -> Result<()> {
        let transaction = self.connection.transaction()?;
        for message in message_list {
            let chat = blob(message.chat);
            let removed = message.kind.is_none();
            if removed {
                transaction.execute(
                    "DELETE FROM call_row WHERE chat = ?1 AND call_seq = ?2",
                    params![chat, int(message.seq)],
                )?;
                let edit_list: Vec<i64> = {
                    let mut statement = transaction.prepare(
                        "SELECT seq FROM message WHERE chat = ?1 AND kind = 1 AND pointer = ?2",
                    )?;
                    let rows = statement
                        .query_map(params![chat, int(message.seq)], |row| row.get::<_, i64>(0))?;
                    rows.collect::<rusqlite::Result<_>>()?
                };
                for seq in edit_list {
                    let wiped = MessageFrame {
                        chat: message.chat,
                        seq: u64::try_from(seq).unwrap_or(0),
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
                    };
                    transaction.execute(
                        "INSERT OR REPLACE INTO message (chat, seq, kind, content_kind, device, \
                         pointer, topic, thread_seq, saved_at, frame) \
                         VALUES (?1, ?2, NULL, NULL, NULL, NULL, NULL, NULL, ?3, ?4)",
                        params![
                            chat,
                            seq,
                            wiped.server_saved_at.unwrap_or(0),
                            serde_json::to_string(&wiped)?
                        ],
                    )?;
                }
            }
            let verb = if removed {
                "INSERT OR REPLACE"
            } else {
                "INSERT OR IGNORE"
            };
            transaction.execute(
                &format!(
                    "{verb} INTO message (chat, seq, kind, content_kind, device, pointer, topic, \
                     thread_seq, saved_at, frame) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)"
                ),
                params![
                    chat,
                    int(message.seq),
                    message.kind.map(u8::from),
                    message.content_kind.map(u8::from),
                    opt_blob(message.device),
                    opt_int(message.pointer_seq),
                    opt_blob(message.topic),
                    opt_int(message.thread_seq),
                    message.server_saved_at.unwrap_or(0),
                    serde_json::to_string(message)?
                ],
            )?;
        }
        for event in call_list {
            transaction.execute(
                "INSERT OR IGNORE INTO call_row (chat, call_seq, n, kind, outcome, frame) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    blob(event.chat),
                    int(event.call_seq),
                    int(event.n),
                    u8::from(event.kind),
                    event.outcome.map(u8::from),
                    serde_json::to_string(event)?
                ],
            )?;
        }
        for event in chat_event_list {
            transaction.execute(
                "INSERT OR IGNORE INTO chat_event (chat, n, audience, server_at, frame) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    blob(event.chat),
                    int(event.n),
                    u8::from(event.audience),
                    event.server_at,
                    serde_json::to_string(event)?
                ],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    /// Core's `ReadRows`: the chat's newest `limit` messages below `before`, oldest first, the rows
    /// of every call among them, and the chat's history rows in the same span, as Android reads them.
    pub fn read_rows(&self, chat: Ulid, before: Option<u64>, limit: u32) -> Result<Rows> {
        let texts: Vec<String> = {
            let mut statement = self.connection.prepare(
                "SELECT frame FROM message WHERE chat = ?1 AND seq < ?2 ORDER BY seq DESC LIMIT ?3",
            )?;
            let rows = statement.query_map(
                params![blob(chat), opt_int(before).unwrap_or(i64::MAX), limit],
                |row| row.get(0),
            )?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let mut message_list: Vec<MessageFrame> = frames(texts)?;
        message_list.reverse();
        self.with_among(chat, message_list, before)
    }

    /// Messages of one chat in a range, oldest first, for a script's `message_list`: after `after`,
    /// below `before`, the newest `limit` of them unless only `after` is given, then the oldest.
    /// Edits and deletes of what it holds come with it, so core shows each message's newest text.
    /// `topic` keeps one topic's, `Some(None)` being General's, and `thread` one thread's.
    pub fn read_range(
        &self,
        chat: Ulid,
        after: Option<u64>,
        before: Option<u64>,
        limit: u32,
        topic: Option<Option<Ulid>>,
        thread: Option<u64>,
    ) -> Result<Rows> {
        let from_oldest = after.is_some() && before.is_none();
        let order = if from_oldest { "ASC" } else { "DESC" };
        let (by_topic, topic) = match topic {
            Some(topic) => (1, opt_blob(topic)),
            None => (0, None),
        };
        let texts: Vec<String> = {
            let mut statement = self.connection.prepare(&format!(
                "SELECT frame FROM message WHERE chat = ?1 AND seq > ?2 AND seq < ?3 \
                 AND (kind IS NULL OR kind NOT IN (1, 2, 3)) \
                 AND (?5 = 0 OR topic IS ?6) AND (?7 IS NULL OR thread_seq = ?7) \
                 ORDER BY seq {order} LIMIT ?4"
            ))?;
            let rows = statement.query_map(
                params![
                    blob(chat),
                    opt_int(after).unwrap_or(0),
                    opt_int(before).unwrap_or(i64::MAX),
                    limit,
                    by_topic,
                    topic,
                    opt_int(thread)
                ],
                |row| row.get(0),
            )?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let mut message_list: Vec<MessageFrame> = frames(texts)?;
        if !from_oldest {
            message_list.reverse();
        }
        let mut pointer_list = Vec::new();
        for message in &message_list {
            pointer_list.push(message.seq);
        }
        for seq in pointer_list {
            message_list.extend(self.pointing_at(chat, seq)?);
        }
        message_list.sort_by_key(|message| message.seq);
        message_list.dedup_by_key(|message| message.seq);
        self.with_among(chat, message_list, before)
    }

    /// One message and the edits and deletes pointing at it.
    pub fn read_one(&self, chat: Ulid, seq: u64) -> Result<Rows> {
        let text: Option<String> = self
            .connection
            .query_row(
                "SELECT frame FROM message WHERE chat = ?1 AND seq = ?2",
                params![blob(chat), int(seq)],
                |row| row.get(0),
            )
            .optional()?;
        let mut message_list = Vec::new();
        if let Some(text) = text {
            message_list.push(serde_json::from_str(&text)?);
            message_list.extend(self.pointing_at(chat, seq)?);
        }
        Ok(Rows {
            message_list,
            call_list: Vec::new(),
            chat_event_list: Vec::new(),
        })
    }

    /// The messages after `seq` in one chat, oldest first, at most `limit`: where its reactions,
    /// answers and acks are, whose pointers are inside their encryption.
    pub fn read_after(&self, chat: Ulid, seq: u64, limit: u32) -> Result<Rows> {
        let texts: Vec<String> = {
            let mut statement = self.connection.prepare(
                "SELECT frame FROM message WHERE chat = ?1 AND seq > ?2 AND kind = 0 \
                 AND content_kind IN (2, 11, 14) ORDER BY seq LIMIT ?3",
            )?;
            let rows =
                statement.query_map(params![blob(chat), int(seq), limit], |row| row.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        Ok(Rows {
            message_list: frames(texts)?,
            call_list: Vec::new(),
            chat_event_list: Vec::new(),
        })
    }

    /// The edits and deletes pointing at one message.
    fn pointing_at(&self, chat: Ulid, seq: u64) -> Result<Vec<MessageFrame>> {
        let texts: Vec<String> = {
            let mut statement = self.connection.prepare(
                "SELECT frame FROM message WHERE chat = ?1 AND kind IN (1, 2) AND pointer = ?2 \
                 ORDER BY seq",
            )?;
            let rows = statement.query_map(params![blob(chat), int(seq)], |row| row.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        frames(texts)
    }

    /// Adds a page's call rows and the history rows in its span.
    fn with_among(
        &self,
        chat: Ulid,
        message_list: Vec<MessageFrame>,
        before: Option<u64>,
    ) -> Result<Rows> {
        let Some(first) = message_list.first() else {
            return Ok(Rows {
                message_list,
                call_list: Vec::new(),
                chat_event_list: Vec::new(),
            });
        };
        let mut call_list = Vec::new();
        for message in &message_list {
            if message.kind != Some(Kind::Content) {
                continue;
            }
            let texts: Vec<String> = {
                let mut statement = self.connection.prepare(
                    "SELECT frame FROM call_row WHERE chat = ?1 AND call_seq = ?2 ORDER BY n",
                )?;
                let rows =
                    statement.query_map(params![blob(chat), int(message.seq)], |row| row.get(0))?;
                rows.collect::<rusqlite::Result<_>>()?
            };
            call_list.extend(frames::<CallEvent>(texts)?);
        }
        let from = if first.seq <= 1 {
            i64::MIN
        } else {
            first.server_saved_at.unwrap_or(i64::MIN)
        };
        let up_to = match before {
            Some(before) => self
                .connection
                .query_row(
                    "SELECT saved_at FROM message WHERE chat = ?1 AND seq = ?2",
                    params![blob(chat), int(before)],
                    |row| row.get::<_, i64>(0),
                )
                .optional()?
                .unwrap_or(i64::MAX),
            None => i64::MAX,
        };
        let texts: Vec<String> = {
            let mut statement = self.connection.prepare(
                "SELECT frame FROM chat_event WHERE chat = ?1 AND server_at >= ?2 \
                 AND server_at <= ?3 ORDER BY n",
            )?;
            let rows = statement.query_map(params![blob(chat), from, up_to], |row| row.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        Ok(Rows {
            message_list,
            call_list,
            chat_event_list: frames(texts)?,
        })
    }

    /// Core's `ScanRows`: messages of every chat, newest by server time first, then by chat and seq,
    /// all below `before`, and how many rows the archive holds.
    pub fn scan_rows(
        &self,
        before: Option<ScanAt>,
        limit: u32,
    ) -> Result<(Vec<MessageFrame>, u64)> {
        let texts: Vec<String> = match before {
            Some(at) => {
                let mut statement = self.connection.prepare(
                    "SELECT frame FROM message WHERE (saved_at, chat, seq) < (?1, ?2, ?3) \
                     ORDER BY saved_at DESC, chat DESC, seq DESC LIMIT ?4",
                )?;
                let rows = statement.query_map(
                    params![at.saved_at, blob(at.chat), int(at.seq), limit],
                    |row| row.get(0),
                )?;
                rows.collect::<rusqlite::Result<_>>()?
            }
            None => {
                let mut statement = self.connection.prepare(
                    "SELECT frame FROM message ORDER BY saved_at DESC, chat DESC, seq DESC LIMIT ?1",
                )?;
                let rows = statement.query_map(params![limit], |row| row.get(0))?;
                rows.collect::<rusqlite::Result<_>>()?
            }
        };
        let total: i64 = self
            .connection
            .query_row("SELECT COUNT(*) FROM message", [], |row| row.get(0))?;
        Ok((frames(texts)?, u64::try_from(total).unwrap_or(0)))
    }

    /// A stored message's sending device and server time, to tell whose it is.
    pub fn message_row(&self, chat: Ulid, seq: u64) -> Result<Option<(Option<Ulid>, i64)>> {
        Ok(self
            .connection
            .query_row(
                "SELECT device, saved_at FROM message WHERE chat = ?1 AND seq = ?2",
                params![blob(chat), int(seq)],
                |row| Ok((opt_ulid_at(row, 0)?, row.get(1)?)),
            )
            .optional()?)
    }

    /// The sending devices of a chat's main-list content after `seq` that counts as unread: not a
    /// reaction, call, ack or activity. The caller leaves out its own.
    pub fn unread_devices(&self, chat: Ulid, seq: u64) -> Result<Vec<Option<Ulid>>> {
        let mut statement = self.connection.prepare(
            "SELECT device FROM message WHERE chat = ?1 AND seq > ?2 AND kind = 0 \
             AND content_kind NOT IN (2, 8, 14, 15) AND thread_seq IS NULL",
        )?;
        let rows = statement.query_map(params![blob(chat), int(seq)], |row| opt_ulid_at(row, 0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// A stored message's topic and thread, from its readable header.
    pub fn place_of(&self, chat: Ulid, seq: u64) -> Result<Option<(Option<Ulid>, Option<u64>)>> {
        Ok(self
            .connection
            .query_row(
                "SELECT topic, thread_seq FROM message WHERE chat = ?1 AND seq = ?2",
                params![blob(chat), int(seq)],
                |row| Ok((opt_ulid_at(row, 0)?, opt_u64_at(row, 1)?)),
            )
            .optional()?)
    }

    /// How many content messages are in the thread a message opened.
    pub fn thread_count(&self, chat: Ulid, seq: u64) -> Result<u64> {
        let count: i64 = self.connection.query_row(
            "SELECT COUNT(*) FROM message WHERE chat = ?1 AND thread_seq = ?2 AND kind = 0",
            params![blob(chat), int(seq)],
            |row| row.get(0),
        )?;
        Ok(u64::try_from(count).unwrap_or(0))
    }

    /// The chats holding content saved after `at`, for the index to catch up on.
    pub fn chats_after(&self, at: i64) -> Result<Vec<Ulid>> {
        let mut statement = self
            .connection
            .prepare("SELECT DISTINCT chat FROM message WHERE saved_at > ?1 AND kind IN (0, 1)")?;
        let rows = statement.query_map(params![at], |row| ulid_at(row, 0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// One chat's content and edits saved after `at`, oldest first, at most `limit`, for the index.
    pub fn rows_after(&self, chat: Ulid, at: i64, limit: u32) -> Result<Rows> {
        let texts: Vec<String> = {
            let mut statement = self.connection.prepare(
                "SELECT frame FROM message WHERE chat = ?1 AND saved_at > ?2 AND kind IN (0, 1) \
                 ORDER BY saved_at, seq LIMIT ?3",
            )?;
            let rows = statement.query_map(params![blob(chat), at, limit], |row| row.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        Ok(Rows {
            message_list: frames(texts)?,
            call_list: Vec::new(),
            chat_event_list: Vec::new(),
        })
    }

    /// The newest server time in the archive: what arrives later is new.
    pub fn newest_saved_at(&self) -> Result<Option<i64>> {
        Ok(self
            .connection
            .query_row("SELECT MAX(saved_at) FROM message", [], |row| row.get(0))?)
    }

    /// A group's or channel's history rows newest first, below `before` when given: the topic's rows
    /// when one is named, otherwise the chat's own.
    pub fn history(
        &self,
        chat: Ulid,
        topic: Option<Ulid>,
        before: Option<u64>,
        limit: u32,
    ) -> Result<Vec<ChatEvent>> {
        let texts: Vec<String> = {
            let mut statement = self.connection.prepare(
                "SELECT frame FROM chat_event WHERE chat = ?1 AND n < ?2 AND audience = 1 \
                 ORDER BY n DESC",
            )?;
            let rows = statement.query_map(
                params![blob(chat), opt_int(before).unwrap_or(i64::MAX)],
                |row| row.get(0),
            )?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let mut list = Vec::new();
        for event in frames::<ChatEvent>(texts)? {
            if event.topic == topic {
                list.push(event);
                if list.len() >= usize::try_from(limit).unwrap_or(usize::MAX) {
                    break;
                }
            }
        }
        Ok(list)
    }

    // ---------------------------------------------------------------------------------------------
    // The incoming queue
    // ---------------------------------------------------------------------------------------------

    /// Keeps an event, unless one of the same kind about the same message is kept already, as a
    /// sync's step back hands over what was already here. Answers its id when kept.
    pub fn add_event(&self, event: &NewEvent, now: i64) -> Result<Option<Ulid>> {
        if let (Some(chat), Some(seq)) = (event.chat, event.seq) {
            let kept: Option<i64> = self
                .connection
                .query_row(
                    "SELECT 1 FROM event WHERE kind = ?1 AND chat = ?2 AND seq = ?3",
                    params![event.kind as u8, blob(chat), int(seq)],
                    |row| row.get(0),
                )
                .optional()?;
            if kept.is_some() {
                return Ok(None);
            }
        }
        let id = new_id();
        let state = if event.until.is_some() { 2 } else { 0 };
        self.connection.execute(
            "INSERT INTO event (id, kind, chat, topic, thread_seq, seq, body, received_at, state, \
             until) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                blob(id),
                event.kind as u8,
                opt_blob(event.chat),
                opt_blob(event.topic),
                opt_int(event.thread_seq),
                opt_int(event.seq),
                serde_json::to_string(&event.body)?,
                now,
                state,
                event.until
            ],
        )?;
        Ok(Some(id))
    }

    /// What waits to be shown: waiting events, and deferred ones whose time has come, oldest first.
    pub fn waiting_events(&self, now: i64) -> Result<Vec<EventRow>> {
        let mut statement = self.connection.prepare(&format!(
            "SELECT {EVENT_COLUMNS} FROM event WHERE state = 0 \
             OR (state = 2 AND (until IS NULL OR until <= ?1)) ORDER BY received_at, id"
        ))?;
        let rows = statement.query_map(params![now], event_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The earliest time a deferred event comes back, if any waits for a time.
    pub fn next_deferred_at(&self) -> Result<Option<i64>> {
        Ok(self.connection.query_row(
            "SELECT MIN(until) FROM event WHERE state = 2 AND until IS NOT NULL",
            [],
            |row| row.get(0),
        )?)
    }

    /// The events with these ids, those that exist.
    pub fn events_by_id(&self, id_list: &[Ulid]) -> Result<Vec<EventRow>> {
        let mut list = Vec::new();
        for id in id_list {
            let row = self
                .connection
                .query_row(
                    &format!("SELECT {EVENT_COLUMNS} FROM event WHERE id = ?1"),
                    params![blob(*id)],
                    event_row,
                )
                .optional()?;
            if let Some(row) = row {
                list.push(row);
            }
        }
        Ok(list)
    }

    /// The events a run took, oldest first.
    pub fn events_of_run(&self, run: Ulid) -> Result<Vec<EventRow>> {
        let mut statement = self.connection.prepare(&format!(
            "SELECT {EVENT_COLUMNS} FROM event WHERE run = ?1 ORDER BY received_at, id"
        ))?;
        let rows = statement.query_map(params![blob(run)], event_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Shown to the loop, so taken, by this run's step.
    pub fn take_events(&mut self, id_list: &[Ulid], run: Ulid, step: u64) -> Result<()> {
        let transaction = self.connection.transaction()?;
        for id in id_list {
            transaction.execute(
                "UPDATE event SET state = 1, until = NULL, run = ?2, step = ?3 WHERE id = ?1",
                params![blob(*id), blob(run), int(step)],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    /// Put back by `inbox.defer`, for the next wake or a later time.
    pub fn defer_events(&mut self, id_list: &[Ulid], until: Option<i64>) -> Result<u64> {
        let transaction = self.connection.transaction()?;
        let mut count = 0;
        for id in id_list {
            count += transaction.execute(
                "UPDATE event SET state = 2, until = ?2, run = NULL, step = NULL WHERE id = ?1",
                params![blob(*id), until],
            )?;
        }
        transaction.commit()?;
        Ok(u64::try_from(count).unwrap_or(0))
    }

    /// Matched by no `[[wake]]` rule, or by an `ignore` one: kept as taken by no run, so it never
    /// wakes the loop and the log still shows it arrived.
    pub fn ignore_events(&mut self, id_list: &[Ulid]) -> Result<()> {
        let transaction = self.connection.transaction()?;
        for id in id_list {
            transaction.execute(
                "UPDATE event SET state = 1, until = NULL, run = NULL, step = NULL WHERE id = ?1",
                params![blob(*id)],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    // ---------------------------------------------------------------------------------------------
    // The outgoing queue
    // ---------------------------------------------------------------------------------------------

    pub fn add_outgoing(&self, row: &OutgoingRow, now: i64) -> Result<()> {
        self.connection.execute(
            "INSERT INTO outgoing (id, run, step, n, chat, command, client_at, state, seq, code, \
             at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                blob(row.id),
                opt_blob(row.run),
                opt_int(row.step),
                opt_int(row.n),
                blob(row.chat),
                serde_json::to_string(&row.command)?,
                row.client_at,
                row.state as u8,
                opt_int(row.seq),
                row.code,
                now
            ],
        )?;
        Ok(())
    }

    pub fn set_outgoing(
        &self,
        id: Ulid,
        state: OutgoingState,
        seq: Option<u64>,
        code: Option<&str>,
        now: i64,
    ) -> Result<()> {
        self.connection.execute(
            "UPDATE outgoing SET state = ?2, seq = COALESCE(?3, seq), code = ?4, at = ?5 \
             WHERE id = ?1",
            params![blob(id), state as u8, opt_int(seq), code, now],
        )?;
        Ok(())
    }

    /// What is still to go or to be answered, oldest first.
    pub fn open_outgoing(&self) -> Result<Vec<OutgoingRow>> {
        let mut statement = self.connection.prepare(&format!(
            "SELECT {OUTGOING_COLUMNS} FROM outgoing WHERE state IN (0, 1) ORDER BY client_at, id"
        ))?;
        let rows = statement.query_map([], outgoing_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    // ---------------------------------------------------------------------------------------------
    // The run log
    // ---------------------------------------------------------------------------------------------

    pub fn start_run(&self, now: i64) -> Result<Ulid> {
        let id = new_id();
        self.connection.execute(
            "INSERT INTO run (id, started_at, state) VALUES (?1, ?2, 0)",
            params![blob(id), now],
        )?;
        Ok(id)
    }

    pub fn end_run(&self, run: Ulid, state: RunState, now: i64) -> Result<()> {
        self.connection.execute(
            "UPDATE run SET state = ?2, ended_at = ?3 WHERE id = ?1",
            params![blob(run), state as u8, now],
        )?;
        Ok(())
    }

    /// A run a restart cut short, if any.
    pub fn running(&self) -> Result<Option<RunRow>> {
        Ok(self
            .connection
            .query_row(
                &format!("SELECT {RUN_COLUMNS} FROM run WHERE state = 0 ORDER BY id DESC LIMIT 1"),
                [],
                run_row,
            )
            .optional()?)
    }

    pub fn run(&self, run: Ulid) -> Result<Option<RunRow>> {
        Ok(self
            .connection
            .query_row(
                &format!("SELECT {RUN_COLUMNS} FROM run WHERE id = ?1"),
                params![blob(run)],
                run_row,
            )
            .optional()?)
    }

    /// Runs newest first, started in `[since, until)`, whose scripts or outputs hold `query` when
    /// given, after `skip` of them.
    pub fn runs(
        &self,
        query: Option<&str>,
        since: Option<i64>,
        until: Option<i64>,
        limit: u32,
        skip: u32,
    ) -> Result<Vec<RunRow>> {
        let pattern = query.map(|query| format!("%{}%", query.replace(['%', '_'], "")));
        let mut statement = self.connection.prepare(&format!(
            "SELECT {RUN_COLUMNS} FROM run WHERE started_at >= ?1 AND started_at < ?2 \
             AND (?3 IS NULL OR EXISTS (SELECT 1 FROM step WHERE step.run = run.id \
                AND (step.script LIKE ?3 OR step.intention_list LIKE ?3)) \
             OR EXISTS (SELECT 1 FROM step_output WHERE step_output.run = run.id \
                AND step_output.body LIKE ?3)) \
             ORDER BY started_at DESC, id DESC LIMIT ?4 OFFSET ?5"
        ))?;
        let rows = statement.query_map(
            params![
                since.unwrap_or(i64::MIN),
                until.unwrap_or(i64::MAX),
                pattern,
                limit,
                skip
            ],
            run_row,
        )?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Writes a step as far as its phases went, replacing what was there.
    pub fn put_step(&self, run: Ulid, step: &StepRow) -> Result<()> {
        self.connection.execute(
            "INSERT OR REPLACE INTO step (run, n, phase, context, intention_list, answer, script, \
             sent_at, started_at, ended_at, cut, tokens_in, tokens_out) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                blob(run),
                int(step.n),
                step.phase,
                step.context,
                step.intention_list,
                step.answer,
                step.script,
                step.sent_at,
                step.started_at,
                step.ended_at,
                i64::from(step.cut),
                opt_int(step.tokens_in),
                opt_int(step.tokens_out)
            ],
        )?;
        Ok(())
    }

    pub fn steps(&self, run: Ulid) -> Result<Vec<StepRow>> {
        let mut statement = self.connection.prepare(&format!(
            "SELECT {STEP_COLUMNS} FROM step WHERE run = ?1 ORDER BY n"
        ))?;
        let rows = statement.query_map(params![blob(run)], step_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// One print or action, kept as it happens.
    pub fn add_output(&self, run: Ulid, step: u64, output: &OutputRow) -> Result<()> {
        self.connection.execute(
            "INSERT OR REPLACE INTO step_output (run, step, n, kind, body, at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                blob(run),
                int(step),
                int(output.n),
                i64::from(output.action),
                output.body,
                output.at
            ],
        )?;
        Ok(())
    }

    pub fn outputs(&self, run: Ulid, step: u64) -> Result<Vec<OutputRow>> {
        let mut statement = self.connection.prepare(
            "SELECT n, kind, body, at FROM step_output WHERE run = ?1 AND step = ?2 ORDER BY n",
        )?;
        let rows = statement.query_map(params![blob(run), int(step)], |row| {
            Ok(OutputRow {
                n: u64_at(row, 0)?,
                action: row.get::<_, i64>(1)? == 1,
                body: row.get(2)?,
                at: row.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Removes finished runs started before `before`, each with its steps, outputs, the events it
    /// took and its answered outgoing rows, in one transaction. Answers how many went.
    pub fn remove_runs(&mut self, before: i64) -> Result<u64> {
        let transaction = self.connection.transaction()?;
        let run_list: Vec<Vec<u8>> = {
            let mut statement =
                transaction.prepare("SELECT id FROM run WHERE started_at < ?1 AND state != 0")?;
            let rows = statement.query_map(params![before], |row| row.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        for run in &run_list {
            transaction.execute("DELETE FROM step_output WHERE run = ?1", params![run])?;
            transaction.execute("DELETE FROM step WHERE run = ?1", params![run])?;
            transaction.execute(
                "DELETE FROM event WHERE run = ?1 AND state = 1",
                params![run],
            )?;
            transaction.execute(
                "DELETE FROM outgoing WHERE run = ?1 AND state IN (2, 3)",
                params![run],
            )?;
            transaction.execute("DELETE FROM run WHERE id = ?1", params![run])?;
        }
        transaction.commit()?;
        Ok(u64::try_from(run_list.len()).unwrap_or(0))
    }

    /// At the storage cap, run logs keep only their action lists: printed output and the input
    /// each step saw are dropped from finished runs.
    pub fn trim_runs(&mut self) -> Result<()> {
        let transaction = self.connection.transaction()?;
        transaction.execute(
            "DELETE FROM step_output WHERE kind = 0 AND run IN (SELECT id FROM run WHERE state != 0)",
            [],
        )?;
        transaction.execute(
            "UPDATE step SET context = '{}', answer = NULL \
             WHERE run IN (SELECT id FROM run WHERE state != 0)",
            [],
        )?;
        transaction.commit()?;
        Ok(())
    }

    // ---------------------------------------------------------------------------------------------
    // Schedules
    // ---------------------------------------------------------------------------------------------

    pub fn add_schedule(&self, row: &ScheduleRow) -> Result<()> {
        self.connection.execute(
            &format!(
                "INSERT INTO schedule ({SCHEDULE_COLUMNS}) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"
            ),
            params![
                blob(row.id),
                row.kind,
                row.spec,
                row.tz,
                row.start,
                row.note,
                chats_blob(&row.chat_list),
                i64::from(row.join),
                row.next_at,
                row.fired_at,
                opt_blob(row.run),
                row.made_at
            ],
        )?;
        Ok(())
    }

    /// Every schedule, the next to fire first.
    pub fn schedules(&self) -> Result<Vec<ScheduleRow>> {
        let mut statement = self.connection.prepare(&format!(
            "SELECT {SCHEDULE_COLUMNS} FROM schedule ORDER BY next_at, id"
        ))?;
        let rows = statement.query_map([], schedule_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn remove_schedule(&self, id: Ulid) -> Result<bool> {
        let count = self
            .connection
            .execute("DELETE FROM schedule WHERE id = ?1", params![blob(id)])?;
        Ok(count > 0)
    }

    /// Saves a schedule's fire event, and in the same transaction removes an `at` or moves the
    /// others to their next time.
    pub fn fire(
        &mut self,
        schedule: &ScheduleRow,
        event: &NewEvent,
        next_at: Option<i64>,
        now: i64,
    ) -> Result<Ulid> {
        let id = new_id();
        let transaction = self.connection.transaction()?;
        transaction.execute(
            "INSERT INTO event (id, kind, chat, topic, thread_seq, seq, body, received_at, state) \
             VALUES (?1, ?2, NULL, NULL, NULL, NULL, ?3, ?4, 0)",
            params![
                blob(id),
                event.kind as u8,
                serde_json::to_string(&event.body)?,
                now
            ],
        )?;
        match next_at {
            Some(next_at) => transaction.execute(
                "UPDATE schedule SET next_at = ?2, fired_at = ?3 WHERE id = ?1",
                params![blob(schedule.id), next_at, now],
            )?,
            None => transaction.execute(
                "DELETE FROM schedule WHERE id = ?1",
                params![blob(schedule.id)],
            )?,
        };
        transaction.commit()?;
        Ok(id)
    }

    // ---------------------------------------------------------------------------------------------
    // Memory
    // ---------------------------------------------------------------------------------------------

    pub fn add_note(&self, row: &NoteRow) -> Result<()> {
        self.connection.execute(
            &format!(
                "INSERT INTO memory ({NOTE_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)"
            ),
            params![
                blob(row.id),
                opt_blob(row.chat),
                row.text,
                blob(row.run),
                int(row.step),
                opt_blob(row.source_chat),
                opt_int(row.seq),
                row.made_at,
                row.updated_at
            ],
        )?;
        Ok(())
    }

    pub fn note(&self, id: Ulid) -> Result<Option<NoteRow>> {
        Ok(self
            .connection
            .query_row(
                &format!("SELECT {NOTE_COLUMNS} FROM memory WHERE id = ?1"),
                params![blob(id)],
                note_row,
            )
            .optional()?)
    }

    /// Notes newest first: of one chat's memory when `chat` is given, otherwise the global one's,
    /// after `skip` of them.
    pub fn notes(&self, chat: Option<Ulid>, limit: u32, skip: u32) -> Result<Vec<NoteRow>> {
        let mut statement = self.connection.prepare(&format!(
            "SELECT {NOTE_COLUMNS} FROM memory WHERE chat IS ?1 \
             ORDER BY updated_at DESC, id DESC LIMIT ?2 OFFSET ?3"
        ))?;
        let rows = statement.query_map(params![opt_blob(chat), limit, skip], note_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Every note, for rebuilding the index.
    pub fn all_notes(&self) -> Result<Vec<NoteRow>> {
        let mut statement = self
            .connection
            .prepare(&format!("SELECT {NOTE_COLUMNS} FROM memory ORDER BY id"))?;
        let rows = statement.query_map([], note_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// How many notes every memory holds together, and the one memory's own when `chat` is given.
    pub fn note_count(&self, chat: Option<Option<Ulid>>) -> Result<u64> {
        let count: i64 = match chat {
            Some(chat) => self.connection.query_row(
                "SELECT COUNT(*) FROM memory WHERE chat IS ?1",
                params![opt_blob(chat)],
                |row| row.get(0),
            )?,
            None => self
                .connection
                .query_row("SELECT COUNT(*) FROM memory", [], |row| row.get(0))?,
        };
        Ok(u64::try_from(count).unwrap_or(0))
    }

    pub fn replace_note(&self, id: Ulid, text: &str, now: i64) -> Result<bool> {
        let count = self.connection.execute(
            "UPDATE memory SET text = ?2, updated_at = ?3 WHERE id = ?1",
            params![blob(id), text, now],
        )?;
        Ok(count > 0)
    }

    pub fn delete_note(&self, id: Ulid) -> Result<bool> {
        let count = self
            .connection
            .execute("DELETE FROM memory WHERE id = ?1", params![blob(id)])?;
        Ok(count > 0)
    }

    // ---------------------------------------------------------------------------------------------
    // Storage
    // ---------------------------------------------------------------------------------------------

    /// The file's size, and roughly what each kind of row holds of it.
    pub fn usage(&self) -> Result<Usage> {
        let pages: i64 = self
            .connection
            .query_row("PRAGMA page_count", [], |row| row.get(0))?;
        let page_size: i64 = self
            .connection
            .query_row("PRAGMA page_size", [], |row| row.get(0))?;
        let sum = |sql: &str| -> Result<u64> {
            let value: Option<i64> = self.connection.query_row(sql, [], |row| row.get(0))?;
            Ok(u64::try_from(value.unwrap_or(0)).unwrap_or(0))
        };
        Ok(Usage {
            file: u64::try_from(pages.saturating_mul(page_size)).unwrap_or(0),
            runs: sum(
                "SELECT (SELECT COALESCE(SUM(length(context) + COALESCE(length(answer), 0) \
                 + COALESCE(length(script), 0)), 0) FROM step) \
                 + (SELECT COALESCE(SUM(length(body)), 0) FROM step_output)",
            )?,
            memory: sum("SELECT SUM(length(text)) FROM memory")?,
            archive: sum(
                "SELECT (SELECT COALESCE(SUM(length(frame)), 0) FROM message) \
                 + (SELECT COALESCE(SUM(length(frame)), 0) FROM call_row) \
                 + (SELECT COALESCE(SUM(length(frame)), 0) FROM chat_event)",
            )?,
        })
    }
}
