//! Chat logic: the per-chat task that serializes a chat's events, and who is online.
//!
//! The task stores and publishes. It never waits on a connection: each device has a bounded outgoing
//! buffer, and when that fills, the connection is closed. The device reconnects and fetches what it
//! missed, so nothing is lost, because every message is saved. Tasks end after `[chat] idle_after` and
//! restart on the next event, so memory does not grow with old chats. A task whose chat has an open
//! call does not idle out: it holds the call's state, and nothing about a call is rebuilt from the
//! database.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use axum::extract::ws::Utf8Bytes;
use keryx_core::protocol::{
    Bytes, CallEvent, CallEventKind, CallOutcome, CallText, ChatMode, ChatType, ClientFrame,
    ContentKind, Kind, MessageFrame, SendFrame, ServerFrame, WelcomeItem, WrapItem,
};
use keryx_core::ulid::Ulid;
use tokio::sync::{Notify, mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::api::{
    App, Caller, DELETE_OWN, EDIT_OWN, MESSAGE_COMMIT, Need, Refused, SEND_UNENCRYPTED, TRACE,
    check, current_trace, new_id, operation, refused, send_permission,
};
use crate::file;
use crate::moq::{self, Closer, Joined};
use crate::storage::{
    Appended, GroupInfoPut, NewMessage, NewWelcome, NewWrap, Position, PositionReport, Session,
    Storage,
};

/// How many events may wait for a chat's task before their senders wait in turn.
const CHAT_QUEUE: usize = 256;

/// For its first 7 days, a new-device notice cannot be removed by that device or any newer one.
const NOTICE_PROTECTED_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// A call nobody else joins ends as missed after this, fixed in code. `[calls] test_ring` shortens it
/// for the tests only.
const RING: Duration = Duration::from_secs(60);

/// A call socket that dropped without a clean close counts as a leave after this, fixed in code, so a
/// device that reconnects in time writes nothing. `[calls] test_grace` shortens it for the tests only.
const GRACE: Duration = Duration::from_secs(10);

/// What a connection's socket is told to do.
pub enum Out {
    Frame(Utf8Bytes),
    Close(u16, &'static str),
}

struct Connection {
    id: u64,
    outgoing: mpsc::Sender<Out>,
    /// Told when `outgoing` is full, since nothing more can be put in it to say so.
    kill: Arc<Notify>,
}

struct ChatTask {
    jobs: mpsc::Sender<Job>,
    handle: JoinHandle<()>,
}

/// One event for a chat's task, and where its answer goes. Typing has no answer.
pub struct Job {
    pub trace: Ulid,
    pub session: Session,
    pub work: Work,
    pub reply: Option<oneshot::Sender<ServerFrame>>,
}

/// What a chat's task is asked to do: a frame from the chat's socket, or what the call relay saw.
pub enum Work {
    Frame(ClientFrame),
    /// A device's call socket asks to be admitted. `close` is how the task closes it later.
    Join {
        call_seq: u64,
        close: Closer,
        reply: oneshot::Sender<Result<Joined, Refused>>,
    },
    /// A device's call socket ended. `clean` is a close on purpose, a leave at once; otherwise the
    /// device gets the drop grace.
    Gone {
        call_seq: u64,
        connection: u64,
        clean: bool,
    },
    /// Text a device sent on its call's `text` track.
    Text {
        call_seq: u64,
        connection: u64,
        text: CallText,
    },
}

/// Who is online, and the chats' running tasks.
#[derive(Default)]
pub struct Hub {
    next_connection: AtomicU64,
    connections: Mutex<HashMap<Ulid, HashMap<Ulid, Connection>>>,
    chats: Mutex<HashMap<Ulid, ChatTask>>,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Puts a frame in a connection's buffer. A full buffer closes the connection and never waits.
pub fn push(outgoing: &mpsc::Sender<Out>, kill: &Notify, text: Utf8Bytes) {
    if outgoing.try_send(Out::Frame(text)).is_err() {
        tracing::warn!(msg = "Closing a connection whose outgoing buffer is full");
        kill.notify_one();
    }
}

/// The frame as the text a socket sends.
pub fn encode(frame: &ServerFrame) -> Option<Utf8Bytes> {
    match serde_json::to_string(frame) {
        Ok(text) => Some(Utf8Bytes::from(text)),
        Err(error) => {
            tracing::error!(msg = "Could not encode a frame", error = %error);
            None
        }
    }
}

impl Hub {
    /// Registers a device's connection. A device has one, so an older one is closed.
    pub fn connect(
        &self,
        account: Ulid,
        device: Ulid,
        outgoing: mpsc::Sender<Out>,
        kill: Arc<Notify>,
    ) -> u64 {
        let id = self.next_connection.fetch_add(1, Ordering::Relaxed);
        let mut connections = locked(&self.connections);
        let devices = connections.entry(account).or_default();
        let connection = Connection { id, outgoing, kill };
        if let Some(old) = devices.insert(device, connection) {
            old.kill.notify_one();
        }
        id
    }

    /// `true` if this was still the device's connection, so it has gone offline.
    pub fn disconnect(&self, account: Ulid, device: Ulid, id: u64) -> bool {
        let mut connections = locked(&self.connections);
        let Some(devices) = connections.get_mut(&account) else {
            return false;
        };
        match devices.get(&device) {
            Some(connection) if connection.id == id => {}
            _ => return false,
        }
        devices.remove(&device);
        if devices.is_empty() {
            connections.remove(&account);
        }
        true
    }

    pub fn is_online(&self, account: Ulid, device: Ulid) -> bool {
        let connections = locked(&self.connections);
        match connections.get(&account) {
            Some(devices) => devices.contains_key(&device),
            None => false,
        }
    }

    /// To every online device of these accounts, except one.
    pub fn deliver(&self, account_list: &[Ulid], except: Option<Ulid>, frame: &ServerFrame) {
        let Some(text) = encode(frame) else {
            return;
        };
        let connections = locked(&self.connections);
        for account in account_list {
            let Some(devices) = connections.get(account) else {
                continue;
            };
            for (device, connection) in devices {
                if Some(*device) != except {
                    push(&connection.outgoing, &connection.kill, text.clone());
                }
            }
        }
    }

    pub fn deliver_to_device(&self, account: Ulid, device: Ulid, frame: &ServerFrame) {
        let Some(text) = encode(frame) else {
            return;
        };
        let connections = locked(&self.connections);
        if let Some(devices) = connections.get(&account)
            && let Some(connection) = devices.get(&device)
        {
            push(&connection.outgoing, &connection.kill, text);
        }
    }

    /// Closes an ended device's connection, with the refusal's code as the reason.
    pub fn close_device(&self, account: Ulid, device: Ulid, code: u16, reason: &'static str) {
        let connections = locked(&self.connections);
        if let Some(devices) = connections.get(&account)
            && let Some(connection) = devices.get(&device)
            && connection
                .outgoing
                .try_send(Out::Close(code, reason))
                .is_err()
        {
            connection.kill.notify_one();
        }
    }

    /// Hands an event to its chat's task, starting one if none is running. A task that is ending
    /// gives the event back, and the new task waits for it to finish, so one chat never runs two.
    pub async fn submit(&self, app: &Arc<App>, chat: Ulid, mut job: Job) {
        loop {
            let jobs = {
                let mut chats = locked(&self.chats);
                let running = match chats.get(&chat) {
                    Some(task) if !task.jobs.is_closed() => Some(task.jobs.clone()),
                    _ => None,
                };
                match running {
                    Some(jobs) => jobs,
                    None => {
                        let previous = chats.remove(&chat);
                        let (jobs, queue) = mpsc::channel(CHAT_QUEUE);
                        let handle = tokio::spawn(run_chat(Arc::clone(app), chat, queue, previous));
                        chats.insert(
                            chat,
                            ChatTask {
                                jobs: jobs.clone(),
                                handle,
                            },
                        );
                        jobs
                    }
                }
            };
            match jobs.send(job).await {
                Ok(()) => return,
                Err(returned) => job = returned.0,
            }
        }
    }

    /// Stops taking events, and waits for each task to finish what it already received.
    pub async fn finish(&self) {
        let mut handle_list = Vec::new();
        {
            let mut chats = locked(&self.chats);
            for (_, task) in chats.drain() {
                handle_list.push(task.handle);
            }
        }
        for handle in handle_list {
            if let Err(error) = handle.await {
                tracing::error!(msg = "A chat's task failed", error = %error);
            }
        }
    }
}

/// The chat's open call, held by its task. A chat has at most one.
struct OpenCall {
    seq: u64,
    /// The account that started it, which can neither answer nor decline it.
    caller: Ulid,
    caller_device: Ulid,
    /// The next event's number inside the call, from 1.
    next_n: u64,
    /// When it ends as missed, unless a device of the other account joined first.
    ring_until: Instant,
    answered: bool,
    room: Room,
}

/// A call's MoQ: the shared origin everyone in it reads, the `server` broadcast's `events` track,
/// and who is in it.
struct Room {
    shared: moq_net::origin::Producer,
    events: moq_net::track::Producer,
    /// Keeps the `server` broadcast announced while the call is open.
    _server: moq_net::broadcast::Producer,
    sockets: HashMap<Ulid, Socket>,
    /// Devices whose socket dropped, and when that counts as their leave.
    gone: HashMap<Ulid, Instant>,
    next_connection: u64,
}

/// A device's call socket, as the chat's task knows it.
struct Socket {
    connection: u64,
    close: Closer,
}

/// Publishes call events on the call's `events` track, each its own group. A failed write only means
/// the track ended, and `sync` still has them.
fn publish(events: &mut moq_net::track::Producer, event_list: &[CallEvent]) {
    for event in event_list {
        match serde_json::to_vec(event) {
            Ok(json) => {
                if let Err(error) = events.write_frame(moq_net::Timestamp::ZERO, json) {
                    tracing::debug!(msg = "Could not publish a call event", error = %error);
                }
            }
            Err(error) => tracing::error!(msg = "Could not encode a call event", error = %error),
        }
    }
}

impl OpenCall {
    /// Events numbered from `next_n` on, by the server's clock. Only saving them moves `next_n`.
    fn events(&self, chat: Ulid, kind_list: &[(CallEventKind, Ulid)], now: i64) -> Vec<CallEvent> {
        let mut event_list = Vec::with_capacity(kind_list.len());
        for (n, (kind, device)) in (self.next_n..).zip(kind_list) {
            event_list.push(CallEvent {
                chat,
                call_seq: self.seq,
                n,
                kind: *kind,
                device: *device,
                server_at: now,
                client_at: None,
                epoch: None,
                signature: None,
                body: None,
                outcome: None,
            });
        }
        event_list
    }
}

/// What a chat's task keeps between events.
struct ChatState {
    chat: Ulid,
    call: Option<OpenCall>,
}

/// What a task's wait ended with.
enum Woken {
    Job(Option<Job>),
    /// The ring or a dropped device's grace ran out.
    Timer,
    Idle,
}

async fn run_chat(
    app: Arc<App>,
    chat: Ulid,
    mut queue: mpsc::Receiver<Job>,
    previous: Option<ChatTask>,
) {
    if let Some(previous) = previous {
        drop(previous.jobs);
        if let Err(error) = previous.handle.await {
            tracing::error!(msg = "A chat's task failed", error = %error);
        }
    }
    if let Err(refusal) = file::sweep(&app, Some(chat)).await {
        tracing::error!(msg = "Could not remove unused uploads", code = refusal.code);
    }
    let idle_after = Duration::from_secs(app.settings.chat.idle_after.max(1));
    let mut state = ChatState { chat, call: None };
    loop {
        let woken = match &state.call {
            Some(call) => {
                let mut until = None;
                if !call.answered {
                    until = Some(call.ring_until);
                }
                for deadline in call.room.gone.values() {
                    if until.is_none_or(|until| *deadline < until) {
                        until = Some(*deadline);
                    }
                }
                match until {
                    Some(until) => tokio::select! {
                        job = queue.recv() => Woken::Job(job),
                        () = tokio::time::sleep_until(until) => Woken::Timer,
                    },
                    None => Woken::Job(queue.recv().await),
                }
            }
            None => match tokio::time::timeout(idle_after, queue.recv()).await {
                Ok(job) => Woken::Job(job),
                Err(_) => Woken::Idle,
            },
        };
        match woken {
            Woken::Job(Some(job)) => handle(&app, &mut state, job).await,
            Woken::Job(None) => return,
            Woken::Timer => {
                let trace = current_trace();
                let result = TRACE
                    .scope(trace, async {
                        operation!(
                            "CallTimer",
                            "call_timer",
                            call_timer(&app, chat, &mut state)
                        )
                    })
                    .await;
                if result.is_err() {
                    // Kept out of memory anyway, or the timer would fire again at once. A restart
                    // ends it in the database.
                    state.call = None;
                }
            }
            Woken::Idle => {
                queue.close();
                while let Some(job) = queue.recv().await {
                    handle(&app, &mut state, job).await;
                }
                let mut chats = locked(&app.hub.chats);
                if let Some(task) = chats.get(&chat)
                    && task.jobs.is_closed()
                {
                    chats.remove(&chat);
                }
                return;
            }
        }
    }
}

async fn handle(app: &Arc<App>, state: &mut ChatState, job: Job) {
    let Job {
        trace,
        session,
        work,
        reply,
    } = job;
    let frame = match work {
        Work::Frame(frame) => frame,
        Work::Join {
            call_seq,
            close,
            reply,
        } => {
            let result = TRACE
                .scope(trace, async {
                    operation!(
                        "CallJoin",
                        "call_join",
                        run_join(app, state, session, call_seq, close)
                    )
                })
                .await;
            if reply.send(result).is_err() {
                tracing::debug!(msg = "The call socket left before its answer");
            }
            return;
        }
        Work::Gone {
            call_seq,
            connection,
            clean,
        } => {
            let result = TRACE
                .scope(trace, async {
                    operation!(
                        "CallGone",
                        "call_gone",
                        run_gone(app, state, session, call_seq, connection, clean)
                    )
                })
                .await;
            if let Err(refusal) = result {
                tracing::error!(
                    msg = "Could not record a call socket's end",
                    code = refusal.code
                );
            }
            return;
        }
        Work::Text {
            call_seq,
            connection,
            text,
        } => {
            let result = TRACE
                .scope(trace, async {
                    operation!(
                        "CallText",
                        "call_text",
                        run_call_text(app, state, session, call_seq, connection, text)
                    )
                })
                .await;
            if let Err(refusal) = result {
                tracing::debug!(msg = "Dropped call text", code = refusal.code);
            }
            return;
        }
    };
    let answer = TRACE
        .scope(trace, async move {
            match frame {
                ClientFrame::Send(frame) => {
                    let reference = frame.reference;
                    let result = operation!("Send", "send", run_send(app, state, session, frame));
                    Some(answer_of(reference, result))
                }
                ClientFrame::Seen {
                    reference,
                    chat,
                    up_to_seq,
                    client_seen_at,
                } => {
                    let result = operation!(
                        "Seen",
                        "seen",
                        run_position(
                            app,
                            session,
                            reference,
                            Position::Seen,
                            chat,
                            up_to_seq,
                            client_seen_at
                        )
                    );
                    Some(answer_of(reference, result))
                }
                ClientFrame::Received {
                    reference,
                    chat,
                    up_to_seq,
                    client_received_at,
                } => {
                    let result = operation!(
                        "Received",
                        "received",
                        run_position(
                            app,
                            session,
                            reference,
                            Position::Received,
                            chat,
                            up_to_seq,
                            client_received_at
                        )
                    );
                    Some(answer_of(reference, result))
                }
                ClientFrame::Typing { chat, on } => {
                    let result = operation!("Typing", "typing", run_typing(app, session, chat, on));
                    if let Err(refusal) = result {
                        tracing::debug!(msg = "Dropped a typing event", code = refusal.code);
                    }
                    None
                }
                ClientFrame::GroupInfo {
                    reference,
                    chat,
                    epoch,
                    body,
                } => {
                    let result = operation!(
                        "GroupInfo",
                        "group_info",
                        run_group_info(app, session, reference, chat, epoch, body)
                    );
                    Some(answer_of(reference, result))
                }
                ClientFrame::Wraps {
                    reference,
                    chat,
                    epoch,
                    wrap_list,
                } => {
                    let result = operation!(
                        "Wraps",
                        "wraps",
                        run_wraps(app, session, reference, chat, epoch, wrap_list)
                    );
                    Some(answer_of(reference, result))
                }
                ClientFrame::CallEvent {
                    reference,
                    chat,
                    call_seq,
                    kind,
                } => {
                    let result = operation!(
                        "CallEvent",
                        "call_event",
                        run_call_event(app, state, session, reference, chat, call_seq, kind)
                    );
                    Some(answer_of(reference, result))
                }
                ClientFrame::Sync { reference, .. }
                | ClientFrame::Draft { reference, .. }
                | ClientFrame::Times { reference, .. }
                | ClientFrame::Ping { reference } => Some(answer_of(
                    reference,
                    Err(refused(500, "internal", "This event has no chat's task")),
                )),
            }
        })
        .await;
    if let (Some(reply), Some(answer)) = (reply, answer)
        && reply.send(answer).is_err()
    {
        tracing::debug!(msg = "The connection left before its answer");
    }
}

/// The answer a request gets on the socket.
pub fn answer_of(reference: Ulid, result: Result<ServerFrame, Refused>) -> ServerFrame {
    match result {
        Ok(frame) => frame,
        Err(refusal) => ServerFrame::Refused {
            reference,
            code: refusal.code.to_string(),
            message: refusal.message,
            call_seq: refusal.call_seq,
        },
    }
}

fn bad(message: &str) -> Refused {
    refused(400, "bad_message", message)
}

async fn run_send(
    app: &Arc<App>,
    state: &mut ChatState,
    session: Session,
    frame: Box<SendFrame>,
) -> Result<ServerFrame, Refused> {
    let caller = Caller::Session(session);
    let chat = frame.chat;
    check(app, &caller, Need::Member(chat)).await?;
    let device = session.device()?;
    let frame = *frame;

    // What the schema's checks would refuse is refused here first, with a reason.
    if frame.kind == Kind::Content && frame.content_kind.is_none() {
        return Err(bad("A content message needs its content kind"));
    }
    if frame.kind != Kind::Content && frame.content_kind.is_some() {
        return Err(bad("Only a content message has a content kind"));
    }
    if frame.encrypted && frame.epoch.is_none() {
        return Err(bad("An encrypted message needs its epoch"));
    }
    if frame.kind == Kind::Commit && frame.epoch.is_none() {
        return Err(bad("A commit needs its epoch"));
    }
    if frame.kind == Kind::Commit && frame.group_info.is_none() {
        return Err(bad("A commit needs its group info"));
    }
    let is_action = matches!(frame.kind, Kind::Edit | Kind::Delete);
    if is_action && frame.pointer_seq.is_none() {
        return Err(bad("An edit or delete needs its target"));
    }
    if frame.kind == Kind::Content && frame.encrypted && frame.pointer_seq.is_some() {
        return Err(bad("An encrypted message's pointer stays inside it"));
    }
    if frame.kind == Kind::Content
        && !frame.encrypted
        && frame.content_kind == Some(ContentKind::Reaction)
        && frame.pointer_seq.is_none()
    {
        return Err(bad("A reaction needs its target"));
    }
    if frame.kind != Kind::Commit
        && !(frame.wrap_list.is_empty()
            && frame.welcome_list.is_empty()
            && frame.group_info.is_none())
    {
        return Err(bad("Only a commit carries wraps, welcomes and group info"));
    }

    check_files(app, session, &frame).await?;

    let mut body_limit = None;
    match frame.kind {
        Kind::Content => {
            if let Some(content_kind) = frame.content_kind {
                check(
                    app,
                    &caller,
                    Need::MemberWith(chat, send_permission(content_kind)),
                )
                .await?;
                body_limit = Some(app.settings.limits.body.of(content_kind));
            }
        }
        Kind::Edit | Kind::Delete => {
            let permission = if frame.kind == Kind::Edit {
                EDIT_OWN
            } else {
                DELETE_OWN
            };
            check(app, &caller, Need::MemberWith(chat, permission)).await?;
            let mut target = None;
            if let Some(seq) = frame.pointer_seq {
                target = app.storage.message_brief(chat, seq).await?;
            }
            let Some(target) = target else {
                return Err(refused(
                    404,
                    "no_such_message",
                    "That message does not exist",
                ));
            };
            if target.account != session.account {
                return Err(refused(403, "not_yours", "That message is someone else's"));
            }
            if frame.kind == Kind::Edit {
                let Some(content_kind) = target.content_kind else {
                    return Err(refused(409, "cannot_edit", "Only content can be edited"));
                };
                if content_kind == ContentKind::Reaction {
                    return Err(refused(409, "cannot_edit", "A reaction cannot be edited"));
                }
                if content_kind == ContentKind::Sticker {
                    return Err(refused(409, "cannot_edit", "A sticker cannot be edited"));
                }
                body_limit = Some(app.settings.limits.body.of(content_kind));
            } else {
                if !matches!(target.kind, Kind::Content | Kind::DeviceNotice) {
                    return Err(refused(409, "cannot_delete", "Only content can be deleted"));
                }
                if target.kind == Kind::DeviceNotice
                    && Ulid::now() - target.saved_at < NOTICE_PROTECTED_MS
                {
                    let device_list = app.storage.list_devices(session.account).await?;
                    let mut created = None;
                    for item in &device_list {
                        if item.id == device {
                            created = Some(item.created_at);
                        }
                    }
                    if created >= Some(target.device_created_at) {
                        return Err(refused(
                            403,
                            "notice_protected",
                            "This device cannot remove that notice yet",
                        ));
                    }
                }
            }
        }
        Kind::Commit => check(app, &caller, Need::MemberWith(chat, MESSAGE_COMMIT)).await?,
        Kind::DeviceNotice => {
            return Err(refused(
                403,
                "server_only",
                "Only the server writes notices",
            ));
        }
    }
    if let Some(limit) = body_limit
        && frame.body.0.len() as u64 > limit
    {
        return Err(refused(413, "body_too_large", "That message is too large"));
    }

    if frame.kind != Kind::Commit {
        let Some(access) = app.storage.chat_access(chat, session.account).await? else {
            return Err(refused(404, "not_member", "You are not in that chat"));
        };
        match (access.mode, frame.encrypted) {
            (ChatMode::Private, false) => {
                return Err(refused(
                    403,
                    "unencrypted_refused",
                    "This chat refuses unencrypted messages",
                ));
            }
            (ChatMode::Open, true) => {
                return Err(refused(
                    403,
                    "encrypted_refused",
                    "Everything in this chat is unencrypted",
                ));
            }
            (ChatMode::Mixed, false) => {
                check(app, &caller, Need::MemberWith(chat, SEND_UNENCRYPTED)).await?;
            }
            _ => {}
        }
        if frame.kind == Kind::Content && frame.content_kind == Some(ContentKind::Call) {
            if access.chat_type != ChatType::Direct {
                return Err(refused(
                    403,
                    "call_not_allowed",
                    "Calls are for direct chats only",
                ));
            }
            if let Some(call) = &state.call {
                return Err(Refused {
                    status: 409,
                    code: "call_open",
                    message: "This chat already has an open call".to_string(),
                    call_seq: Some(call.seq),
                });
            }
        }
    }

    let member_list = app.storage.member_accounts(chat).await?;
    let wrap_list = check_wraps(&member_list, frame.wrap_list)?;
    let mut welcome_list = Vec::with_capacity(frame.welcome_list.len());
    let mut welcome_devices = Vec::with_capacity(frame.welcome_list.len());
    if !frame.welcome_list.is_empty() {
        let device_list = app.storage.chat_devices(chat).await?;
        for WelcomeItem { device, body } in frame.welcome_list {
            let mut account = None;
            for known in &device_list {
                if known.device == device && known.statement.is_none() {
                    account = Some(known.account);
                }
            }
            let Some(account) = account else {
                return Err(refused(
                    404,
                    "no_such_device",
                    "A Welcome is for a device not in this chat",
                ));
            };
            welcome_devices.push((account, device));
            welcome_list.push(NewWelcome {
                id: new_id()?,
                device,
                body: body.0,
            });
        }
    }

    // Deleting an open call ends it first, so its devices see the end before the message goes.
    if frame.kind == Kind::Delete
        && let Some(call) = &state.call
        && Some(call.seq) == frame.pointer_seq
    {
        end_call(app, chat, state, None, device).await?;
    }

    let now = app.storage.saved_now();
    let appended = app
        .storage
        .append_message(NewMessage {
            chat,
            device,
            kind: frame.kind,
            content_kind: frame.content_kind,
            encrypted: frame.encrypted,
            epoch: frame.epoch,
            pointer_seq: frame.pointer_seq,
            client_sent_at: frame.client_sent_at,
            now,
            signature: frame.signature.0.clone(),
            body: frame.body.0.clone(),
            wrap_list,
            welcome_list,
            group_info: frame.group_info.map(|group_info| group_info.0),
            persist: frame.persist,
            remove: frame.remove.clone(),
        })
        .await?;
    let seq = match appended {
        Appended::Saved(seq) => seq,
        Appended::CommitLost => {
            return Err(refused(
                409,
                "commit_lost",
                "Another commit got this epoch, or the group is not started",
            ));
        }
    };
    if frame.kind == Kind::Content && frame.content_kind == Some(ContentKind::Call) {
        let ring = match app.settings.calls.test_ring {
            Some(seconds) => Duration::from_secs(seconds),
            None => RING,
        };
        let shared = moq_net::origin::Info::new(moq_net::Origin::random())
            .with_pool(app.call_pool.clone())
            .produce();
        let room = match shared.create_broadcast("server", moq_net::broadcast::Route::announced()) {
            Ok(mut server) => match server.create_track(moq::EVENTS.name, moq::EVENTS.info()) {
                Ok(events) => Some(Room {
                    shared,
                    events,
                    _server: server,
                    sockets: HashMap::new(),
                    gone: HashMap::new(),
                    next_connection: 0,
                }),
                Err(error) => {
                    tracing::error!(msg = "Could not make a call's events track", error = %error);
                    None
                }
            },
            Err(error) => {
                tracing::error!(msg = "Could not make a call's broadcast", error = %error);
                None
            }
        };
        // Without a room nobody can join, so the call rings out as missed and a new one can start.
        if let Some(room) = room {
            state.call = Some(OpenCall {
                seq,
                caller: session.account,
                caller_device: device,
                next_n: 1,
                ring_until: Instant::now() + ring,
                answered: false,
                room,
            });
        }
    }
    app.hub.deliver(
        &member_list,
        Some(device),
        &ServerFrame::Message(Box::new(MessageFrame {
            chat,
            seq,
            device: Some(device),
            kind: Some(frame.kind),
            content_kind: frame.content_kind,
            encrypted: Some(frame.encrypted),
            epoch: frame.epoch,
            pointer_seq: frame.pointer_seq,
            client_sent_at: Some(frame.client_sent_at),
            server_saved_at: Some(now),
            body: Some(frame.body),
            signature: Some(frame.signature),
        })),
    );
    for file in frame.remove {
        file::remove(app, chat, file).await;
    }
    for (account, device) in welcome_devices {
        hand_welcomes(app, account, device).await?;
    }
    Ok(ServerFrame::Sent {
        reference: frame.reference,
        seq,
        server_saved_at: now,
    })
}

/// A decline, from the other account's device, while the call rings. In a direct chat it ends the call
/// at once as missed.
async fn run_call_event(
    app: &Arc<App>,
    state: &mut ChatState,
    session: Session,
    reference: Ulid,
    chat: Ulid,
    call_seq: u64,
    kind: CallEventKind,
) -> Result<ServerFrame, Refused> {
    check(app, &Caller::Session(session), Need::Member(chat)).await?;
    let device = session.device()?;
    if kind != CallEventKind::Decline {
        return Err(bad("Only a decline comes over the chat's socket"));
    }
    let Some(call) = state.call.as_ref().filter(|call| call.seq == call_seq) else {
        return Err(refused(409, "call_ended", "That call has ended"));
    };
    if call.caller == session.account {
        return Err(bad("Only the other account declines a call"));
    }
    if call.answered {
        return Err(refused(409, "call_answered", "That call was answered"));
    }
    end_call(
        app,
        chat,
        state,
        Some((CallEventKind::Decline, device)),
        device,
    )
    .await?;
    Ok(ServerFrame::Done { reference })
}

/// The ring or a dropped device's grace ran out. Nobody else joined in time: the call ends as missed,
/// by the caller's device. A device that did not come back within its grace has left.
async fn call_timer(app: &Arc<App>, chat: Ulid, state: &mut ChatState) -> Result<(), Refused> {
    let Some(call) = &mut state.call else {
        return Ok(());
    };
    let now = Instant::now();
    if !call.answered && call.ring_until <= now {
        let caller_device = call.caller_device;
        return end_call(app, chat, state, None, caller_device).await;
    }
    let mut left = Vec::new();
    for (device, deadline) in &call.room.gone {
        if *deadline <= now {
            left.push(*device);
        }
    }
    for device in left {
        if let Some(call) = &mut state.call {
            call.room.gone.remove(&device);
        }
        leave(app, chat, state, device).await?;
    }
    Ok(())
}

/// Admits a device to the open call. A device already in it, or back within its grace, writes nothing:
/// a replaced socket is closed with `replaced`. Otherwise it writes `join`, and a device of the other
/// account answers the call and stops its ring.
async fn run_join(
    app: &Arc<App>,
    state: &mut ChatState,
    session: Session,
    call_seq: u64,
    close: Closer,
) -> Result<Joined, Refused> {
    let device = session.device()?;
    let chat = state.chat;
    let Some(call) = state.call.as_mut().filter(|call| call.seq == call_seq) else {
        return Err(refused(409, "call_ended", "That call has ended"));
    };
    call.room.next_connection += 1;
    let connection = call.room.next_connection;
    let joined = Joined {
        shared: call.room.shared.clone(),
        connection,
    };
    let socket = Socket { connection, close };
    if let Some(old) = call.room.sockets.insert(device, socket) {
        if old.close.send((1000, "replaced")).is_err() {
            tracing::debug!(msg = "A replaced call socket had already ended");
        }
        return Ok(joined);
    }
    if call.room.gone.remove(&device).is_some() {
        return Ok(joined);
    }
    let answers = session.account != call.caller && !call.answered;
    let event_list = call.events(
        chat,
        &[(CallEventKind::Join, device)],
        app.storage.saved_now(),
    );
    let saved = app.storage.save_call(event_list.clone()).await;
    if let Err(error) = saved {
        call.room.sockets.remove(&device);
        return Err(error.into());
    }
    call.next_n += 1;
    if answers {
        call.answered = true;
    }
    publish(&mut call.room.events, &event_list);
    let member_list = app.storage.member_accounts(chat).await?;
    for event in event_list {
        app.hub.deliver(
            &member_list,
            Some(device),
            &ServerFrame::CallEvent(Box::new(event)),
        );
    }
    Ok(joined)
}

/// A device's call socket ended. A clean close is a leave at once; a drop gets the grace, and a
/// socket a newer one replaced writes nothing.
async fn run_gone(
    app: &Arc<App>,
    state: &mut ChatState,
    session: Session,
    call_seq: u64,
    connection: u64,
    clean: bool,
) -> Result<(), Refused> {
    let device = session.device()?;
    let Some(call) = &mut state.call else {
        return Ok(());
    };
    if call.seq != call_seq {
        return Ok(());
    }
    match call.room.sockets.get(&device) {
        Some(socket) if socket.connection == connection => {}
        _ => return Ok(()),
    }
    call.room.sockets.remove(&device);
    if clean {
        return leave(app, state.chat, state, device).await;
    }
    let grace = match app.settings.calls.test_grace {
        Some(seconds) => Duration::from_secs(seconds),
        None => GRACE,
    };
    call.room.gone.insert(device, Instant::now() + grace);
    Ok(())
}

/// A device left the call. The last one to leave ends it, by that device.
async fn leave(
    app: &Arc<App>,
    chat: Ulid,
    state: &mut ChatState,
    device: Ulid,
) -> Result<(), Refused> {
    let Some(call) = &mut state.call else {
        return Ok(());
    };
    if call.room.sockets.is_empty() && call.room.gone.is_empty() {
        return end_call(
            app,
            chat,
            state,
            Some((CallEventKind::Leave, device)),
            device,
        )
        .await;
    }
    let event_list = call.events(
        chat,
        &[(CallEventKind::Leave, device)],
        app.storage.saved_now(),
    );
    app.storage.save_call(event_list.clone()).await?;
    call.next_n += 1;
    publish(&mut call.room.events, &event_list);
    let member_list = app.storage.member_accounts(chat).await?;
    for event in event_list {
        app.hub.deliver(
            &member_list,
            Some(device),
            &ServerFrame::CallEvent(Box::new(event)),
        );
    }
    Ok(())
}

/// Text a device in the call typed: saved with the next `n` and republished on `events`, so everyone
/// in the call reads one order. It stays inside the call, off the chat's socket.
async fn run_call_text(
    app: &Arc<App>,
    state: &mut ChatState,
    session: Session,
    call_seq: u64,
    connection: u64,
    text: CallText,
) -> Result<(), Refused> {
    let device = session.device()?;
    let chat = state.chat;
    let Some(call) = &mut state.call else {
        return Err(refused(409, "call_ended", "That call has ended"));
    };
    if call.seq != call_seq {
        return Err(refused(409, "call_ended", "That call has ended"));
    }
    match call.room.sockets.get(&device) {
        Some(socket) if socket.connection == connection => {}
        _ => return Err(refused(409, "call_ended", "That socket left the call")),
    }
    let mut event_list = call.events(
        chat,
        &[(CallEventKind::Text, device)],
        app.storage.saved_now(),
    );
    for event in &mut event_list {
        event.client_at = Some(text.client_at);
        event.epoch = Some(text.epoch);
        event.signature = Some(text.signature.clone());
        event.body = Some(text.body.clone());
    }
    app.storage.save_call(event_list.clone()).await?;
    call.next_n += 1;
    publish(&mut call.room.events, &event_list);
    Ok(())
}

/// Ends the open call, after `first` if given, as missed unless it was answered. It tells the chat's
/// online devices and everyone in the call, then closes every call socket still open with
/// `call_ended`. `first`'s own device is left out on the chat's socket, as it knows already.
async fn end_call(
    app: &Arc<App>,
    chat: Ulid,
    state: &mut ChatState,
    first: Option<(CallEventKind, Ulid)>,
    ender: Ulid,
) -> Result<(), Refused> {
    let Some(call) = &state.call else {
        return Ok(());
    };
    let mut kind_list = Vec::with_capacity(2);
    kind_list.extend(first);
    kind_list.push((CallEventKind::End, ender));
    let mut event_list = call.events(chat, &kind_list, app.storage.saved_now());
    // The end carries the outcome, so the call's rows hold everything about it.
    for event in &mut event_list {
        if event.kind == CallEventKind::End {
            event.outcome = Some(if call.answered {
                CallOutcome::Answered
            } else {
                CallOutcome::Missed
            });
        }
    }
    app.storage.save_call(event_list.clone()).await?;
    let Some(mut call) = state.call.take() else {
        return Ok(());
    };
    publish(&mut call.room.events, &event_list);
    for (_, socket) in call.room.sockets.drain() {
        if socket.close.send((1000, "call_ended")).is_err() {
            tracing::debug!(msg = "A call socket had already ended");
        }
    }
    let member_list = app.storage.member_accounts(chat).await?;
    for event in event_list {
        let except = match event.kind {
            CallEventKind::End => None,
            _ => Some(event.device),
        };
        app.hub.deliver(
            &member_list,
            except,
            &ServerFrame::CallEvent(Box::new(event)),
        );
    }
    Ok(())
}

/// The most files one bundle holds, fixed by the author.
const BUNDLE_FILES: usize = 20;

/// A content message keeps its own complete, unsent uploads in this chat: one for a file kind and up
/// to 20 for a bundle. An edit or delete removes this account's files in this chat.
async fn check_files(app: &App, session: Session, frame: &SendFrame) -> Result<(), Refused> {
    let most = match (frame.kind, frame.content_kind) {
        (Kind::Content, Some(ContentKind::MediaBundle)) => BUNDLE_FILES,
        (
            Kind::Content,
            Some(
                ContentKind::Voice
                | ContentKind::Image
                | ContentKind::Music
                | ContentKind::Video
                | ContentKind::Doc
                | ContentKind::File,
            ),
        ) => 1,
        _ => 0,
    };
    if frame.persist.len() > most {
        return Err(bad("This message cannot keep that many files"));
    }
    if !frame.remove.is_empty() && !matches!(frame.kind, Kind::Edit | Kind::Delete) {
        return Err(bad("Only an edit or delete removes files"));
    }
    let mut seen = Vec::with_capacity(frame.persist.len() + frame.remove.len());
    for (file, keeping) in frame
        .persist
        .iter()
        .map(|file| (file, true))
        .chain(frame.remove.iter().map(|file| (file, false)))
    {
        if seen.contains(file) {
            return Err(bad("A file is named twice"));
        }
        seen.push(*file);
        let Some(stored) = app.storage.file(*file).await? else {
            return Err(refused(404, "no_such_file", "That file does not exist"));
        };
        if stored.chat != frame.chat || stored.removed {
            return Err(refused(404, "no_such_file", "That file does not exist"));
        }
        if stored.account != session.account {
            return Err(refused(403, "not_yours", "That file is someone else's"));
        }
        if keeping && stored.used {
            return Err(refused(409, "file_sent", "That file is already sent"));
        }
        if keeping && stored.received < stored.chunks {
            return Err(refused(
                409,
                "file_incomplete",
                "That file is not all uploaded",
            ));
        }
    }
    Ok(())
}

/// A wrap can only be for one of the chat's accounts.
fn check_wraps(member_list: &[Ulid], wrap_list: Vec<WrapItem>) -> Result<Vec<NewWrap>, Refused> {
    let mut checked = Vec::with_capacity(wrap_list.len());
    for WrapItem { account, wrapped } in wrap_list {
        if !member_list.contains(&account) {
            return Err(refused(
                404,
                "not_member",
                "A wrapped key is for an account not in this chat",
            ));
        }
        checked.push(NewWrap {
            account,
            wrapped: wrapped.0,
        });
    }
    Ok(checked)
}

/// Gives an online device its waiting Welcomes. Each is deleted as it is handed to the connection,
/// and if that connection then dies, the device joins by external commit instead.
pub async fn hand_welcomes(app: &App, account: Ulid, device: Ulid) -> Result<(), Refused> {
    if !app.hub.is_online(account, device) {
        return Ok(());
    }
    for (chat, body) in app.storage.take_welcomes(device).await? {
        app.hub.deliver_to_device(
            account,
            device,
            &ServerFrame::Welcome {
                chat,
                body: Bytes(body),
            },
        );
    }
    Ok(())
}

/// A `seen` or `received`: keeps the highest, and tells the other members' online devices when it
/// rises, except in a channel, where only the waiting sign is drawn.
async fn run_position(
    app: &Arc<App>,
    session: Session,
    reference: Ulid,
    position: Position,
    chat: Ulid,
    up_to_seq: u64,
    client_at: i64,
) -> Result<ServerFrame, Refused> {
    check(app, &Caller::Session(session), Need::Member(chat)).await?;
    let device = session.device()?;
    let Some(access) = app.storage.chat_access(chat, session.account).await? else {
        return Err(refused(404, "not_member", "You are not in that chat"));
    };
    if up_to_seq > access.last_seq {
        let code = match position {
            Position::Received => "bad_received",
            Position::Seen => "bad_seen",
        };
        return Err(refused(400, code, "That message does not exist yet"));
    }
    let mark = app
        .storage
        .record_position(PositionReport {
            position,
            chat,
            account: session.account,
            device,
            up_to_seq,
            client_at,
            now: Ulid::now(),
        })
        .await?;
    if let Some(mark) = mark
        && access.chat_type != ChatType::Channel
    {
        let mut member_list = app.storage.member_accounts(chat).await?;
        member_list.retain(|account| *account != session.account);
        app.hub
            .deliver(&member_list, None, &ServerFrame::Marks { chat, mark });
    }
    Ok(ServerFrame::Done { reference })
}

async fn run_typing(app: &Arc<App>, session: Session, chat: Ulid, on: bool) -> Result<(), Refused> {
    check(app, &Caller::Session(session), Need::Member(chat)).await?;
    let member_list = app.storage.member_accounts(chat).await?;
    app.hub.deliver(
        &member_list,
        session.device,
        &ServerFrame::Typing {
            chat,
            account: session.account,
            on,
        },
    );
    Ok(())
}

async fn run_group_info(
    app: &Arc<App>,
    session: Session,
    reference: Ulid,
    chat: Ulid,
    epoch: u64,
    body: Bytes,
) -> Result<ServerFrame, Refused> {
    check(app, &Caller::Session(session), Need::Member(chat)).await?;
    match app
        .storage
        .put_group_info(chat, epoch, body.0, Ulid::now())
        .await?
    {
        GroupInfoPut::Saved => Ok(ServerFrame::Done { reference }),
        GroupInfoPut::AlreadyStarted => Err(refused(
            409,
            "chat_started",
            "Someone else started this chat first",
        )),
        GroupInfoPut::Stale => Err(refused(
            409,
            "stale_epoch",
            "That is not this chat's current epoch",
        )),
    }
}

async fn run_wraps(
    app: &Arc<App>,
    session: Session,
    reference: Ulid,
    chat: Ulid,
    epoch: u64,
    wrap_list: Vec<WrapItem>,
) -> Result<ServerFrame, Refused> {
    check(app, &Caller::Session(session), Need::Member(chat)).await?;
    let device = session.device()?;
    let member_list = app.storage.member_accounts(chat).await?;
    let wrap_list = check_wraps(&member_list, wrap_list)?;
    app.storage
        .put_wraps(chat, epoch, wrap_list, device)
        .await?;
    Ok(ServerFrame::Done { reference })
}
