//! Chat logic: the per-chat task that serializes a chat's events, and who is online.
//!
//! The task stores and publishes. It never waits on a connection: each device has a bounded outgoing
//! buffer, and when that fills, the connection is closed. The device reconnects and fetches what it
//! missed, so nothing is lost, because every message is saved. Tasks end after `[chat] idle_after` and
//! restart on the next event, so memory does not grow with old chats.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use axum::extract::ws::Utf8Bytes;
use keryx_core::protocol::{
    Bytes, ChatMode, ChatType, ClientFrame, ContentKind, Kind, MessageFrame, SendFrame,
    ServerFrame, WelcomeItem, WrapItem,
};
use keryx_core::ulid::Ulid;
use tokio::sync::{Notify, mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::api::{
    App, Caller, DELETE_OWN, EDIT_OWN, MESSAGE_COMMIT, Need, Refused, SEND_UNENCRYPTED, TRACE,
    check, new_id, operation, refused, send_permission,
};
use crate::storage::{
    Appended, GroupInfoPut, NewMessage, NewWelcome, NewWrap, Position, PositionReport, Session,
    Storage,
};

/// How many events may wait for a chat's task before their senders wait in turn.
const CHAT_QUEUE: usize = 256;

/// For its first 7 days, a new-device notice cannot be removed by that device or any newer one.
const NOTICE_PROTECTED_MS: i64 = 7 * 24 * 60 * 60 * 1000;

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
    pub frame: ClientFrame,
    pub reply: Option<oneshot::Sender<ServerFrame>>,
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
    let idle_after = Duration::from_secs(app.settings.chat.idle_after.max(1));
    loop {
        match tokio::time::timeout(idle_after, queue.recv()).await {
            Ok(Some(job)) => handle(&app, job).await,
            Ok(None) => return,
            Err(_) => {
                queue.close();
                while let Some(job) = queue.recv().await {
                    handle(&app, job).await;
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

async fn handle(app: &Arc<App>, job: Job) {
    let Job {
        trace,
        session,
        frame,
        reply,
    } = job;
    let answer = TRACE
        .scope(trace, async move {
            match frame {
                ClientFrame::Send(frame) => {
                    let reference = frame.reference;
                    let result = operation!("Send", "send", run_send(app, session, frame));
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
                ClientFrame::Sync { reference }
                | ClientFrame::History { reference, .. }
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
        },
    }
}

fn bad(message: &str) -> Refused {
    refused(400, "bad_message", message)
}

async fn run_send(
    app: &Arc<App>,
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
    if frame.kind != Kind::Content && (frame.content_kind.is_some() || frame.call_seq.is_some()) {
        return Err(bad("Only a content message has a content kind or a call"));
    }
    if frame.encrypted && frame.epoch.is_none() {
        return Err(bad("An encrypted message needs its epoch"));
    }
    if frame.kind == Kind::Commit && frame.epoch.is_none() {
        return Err(bad("A commit needs its epoch"));
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
    if frame.kind != Kind::Commit && !(frame.wrap_list.is_empty() && frame.welcome_list.is_empty())
    {
        return Err(bad("Only a commit carries wraps and welcomes"));
    }

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

    let now = Ulid::now();
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
            call_seq: frame.call_seq,
            client_sent_at: frame.client_sent_at,
            now,
            signature: frame.signature.0.clone(),
            body: frame.body.0.clone(),
            wrap_list,
            welcome_list,
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
            call_seq: frame.call_seq,
            client_sent_at: Some(frame.client_sent_at),
            server_saved_at: Some(now),
            body: Some(frame.body),
            signature: Some(frame.signature),
        })),
    );
    for (account, device) in welcome_devices {
        hand_welcomes(app, account, device).await?;
    }
    Ok(ServerFrame::Sent {
        reference: frame.reference,
        seq,
        server_saved_at: now,
    })
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
