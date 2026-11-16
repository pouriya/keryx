mod common;
#[path = "common/person.rs"]
mod person;

use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

use common::{OPERATOR_TOKEN, Server};
use keryx_core::client::{
    Command, Draft, Event, FileSpec, Folder, Hit, PhotoInput, Pointer, Secret, Setting,
    ShownChatProfile, ShownProfile, Thumbnail,
};
use keryx_core::file;
use keryx_core::mls::{AccountWrap, Commit, Device};
use keryx_core::protocol::{
    Ack, Answer, Audience, Bytes, CallEvent, CallEventKind, CallOutcome, ChatAction, ChatEvent,
    ChatFolders, ChatOptions, ChatRole, ChatType, ClientFrame, ContentKind, FolderItem,
    FolderSuggestion, GENERAL_TOPIC, Header, InvitationItem, InvitationShown, Kind, KvChange,
    KvItem, LineAction, MessageFrame, PresenceItem, PresenceState, ProfileKeyShare, ProfileLink,
    Question, RequestItem, RequestState, SendFrame, ServerFrame, Suggestion, SyncPosition,
    TimesItem, TopicName, TopicOptionsBody, TopicWrite, WelcomeItem, WrapItem,
};
use keryx_core::ulid::Ulid;
use person::{
    App, BABAK_WORDS, PASSWORD, Person, SARA_WORDS, add_device, log_in, raw, sign_up, ulid_of,
};
use rustls::pki_types::pem::PemObject;
use serde_json::{Value, json};
use tungstenite::protocol::frame::coding::CloseCode;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{ClientRequestBuilder, Message, WebSocket};

struct Socket {
    inner: WebSocket<MaybeTlsStream<TcpStream>>,
    waiting: Vec<ServerFrame>,
}

fn request(server: &Server, token: Option<&str>, protocol: bool) -> ClientRequestBuilder {
    let url = format!("{}/api/v1/ws", server.origin.replace("http://", "ws://"));
    let mut builder = ClientRequestBuilder::new(url.parse().unwrap());
    if let Some(token) = token {
        builder = builder.with_header("Authorization", format!("Bearer {token}"));
    }
    if protocol {
        builder = builder.with_sub_protocol("keryx.v1");
    }
    builder
}

fn with_timeout(inner: WebSocket<MaybeTlsStream<TcpStream>>) -> Socket {
    if let MaybeTlsStream::Plain(stream) = inner.get_ref() {
        stream
            .set_read_timeout(Some(Duration::from_secs(20)))
            .unwrap();
    }
    Socket {
        inner,
        waiting: Vec::new(),
    }
}

fn connect(server: &Server, token: &str) -> Socket {
    let (inner, response) = tungstenite::connect(request(server, Some(token), true)).unwrap();
    assert_eq!(response.headers()["sec-websocket-protocol"], "keryx.v1");
    let mut socket = with_timeout(inner);
    assert!(matches!(socket.read(), ServerFrame::Features { .. }));
    socket
}

fn reference_of(frame: &ServerFrame) -> Option<Ulid> {
    match frame {
        ServerFrame::Sent { reference, .. }
        | ServerFrame::Done { reference }
        | ServerFrame::Refused { reference, .. }
        | ServerFrame::Chats { reference, .. }
        | ServerFrame::FolderId { reference, .. }
        | ServerFrame::PresenceOf { reference, .. } => Some(*reference),
        ServerFrame::Kv { reference, .. } => *reference,
        _ => None,
    }
}

impl Socket {
    fn send(&mut self, frame: &ClientFrame) {
        let text = serde_json::to_string(frame).unwrap();
        self.inner.send(Message::text(text)).unwrap();
    }

    fn read(&mut self) -> ServerFrame {
        let started = std::time::Instant::now();
        loop {
            assert!(started.elapsed() < Duration::from_secs(20), "No frame came");
            match self.inner.read().unwrap() {
                Message::Text(text) => return serde_json::from_str(text.as_str()).unwrap(),
                Message::Close(frame) => panic!("The socket closed: {frame:?}"),
                _ => {}
            }
        }
    }

    /// The next frame that matches, keeping the others for later.
    fn next_where(&mut self, wanted: impl Fn(&ServerFrame) -> bool) -> ServerFrame {
        for index in 0..self.waiting.len() {
            if wanted(&self.waiting[index]) {
                return self.waiting.remove(index);
            }
        }
        // Pings keep a socket's reads from timing out, so a frame that never comes ends here.
        let started = std::time::Instant::now();
        loop {
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "Waited too long, holding {:?}",
                self.waiting
            );
            let frame = self.read();
            if wanted(&frame) {
                return frame;
            }
            self.waiting.push(frame);
        }
    }

    fn answer(&mut self, reference: Ulid) -> ServerFrame {
        self.next_where(|frame| reference_of(frame) == Some(reference))
    }

    fn request(&mut self, frame: &ClientFrame, reference: Ulid) -> ServerFrame {
        self.send(frame);
        self.answer(reference)
    }

    /// The next stored message of a chat with this sequence id.
    fn message(&mut self, chat: Ulid, seq: u64) -> MessageFrame {
        let frame = self.next_where(|frame| {
            matches!(frame, ServerFrame::Message(message) if message.chat == chat && message.seq == seq)
        });
        match frame {
            ServerFrame::Message(message) => *message,
            _ => unreachable!(),
        }
    }

    /// How the server closed this socket.
    fn closed(&mut self) -> (u16, String) {
        loop {
            match self.inner.read() {
                Ok(Message::Close(Some(frame))) => {
                    return (u16::from(frame.code), frame.reason.to_string());
                }
                Ok(Message::Close(None)) | Err(_) => {
                    return (u16::from(CloseCode::Abnormal), String::new());
                }
                Ok(_) => {}
            }
        }
    }
}

fn reference() -> Ulid {
    Ulid::new().unwrap()
}

fn assert_refused(frame: ServerFrame, wanted: &str) {
    match frame {
        ServerFrame::Refused { code, message, .. } => {
            assert_eq!(code, wanted, "{message}");
            assert!(!message.is_empty());
        }
        other => panic!("Wanted a refusal of {wanted}, and got {other:?}"),
    }
}

fn seq_of(frame: ServerFrame) -> u64 {
    match frame {
        ServerFrame::Sent { seq, .. } => seq,
        other => panic!("Wanted it stored, and got {other:?}"),
    }
}

fn done(frame: ServerFrame) {
    assert!(matches!(frame, ServerFrame::Done { .. }), "{frame:?}");
}

fn wrap_items(wrap_list: &[AccountWrap]) -> Vec<WrapItem> {
    let mut items = Vec::new();
    for wrap in wrap_list {
        items.push(WrapItem {
            account: wrap.account,
            wrapped: Bytes(wrap.wrapped.to_vec()),
        });
    }
    items
}

/// A sealed text from this person's device.
fn sealed(person: &Person, chat: Ulid, kind: Kind, pointer: Option<u64>, text: &[u8]) -> SendFrame {
    let epoch = person.device.epoch(chat).unwrap();
    let content_kind = if kind == Kind::Content {
        Some(ContentKind::Text)
    } else {
        None
    };
    sealed_as(person, chat, kind, content_kind, epoch, pointer, text)
}

fn sealed_as(
    person: &Person,
    chat: Ulid,
    kind: Kind,
    content_kind: Option<ContentKind>,
    epoch: u64,
    pointer_seq: Option<u64>,
    text: &[u8],
) -> SendFrame {
    let client_sent_at = Ulid::now();
    let header = Header {
        chat,
        device: person.device.record().device,
        kind,
        content_kind,
        encrypted: true,
        epoch: Some(epoch),
        pointer_seq,
        call_seq: None,
        client_sent_at,
        topic: None,
        thread_seq: None,
    };
    let (body, signature) = person.device.seal(&header, text).unwrap();
    SendFrame {
        reference: reference(),
        chat,
        kind,
        content_kind,
        encrypted: true,
        epoch: Some(epoch),
        pointer_seq,
        client_sent_at,
        body: Bytes(body),
        signature: Bytes(signature.to_vec()),
        wrap_list: Vec::new(),
        welcome_list: Vec::new(),
        group_info: None,
        persist: Vec::new(),
        remove: Vec::new(),
        profile_key_list: Vec::new(),
        topic: None,
        thread_seq: None,
        action: None,
        chat_key: None,
    }
}

fn post(socket: &mut Socket, frame: SendFrame) -> ServerFrame {
    let wanted = frame.reference;
    socket.request(&ClientFrame::Send(Box::new(frame)), wanted)
}

fn commit_frame(
    device: &Device,
    chat: Ulid,
    commit: &Commit,
    welcome_list: Vec<WelcomeItem>,
) -> SendFrame {
    let client_sent_at = Ulid::now();
    let header = Header {
        chat,
        device: device.record().device,
        kind: Kind::Commit,
        content_kind: None,
        encrypted: false,
        epoch: Some(commit.epoch),
        pointer_seq: None,
        call_seq: None,
        client_sent_at,
        topic: None,
        thread_seq: None,
    };
    let signature = device.sign(&header, &commit.commit).unwrap();
    SendFrame {
        reference: reference(),
        chat,
        kind: Kind::Commit,
        content_kind: None,
        encrypted: false,
        epoch: Some(commit.epoch),
        pointer_seq: None,
        client_sent_at,
        body: Bytes(commit.commit.clone()),
        signature: Bytes(signature.to_vec()),
        wrap_list: wrap_items(&commit.wrap_list),
        welcome_list,
        group_info: Some(Bytes(commit.group_info.clone())),
        persist: Vec::new(),
        remove: Vec::new(),
        profile_key_list: commit
            .profile_key_list
            .iter()
            .map(|(account, sealed)| ProfileKeyShare {
                account: *account,
                sealed: Bytes(sealed.clone()),
            })
            .collect(),
        topic: None,
        thread_seq: None,
        action: None,
        chat_key: None,
    }
}

fn upload_group_info(socket: &mut Socket, chat: Ulid, epoch: u64, body: Vec<u8>) -> ServerFrame {
    let wanted = reference();
    socket.request(
        &ClientFrame::GroupInfo {
            reference: wanted,
            chat,
            epoch,
            body: Bytes(body),
            chat_profile: None,
            chat_key: None,
            chat_options: None,
        },
        wanted,
    )
}

/// Two people with sockets, in a started chat at epoch 1, the way real clients get there.
struct Pair {
    server: Server,
    sara: Person,
    babak: Person,
    sara_socket: Socket,
    babak_socket: Socket,
    chat: Ulid,
}

/// Two people sign up and start their chat through their clients, which takes it to epoch 1. Then
/// each hands over the device it set up, for the tests that sign by hand.
fn pair_on(server: Server) -> Pair {
    let (sara, babak, chat) = apps_on(&server);
    let mut sara = sara.into_person(SARA_WORDS);
    sara.username = "sara_x".to_string();
    let mut babak = babak.into_person(BABAK_WORDS);
    babak.username = "babak_x".to_string();
    let sara_socket = connect(&server, &sara.token);
    let babak_socket = connect(&server, &babak.token);
    Pair {
        server,
        sara,
        babak,
        sara_socket,
        babak_socket,
        chat,
    }
}

/// Two people's clients in a started chat.
fn apps_on(server: &Server) -> (App, App, Ulid) {
    let mut sara = App::sign_up(server, "sara_x", SARA_WORDS);
    let mut babak = App::sign_up(server, "babak_x", BABAK_WORDS);
    sara.command(
        server,
        Command::StartChat {
            account: babak.id(),
        },
    );
    let self_chat = sara.self_chat;
    let Event::ChatReady { chat } = sara.wait(
        server,
        |event| matches!(event, Event::ChatReady { chat } if Some(*chat) != self_chat),
    ) else {
        unreachable!();
    };
    babak.wait(
        server,
        |event| matches!(event, Event::ChatReady { chat: ready } if *ready == chat),
    );
    (sara, babak, chat)
}

fn shown(event: Event) -> keryx_core::client::Shown {
    match event {
        Event::Message { message } => message,
        other => panic!("Wanted a message, and got {other:?}"),
    }
}

fn text_of(message: &keryx_core::client::Shown) -> Option<&str> {
    match &message.text {
        Some(text) => Some(text.0.as_str()),
        None => None,
    }
}

fn is_message(event: &Event, chat: Ulid, seq: u64) -> bool {
    matches!(event, Event::Message { message } if message.chat == chat && message.seq == seq)
}

fn open_server() -> Server {
    Server::start("", "[registration]\nallow = true\n")
}

fn pair() -> Pair {
    pair_on(open_server())
}

/// One chat's messages after `after_seq`, at most `limit`, through `sync` from the start, page by
/// page, in sequence order.
fn chat_messages(socket: &mut Socket, chat: Ulid, after_seq: u64, limit: u32) -> Vec<MessageFrame> {
    let (message_list, _) = sync_chat(socket, chat);
    message_list
        .into_iter()
        .filter(|message| message.seq > after_seq)
        .take(usize::try_from(limit).unwrap())
        .collect()
}

/// Everything `sync` holds for one chat from the start, following `more` to the last page: its
/// messages in sequence order, and its call rows.
fn sync_chat(socket: &mut Socket, chat: Ulid) -> (Vec<MessageFrame>, Vec<CallEvent>) {
    let mut message_list = Vec::new();
    let mut call_list = Vec::new();
    let mut after = None;
    loop {
        let wanted = reference();
        let answer = socket.request(
            &ClientFrame::Sync {
                reference: wanted,
                since: None,
                after,
                chat: Some(chat),
            },
            wanted,
        );
        let ServerFrame::Chats {
            message_list: messages,
            call_list: calls,
            more,
            ..
        } = answer
        else {
            panic!("Wanted a sync page, and got {answer:?}");
        };
        let mut last = after;
        for message in &messages {
            let position = SyncPosition {
                at: message.server_saved_at.unwrap(),
                chat,
                seq: message.seq,
                n: 0,
            };
            last = last.max(Some(position));
        }
        for event in &calls {
            let position = SyncPosition {
                at: event.server_at,
                chat,
                seq: event.call_seq,
                n: event.n,
            };
            last = last.max(Some(position));
        }
        message_list.extend(messages);
        call_list.extend(calls);
        if !more {
            break;
        }
        after = last;
    }
    message_list.sort_by_key(|message| message.seq);
    (message_list, call_list)
}

fn sync_frame(reference: Ulid) -> ClientFrame {
    ClientFrame::Sync {
        reference,
        since: None,
        after: None,
        chat: None,
    }
}

fn chat_item(socket: &mut Socket, chat: Ulid) -> keryx_core::protocol::ChatItem {
    let wanted = reference();
    let answer = socket.request(&sync_frame(wanted), wanted);
    let ServerFrame::Chats { chat_list, .. } = answer else {
        panic!("Wanted the chats");
    };
    for item in chat_list {
        if item.chat == chat {
            return item;
        }
    }
    panic!("The chat is not on the list");
}

fn open_message(reader: &Device, sender: &Device, message: &MessageFrame) -> Vec<u8> {
    let signature: [u8; 64] = message.signature.clone().unwrap().0.try_into().unwrap();
    reader
        .open(
            &message.header().unwrap(),
            &message.body.clone().unwrap().0,
            &signature,
            &sender.record().device_public,
        )
        .unwrap()
        .to_vec()
}

#[test]
fn two_people_exchange_encrypted_text_the_server_cannot_read() {
    let server = open_server();
    let (mut sara, mut babak, chat) = apps_on(&server);
    sara.command(
        &server,
        Command::SendText {
            mentions: Vec::new(),
            thread: None,
            topic: None,
            chat,
            text: Secret("سلام".to_string()),
            reply: None,
        },
    );
    let Event::Pending { local, .. } =
        sara.wait(&server, |event| matches!(event, Event::Pending { .. }))
    else {
        unreachable!();
    };
    let Event::Sent {
        local: sent,
        seq,
        saved_at,
        ..
    } = sara.wait(&server, |event| matches!(event, Event::Sent { .. }))
    else {
        unreachable!();
    };
    assert_eq!(sent, local);
    assert!(saved_at > 0);

    // The other device gets it with its sequence id, and it opens.
    let message = shown(babak.wait(&server, |event| is_message(event, chat, seq)));
    assert_eq!(text_of(&message), Some("سلام"));
    assert_eq!(message.account, Some(sara.id()));

    // The reply goes the other way, and sequence ids keep growing.
    babak.command(
        &server,
        Command::SendText {
            mentions: Vec::new(),
            thread: None,
            topic: None,
            chat,
            text: Secret("hello".to_string()),
            reply: None,
        },
    );
    let Event::Sent { seq: reply_seq, .. } =
        babak.wait(&server, |event| matches!(event, Event::Sent { .. }))
    else {
        unreachable!();
    };
    assert_eq!(reply_seq, seq + 1);
    let message = shown(sara.wait(&server, |event| is_message(event, chat, reply_seq)));
    assert_eq!(text_of(&message), Some("hello"));

    // History and the list agree, from either side.
    babak.events.clear();
    babak.command(&server, Command::OpenChat { chat });
    let Event::PageLoaded { has_older, .. } =
        babak.wait(&server, |event| matches!(event, Event::PageLoaded { .. }))
    else {
        unreachable!();
    };
    assert!(!has_older, "the page reached the chat's first message");
    let mut texts = Vec::new();
    for event in babak.events.drain(..) {
        if let Event::Message { message } = event {
            texts.push(message.text.unwrap().0.clone());
        }
    }
    assert_eq!(texts, vec!["سلام".to_string(), "hello".to_string()]);
    sara.events.clear();
    sara.command(&server, Command::ChatList);
    let Event::Chats { chat_list } =
        sara.wait(&server, |event| matches!(event, Event::Chats { .. }))
    else {
        unreachable!();
    };
    let mut item = None;
    for listed in chat_list {
        if listed.chat == chat {
            item = Some(listed);
        }
    }
    let item = item.unwrap();
    assert_eq!(item.chat_type, ChatType::Direct);
    assert_eq!(item.last_seq, Some(reply_seq));
    assert_eq!(item.last_content_seq, Some(reply_seq));
    assert_eq!(item.last_kind, Some(Kind::Content));
    assert!(item.last_at.is_some());

    // The wraps went up when the chat started and with the commit, and the words open them.
    let (status, wraps) = server.call(
        "GET",
        &format!("/api/v1/chats/{chat}/wraps"),
        sara.token.as_deref(),
        None,
    );
    assert_eq!(status, 200);
    let wrap_list = wraps["data"]["wrap_list"].as_array().unwrap();
    assert_eq!(wrap_list.len(), 4, "both people at epoch 0, and both at 1");

    // The ciphertext the server stores does not hold the text.
    let database = std::fs::read(server.path("keryx.db")).unwrap();
    assert!(
        !database
            .windows("سلام".len())
            .any(|bytes| bytes == "سلام".as_bytes())
    );
}

#[test]
fn a_new_device_gets_everything_with_only_the_words() {
    let server = open_server();
    let (mut sara, mut babak, chat) = apps_on(&server);
    babak.command(
        &server,
        Command::SendText {
            mentions: Vec::new(),
            thread: None,
            topic: None,
            chat,
            text: Secret("before".to_string()),
            reply: None,
        },
    );
    let Event::Sent { seq, .. } = babak.wait(&server, |event| matches!(event, Event::Sent { .. }))
    else {
        unreachable!();
    };

    // Sara arrives on a new device, with nobody approving anything, and types only her words.
    let mut new = App::new();
    new.command(
        &server,
        Command::LogIn {
            username: "sara_x".to_string(),
            password: Secret(PASSWORD.to_string()),
            captcha: None,
        },
    );
    new.wait(&server, |event| {
        matches!(event, Event::NeedWords { restore: true })
    });
    new.command(
        &server,
        Command::Restore {
            words: Secret(BABAK_WORDS.to_string()),
        },
    );
    new.wait(
        &server,
        |event| matches!(event, Event::Error { code, .. } if code == "wrong_words"),
    );
    new.command(
        &server,
        Command::Restore {
            words: Secret(SARA_WORDS.to_string()),
        },
    );
    let Event::SignedIn {
        device: Some(new_id),
        ..
    } = new.wait(&server, |event| {
        matches!(
            event,
            Event::SignedIn {
                device: Some(_),
                ..
            }
        )
    })
    else {
        unreachable!();
    };

    // Her old device sees the notice in her self chat.
    let self_chat = sara.self_chat.unwrap();
    sara.wait(&server, |event| {
        matches!(event, Event::Notice { chat, device, .. } if *chat == self_chat && *device == new_id)
    });

    // It joins by itself, catches up through the wraps, and reads what came before.
    new.wait(
        &server,
        |event| matches!(event, Event::ChatReady { chat: ready } if *ready == chat),
    );
    new.events.clear();
    new.command(&server, Command::OpenChat { chat });
    let message = shown(new.wait(&server, |event| is_message(event, chat, seq)));
    assert_eq!(text_of(&message), Some("before"));
    assert_eq!(message.account, Some(babak.id()));

    // The others apply its join, so what it sends next opens for them.
    new.command(
        &server,
        Command::SendText {
            mentions: Vec::new(),
            thread: None,
            topic: None,
            chat,
            text: Secret("from the new one".to_string()),
            reply: None,
        },
    );
    let Event::Sent { seq, .. } = new.wait(&server, |event| matches!(event, Event::Sent { .. }))
    else {
        unreachable!();
    };
    let message = shown(babak.wait(&server, |event| is_message(event, chat, seq)));
    assert_eq!(text_of(&message), Some("from the new one"));
    let message = shown(sara.wait(&server, |event| is_message(event, chat, seq)));
    assert_eq!(text_of(&message), Some("from the new one"));
}

#[test]
fn a_new_device_cannot_remove_its_notice_but_an_older_one_can() {
    let mut pair = pair();
    let self_chat = pair.sara.self_chat;
    let token = log_in(&pair.server, "sara_x");
    let new_device = add_device(&pair.server, pair.sara.id, &pair.sara.keys, &token);
    let mut new_socket = connect(&pair.server, &token);
    let page = chat_messages(&mut new_socket, self_chat, 0, 200);
    assert_eq!(page.len(), 2, "one notice per device so far");
    let first_seq = page[0].seq;
    let notice_seq = page[1].seq;
    assert_eq!(page[1].device, Some(new_device.record().device));
    // The notice is the self chat's last content, so a later commit cannot hide the preview.
    let item = chat_item(&mut new_socket, self_chat);
    assert_eq!(item.last_content_seq, Some(notice_seq));
    assert_eq!(
        item.last_content.map(|message| (message.seq, message.kind)),
        Some((notice_seq, Some(Kind::DeviceNotice)))
    );

    let delete = |device: &Device| {
        let client_sent_at = Ulid::now();
        let header = Header {
            chat: self_chat,
            device: device.record().device,
            kind: Kind::Delete,
            content_kind: None,
            encrypted: true,
            epoch: Some(0),
            pointer_seq: Some(notice_seq),
            call_seq: None,
            client_sent_at,
            topic: None,
            thread_seq: None,
        };
        let signature = device.sign(&header, b"").unwrap();
        SendFrame {
            reference: reference(),
            chat: self_chat,
            kind: Kind::Delete,
            content_kind: None,
            encrypted: true,
            epoch: Some(0),
            pointer_seq: Some(notice_seq),
            client_sent_at,
            body: Bytes(Vec::new()),
            signature: Bytes(signature.to_vec()),
            wrap_list: Vec::new(),
            welcome_list: Vec::new(),
            group_info: None,
            persist: Vec::new(),
            remove: Vec::new(),
            profile_key_list: Vec::new(),
            topic: None,
            thread_seq: None,
            action: None,
            chat_key: None,
        }
    };

    assert_refused(
        post(&mut new_socket, delete(&new_device)),
        "notice_protected",
    );
    let seq = seq_of(post(&mut pair.sara_socket, delete(&pair.sara.device)));
    let page = chat_messages(&mut pair.sara_socket, self_chat, notice_seq - 1, 200);
    assert_eq!(page[0].kind, None, "it is wiped to its sequence id");
    assert_eq!(page[1].seq, seq);

    // Walks the preview back to the notice before it, as the list is not moved by a delete.
    let item = chat_item(&mut pair.sara_socket, self_chat);
    assert_eq!(item.chat_type, ChatType::SelfChat);
    assert_eq!(item.last_kind, Some(Kind::Delete));
    assert_eq!(item.last_content_seq, Some(first_seq));
    assert!(item.last_at.is_some());
}

#[test]
fn only_the_sender_edits_and_a_delete_wipes_the_target_and_its_edits() {
    let mut pair = pair();
    let chat = pair.chat;
    let first = sealed(&pair.sara, chat, Kind::Content, None, b"first");
    let first_seq = seq_of(post(&mut pair.sara_socket, first));
    let second = sealed(&pair.sara, chat, Kind::Content, None, b"second");
    let second_seq = seq_of(post(&mut pair.sara_socket, second));
    let item = chat_item(&mut pair.sara_socket, chat);
    assert_eq!(item.last_content_seq, Some(second_seq));
    // The chat's epoch is the one its last accepted commit reached: starting it added Babak.
    assert_eq!(item.epoch, Some(1));
    let last_at = item.last_at;

    // Babak cannot edit or delete Sara's words. Not even a message's sender can edit what is gone.
    let edit = sealed(
        &pair.babak,
        chat,
        Kind::Edit,
        Some(second_seq),
        b"not yours",
    );
    assert_refused(post(&mut pair.babak_socket, edit), "not_yours");
    let delete = sealed(&pair.babak, chat, Kind::Delete, Some(second_seq), b"");
    assert_refused(post(&mut pair.babak_socket, delete), "not_yours");
    let edit = sealed(&pair.sara, chat, Kind::Edit, Some(9_999), b"nothing there");
    assert_refused(post(&mut pair.sara_socket, edit), "no_such_message");

    // Her edit is saved, doesn't move the chat up, and the other person gets it.
    let edit = sealed(
        &pair.sara,
        chat,
        Kind::Edit,
        Some(second_seq),
        b"second, edited",
    );
    let edit_seq = seq_of(post(&mut pair.sara_socket, edit));
    let received = pair.babak_socket.message(chat, edit_seq);
    assert_eq!(received.pointer_seq, Some(second_seq));
    assert_eq!(
        open_message(&pair.babak.device, &pair.sara.device, &received),
        b"second, edited"
    );
    let item = chat_item(&mut pair.sara_socket, chat);
    assert_eq!(item.last_seq, Some(edit_seq));
    assert_eq!(item.last_kind, Some(Kind::Edit));
    assert_eq!(item.last_content_seq, Some(second_seq));
    assert_eq!(item.last_at, last_at);

    // A reaction is content, moves the preview, and cannot be edited.
    let reaction = sealed_as(
        &pair.babak,
        chat,
        Kind::Content,
        Some(ContentKind::Reaction),
        1,
        None,
        "👍".as_bytes(),
    );
    let reaction_seq = seq_of(post(&mut pair.babak_socket, reaction));
    assert_eq!(
        chat_item(&mut pair.sara_socket, chat).last_content_seq,
        Some(reaction_seq)
    );
    let edit = sealed(&pair.babak, chat, Kind::Edit, Some(reaction_seq), b"no");
    assert_refused(post(&mut pair.babak_socket, edit), "cannot_edit");

    // Removing a reaction is a delete pointing at it, and the preview walks back.
    let delete = sealed(&pair.babak, chat, Kind::Delete, Some(reaction_seq), b"");
    let removal_seq = seq_of(post(&mut pair.babak_socket, delete));
    assert_eq!(
        chat_item(&mut pair.sara_socket, chat).last_content_seq,
        Some(second_seq)
    );
    assert_eq!(
        chat_item(&mut pair.sara_socket, chat).last_seq,
        Some(removal_seq)
    );

    // Deleting the message takes its edit too, keeps every sequence id, and walks back again.
    let delete = sealed(&pair.sara, chat, Kind::Delete, Some(second_seq), b"");
    let delete_seq = seq_of(post(&mut pair.sara_socket, delete));
    let received = pair.babak_socket.message(chat, delete_seq);
    assert_eq!(received.kind, Some(Kind::Delete));
    assert_eq!(received.pointer_seq, Some(second_seq));
    let page = chat_messages(&mut pair.sara_socket, chat, first_seq - 1, 200);
    let mut seen = Vec::new();
    for message in &page {
        seen.push(message.seq);
    }
    assert_eq!(
        seen,
        (first_seq..=delete_seq).collect::<Vec<_>>(),
        "no gaps"
    );
    for message in &page {
        if message.seq == second_seq || message.seq == edit_seq || message.seq == reaction_seq {
            assert_eq!(message.kind, None, "{}", message.seq);
            assert_eq!(message.body, None);
            assert!(message.header().is_none());
        }
    }
    assert_eq!(page[0].kind, Some(Kind::Content));
    let item = chat_item(&mut pair.sara_socket, chat);
    assert_eq!(item.last_content_seq, Some(first_seq));
    assert!(item.last_at.unwrap() <= last_at.unwrap());

    // What is gone cannot be edited or deleted, a commit cannot be deleted, and a commit has no text.
    let again = sealed(&pair.sara, chat, Kind::Delete, Some(second_seq), b"");
    assert_refused(post(&mut pair.sara_socket, again), "no_such_message");
    let on_edit = sealed(&pair.sara, chat, Kind::Edit, Some(1), b"the commit");
    assert_refused(post(&mut pair.sara_socket, on_edit), "cannot_edit");
    let on_commit = sealed(&pair.sara, chat, Kind::Delete, Some(1), b"");
    assert_refused(post(&mut pair.sara_socket, on_commit), "cannot_delete");
}

#[test]
fn the_first_commit_for_each_epoch_wins_and_group_info_follows_the_rules() {
    let mut pair = pair();
    let chat = pair.chat;
    let accounts = [pair.sara.key.clone(), pair.babak.key.clone()];
    let sara_commit = pair.sara.device.rotate(chat, &accounts).unwrap();
    let babak_commit = pair.babak.device.rotate(chat, &accounts).unwrap();
    assert_eq!(sara_commit.epoch, babak_commit.epoch);

    let frame = commit_frame(&pair.sara.device, chat, &sara_commit, Vec::new());
    seq_of(post(&mut pair.sara_socket, frame));
    // The commit's group info was saved with it, so a new device joins from the right epoch.
    let (_, reply) = pair.server.call(
        "GET",
        &format!("/api/v1/chats/{chat}/group_info"),
        Some(&pair.babak.token),
        None,
    );
    assert_eq!(reply["data"]["epoch"], sara_commit.epoch);
    assert_eq!(
        reply["data"]["group_info"],
        json!(Bytes(sara_commit.group_info.clone()))
    );
    let frame = commit_frame(&pair.babak.device, chat, &babak_commit, Vec::new());
    assert_refused(post(&mut pair.babak_socket, frame), "commit_lost");
    pair.babak.device.commit_rejected(chat).unwrap();

    // A second group for epoch 0 loses, and so does anything but the current epoch.
    assert_refused(
        upload_group_info(&mut pair.babak_socket, chat, 0, vec![1]),
        "chat_started",
    );
    assert_refused(
        upload_group_info(&mut pair.babak_socket, chat, 99, vec![1]),
        "stale_epoch",
    );
    pair.sara.device.commit_accepted(chat, Ulid::now()).unwrap();
    let group_info = pair.sara.device.group_info(chat).unwrap();
    done(upload_group_info(
        &mut pair.babak_socket,
        chat,
        sara_commit.epoch,
        group_info,
    ));

    // Before epoch 0 is uploaded, no commit or later epoch can come in: here a chat with a third
    // person that nobody has started.
    let third = sign_up(&pair.server, "third_x", BABAK_WORDS);
    let (_, reply) = pair.server.call(
        "POST",
        "/api/v1/chats",
        Some(&pair.sara.token),
        Some(&json!({"account": third.id})),
    );
    let unstarted = ulid_of(&reply["data"]["chat"]);
    assert_refused(
        upload_group_info(&mut pair.sara_socket, unstarted, 3, vec![1]),
        "stale_epoch",
    );
    let mut lost = commit_frame(&pair.sara.device, unstarted, &sara_commit, Vec::new());
    lost.epoch = Some(1);
    lost.wrap_list.clear();
    assert_refused(post(&mut pair.sara_socket, lost), "commit_lost");

    // Epoch 0 must be the chat's own group info, which starts the public group commits are read
    // against.
    assert_refused(
        upload_group_info(&mut pair.sara_socket, unstarted, 0, vec![1]),
        "bad_message",
    );
    let accounts = [pair.sara.key.clone(), third.key.clone()];
    let started = pair
        .sara
        .device
        .create_chat(unstarted, Ulid::now(), &accounts)
        .unwrap();
    assert_refused(
        upload_group_info(
            &mut pair.sara_socket,
            unstarted,
            0,
            sara_commit.group_info.clone(),
        ),
        "bad_message",
    );

    // The same epoch 0 again is a resend whose answer was lost, and anything else lost the start.
    done(upload_group_info(
        &mut pair.sara_socket,
        unstarted,
        0,
        started.group_info.clone(),
    ));
    done(upload_group_info(
        &mut pair.sara_socket,
        unstarted,
        0,
        started.group_info,
    ));
    assert_refused(
        upload_group_info(&mut pair.sara_socket, unstarted, 0, vec![2]),
        "chat_started",
    );
}

#[test]
fn a_commit_is_read_by_the_server_and_changes_only_what_it_allowed() {
    let mut pair = pair();
    let chat = pair.chat;
    let accounts = [pair.sara.key.clone(), pair.babak.key.clone()];

    // Taking out a device that is not ended, of an account still in, is refused.
    let babak_device = pair.babak.device.record().device;
    let commit = pair
        .sara
        .device
        .remove_devices(chat, &[babak_device], &accounts)
        .unwrap();
    let frame = commit_frame(&pair.sara.device, chat, &commit, Vec::new());
    assert_refused(post(&mut pair.sara_socket, frame), "bad_commit");
    pair.sara.device.commit_rejected(chat).unwrap();

    // So is bringing in a device of an account the chat does not hold.
    let third = sign_up(&pair.server, "third_x", MINA_WORDS);
    let package = third.device.key_package().unwrap();
    let with_third = [
        pair.sara.key.clone(),
        pair.babak.key.clone(),
        third.key.clone(),
    ];
    let commit = pair
        .sara
        .device
        .add_devices(chat, &[package], &with_third)
        .unwrap();
    // Its wrap for the stranger would be refused first, so only the MLS add is left to name them.
    let mut frame = commit_frame(&pair.sara.device, chat, &commit, Vec::new());
    frame.wrap_list.retain(|wrap| wrap.account != third.id);
    frame
        .profile_key_list
        .retain(|share| share.account != third.id);
    assert_refused(post(&mut pair.sara_socket, frame), "bad_commit");
    pair.sara.device.commit_rejected(chat).unwrap();

    // A device joins by itself only as itself: Babak's new device's join, sent by Sara, is refused.
    let token = log_in(&pair.server, "babak_x");
    let mut fresh = add_device(&pair.server, pair.babak.id, &pair.babak.keys, &token);
    let group_info = pair.sara.device.group_info(chat).unwrap();
    let join = fresh
        .join_external(chat, &group_info, Ulid::now(), &accounts, None)
        .unwrap();
    let frame = commit_frame(&fresh, chat, &join, Vec::new());
    assert_refused(post(&mut pair.sara_socket, frame), "bad_commit");

    // Bytes that are no commit are refused too, and the real one goes in after them.
    let commit = pair.sara.device.rotate(chat, &accounts).unwrap();
    let mut frame = commit_frame(&pair.sara.device, chat, &commit, Vec::new());
    frame.body = Bytes(vec![0, 1, 2]);
    assert_refused(post(&mut pair.sara_socket, frame), "bad_commit");
    let frame = commit_frame(&pair.sara.device, chat, &commit, Vec::new());
    seq_of(post(&mut pair.sara_socket, frame));
}

#[test]
fn frames_that_break_the_rules_are_refused_with_their_reference() {
    let mut pair = pair();
    let chat = pair.chat;
    let good = sealed(&pair.sara, chat, Kind::Content, None, b"x");
    let socket = &mut pair.sara_socket;

    for (change, code) in [
        (
            Box::new(|frame: &mut SendFrame| frame.content_kind = None)
                as Box<dyn Fn(&mut SendFrame)>,
            "bad_message",
        ),
        (
            Box::new(|frame: &mut SendFrame| frame.epoch = None),
            "bad_message",
        ),
        (
            Box::new(|frame: &mut SendFrame| frame.pointer_seq = Some(1)),
            "bad_message",
        ),
        (
            Box::new(|frame: &mut SendFrame| {
                frame.kind = Kind::Commit;
            }),
            "bad_message",
        ),
        (
            Box::new(|frame: &mut SendFrame| {
                frame.kind = Kind::Commit;
                frame.content_kind = None;
                frame.epoch = None;
            }),
            "bad_message",
        ),
        (
            Box::new(|frame: &mut SendFrame| {
                frame.kind = Kind::Commit;
                frame.content_kind = None;
            }),
            "bad_message",
        ),
        (
            Box::new(|frame: &mut SendFrame| frame.group_info = Some(Bytes(vec![0]))),
            "bad_message",
        ),
        (
            Box::new(|frame: &mut SendFrame| {
                frame.kind = Kind::Edit;
                frame.content_kind = None;
            }),
            "bad_message",
        ),
        (
            Box::new(|frame: &mut SendFrame| {
                frame.kind = Kind::DeviceNotice;
                frame.content_kind = None;
            }),
            "server_only",
        ),
        (
            Box::new(|frame: &mut SendFrame| {
                frame.wrap_list = vec![WrapItem {
                    account: frame.chat,
                    wrapped: Bytes(vec![0]),
                }];
            }),
            "bad_message",
        ),
        (
            Box::new(|frame: &mut SendFrame| {
                frame.encrypted = false;
            }),
            "no_permission",
        ),
        (
            Box::new(|frame: &mut SendFrame| {
                frame.encrypted = false;
                frame.content_kind = Some(ContentKind::Reaction);
            }),
            "bad_message",
        ),
    ] {
        let mut frame = good.clone();
        frame.reference = reference();
        change(&mut frame);
        if code == "commit_or_ok" {
            // A call's own message is saved, and does not move the preview.
            let before = chat_item(socket, chat).last_content_seq;
            seq_of(post(socket, frame));
            assert_eq!(chat_item(socket, chat).last_content_seq, before);
            continue;
        }
        assert_refused(post(socket, frame), code);
    }

    // Who a wrap or Welcome is for is checked, and a commit can't name a stranger.
    let stranger = Ulid::new().unwrap();
    let mut frame = good.clone();
    frame.reference = reference();
    frame.kind = Kind::Commit;
    frame.content_kind = None;
    frame.epoch = Some(2);
    frame.group_info = Some(Bytes(vec![0]));
    frame.wrap_list = vec![WrapItem {
        account: stranger,
        wrapped: Bytes(vec![0]),
    }];
    assert_refused(post(socket, frame.clone()), "not_member");
    frame.reference = reference();
    frame.wrap_list = Vec::new();
    frame.welcome_list = vec![WelcomeItem {
        device: stranger,
        body: Bytes(vec![0]),
    }];
    assert_refused(post(socket, frame), "no_such_device");

    let wanted = reference();
    assert_refused(
        socket.request(
            &ClientFrame::Wraps {
                reference: wanted,
                chat,
                epoch: 1,
                wrap_list: vec![WrapItem {
                    account: stranger,
                    wrapped: Bytes(vec![0]),
                }],
            },
            wanted,
        ),
        "not_member",
    );
}

#[test]
fn a_stranger_gets_nothing_from_a_chat() {
    let mut pair = pair();
    let chat = pair.chat;
    let stranger = sign_up(&pair.server, "third_x", BABAK_WORDS);
    let mut socket = connect(&pair.server, &stranger.token);
    let mut frame = sealed(&pair.sara, chat, Kind::Content, None, b"x");
    frame.reference = reference();
    assert_refused(post(&mut socket, frame), "not_member");
    assert_refused(
        {
            let wanted = reference();
            socket.request(
                &ClientFrame::Sync {
                    reference: wanted,
                    since: None,
                    after: None,
                    chat: Some(chat),
                },
                wanted,
            )
        },
        "not_member",
    );
    assert_refused(
        {
            let wanted = reference();
            socket.request(
                &ClientFrame::Seen {
                    reference: wanted,
                    chat,
                    up_to_seq: 1,
                    client_seen_at: 1,
                    topic: None,
                    thread_seq: None,
                },
                wanted,
            )
        },
        "not_member",
    );
    assert_refused(
        upload_group_info(&mut socket, chat, 1, vec![1]),
        "not_member",
    );
    assert_refused(
        {
            let wanted = reference();
            socket.request(
                &ClientFrame::Wraps {
                    reference: wanted,
                    chat,
                    epoch: 1,
                    wrap_list: Vec::new(),
                },
                wanted,
            )
        },
        "not_member",
    );
    // Typing into someone else's chat reaches nobody, and takes no answer.
    socket.send(&ClientFrame::Typing {
        chat,
        on: true,
        topic: None,
        thread_seq: None,
    });

    // An admin passes every permission and is still no member.
    let (status, reply) = pair.server.call(
        "POST",
        "/api/v1/accounts",
        Some(OPERATOR_TOKEN),
        Some(&json!({"username": "boss_x", "admin": true})),
    );
    assert_eq!(status, 200, "{reply}");
    let link = reply["data"]["link"].as_str().unwrap();
    let setup = json!({"token": link.split_once('#').unwrap().1, "password": PASSWORD});
    assert_eq!(
        pair.server
            .call("POST", "/api/v1/setup", None, Some(&setup))
            .0,
        200
    );
    let boss_token = log_in(&pair.server, "boss_x");
    let boss_id = ulid_of(&reply["data"]["account"]);
    let keys = keryx_core::seed::derive(SARA_WORDS).unwrap();
    let body = json!({
        "master_public": Bytes(keys.master_public().to_vec()),
        "account_public": Bytes(keys.account_public().to_vec()),
        "account_stamp": Bytes(keryx_core::mls::stamp_account_key(&keys, &boss_id).to_vec()),
        "invite_public": Bytes(keys.invite_public().to_vec()),
        "invite_stamp": Bytes(keryx_core::mls::stamp_invite_key(&keys, &boss_id).to_vec()),
    });
    assert_eq!(
        pair.server
            .call(
                "PUT",
                "/api/v1/account/keys",
                Some(&boss_token),
                Some(&body)
            )
            .0,
        200
    );
    add_device(&pair.server, boss_id, &keys, &boss_token);
    let mut boss_socket = connect(&pair.server, &boss_token);
    let mut frame = sealed(&pair.sara, chat, Kind::Content, None, b"x");
    frame.reference = reference();
    assert_refused(post(&mut boss_socket, frame), "not_member");
    assert_refused(
        {
            let wanted = reference();
            boss_socket.request(
                &ClientFrame::Sync {
                    reference: wanted,
                    since: None,
                    after: None,
                    chat: Some(chat),
                },
                wanted,
            )
        },
        "not_member",
    );

    // Meanwhile the members carry on.
    let frame = sealed(&pair.sara, chat, Kind::Content, None, b"still here");
    seq_of(post(&mut pair.sara_socket, frame));
}

#[test]
fn body_limits_go_by_content_kind_and_an_edit_by_its_target() {
    let server = Server::start(
        "",
        "[registration]\nallow = true\n[limits.body]\ntext = \"100B\"\nreaction = \"40B\"\n",
    );
    let mut pair = pair_on(server);
    let chat = pair.chat;
    let large = sealed(&pair.sara, chat, Kind::Content, None, &[b'x'; 200]);
    assert_refused(post(&mut pair.sara_socket, large), "body_too_large");
    let fits = sealed(&pair.sara, chat, Kind::Content, None, &[b'x'; 50]);
    let seq = seq_of(post(&mut pair.sara_socket, fits));
    let reaction = sealed_as(
        &pair.sara,
        chat,
        Kind::Content,
        Some(ContentKind::Reaction),
        1,
        None,
        &[b'x'; 50],
    );
    assert_refused(post(&mut pair.sara_socket, reaction), "body_too_large");
    let large_edit = sealed(&pair.sara, chat, Kind::Edit, Some(seq), &[b'x'; 200]);
    assert_refused(post(&mut pair.sara_socket, large_edit), "body_too_large");
    let edit = sealed(&pair.sara, chat, Kind::Edit, Some(seq), &[b'x'; 60]);
    seq_of(post(&mut pair.sara_socket, edit));
}

#[test]
fn seen_and_received_keep_the_highest_and_tell_the_sender_and_typing_is_not_saved() {
    let mut pair = pair();
    let chat = pair.chat;
    let frame = sealed(&pair.sara, chat, Kind::Content, None, b"x");
    let seq = seq_of(post(&mut pair.sara_socket, frame));
    let seen = |socket: &mut Socket, up_to_seq| {
        let wanted = reference();
        socket.request(
            &ClientFrame::Seen {
                reference: wanted,
                chat,
                up_to_seq,
                client_seen_at: Ulid::now(),
                topic: None,
                thread_seq: None,
            },
            wanted,
        )
    };
    let received = |socket: &mut Socket, up_to_seq| {
        let wanted = reference();
        socket.request(
            &ClientFrame::Received {
                reference: wanted,
                chat,
                up_to_seq,
                client_received_at: Ulid::now(),
                topic: None,
                thread_seq: None,
            },
            wanted,
        )
    };
    let marks = |socket: &mut Socket| match socket
        .next_where(|frame| matches!(frame, ServerFrame::Marks { .. }))
    {
        ServerFrame::Marks { chat: marked, mark } => {
            assert_eq!(marked, chat);
            mark
        }
        _ => unreachable!(),
    };

    done(received(&mut pair.babak_socket, seq));
    let mark = marks(&mut pair.sara_socket);
    assert_eq!(mark.account, pair.babak.id);
    assert_eq!((mark.received_seq, mark.seen_seq), (seq, Some(0)));
    done(received(&mut pair.babak_socket, 1));
    assert_refused(received(&mut pair.babak_socket, seq + 1), "bad_received");

    done(seen(&mut pair.babak_socket, seq));
    let mark = marks(&mut pair.sara_socket);
    assert_eq!((mark.received_seq, mark.seen_seq), (seq, Some(seq)));
    done(seen(&mut pair.babak_socket, 1));
    done(seen(&mut pair.babak_socket, seq));
    assert_refused(seen(&mut pair.babak_socket, seq + 1), "bad_seen");

    let item = chat_item(&mut pair.sara_socket, chat);
    assert_eq!(item.last_seq, Some(seq));
    assert_eq!(item.seen_seq, Some(0));
    assert_eq!(item.last_content.unwrap().seq, seq);
    assert_eq!(item.mark_list.len(), 1);
    assert_eq!(item.mark_list[0].account, pair.babak.id);
    assert_eq!(item.mark_list[0].received_seq, seq);
    assert_eq!(item.mark_list[0].seen_seq, Some(seq));
    let item = chat_item(&mut pair.babak_socket, chat);
    assert_eq!(item.seen_seq, Some(seq));
    assert_eq!(item.mark_list[0].account, pair.sara.id);
    assert_eq!(item.mark_list[0].received_seq, 0);

    pair.sara_socket.send(&ClientFrame::Typing {
        chat,
        on: true,
        topic: None,
        thread_seq: None,
    });
    let typing = pair
        .babak_socket
        .next_where(|frame| matches!(frame, ServerFrame::Typing { .. }));
    let ServerFrame::Typing { account, on, .. } = typing else {
        unreachable!();
    };
    assert_eq!(account, pair.sara.id);
    assert!(on);
    assert_eq!(chat_messages(&mut pair.sara_socket, chat, seq, 10).len(), 0);
}

#[test]
fn sync_pages_follow_the_cursor_and_step_back_a_second() {
    let mut pair = pair_on(Server::start(
        "",
        "[registration]\nallow = true\n[limits]\nhistory_page = 2\n",
    ));
    let chat = pair.chat;
    let mut seq_list = Vec::new();
    for text in [b"a", b"b", b"c", b"d", b"e"] {
        let frame = sealed(&pair.sara, chat, Kind::Content, None, text);
        seq_list.push(seq_of(post(&mut pair.sara_socket, frame)));
    }
    let page = |socket: &mut Socket, since, after| {
        let wanted = reference();
        let answer = socket.request(
            &ClientFrame::Sync {
                reference: wanted,
                since,
                after,
                chat: Some(chat),
            },
            wanted,
        );
        let ServerFrame::Chats {
            chat_list,
            message_list,
            call_list,
            more,
            ..
        } = answer
        else {
            panic!("Wanted a sync page, and got {answer:?}");
        };
        assert!(call_list.is_empty());
        (chat_list.len(), message_list, more)
    };
    let mut found = Vec::new();
    let mut after = None;
    let mut pages = 0;
    loop {
        let (listed, message_list, more) = page(&mut pair.babak_socket, None, after);
        assert!(message_list.len() <= 2);
        assert_eq!(
            listed > 0,
            after.is_none(),
            "Only a first page lists the chats"
        );
        pages += 1;
        for message in &message_list {
            after = after.max(Some(SyncPosition {
                at: message.server_saved_at.unwrap(),
                chat,
                seq: message.seq,
                n: 0,
            }));
            found.push(message.seq);
        }
        if !more {
            break;
        }
    }
    assert!(pages >= 3);
    let mut sorted = found.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), found.len(), "No row comes twice");
    for seq in &seq_list {
        assert!(found.contains(seq));
    }
    let (_, message_list, more) = page(&mut pair.babak_socket, None, after);
    assert!(message_list.is_empty());
    assert!(!more);
    // A `since` just under a second after the newest row still finds it, from the step back.
    let newest = after.unwrap().at;
    let mut since = Some(newest + 999);
    let mut after = None;
    let mut found = Vec::new();
    loop {
        let (_, message_list, more) = page(&mut pair.babak_socket, since, after);
        for message in &message_list {
            let at = message.server_saved_at.unwrap();
            assert!(at >= newest - 1);
            after = after.max(Some(SyncPosition {
                at,
                chat,
                seq: message.seq,
                n: 0,
            }));
            found.push(message.seq);
        }
        since = None;
        if !more {
            break;
        }
    }
    assert!(found.contains(seq_list.last().unwrap()));
}

#[test]
fn an_ended_device_is_cut_off_and_everyone_sharing_a_chat_is_told() {
    let server = open_server();
    let (mut sara, babak, chat) = apps_on(&server);
    let babak = babak.into_person(BABAK_WORDS);
    let id = babak.device.record().device;
    let mut babak_socket = connect(&server, &babak.token);

    // Babak terminates it from a password-only session, with his words.
    let mut other = App::new();
    other.command(
        &server,
        Command::LogIn {
            username: "babak_x".to_string(),
            password: Secret(PASSWORD.to_string()),
            captcha: None,
        },
    );
    other.wait(&server, |event| matches!(event, Event::NeedWords { .. }));
    other.command(
        &server,
        Command::Terminate {
            device: id,
            words: Secret(BABAK_WORDS.to_string()),
        },
    );
    other.wait(
        &server,
        |event| matches!(event, Event::Terminated { device } if *device == id),
    );
    assert_eq!(babak_socket.closed(), (4403, "device_ended".to_string()));

    // Sara's client checks the statement and commits the removal by itself.
    let before = sara.client.device().unwrap().epoch(chat).unwrap();
    let started = std::time::Instant::now();
    loop {
        sara.read_once(&server);
        let device = sara.client.device().unwrap();
        let mut still_in = false;
        for member in device.member_list(chat).unwrap() {
            if member.device == id {
                still_in = true;
            }
        }
        if !still_in {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "the device was never removed"
        );
    }
    assert_eq!(
        sara.client.device().unwrap().epoch(chat).unwrap(),
        before + 1
    );

    // A client that was offline reads it from the chat's device list.
    let (_, listed) = server.call(
        "GET",
        &format!("/api/v1/chats/{chat}/devices"),
        sara.token.as_deref(),
        None,
    );
    let mut with_statement = 0;
    for device in listed["data"]["device_list"].as_array().unwrap() {
        if device["device"] == json!(id) {
            assert!(device["statement"].is_string());
            with_statement += 1;
        }
    }
    assert_eq!(with_statement, 1);

    // It cannot come back, by login or by Welcome.
    let (_, asked) = server.call(
        "POST",
        "/api/v1/challenge",
        None,
        Some(&json!({"username": "babak_x", "purpose": "device_login"})),
    );
    let challenge: Bytes = serde_json::from_value(asked["data"]["challenge"].clone()).unwrap();
    let fixed: [u8; 32] = challenge.0.clone().try_into().unwrap();
    let signature = babak.device.sign_device_login(&fixed).unwrap();
    let (status, reply) = server.call(
        "POST",
        "/api/v1/login/token",
        None,
        Some(&json!({
            "username": "babak_x",
            "password": PASSWORD,
            "device": {"id": id, "challenge": challenge, "signature": Bytes(signature.to_vec())},
        })),
    );
    assert_eq!(status, 403, "{reply}");
    assert_eq!(reply["code"], "device_ended");
    let sara_person = sara.into_person(SARA_WORDS);
    let mut sara_socket = connect(&server, &sara_person.token);
    let mut welcome = sealed(&sara_person, chat, Kind::Content, None, b"");
    welcome.reference = reference();
    welcome.kind = Kind::Commit;
    welcome.content_kind = None;
    welcome.epoch = Some(99);
    welcome.group_info = Some(Bytes(vec![1]));
    welcome.welcome_list = vec![WelcomeItem {
        device: id,
        body: Bytes(vec![1]),
    }];
    assert_refused(post(&mut sara_socket, welcome), "no_such_device");
}

#[test]
fn logging_out_is_signed_by_the_device_and_a_returning_one_proves_itself() {
    let pair = pair();
    let server = &pair.server;
    let id = pair.sara.device.record().device;

    // Returning: a password alone is no device, and a signed challenge is.
    let ask = || {
        let (_, asked) = server.call(
            "POST",
            "/api/v1/challenge",
            None,
            Some(&json!({"username": "sara_x", "purpose": "device_login"})),
        );
        let challenge: Bytes = serde_json::from_value(asked["data"]["challenge"].clone()).unwrap();
        challenge
    };
    let login = |challenge: &Bytes, signature: Vec<u8>, device: Ulid| {
        server.call(
            "POST",
            "/api/v1/login/token",
            None,
            Some(&json!({
                "username": "sara_x",
                "password": PASSWORD,
                "device": {"id": device, "challenge": challenge, "signature": Bytes(signature)},
            })),
        )
    };
    let challenge = ask();
    let fixed: [u8; 32] = challenge.0.clone().try_into().unwrap();
    let signature = pair.sara.device.sign_device_login(&fixed).unwrap();
    let (status, reply) = login(&challenge, signature.to_vec(), id);
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["data"]["device"], json!(id));
    let returning = reply["data"]["token"].as_str().unwrap().to_string();
    assert_eq!(
        server
            .call("GET", "/api/v1/devices", Some(&returning), None)
            .1["data"]["device_list"][0]["os"],
        "linux"
    );

    // The challenge worked once, another's device is refused, and a bad signature too.
    let (status, reply) = login(&challenge, signature.to_vec(), id);
    assert_eq!(status, 403);
    assert_eq!(reply["code"], "bad_device_proof");
    let other = pair.babak.device.record().device;
    let challenge = ask();
    let (status, reply) = login(&challenge, signature.to_vec(), other);
    assert_eq!(
        (status, reply["code"].as_str().unwrap()),
        (403, "bad_device_proof")
    );
    let (status, reply) = login(&challenge, signature.to_vec(), id);
    assert_eq!(
        (status, reply["code"].as_str().unwrap()),
        (403, "bad_device_proof")
    );
    let (status, reply) = login(&challenge, signature.to_vec(), Ulid::new().unwrap());
    assert_eq!(
        (status, reply["code"].as_str().unwrap()),
        (403, "bad_device_proof")
    );

    // Logging out needs its own key's signature, on its own account's device.
    let route = format!("/api/v1/devices/{id}/logout");
    let (status, reply) = server.call(
        "POST",
        &route,
        Some(&returning),
        Some(&json!({"signature": Bytes(vec![0; 64])})),
    );
    assert_eq!(
        (status, reply["code"].as_str().unwrap()),
        (403, "bad_signature")
    );
    let signed = pair.sara.device.sign_logout().unwrap();
    let body = json!({"signature": Bytes(signed.to_vec())});
    let (status, reply) = server.call(
        "POST",
        &format!("/api/v1/devices/{other}/logout"),
        Some(&returning),
        Some(&body),
    );
    assert_eq!(
        (status, reply["code"].as_str().unwrap()),
        (404, "no_such_device")
    );
    let (status, reply) = server.call(
        "POST",
        &format!("/api/v1/devices/{other}/terminate"),
        Some(&returning),
        Some(&body),
    );
    assert_eq!(
        (status, reply["code"].as_str().unwrap()),
        (404, "no_such_device")
    );
    let (status, reply) = server.call("POST", &route, Some(&returning), Some(&body));
    assert_eq!(status, 200, "{reply}");
    let (status, _) = server.call("GET", "/api/v1/sessions", Some(&returning), None);
    assert_eq!(status, 401);
}

#[test]
fn websocket_refusals_close_with_4403_and_the_code() {
    let server = Server::start("max_devices = 1", "[registration]\nallow = true\n");
    let sara = sign_up(&server, "sara_x", SARA_WORDS);

    let (no_login, _) = tungstenite::connect(request(&server, None, true)).unwrap();
    assert_eq!(
        with_timeout(no_login).closed(),
        (4403, "not_logged_in".to_string())
    );

    let (no_version, _) = tungstenite::connect(request(&server, Some(&sara.token), false)).unwrap();
    assert_eq!(
        with_timeout(no_version).closed(),
        (4403, "version_mismatch".to_string())
    );

    let password_only = log_in(&server, "sara_x");
    let (no_device, _) =
        tungstenite::connect(request(&server, Some(&password_only), true)).unwrap();
    assert_eq!(
        with_timeout(no_device).closed(),
        (4403, "no_device".to_string())
    );

    let (operator, _) = tungstenite::connect(request(&server, Some(OPERATOR_TOKEN), true)).unwrap();
    assert_eq!(
        with_timeout(operator).closed(),
        (4403, "needs_account".to_string())
    );

    // Another site's page gets no upgrade at all, and our own origin does.
    let foreign =
        request(&server, Some(&sara.token), true).with_header("Origin", "https://evil.example");
    assert!(tungstenite::connect(foreign).is_err());
    let own =
        request(&server, Some(&sara.token), true).with_header("Origin", server.origin.clone());
    let (own, _) = tungstenite::connect(own).unwrap();
    drop(own);
}

#[test]
fn bad_frames_are_answered_when_they_can_be_and_a_new_connection_replaces_the_old() {
    let pair = pair();
    let token = pair.sara.token.clone();
    let mut socket = connect(&pair.server, &token);
    let wanted = reference();
    socket
        .inner
        .send(Message::text(format!(
            r#"{{"type": "nope", "ref": "{wanted}"}}"#
        )))
        .unwrap();
    assert_refused(socket.answer(wanted), "bad_frame");
    socket.inner.send(Message::text("not json")).unwrap();
    assert_eq!(socket.closed(), (4403, "bad_frame".to_string()));

    let mut binary = connect(&pair.server, &token);
    binary.inner.send(Message::binary(vec![1, 2, 3])).unwrap();
    assert_eq!(binary.closed(), (4403, "bad_frame".to_string()));

    let mut first = connect(&pair.server, &token);
    let mut second = connect(&pair.server, &token);
    let (code, _) = first.closed();
    assert_eq!(code, u16::from(CloseCode::Abnormal));
    assert_eq!(chat_item(&mut second, pair.chat).chat, pair.chat);
}

#[test]
fn a_chat_task_restarts_after_going_idle() {
    let server = Server::start(
        "",
        "[registration]\nallow = true\n[chat]\nidle_after = \"1s\"\n",
    );
    let mut pair = pair_on(server);
    let chat = pair.chat;
    let frame = sealed(&pair.sara, chat, Kind::Content, None, b"one");
    let first = seq_of(post(&mut pair.sara_socket, frame));
    std::thread::sleep(Duration::from_millis(1500));
    let frame = sealed(&pair.sara, chat, Kind::Content, None, b"two");
    assert_eq!(seq_of(post(&mut pair.sara_socket, frame)), first + 1);
}

#[test]
fn a_silent_connection_is_closed_and_the_log_keeps_one_row_across_quick_reconnects() {
    let server = Server::start(
        "",
        "[registration]\nallow = true\n[limits]\nping_after = \"1s\"\nping_every = \"1s\"\n\
         ping_timeout = \"1s\"\n",
    );
    let sara = sign_up(&server, "sara_x", SARA_WORDS);
    let socket = connect(&server, &sara.token);
    // This socket is never read, so it never answers a ping.
    let started = std::time::Instant::now();
    let mut gone = false;
    while started.elapsed() < Duration::from_secs(15) {
        let (_, listed) = server.call("GET", "/api/v1/devices", Some(&sara.token), None);
        let log_list = listed["data"]["device_list"][0]["log_list"]
            .as_array()
            .unwrap();
        if log_list.len() == 1 && log_list[0].get("offline_at").is_some() {
            gone = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let silent_for = started.elapsed();
    drop(socket);
    assert!(gone, "the silent connection stayed open");
    // One second to the ping, and one to answer it.
    assert!(
        silent_for < Duration::from_millis(4500),
        "it closed {silent_for:?} after going silent"
    );

    let again = connect(&server, &sara.token);
    let (_, listed) = server.call("GET", "/api/v1/devices", Some(&sara.token), None);
    let log_list = listed["data"]["device_list"][0]["log_list"]
        .as_array()
        .unwrap();
    assert_eq!(log_list.len(), 1, "a quick reconnect extends the same row");
    assert!(log_list[0].get("offline_at").is_none());
    drop(again);
}

#[test]
fn a_shutdown_closes_sockets_with_1001() {
    let mut pair = pair();
    let chat = pair.chat;
    let frame = sealed(&pair.sara, chat, Kind::Content, None, b"just before");
    seq_of(post(&mut pair.sara_socket, frame));
    let started = std::time::Instant::now();
    assert!(pair.server.stop().success());
    assert!(started.elapsed() < Duration::from_secs(8));
    assert_eq!(
        pair.sara_socket.closed(),
        (1001, "shutting_down".to_string())
    );
    let log = std::fs::read_to_string(pair.server.path("server.log")).unwrap();
    assert!(log.contains("\"op\":\"send\""));
    assert!(!log.contains(PASSWORD));
    assert!(!log.contains(&pair.sara.token));
}

#[test]
fn a_welcome_waits_for_an_offline_device() {
    let server = open_server();
    let mut sara = App::sign_up(&server, "sara_x", SARA_WORDS);
    let babak = App::sign_up(&server, "babak_x", BABAK_WORDS);
    let babak_id = babak.id();
    let slots = babak.slots.clone();
    drop(babak);

    // Babak is offline while Sara starts the chat.
    sara.command(&server, Command::StartChat { account: babak_id });
    let self_chat = sara.self_chat;
    let Event::ChatReady { chat } = sara.wait(
        &server,
        |event| matches!(event, Event::ChatReady { chat } if Some(*chat) != self_chat),
    ) else {
        unreachable!();
    };

    // He opens the app again: it loads his saved state, he logs in, and the Welcome is waiting.
    let mut back = App::new();
    let [first, second] = slots;
    back.feed(
        &server,
        keryx_core::client::Input::Loaded {
            first,
            second,
            resume: true,
            token: None,
        },
    );
    back.wait(&server, |event| matches!(event, Event::NeedLogin));
    back.command(
        &server,
        Command::LogIn {
            username: "babak_x".to_string(),
            password: Secret(PASSWORD.to_string()),
            captcha: None,
        },
    );
    back.wait(
        &server,
        |event| matches!(event, Event::ChatReady { chat: ready } if *ready == chat),
    );
    sara.command(
        &server,
        Command::SendText {
            mentions: Vec::new(),
            thread: None,
            topic: None,
            chat,
            text: Secret("you are back".to_string()),
            reply: None,
        },
    );
    let Event::Sent { seq, .. } = sara.wait(&server, |event| matches!(event, Event::Sent { .. }))
    else {
        unreachable!();
    };
    let message = shown(back.wait(&server, |event| is_message(event, chat, seq)));
    assert_eq!(text_of(&message), Some("you are back"));
}

#[test]
fn marks_reach_the_sender_and_typing_reaches_the_other() {
    let server = open_server();
    let (mut sara, mut babak, chat) = apps_on(&server);
    sara.command(
        &server,
        Command::SendText {
            mentions: Vec::new(),
            thread: None,
            topic: None,
            chat,
            text: Secret("x".to_string()),
            reply: None,
        },
    );
    let Event::Sent { seq, .. } = sara.wait(&server, |event| matches!(event, Event::Sent { .. }))
    else {
        unreachable!();
    };

    // Holding it sends `received` by itself, and the sender's client hears of it.
    babak.wait(&server, |event| is_message(event, chat, seq));
    let Event::Marks { mark, .. } = sara.wait(
        &server,
        |event| matches!(event, Event::Marks { mark, .. } if mark.received_seq >= seq),
    ) else {
        unreachable!();
    };
    assert_eq!(mark.account, babak.id());
    assert_eq!(mark.seen_seq, Some(0));

    babak.command(
        &server,
        Command::Seen {
            thread: None,
            topic: None,
            chat,
            up_to_seq: seq,
        },
    );
    let Event::Marks { mark, .. } = sara.wait(
        &server,
        |event| matches!(event, Event::Marks { mark, .. } if mark.seen_seq == Some(seq)),
    ) else {
        unreachable!();
    };
    assert_eq!(mark.received_seq, seq);

    // A reload draws the same marks from the list.
    sara.events.clear();
    sara.command(&server, Command::ChatList);
    let Event::Chats { chat_list } =
        sara.wait(&server, |event| matches!(event, Event::Chats { .. }))
    else {
        unreachable!();
    };
    let mut found = false;
    for item in chat_list {
        if item.chat == chat {
            assert_eq!(item.mark_list[0].account, babak.id());
            assert_eq!(item.mark_list[0].seen_seq, Some(seq));
            found = true;
        }
    }
    assert!(found);

    sara.command(
        &server,
        Command::Typing {
            thread: None,
            topic: None,
            chat,
            on: true,
        },
    );
    let Event::Typing { account, on, .. } =
        babak.wait(&server, |event| matches!(event, Event::Typing { .. }))
    else {
        unreachable!();
    };
    assert_eq!(account, sara.id());
    assert!(on);
}

#[test]
fn a_reply_is_a_message_with_a_target_inside_its_encryption_and_times_show_each_members_clock() {
    let server = open_server();
    let (mut sara, mut babak, chat) = apps_on(&server);
    sara.command(
        &server,
        Command::SendText {
            mentions: Vec::new(),
            thread: None,
            topic: None,
            chat,
            text: Secret("first".to_string()),
            reply: None,
        },
    );
    let Event::Pending { local, .. } =
        sara.wait(&server, |event| matches!(event, Event::Pending { .. }))
    else {
        unreachable!();
    };
    // A reply to a message still waiting for its sequence id waits for it too.
    sara.command(
        &server,
        Command::SendText {
            mentions: Vec::new(),
            thread: None,
            topic: None,
            chat,
            text: Secret("second".to_string()),
            reply: Some(Pointer::Local(local)),
        },
    );
    let Event::Sent { seq: first, .. } =
        sara.wait(&server, |event| matches!(event, Event::Sent { .. }))
    else {
        unreachable!();
    };
    let Event::Sent { seq: second, .. } =
        sara.wait(&server, |event| matches!(event, Event::Sent { .. }))
    else {
        unreachable!();
    };
    assert_eq!(second, first + 1);
    let message = shown(babak.wait(&server, |event| is_message(event, chat, second)));
    assert_eq!(text_of(&message), Some("second"));
    assert_eq!(message.reply, Some(first));
    let plain = shown(babak.wait(&server, |event| is_message(event, chat, first)));
    assert_eq!(plain.reply, None);

    // The server holds no pointer for it: the target is only in the ciphertext.
    let mut socket = connect(&server, sara.token.as_deref().unwrap());
    let (rows, _) = sync_chat(&mut socket, chat);
    for row in rows {
        if row.seq == second {
            assert_eq!(row.pointer_seq, None);
        }
    }

    // A reply to a message already stored carries its sequence id.
    babak.command(
        &server,
        Command::SendText {
            mentions: Vec::new(),
            thread: None,
            topic: None,
            chat,
            text: Secret("third".to_string()),
            reply: Some(Pointer::Seq(first)),
        },
    );
    let Event::Sent { seq: third, .. } =
        babak.wait(&server, |event| matches!(event, Event::Sent { .. }))
    else {
        unreachable!();
    };
    let message = shown(sara.wait(&server, |event| is_message(event, chat, third)));
    assert_eq!(message.reply, Some(first));

    // Times: nobody but the sender's counterpart is listed, and "seen" stays empty until it is.
    sara.command(
        &server,
        Command::Times {
            chat,
            seq: first,
            after: None,
        },
    );
    let Event::Times { member_list, .. } =
        sara.wait(&server, |event| matches!(event, Event::Times { .. }))
    else {
        unreachable!();
    };
    assert_eq!(member_list.len(), 1);
    assert_eq!(member_list[0].account, babak.id());
    assert!(member_list[0].received_at.is_some());
    assert_eq!(member_list[0].seen_at, None);
    babak.command(
        &server,
        Command::Seen {
            thread: None,
            topic: None,
            chat,
            up_to_seq: third,
        },
    );
    sara.wait(
        &server,
        |event| matches!(event, Event::Marks { mark, .. } if mark.seen_seq == Some(third)),
    );
    let (status, reply) = server.call(
        "GET",
        &format!("/api/v1/chats/{chat}/messages/{first}/times"),
        sara.token.as_deref(),
        None,
    );
    assert_eq!(status, 200, "{reply}");
    assert!(reply["data"]["member_list"][0]["seen_at"].is_i64());
    // Someone else's message lists the other members, with the asker included.
    let (_, theirs) = server.call(
        "GET",
        &format!("/api/v1/chats/{chat}/messages/{third}/times"),
        sara.token.as_deref(),
        None,
    );
    assert_eq!(
        theirs["data"]["member_list"][0]["account"],
        json!(sara.id())
    );
    // A message that is not there, and a stranger.
    let (status, reply) = server.call(
        "GET",
        &format!("/api/v1/chats/{chat}/messages/999/times"),
        sara.token.as_deref(),
        None,
    );
    assert_eq!((status, &reply["code"]), (404, &json!("not_found")));
    let stranger = App::sign_up(&server, "mina_x", SARA_WORDS);
    let (status, reply) = server.call(
        "GET",
        &format!("/api/v1/chats/{chat}/messages/{first}/times"),
        stranger.token.as_deref(),
        None,
    );
    assert_eq!((status, &reply["code"]), (404, &json!("not_member")));
}

#[test]
fn a_draft_belongs_to_its_chat_is_kept_sealed_and_comes_back_in_the_list() {
    let server = open_server();
    let (mut sara, mut babak, chat) = apps_on(&server);
    sara.command(
        &server,
        Command::Draft {
            chat,
            draft: Some(Draft {
                text: Secret("half a thought".to_string()),
                reply: Some(1),
                edit: None,
            }),
        },
    );
    // The draft went first on the same socket, so a list asked now includes it.
    sara.command(&server, Command::ChatList);
    let Event::Draft { draft, .. } = sara.wait(
        &server,
        |event| matches!(event, Event::Draft { chat: c, .. } if *c == chat),
    ) else {
        unreachable!();
    };
    let draft = draft.unwrap();
    assert_eq!(draft.text.0, "half a thought");
    assert_eq!(draft.reply, Some(1));
    assert_eq!(draft.edit, None);

    // It is no one else's: the other member's list has none, and nothing was stored as a message.
    babak.command(&server, Command::ChatList);
    babak.wait(&server, |event| matches!(event, Event::Chats { .. }));
    let mut socket = connect(&server, sara.token.as_deref().unwrap());
    let (rows, _) = sync_chat(&mut socket, chat);
    assert!(rows.iter().all(|row| row.content_kind.is_none()));

    // The server holds it sealed, and clearing it leaves none.
    let held = |socket: &mut Socket| {
        let wanted = reference();
        let answer = socket.request(
            &ClientFrame::Sync {
                reference: wanted,
                since: None,
                after: None,
                chat: Some(chat),
            },
            wanted,
        );
        let ServerFrame::Chats { chat_list, .. } = answer else {
            panic!("Wanted a sync page, and got {answer:?}");
        };
        chat_list
            .into_iter()
            .find(|item| item.chat == chat)
            .unwrap()
            .draft
    };
    let sealed = held(&mut socket).unwrap();
    assert!(
        !sealed
            .body
            .0
            .windows(14)
            .any(|part| part == b"half a thought")
    );
    sara.command(&server, Command::Draft { chat, draft: None });
    for _ in 0..5 {
        sara.read_once(&server);
    }
    // A second connection from the same device replaces the app's, so the clear goes by hand.
    let wanted = reference();
    let answer = socket.request(
        &ClientFrame::Draft {
            reference: wanted,
            chat,
            draft: None,
        },
        wanted,
    );
    assert!(matches!(answer, ServerFrame::Done { .. }), "{answer:?}");
    assert!(held(&mut socket).is_none());

    // REST keeps and clears one, refuses a large one, and needs membership.
    let path = format!("/api/v1/chats/{chat}/draft");
    let (status, reply) = server.call(
        "PUT",
        &path,
        sara.token.as_deref(),
        Some(&json!({"draft": {"epoch": 0, "body": "AAAA"}})),
    );
    assert_eq!(status, 200, "{reply}");
    let (status, reply) = server.call("PUT", &path, sara.token.as_deref(), Some(&json!({})));
    assert_eq!(status, 200, "{reply}");
    let large = "A".repeat(100_000);
    let (status, reply) = server.call(
        "PUT",
        &path,
        sara.token.as_deref(),
        Some(&json!({"draft": {"epoch": 0, "body": large}})),
    );
    assert_eq!((status, &reply["code"]), (413, &json!("body_too_large")));
    let stranger = App::sign_up(&server, "mina_x", SARA_WORDS);
    let (status, reply) = server.call("PUT", &path, stranger.token.as_deref(), Some(&json!({})));
    assert_eq!((status, &reply["code"]), (404, &json!("not_member")));
}

#[test]
fn folders_are_saved_whole_checked_against_what_is_kept_and_pushed_to_the_other_devices() {
    let mut pair = pair_on(open_server());
    let chat = pair.chat;
    let own = pair.sara.self_chat;
    let second_token = log_in(&pair.server, &pair.sara.username);
    add_device(&pair.server, pair.sara.id, &pair.sara.keys, &second_token);
    let mut second = connect(&pair.server, &second_token);
    let listed = |socket: &mut Socket| {
        let wanted = reference();
        let answer = socket.request(&sync_frame(wanted), wanted);
        let ServerFrame::Chats {
            chat_folders: Some(folders),
            ..
        } = answer
        else {
            panic!("Wanted the folders, got {answer:?}");
        };
        folders
    };
    let issue = |socket: &mut Socket| {
        let wanted = reference();
        let answer = socket.request(&ClientFrame::FolderId { reference: wanted }, wanted);
        let ServerFrame::FolderId { folder, .. } = answer else {
            panic!("Wanted an id, got {answer:?}");
        };
        folder
    };
    let save = |socket: &mut Socket, folders: &ChatFolders| {
        let wanted = reference();
        socket.request(
            &ClientFrame::ChatFolders {
                reference: wanted,
                folders: folders.clone(),
            },
            wanted,
        )
    };
    let named = |id: Ulid, chat_list: Vec<Ulid>, pin_list: Vec<Ulid>| FolderItem {
        id,
        name: Some(Bytes(b"sealed".to_vec())),
        chat_list: Some(chat_list),
        pin_list,
    };

    // A new account has only `all`, its own and its default.
    let held = listed(&mut pair.sara_socket);
    let all = held.default;
    assert_eq!(
        held.folder_list,
        vec![FolderItem {
            id: all,
            name: None,
            chat_list: None,
            pin_list: Vec::new(),
        }]
    );
    let babak_all = listed(&mut pair.babak_socket).default;
    assert_ne!(babak_all, all);
    let all_with = |pin_list: Vec<Ulid>| FolderItem {
        id: all,
        name: None,
        chat_list: None,
        pin_list,
    };

    // A folder under an issued id, with chats and pins in it and in `all`: kept, listed, and pushed
    // to the account's other device.
    let work = issue(&mut pair.sara_socket);
    let arranged = ChatFolders {
        folder_list: vec![
            all_with(vec![own, chat]),
            named(work, vec![chat, own], vec![chat]),
        ],
        default: work,
    };
    done(save(&mut pair.sara_socket, &arranged));
    let pushed = second.next_where(|frame| matches!(frame, ServerFrame::ChatFolders(_)));
    assert_eq!(pushed, ServerFrame::ChatFolders(Box::new(arranged.clone())));
    assert_eq!(listed(&mut pair.sara_socket), arranged);
    assert_eq!(listed(&mut second), arranged);

    // Broken in any part, refused whole, and nothing changes.
    let stranger = reference();
    let mut case_list = Vec::new();
    let mut broken = arranged.clone();
    broken.folder_list.remove(0);
    case_list.push((broken, "bad_body"));
    let mut broken = arranged.clone();
    broken.folder_list[0].id = stranger;
    case_list.push((broken, "bad_body"));
    let mut broken = arranged.clone();
    broken.folder_list.push(FolderItem {
        id: stranger,
        name: None,
        chat_list: None,
        pin_list: Vec::new(),
    });
    case_list.push((broken, "bad_body"));
    let mut broken = arranged.clone();
    broken.folder_list.push(named(work, Vec::new(), Vec::new()));
    case_list.push((broken, "bad_body"));
    let mut broken = arranged.clone();
    broken.folder_list[1] = named(work, vec![chat, chat], Vec::new());
    case_list.push((broken, "bad_body"));
    let mut broken = arranged.clone();
    broken.folder_list[1] = named(work, vec![chat], vec![own]);
    case_list.push((broken, "bad_body"));
    let mut broken = arranged.clone();
    broken.folder_list[0] = all_with(vec![chat, chat]);
    case_list.push((broken, "bad_body"));
    let mut broken = arranged.clone();
    broken.folder_list[1].chat_list = None;
    case_list.push((broken, "bad_body"));
    let mut broken = arranged.clone();
    broken.default = stranger;
    case_list.push((broken, "bad_body"));
    let mut broken = arranged.clone();
    broken.folder_list[1].name = Some(Bytes(vec![7; 300]));
    case_list.push((broken, "body_too_large"));
    let mut broken = arranged.clone();
    broken
        .folder_list
        .push(named(stranger, Vec::new(), Vec::new()));
    case_list.push((broken, "unknown_folder"));
    let mut broken = arranged.clone();
    broken
        .folder_list
        .push(named(babak_all, Vec::new(), Vec::new()));
    case_list.push((broken, "unknown_folder"));
    for (broken, code) in case_list {
        assert_refused(save(&mut pair.sara_socket, &broken), code);
    }
    assert_eq!(listed(&mut pair.sara_socket), arranged);

    // Chats this account is not in are dropped, not refused.
    let mut with_strangers = arranged.clone();
    with_strangers.folder_list[0] = all_with(vec![own, pair.babak.self_chat, chat]);
    with_strangers.folder_list[1] = named(work, vec![chat, stranger, own], vec![chat, stranger]);
    done(save(&mut pair.sara_socket, &with_strangers));
    assert_eq!(listed(&mut pair.sara_socket), arranged);

    // At most 20 folders besides `all`.
    let mut many = ChatFolders {
        folder_list: vec![all_with(Vec::new()), named(work, Vec::new(), Vec::new())],
        default: all,
    };
    for _ in 0..20 {
        many.folder_list
            .push(named(issue(&mut pair.sara_socket), Vec::new(), Vec::new()));
    }
    assert_refused(save(&mut pair.sara_socket, &many), "too_many_folders");
    many.folder_list.pop();
    done(save(&mut pair.sara_socket, &many));
    assert_eq!(listed(&mut pair.sara_socket), many);

    // Removing a folder, the default too, which makes `all` the default. It never comes back.
    done(save(&mut pair.sara_socket, &arranged));
    let only_all = ChatFolders {
        folder_list: vec![all_with(Vec::new())],
        default: all,
    };
    done(save(&mut pair.sara_socket, &only_all));
    second.next_where(|frame| {
        matches!(frame, ServerFrame::ChatFolders(folders) if folders.folder_list.len() == 1)
    });
    assert_refused(save(&mut pair.sara_socket, &arranged), "unknown_folder");
    assert_eq!(listed(&mut pair.sara_socket), only_all);

    // REST issues ids and saves the same body, and the other device hears of it.
    let (status, reply) = pair.server.call(
        "POST",
        "/api/v1/chat_folders/id",
        Some(&pair.sara.token),
        None,
    );
    assert_eq!(status, 200, "{reply}");
    let home = ulid_of(&reply["data"]["folder"]);
    let by_rest = ChatFolders {
        folder_list: vec![all_with(vec![chat]), named(home, vec![own], Vec::new())],
        default: home,
    };
    let (status, reply) = pair.server.call(
        "PUT",
        "/api/v1/chat_folders",
        Some(&pair.sara.token),
        Some(&serde_json::to_value(&by_rest).unwrap()),
    );
    assert_eq!(status, 200, "{reply}");
    let pushed = second.next_where(
        |frame| matches!(frame, ServerFrame::ChatFolders(folders) if folders.default == home),
    );
    assert_eq!(pushed, ServerFrame::ChatFolders(Box::new(by_rest.clone())));
    let (status, reply) = pair.server.call(
        "PUT",
        "/api/v1/chat_folders",
        Some(&pair.sara.token),
        Some(&json!({"folder_list": [], "default": all})),
    );
    assert_eq!((status, &reply["code"]), (400, &json!("bad_body")));
    assert_eq!(listed(&mut pair.sara_socket), by_rest);
}

/// The newest folders this app was shown, as (id, name, chats, pins), and the default.
type ShownFolders = (
    Vec<(Ulid, Option<String>, Option<Vec<Ulid>>, Vec<Ulid>)>,
    Ulid,
);

/// Waits for the folders an app shows to match, dropping older ones on the way.
fn folders_shown(
    app: &mut App,
    server: &Server,
    wanted: impl Fn(&ShownFolders) -> bool,
) -> ShownFolders {
    loop {
        let Event::Folders {
            folder_list,
            default,
        } = app.wait(server, |event| matches!(event, Event::Folders { .. }))
        else {
            unreachable!();
        };
        let mut shown = Vec::new();
        for folder in folder_list {
            let name = folder.name.as_ref().map(|name| name.0.clone());
            shown.push((folder.id, name, folder.chat_list, folder.pin_list));
        }
        let shown = (shown, default);
        if wanted(&shown) {
            return shown;
        }
    }
}

/// A new folder's id from the server, which saves nothing.
fn new_folder_id(app: &mut App, server: &Server) -> Ulid {
    app.command(server, Command::FolderId);
    let Event::FolderId { folder } =
        app.wait(server, |event| matches!(event, Event::FolderId { .. }))
    else {
        unreachable!();
    };
    folder
}

/// Opens an app's saved state again with no network: it shows what it kept and never connects.
fn reopen_offline(server: &Server, app: App) -> App {
    let [first, second] = app.slots.clone();
    let token = app.token.clone().map(Secret);
    drop(app);
    let mut back = App::new();
    back.slots = [first.clone(), second.clone()];
    back.offline = true;
    back.feed(
        server,
        keryx_core::client::Input::Loaded {
            first,
            second,
            resume: false,
            token,
        },
    );
    back
}

#[test]
fn folders_follow_the_account_with_sealed_names_and_an_offline_change_wins() {
    let server = open_server();
    let (mut sara, _babak, chat) = apps_on(&server);
    let own = sara.self_chat.unwrap();
    let (start, all) = folders_shown(&mut sara, &server, |_| true);
    assert_eq!(start, vec![(all, None, None, Vec::new())]);

    // A second device, from the words alone.
    let mut second = App::new();
    second.command(
        &server,
        Command::LogIn {
            username: "sara_x".to_string(),
            password: Secret(PASSWORD.to_string()),
            captcha: None,
        },
    );
    second.wait(&server, |event| {
        matches!(event, Event::NeedWords { restore: true })
    });
    second.command(
        &server,
        Command::Restore {
            words: Secret(SARA_WORDS.to_string()),
        },
    );
    folders_shown(&mut second, &server, |_| true);

    // A new folder's id saves nothing until an arrangement names it, which shows here at once, and
    // its name opens on the other device.
    let work = new_folder_id(&mut sara, &server);
    let unused = new_folder_id(&mut sara, &server);
    assert_ne!(work, unused);
    sara.command(
        &server,
        Command::Folders {
            folder_list: vec![
                Folder {
                    id: all,
                    name: None,
                    chat_list: None,
                    pin_list: Vec::new(),
                },
                Folder {
                    id: work,
                    name: Some(Secret("Work".to_string())),
                    chat_list: Some(Vec::new()),
                    pin_list: Vec::new(),
                },
            ],
            default: all,
        },
    );
    let (shown, _) = folders_shown(&mut sara, &server, |(list, _)| list.len() == 2);
    assert_eq!(
        shown[1],
        (work, Some("Work".to_string()), Some(Vec::new()), Vec::new())
    );
    let (seen, _) = folders_shown(&mut second, &server, |(list, _)| list.len() == 2);
    assert_eq!(seen, shown);

    // Moving a chat in and pinning it, the other device follows, and the server holds no name.
    let arranged = vec![
        Folder {
            id: all,
            name: None,
            chat_list: None,
            pin_list: vec![own],
        },
        Folder {
            id: work,
            name: Some(Secret("Work".to_string())),
            chat_list: Some(vec![chat]),
            pin_list: vec![chat],
        },
    ];
    sara.command(
        &server,
        Command::Folders {
            folder_list: arranged.clone(),
            default: work,
        },
    );
    let (seen, default) = folders_shown(&mut second, &server, |(_, default)| *default == work);
    assert_eq!(default, work);
    assert_eq!(seen[0], (all, None, None, vec![own]));
    assert_eq!(
        seen[1],
        (work, Some("Work".to_string()), Some(vec![chat]), vec![chat])
    );
    folders_shown(&mut sara, &server, |(_, default)| *default == work);

    // Offline, a change shows at once, and a new folder needs a connection.
    let mut sara = reopen_offline(&server, sara);
    let mut renamed = arranged.clone();
    renamed[1].name = Some(Secret("Home".to_string()));
    sara.command(
        &server,
        Command::Folders {
            folder_list: renamed.clone(),
            default: all,
        },
    );
    folders_shown(&mut sara, &server, |(list, default)| {
        *default == all && list[1].1.as_deref() == Some("Home")
    });
    sara.command(&server, Command::FolderId);
    sara.wait(
        &server,
        |event| matches!(event, Event::Error { code, .. } if code == "offline"),
    );
    // Meanwhile the other device moves the chat out.
    let mut moved = arranged.clone();
    moved[1].chat_list = Some(Vec::new());
    moved[1].pin_list = Vec::new();
    second.command(
        &server,
        Command::Folders {
            folder_list: moved,
            default: work,
        },
    );
    folders_shown(&mut second, &server, |(list, _)| {
        list[1].2 == Some(Vec::new())
    });

    // Back online, the offline change goes first and wins everywhere.
    sara.offline = false;
    sara.command(&server, Command::NetworkBack);
    let wins = |(list, default): &ShownFolders| {
        *default == all && list[1].1.as_deref() == Some("Home") && list[1].2 == Some(vec![chat])
    };
    sara.wait(&server, |event| matches!(event, Event::Online));
    folders_shown(&mut second, &server, wins);

    // An offline change naming a folder another device removed is refused whole, and the server's
    // arrangement shows instead.
    let mut sara = reopen_offline(&server, sara);
    sara.command(
        &server,
        Command::Folders {
            folder_list: renamed.clone(),
            default: work,
        },
    );
    second.command(
        &server,
        Command::Folders {
            folder_list: vec![arranged[0].clone()],
            default: all,
        },
    );
    folders_shown(&mut second, &server, |(list, _)| list.len() == 1);
    sara.offline = false;
    sara.command(&server, Command::NetworkBack);
    sara.wait(
        &server,
        |event| matches!(event, Event::Error { code, .. } if code == "unknown_folder"),
    );
    // It may first show the last arrangement it kept, until the server's list comes.
    let (shown, default) = folders_shown(&mut sara, &server, |(list, _)| {
        list.len() == 1 && !list[0].3.is_empty()
    });
    assert_eq!((shown, default), (vec![(all, None, None, vec![own])], all));
}

#[test]
fn a_chat_opens_on_its_newest_page_and_older_pages_load_upwards() {
    let server = open_server();
    let (mut sara, mut babak, chat) = apps_on(&server);
    let mut last = 0;
    for number in 0..60 {
        sara.command(
            &server,
            Command::SendText {
                mentions: Vec::new(),
                thread: None,
                topic: None,
                chat,
                text: Secret(format!("{number}")),
                reply: None,
            },
        );
        let Event::Sent { seq, .. } =
            sara.wait(&server, |event| matches!(event, Event::Sent { .. }))
        else {
            unreachable!();
        };
        last = seq;
    }
    babak.wait(&server, |event| is_message(event, chat, last));
    babak.events.clear();
    babak.command(&server, Command::OpenChat { chat });
    let Event::PageLoaded { has_older, .. } =
        babak.wait(&server, |event| matches!(event, Event::PageLoaded { .. }))
    else {
        unreachable!();
    };
    assert!(has_older);
    let mut first_page = Vec::new();
    for event in babak.events.drain(..) {
        if let Event::Message { message } = event
            && message.seq + 60 > last
        {
            first_page.push(message.seq);
        }
    }
    assert_eq!(first_page.len(), 50);
    assert_eq!(*first_page.last().unwrap(), last);

    babak.command(&server, Command::LoadOlder { chat });
    let Event::PageLoaded { has_older, .. } =
        babak.wait(&server, |event| matches!(event, Event::PageLoaded { .. }))
    else {
        unreachable!();
    };
    assert!(!has_older);
    let mut older = Vec::new();
    for event in babak.events.drain(..) {
        if let Event::Message { message } = event {
            older.push(text_of(&message).unwrap().to_string());
        }
    }
    assert_eq!(older.len(), 10);
    assert_eq!(older[0], "0");
    babak.command(&server, Command::LoadOlder { chat });
    babak.wait(&server, |event| {
        matches!(
            event,
            Event::PageLoaded {
                has_older: false,
                ..
            }
        )
    });
}

#[test]
fn an_edit_or_delete_can_point_at_a_message_still_waiting() {
    let server = open_server();
    let (mut sara, mut babak, chat) = apps_on(&server);
    sara.command(
        &server,
        Command::SendText {
            mentions: Vec::new(),
            thread: None,
            topic: None,
            chat,
            text: Secret("frist".to_string()),
            reply: None,
        },
    );
    let Event::Pending { local, .. } =
        sara.wait(&server, |event| matches!(event, Event::Pending { .. }))
    else {
        unreachable!();
    };
    sara.command(
        &server,
        Command::Edit {
            chat,
            target: keryx_core::client::Pointer::Local(local),
            text: Secret("first".to_string()),
            files: Vec::new(),
            remove: Vec::new(),
        },
    );
    let Event::Sent { seq, .. } = sara.wait(&server, |event| matches!(event, Event::Sent { .. }))
    else {
        unreachable!();
    };
    let Event::Edited {
        message,
        target_seq,
    } = babak.wait(&server, |event| matches!(event, Event::Edited { .. }))
    else {
        unreachable!();
    };
    assert_eq!(target_seq, seq);
    assert_eq!(text_of(&message), Some("first"));

    sara.command(
        &server,
        Command::Delete {
            chat,
            target: keryx_core::client::Pointer::Seq(seq),
            remove: Vec::new(),
        },
    );
    let Event::Deleted { target_seq, .. } =
        babak.wait(&server, |event| matches!(event, Event::Deleted { .. }))
    else {
        unreachable!();
    };
    assert_eq!(target_seq, seq);
}

#[test]
fn a_device_comes_back_from_its_saved_state_and_logging_out_wipes_it() {
    let server = open_server();
    let (sara, mut babak, chat) = apps_on(&server);
    let sara_id = sara.client.device().unwrap().record().device;
    let slots = sara.slots.clone();
    drop(sara);

    // A reload: the saved state, then the password, and the device proves itself.
    let mut back = App::new();
    let [first, second] = slots;
    back.feed(
        &server,
        keryx_core::client::Input::Loaded {
            first,
            second,
            resume: true,
            token: None,
        },
    );
    back.wait(&server, |event| matches!(event, Event::NeedLogin));
    back.command(
        &server,
        Command::LogIn {
            username: "sara_x".to_string(),
            password: Secret(PASSWORD.to_string()),
            captcha: None,
        },
    );
    let Event::SignedIn { device, .. } =
        back.wait(&server, |event| matches!(event, Event::SignedIn { .. }))
    else {
        unreachable!();
    };
    assert_eq!(device, Some(sara_id));
    back.wait(&server, |event| matches!(event, Event::Online));
    back.command(
        &server,
        Command::SendText {
            mentions: Vec::new(),
            thread: None,
            topic: None,
            chat,
            text: Secret("still me".to_string()),
            reply: None,
        },
    );
    let Event::Sent { seq, .. } = back.wait(&server, |event| matches!(event, Event::Sent { .. }))
    else {
        unreachable!();
    };
    let message = shown(babak.wait(&server, |event| is_message(event, chat, seq)));
    assert_eq!(text_of(&message), Some("still me"));

    // Logging out is confirmed by the server first, then the saved state goes.
    back.command(&server, Command::LogOut);
    back.wait(&server, |event| matches!(event, Event::LoggedOut));
    assert_eq!(back.slots, [None, None]);
    assert!(back.client.device().is_none());
}

#[test]
fn a_device_shows_its_chats_with_no_network() {
    let server = open_server();
    let (sara, mut babak, chat) = apps_on(&server);
    babak.command(
        &server,
        Command::SendText {
            mentions: Vec::new(),
            thread: None,
            topic: None,
            chat,
            text: Secret("kept for later".to_string()),
            reply: None,
        },
    );
    let Event::Sent { .. } = babak.wait(&server, |event| matches!(event, Event::Sent { .. }))
    else {
        unreachable!();
    };
    let [first, second] = sara.slots.clone();
    drop(sara);

    // Back online, the list comes with its preview, and the state is saved with it.
    let mut back = App::new();
    back.feed(
        &server,
        keryx_core::client::Input::Loaded {
            first,
            second,
            resume: true,
            token: None,
        },
    );
    back.wait(&server, |event| matches!(event, Event::NeedLogin));
    back.command(
        &server,
        Command::LogIn {
            username: "sara_x".to_string(),
            password: Secret(PASSWORD.to_string()),
            captcha: None,
        },
    );
    back.wait(&server, |event| {
        matches!(event, Event::Message { message } if message.chat == chat
            && text_of(message) == Some("kept for later"))
    });
    back.wait(&server, |event| matches!(event, Event::Online));
    let [first, second] = back.slots.clone();
    let token = back.token.clone().map(Secret);
    drop(back);

    // Opened again with the server out of reach, it still shows who it is, the list and the preview.
    let mut offline = App::new();
    offline.feed(
        &server,
        keryx_core::client::Input::Loaded {
            first,
            second,
            resume: false,
            token,
        },
    );
    let events: Vec<&Event> = offline.events.iter().collect();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::SignedIn { username, .. } if username == "sara_x"))
    );
    assert!(events.iter().any(
        |event| matches!(event, Event::Chats { chat_list } if chat_list.iter().any(|item| item.chat == chat))
    ));
    assert!(
        events.iter().any(
            |event| matches!(event, Event::Account { account } if account.username == "babak_x")
        ),
        "the other person's name is shown too"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::Message { message }
        if message.chat == chat && text_of(message) == Some("kept for later")))
    );
}

type TlsStream = rustls::StreamOwned<rustls::ClientConnection, TcpStream>;

/// A client that does check the certificate, trusting only what it is given.
fn tls_stream(server: &Server, trust: bool, name: &'static str) -> TlsStream {
    let mut roots = rustls::RootCertStore::empty();
    if trust {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/tls/cert.pem");
        for certificate in rustls::pki_types::CertificateDer::pem_file_iter(path).unwrap() {
            roots.add(certificate.unwrap()).unwrap();
        }
    }
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(roots)
    .with_no_client_auth();
    let connection =
        rustls::ClientConnection::new(Arc::new(config), name.try_into().unwrap()).unwrap();
    let address = server.origin.strip_prefix("https://").unwrap();
    let stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    rustls::StreamOwned::new(connection, stream)
}

#[test]
fn the_websocket_works_over_tls_and_the_certificate_is_checked() {
    // Set up by hand over REST: this is about the transport, and the test client speaks plain
    // WebSocket only.
    let server = Server::start("", "[registration]\nallow = true\n# tls\n");
    let (status, reply) = server.call(
        "POST",
        "/api/v1/register",
        None,
        Some(&json!({"username": "sara_x", "password": PASSWORD})),
    );
    assert_eq!(status, 200, "{reply}");
    let id = ulid_of(&reply["data"]["account"]);
    let token = log_in(&server, "sara_x");
    let keys = keryx_core::seed::derive(SARA_WORDS).unwrap();
    let body = json!({
        "master_public": Bytes(keys.master_public().to_vec()),
        "account_public": Bytes(keys.account_public().to_vec()),
        "account_stamp": Bytes(keryx_core::mls::stamp_account_key(&keys, &id).to_vec()),
        "invite_public": Bytes(keys.invite_public().to_vec()),
        "invite_stamp": Bytes(keryx_core::mls::stamp_invite_key(&keys, &id).to_vec()),
    });
    let (_, reply) = server.call("PUT", "/api/v1/account/keys", Some(&token), Some(&body));
    let self_chat = ulid_of(&reply["data"]["self_chat"]);
    add_device(&server, id, &keys, &token);
    let url = format!("{}/api/v1/ws", server.origin.replace("https://", "wss://"));
    let request = || {
        ClientRequestBuilder::new(url.parse().unwrap())
            .with_header("Authorization", format!("Bearer {token}"))
            .with_sub_protocol("keryx.v1")
    };

    for name in ["localhost", "127.0.0.1"] {
        let stream = tls_stream(&server, true, name);
        let (mut socket, response) = tungstenite::client::client(request(), stream).unwrap();
        assert_eq!(response.headers()["sec-websocket-protocol"], "keryx.v1");
        let wanted = reference();
        let frame = serde_json::to_string(&sync_frame(wanted)).unwrap();
        socket.send(Message::text(frame)).unwrap();
        loop {
            let Message::Text(text) = socket.read().unwrap() else {
                continue;
            };
            let frame: ServerFrame = serde_json::from_str(text.as_str()).unwrap();
            if let ServerFrame::Chats { chat_list, .. } = frame {
                assert_eq!(chat_list[0].chat, self_chat);
                break;
            }
        }
    }

    // A certificate nobody trusts, or for another name, gets no connection.
    let untrusted = tls_stream(&server, false, "localhost");
    assert!(tungstenite::client::client(request(), untrusted).is_err());
    let other_name = tls_stream(&server, true, "chat.example.com");
    assert!(tungstenite::client::client(request(), other_name).is_err());
}

#[test]
fn a_full_outgoing_buffer_closes_only_that_connection_and_loses_nothing() {
    // Waits this long mean that anything closing sooner was the buffer.
    let server = Server::start(
        "",
        "[registration]\nallow = true\n[tcp]\nsend_timeout = \"300s\"\n[limits]\noutgoing_frames = 4\n\
         ping_after = \"300s\"\n[limits.body]\ntext = \"512KiB\"\n",
    );
    let mut pair = pair_on(server);
    let chat = pair.chat;

    // Sara's second device keeps reading, while Babak's stops. Frames this large fill what the
    // operating system buffers too, after which his buffer on the server has nowhere to drain to.
    let second_token = log_in(&pair.server, &pair.sara.username);
    add_device(&pair.server, pair.sara.id, &pair.sara.keys, &second_token);
    let mut reading = connect(&pair.server, &second_token);
    let text = vec![b'x'; 400 * 1024];
    let mut seq_list = Vec::new();
    let mut filled = false;
    for _ in 0..500 {
        // The chat's task never waits on a connection: this answer comes back all the same.
        let frame = sealed(&pair.sara, chat, Kind::Content, None, &text);
        let seq = seq_of(post(&mut pair.sara_socket, frame));
        seq_list.push(seq);
        assert_eq!(reading.message(chat, seq).seq, seq);
        let log = std::fs::read_to_string(pair.server.path("server.log")).unwrap();
        if log.contains("Closing a connection whose outgoing buffer is full") {
            filled = true;
            break;
        }
    }
    assert!(
        filled,
        "{} messages did not fill the buffer",
        seq_list.len()
    );

    // That connection closes at once, without waiting for any timeout.
    let started = std::time::Instant::now();
    let mut offline = false;
    while started.elapsed() < Duration::from_secs(15) {
        let (_, listed) = pair
            .server
            .call("GET", "/api/v1/devices", Some(&pair.babak.token), None);
        let log_list = listed["data"]["device_list"][0]["log_list"]
            .as_array()
            .unwrap();
        if log_list[log_list.len() - 1].get("offline_at").is_some() {
            offline = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(offline, "the full connection stayed open");
    let (code, _) = pair.babak_socket.closed();
    assert_eq!(code, u16::from(CloseCode::Abnormal));

    // Everyone else carries on, and what is sent while Babak is away is saved like the rest.
    let frame = sealed(&pair.sara, chat, Kind::Content, None, &text);
    let seq = seq_of(post(&mut pair.sara_socket, frame));
    seq_list.push(seq);
    assert_eq!(reading.message(chat, seq).seq, seq);

    // He reconnects and fetches all of it from his last sequence id, and every message opens.
    let mut back = connect(&pair.server, &pair.babak.token);
    let mut after_seq = seq_list[0] - 1;
    let mut fetched = Vec::new();
    loop {
        let page = chat_messages(&mut back, chat, after_seq, 5);
        let Some(last) = page.last() else {
            break;
        };
        after_seq = last.seq;
        for message in &page {
            assert_eq!(
                open_message(&pair.babak.device, &pair.sara.device, message),
                text
            );
            fetched.push(message.seq);
        }
    }
    assert_eq!(fetched, seq_list);

    // He gets new messages again.
    let frame = sealed(&pair.sara, chat, Kind::Content, None, b"welcome back");
    let seq = seq_of(post(&mut pair.sara_socket, frame));
    let received = back.message(chat, seq);
    assert_eq!(
        open_message(&pair.babak.device, &pair.sara.device, &received),
        b"welcome back"
    );
}

impl Socket {
    /// Reads for this long, saving frames for later, and says when each WebSocket ping arrived.
    /// Reading is what answers a ping, as a browser does by itself.
    fn watch_pings(&mut self, how_long: Duration) -> Vec<std::time::Instant> {
        let MaybeTlsStream::Plain(stream) = self.inner.get_ref() else {
            panic!("This needs a plain socket");
        };
        stream
            .set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();
        let started = std::time::Instant::now();
        let mut ping_list = Vec::new();
        while started.elapsed() < how_long {
            match self.inner.read() {
                Ok(Message::Ping(_)) => ping_list.push(std::time::Instant::now()),
                Ok(Message::Text(text)) => {
                    self.waiting
                        .push(serde_json::from_str(text.as_str()).unwrap());
                }
                Ok(_) => {}
                Err(tungstenite::Error::Io(error))
                    if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => panic!("The socket failed: {error}"),
            }
        }
        let MaybeTlsStream::Plain(stream) = self.inner.get_ref() else {
            panic!("This needs a plain socket");
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        ping_list
    }

    fn ping(&mut self) -> Ulid {
        let wanted = reference();
        self.send(&ClientFrame::Ping { reference: wanted });
        wanted
    }
}

fn is_online(server: &Server, token: &str) -> bool {
    let (_, listed) = server.call("GET", "/api/v1/devices", Some(token), None);
    let log_list = listed["data"]["device_list"][0]["log_list"]
        .as_array()
        .unwrap();
    log_list[log_list.len() - 1].get("offline_at").is_none()
}

#[test]
fn the_first_frame_holds_the_features() {
    let server = open_server();
    let sara = sign_up(&server, "sara_x", SARA_WORDS);
    let (inner, _) = tungstenite::connect(request(&server, Some(&sara.token), true)).unwrap();
    assert_eq!(
        with_timeout(inner).read(),
        ServerFrame::Features {
            ping_after: 5,
            ping_every: 15,
            ping_timeout: 5,
        }
    );

    let server = Server::start(
        "",
        "[registration]\nallow = true\n[limits]\nping_after = \"7s\"\nping_every = \"8s\"\n\
         ping_timeout = \"9s\"\n",
    );
    let sara = sign_up(&server, "sara_x", SARA_WORDS);
    let (inner, _) = tungstenite::connect(request(&server, Some(&sara.token), true)).unwrap();
    assert_eq!(
        with_timeout(inner).read(),
        ServerFrame::Features {
            ping_after: 7,
            ping_every: 8,
            ping_timeout: 9,
        }
    );
}

#[test]
fn a_quiet_client_that_answers_is_pinged_on_schedule_and_stays() {
    let server = Server::start(
        "",
        "[registration]\nallow = true\n[limits]\nping_after = \"1s\"\nping_every = \"2s\"\n\
         ping_timeout = \"1s\"\n",
    );
    let sara = sign_up(&server, "sara_x", SARA_WORDS);
    let mut socket = connect(&server, &sara.token);
    // At about 1, 3 and 5 seconds.
    let ping_list = socket.watch_pings(Duration::from_millis(5800));
    assert_eq!(ping_list.len(), 3, "{ping_list:?}");
    for pair in ping_list.windows(2) {
        let gap = pair[1] - pair[0];
        assert!(
            gap > Duration::from_millis(1500) && gap < Duration::from_millis(2500),
            "{gap:?}"
        );
    }
    let wanted = socket.ping();
    assert_eq!(
        socket.answer(wanted),
        ServerFrame::Done { reference: wanted }
    );
    assert!(is_online(&server, &sara.token));
}

#[test]
fn what_a_client_sends_holds_pings_back_but_its_own_pings_do_not() {
    let server = Server::start(
        "",
        "[registration]\nallow = true\n[limits]\nping_after = \"2s\"\nping_every = \"2s\"\n\
         ping_timeout = \"1s\"\n",
    );
    let sara = sign_up(&server, "sara_x", SARA_WORDS);
    let mut socket = connect(&server, &sara.token);
    for _ in 0..6 {
        socket.send(&sync_frame(reference()));
        let ping_list = socket.watch_pings(Duration::from_millis(500));
        assert!(ping_list.is_empty(), "a busy connection was pinged");
    }
    let mut pinged = 0;
    let mut last = reference();
    for _ in 0..6 {
        last = socket.ping();
        pinged += socket.watch_pings(Duration::from_millis(500)).len();
    }
    assert!(pinged > 0, "a client's own pings held the server's back");
    assert_eq!(socket.answer(last), ServerFrame::Done { reference: last });
    assert!(is_online(&server, &sara.token));
}

#[test]
fn a_real_ping_gets_a_real_pong() {
    let server = open_server();
    let sara = sign_up(&server, "sara_x", SARA_WORDS);
    let mut socket = connect(&server, &sara.token);
    socket
        .inner
        .send(Message::Ping(b"are you there".to_vec().into()))
        .unwrap();
    loop {
        if let Message::Pong(payload) = socket.inner.read().unwrap() {
            assert_eq!(payload.as_ref(), b"are you there");
            break;
        }
    }
}

#[test]
fn any_frame_answers_a_ping_even_from_a_client_that_never_reads() {
    let server = Server::start(
        "",
        "[registration]\nallow = true\n[limits]\nping_after = \"1s\"\nping_every = \"1s\"\n\
         ping_timeout = \"1s\"\n",
    );
    let sara = sign_up(&server, "sara_x", SARA_WORDS);
    let mut socket = connect(&server, &sara.token);
    for _ in 0..8 {
        socket.ping();
        std::thread::sleep(Duration::from_millis(500));
    }
    assert!(is_online(&server, &sara.token));
}

#[test]
fn a_send_stuck_past_send_timeout_closes_that_connection() {
    // The buffer is too large to fill, and pings are too far off to matter, so only the timer is left.
    let server = Server::start(
        "",
        "[registration]\nallow = true\n[tcp]\nsend_timeout = \"1s\"\nsend_buffer = \"64KiB\"\n\
         [limits]\noutgoing_frames = 100000\nping_after = \"300s\"\n[limits.body]\ntext = \"512KiB\"\n",
    );
    let mut pair = pair_on(server);
    let chat = pair.chat;
    let text = vec![b'x'; 400 * 1024];
    let mut closed = false;
    for _ in 0..500 {
        let frame = sealed(&pair.sara, chat, Kind::Content, None, &text);
        seq_of(post(&mut pair.sara_socket, frame));
        if !is_online(&pair.server, &pair.babak.token) {
            closed = true;
            break;
        }
    }
    if !closed {
        // The sends have all stuck by now, and the timer needs a moment.
        std::thread::sleep(Duration::from_secs(3));
        closed = !is_online(&pair.server, &pair.babak.token);
    }
    assert!(closed, "the stuck connection stayed open");
    let log = std::fs::read_to_string(pair.server.path("server.log")).unwrap();
    assert!(!log.contains("outgoing buffer is full"));
}

#[test]
fn every_tcp_setting_is_taken() {
    let server = Server::start(
        "",
        "[registration]\nallow = true\n[tcp]\nsend_timeout = \"20s\"\nuser_timeout = \"20s\"\n\
         nodelay = false\nkeepalive_idle = \"30s\"\nkeepalive_interval = \"10s\"\n\
         keepalive_retries = 3\nsend_buffer = \"128KiB\"\nreceive_buffer = \"128KiB\"\nbacklog = 16\n",
    );
    let mut pair = pair_on(server);
    let chat = pair.chat;
    let frame = sealed(&pair.sara, chat, Kind::Content, None, b"over tuned sockets");
    let seq = seq_of(post(&mut pair.sara_socket, frame));
    let received = pair.babak_socket.message(chat, seq);
    assert_eq!(
        open_message(&pair.babak.device, &pair.sara.device, &received),
        b"over tuned sockets"
    );
    let log = std::fs::read_to_string(pair.server.path("server.log")).unwrap();
    assert!(!log.contains("Could not set a TCP option"), "{log}");
}

/// Chunk `n` of a two-chunk file: a full one, then a short one, each with its tag.
fn chunk_bytes(n: u8) -> Vec<u8> {
    if n == 0 {
        vec![7; 1024 * 1024 + 16]
    } else {
        vec![9; 116]
    }
}

fn create_file(pair: &Pair, token: &str, kind: u8, size: u64, chunks: u64) -> (u16, Value) {
    pair.server.call(
        "POST",
        &format!("/api/v1/chats/{}/files", pair.chat),
        Some(token),
        Some(&json!({"kind": kind, "size": size, "chunks": chunks})),
    )
}

fn put_chunk(pair: &Pair, token: &str, file: &str, n: u8, bytes: &[u8]) -> (u16, Vec<u8>) {
    raw(
        &pair.server,
        "PUT",
        &format!("/api/v1/files/{file}/chunks/{n}"),
        token,
        Some(bytes),
    )
}

fn code_of(reply: &[u8]) -> String {
    let value: Value = serde_json::from_slice(reply).unwrap();
    value["code"].as_str().unwrap().to_string()
}

fn with_files(mut frame: SendFrame, persist: &[Ulid], remove: &[Ulid]) -> SendFrame {
    frame.persist = persist.to_vec();
    frame.remove = remove.to_vec();
    frame
}

#[test]
fn files_go_up_in_order_are_kept_by_a_sent_message_and_go_with_its_delete() {
    let mut pair = pair();
    let size = 1024 * 1024 + 16 + 116;
    let sara = pair.sara.token.clone();
    let babak = pair.babak.token.clone();

    // The declared type picks the limit, and the size must fit the chunks.
    assert_eq!(create_file(&pair, &sara, 0, size, 2).1["code"], "bad_body");
    assert_eq!(create_file(&pair, &sara, 3, size, 1).1["code"], "bad_body");
    assert_eq!(create_file(&pair, &sara, 3, 15, 1).1["code"], "bad_body");
    assert_eq!(create_file(&pair, &sara, 3, size, 0).1["code"], "bad_body");
    let huge = 21 * (1024 * 1024 + 16);
    assert_eq!(
        create_file(&pair, &sara, 3, huge, 21).1["code"],
        "file_too_large"
    );
    // Music, an MP3, has its own limit of 200 MiB.
    let album = 201 * (1024 * 1024 + 16);
    assert_eq!(
        create_file(&pair, &sara, 4, album, 201).1["code"],
        "file_too_large"
    );
    let (status, reply) = create_file(&pair, &sara, 3, size, 2);
    assert_eq!(status, 200);
    let file = reply["data"]["file"].as_str().unwrap().to_string();
    let file_id = Ulid::parse(&file).unwrap();

    // Chunks go up in order, at their length, from the uploader's account only.
    let (status, reply) = put_chunk(&pair, &sara, &file, 1, &chunk_bytes(1));
    assert_eq!(
        (status, code_of(&reply).as_str()),
        (409, "chunk_out_of_order")
    );
    let (status, reply) = put_chunk(&pair, &sara, &file, 0, &chunk_bytes(1));
    assert_eq!((status, code_of(&reply).as_str()), (400, "bad_chunk"));
    let (status, reply) = put_chunk(&pair, &babak, &file, 0, &chunk_bytes(0));
    assert_eq!((status, code_of(&reply).as_str()), (403, "not_yours"));
    let (status, reply) = put_chunk(&pair, &sara, &file, 0, &chunk_bytes(0));
    assert_eq!(status, 200);
    let reply: Value = serde_json::from_slice(&reply).unwrap();
    assert_eq!(reply["data"]["received"], 1);
    assert_eq!(put_chunk(&pair, &sara, &file, 0, &chunk_bytes(0)).0, 200);
    let (status, reply) =
        pair.server
            .call("GET", &format!("/api/v1/files/{file}"), Some(&babak), None);
    assert_eq!(status, 200);
    assert_eq!(
        reply["data"],
        json!({"size": size, "chunks": 2, "received": 1})
    );

    // An incomplete file cannot be sent.
    let image = |pair: &Pair| {
        let epoch = pair.sara.device.epoch(pair.chat).unwrap();
        sealed_as(
            &pair.sara,
            pair.chat,
            Kind::Content,
            Some(ContentKind::Image),
            epoch,
            None,
            b"an image",
        )
    };
    let frame = with_files(image(&pair), &[file_id], &[]);
    assert_refused(post(&mut pair.sara_socket, frame), "file_incomplete");
    assert_eq!(put_chunk(&pair, &sara, &file, 1, &chunk_bytes(1)).0, 200);
    let (status, reply) = put_chunk(&pair, &sara, &file, 2, &chunk_bytes(1));
    assert_eq!(
        (status, code_of(&reply).as_str()),
        (409, "chunk_out_of_order")
    );

    // Until a sent message keeps it, only its uploader reads it.
    let route = format!("/api/v1/files/{file}/chunks/0");
    assert_eq!(
        raw(&pair.server, "GET", &route, &sara, None),
        (200, chunk_bytes(0))
    );
    let (status, reply) = raw(&pair.server, "GET", &route, &babak, None);
    assert_eq!((status, code_of(&reply).as_str()), (404, "no_such_file"));

    // Text keeps no file, nor does a message keep another account's, nor the same one twice.
    let text = with_files(
        sealed(&pair.sara, pair.chat, Kind::Content, None, b"hi"),
        &[file_id],
        &[],
    );
    assert_refused(post(&mut pair.sara_socket, text), "bad_message");
    let twice = with_files(image(&pair), &[file_id, file_id], &[]);
    assert_refused(post(&mut pair.sara_socket, twice), "bad_message");
    let stranger = with_files(image(&pair), &[Ulid::new().unwrap()], &[]);
    assert_refused(post(&mut pair.sara_socket, stranger), "no_such_file");
    let epoch = pair.babak.device.epoch(pair.chat).unwrap();
    let theirs = with_files(
        sealed_as(
            &pair.babak,
            pair.chat,
            Kind::Content,
            Some(ContentKind::Image),
            epoch,
            None,
            b"not mine",
        ),
        &[file_id],
        &[],
    );
    assert_refused(post(&mut pair.babak_socket, theirs), "not_yours");
    let removing = with_files(image(&pair), &[], &[file_id]);
    assert_refused(post(&mut pair.sara_socket, removing), "bad_message");

    let keeping = with_files(image(&pair), &[file_id], &[]);
    let kept = seq_of(post(&mut pair.sara_socket, keeping));
    assert_eq!(
        raw(&pair.server, "GET", &route, &babak, None),
        (200, chunk_bytes(0))
    );
    let last = format!("/api/v1/files/{file}/chunks/1");
    assert_eq!(
        raw(&pair.server, "GET", &last, &babak, None),
        (200, chunk_bytes(1))
    );
    let beyond = format!("/api/v1/files/{file}/chunks/2");
    let (status, reply) = raw(&pair.server, "GET", &beyond, &babak, None);
    assert_eq!((status, code_of(&reply).as_str()), (404, "no_such_chunk"));
    let (status, reply) = put_chunk(&pair, &sara, &file, 1, &chunk_bytes(1));
    assert_eq!((status, code_of(&reply).as_str()), (409, "file_sent"));
    let again = with_files(image(&pair), &[file_id], &[]);
    assert_refused(post(&mut pair.sara_socket, again), "file_sent");

    // Its delete removes it, and its bytes leave the disk.
    let on_disk = pair
        .server
        .path("files")
        .join(pair.chat.to_string())
        .join(&file);
    assert!(on_disk.exists());
    let delete = with_files(
        sealed(&pair.sara, pair.chat, Kind::Delete, Some(kept), b""),
        &[],
        &[file_id],
    );
    seq_of(post(&mut pair.sara_socket, delete));
    assert!(!on_disk.exists());
    let (status, reply) = raw(&pair.server, "GET", &route, &babak, None);
    assert_eq!((status, code_of(&reply).as_str()), (404, "no_such_file"));
    let (status, _) = pair
        .server
        .call("GET", &format!("/api/v1/files/{file}"), Some(&sara), None);
    assert_eq!(status, 404);
    let (status, _) = pair.server.call(
        "GET",
        &format!("/api/v1/files/{}", Ulid::new().unwrap()),
        Some(&sara),
        None,
    );
    assert_eq!(status, 404);
    let (status, _) = raw(&pair.server, "GET", "/api/v1/files/x/chunks/y", &sara, None);
    assert_eq!(status, 404);
}

fn spec(handle: u64, kind: ContentKind, size: usize) -> FileSpec {
    FileSpec {
        handle,
        kind,
        name: format!("file-{handle}"),
        mime: "application/octet-stream".to_string(),
        size: size as u64,
        width: None,
        height: None,
        thumbnail: None,
        description: None,
        title: None,
        artist: None,
        duration: None,
    }
}

/// Bytes that differ from chunk to chunk, so a swapped chunk would show.
fn pattern(size: usize, seed: u8) -> Vec<u8> {
    (0..size).map(|at| (at % 251) as u8 ^ seed).collect()
}

#[test]
fn a_file_and_a_bundle_go_up_encrypted_and_come_down_whole() {
    let server = open_server();
    let (mut sara, mut babak, chat) = apps_on(&server);

    // A file of three chunks with a caption: it goes up chunk by chunk, then its message.
    let video = pattern(2 * 1024 * 1024 + 300, 1);
    sara.files.insert(1, video.clone());
    sara.command(
        &server,
        Command::SendFiles {
            thread: None,
            topic: None,
            chat,
            files: vec![spec(1, ContentKind::Video, video.len())],
            text: Some(Secret("watch this".to_string())),
        },
    );
    sara.wait(&server, |event| {
        matches!(
            event,
            Event::Progress {
                handle: 1,
                done: 3,
                total: 3
            }
        )
    });
    let Event::Pending { local, handles, .. } =
        sara.wait(&server, |event| matches!(event, Event::Pending { .. }))
    else {
        unreachable!();
    };
    assert_eq!(handles, vec![1]);
    sara.wait(
        &server,
        |event| matches!(event, Event::Sent { local: sent, .. } if *sent == local),
    );
    let message = shown(babak.wait(&server, |event| {
        matches!(event, Event::Message { message } if message.chat == chat && !message.files.is_empty())
    }));
    assert_eq!(message.content_kind, Some(ContentKind::Video));
    assert_eq!(text_of(&message), Some("watch this"));
    let info = message.files[0].clone();
    assert_eq!((info.size, info.chunks), (video.len() as u64, 3));
    assert_eq!(info.name, "file-1");

    // It comes down chunk by chunk, each opened, into the same bytes.
    babak.command(
        &server,
        Command::Download {
            file: info.clone(),
            handle: 7,
        },
    );
    babak.wait(&server, |event| {
        matches!(event, Event::Downloaded { handle: 7 })
    });
    assert_eq!(babak.files[&7], video);

    // The server only ever held ciphertext.
    let stored = std::fs::read(
        server
            .path("files")
            .join(chat.to_string())
            .join(info.id.to_string()),
    )
    .unwrap();
    assert_eq!(stored.len(), video.len() + 3 * 16);
    assert!(!stored.windows(64).any(|window| window == &video[..64]));

    // A bundle of an image and a video, each with its own description, and the image's BlurHash.
    let image = pattern(5000, 2);
    let clip = pattern(10, 3);
    sara.files.insert(2, image.clone());
    sara.files.insert(3, clip.clone());
    let mut first = spec(2, ContentKind::Image, image.len());
    first.width = Some(640);
    first.height = Some(480);
    first.description = Some("the view".to_string());
    first.thumbnail = Some(Thumbnail {
        width: 4,
        height: 4,
        rgba: pattern(64, 9),
    });
    let mut second = spec(3, ContentKind::Video, clip.len());
    second.description = Some("the sound".to_string());
    sara.command(
        &server,
        Command::SendFiles {
            thread: None,
            topic: None,
            chat,
            files: vec![first, second],
            text: None,
        },
    );
    let bundle = shown(babak.wait(
        &server,
        |event| matches!(event, Event::Message { message } if message.files.len() == 2),
    ));
    assert_eq!(bundle.content_kind, Some(ContentKind::MediaBundle));
    assert_eq!(bundle.text, None);
    assert_eq!(bundle.files[0].description.as_deref(), Some("the view"));
    assert_eq!(bundle.files[1].description.as_deref(), Some("the sound"));
    assert_eq!(bundle.files[0].width, Some(640));
    let hash = bundle.files[0].blurhash.clone().unwrap();
    assert_eq!(file::blurhash_pixels(&hash, 4, 4).unwrap().len(), 64);
    babak.command(
        &server,
        Command::Download {
            file: bundle.files[1].clone(),
            handle: 8,
        },
    );
    babak.wait(&server, |event| {
        matches!(event, Event::Downloaded { handle: 8 })
    });
    assert_eq!(babak.files[&8], clip);

    // Deleting the bundle removes its files with it.
    sara.command(
        &server,
        Command::Delete {
            chat,
            target: Pointer::Seq(bundle.seq),
            remove: bundle.files.iter().map(|info| info.id).collect(),
        },
    );
    babak.wait(
        &server,
        |event| matches!(event, Event::Deleted { target_seq, .. } if *target_seq == bundle.seq),
    );
    babak.command(
        &server,
        Command::Download {
            file: bundle.files[0].clone(),
            handle: 9,
        },
    );
    let failed = babak.wait(&server, |event| {
        matches!(event, Event::TransferFailed { handle: 9, .. })
    });
    assert!(matches!(failed, Event::TransferFailed { code, .. } if code == "no_such_file"));

    // A message holds one file, or up to 20 images and videos.
    sara.command(
        &server,
        Command::SendFiles {
            thread: None,
            topic: None,
            chat,
            files: vec![spec(4, ContentKind::Doc, 1), spec(5, ContentKind::Image, 1)],
            text: None,
        },
    );
    let refused = sara.wait(&server, |event| matches!(event, Event::Error { .. }));
    assert!(matches!(refused, Event::Error { code, .. } if code == "bad_files"));

    // An upload the server refuses tells every file of its message.
    sara.files.insert(6, pattern(21 * 1024 * 1024, 4));
    sara.command(
        &server,
        Command::SendFiles {
            thread: None,
            topic: None,
            chat,
            files: vec![spec(6, ContentKind::Image, 21 * 1024 * 1024)],
            text: None,
        },
    );
    let failed = sara.wait(&server, |event| {
        matches!(event, Event::TransferFailed { handle: 6, .. })
    });
    assert!(matches!(failed, Event::TransferFailed { code, .. } if code == "file_too_large"));
}

/// A call's start from this person, in this chat.
fn call_frame(person: &Person, chat: Ulid) -> SendFrame {
    let epoch = person.device.epoch(chat).unwrap();
    sealed_as(
        person,
        chat,
        Kind::Content,
        Some(ContentKind::Call),
        epoch,
        None,
        br#"{"video":true,"voice":true}"#,
    )
}

fn call_event_frame(chat: Ulid, call_seq: u64, kind: CallEventKind) -> (ClientFrame, Ulid) {
    let wanted = reference();
    let frame = ClientFrame::CallEvent {
        reference: wanted,
        chat,
        call_seq,
        kind,
    };
    (frame, wanted)
}

fn decline(socket: &mut Socket, chat: Ulid, call_seq: u64) -> ServerFrame {
    let (frame, wanted) = call_event_frame(chat, call_seq, CallEventKind::Decline);
    socket.request(&frame, wanted)
}

/// The next call event this socket gets.
fn call_event(socket: &mut Socket) -> CallEvent {
    match socket.next_where(|frame| matches!(frame, ServerFrame::CallEvent(_))) {
        ServerFrame::CallEvent(event) => *event,
        _ => unreachable!(),
    }
}

/// One call's rows, as `sync` gives them, in order of `n`.
fn call_rows(socket: &mut Socket, chat: Ulid, call_seq: u64) -> Vec<CallEvent> {
    let (_, call_list) = sync_chat(socket, chat);
    let mut row_list: Vec<CallEvent> = call_list
        .into_iter()
        .filter(|event| event.call_seq == call_seq)
        .collect();
    row_list.sort_by_key(|event| event.n);
    row_list
}

/// The call's `end` row, once it has ended.
fn call_end(socket: &mut Socket, chat: Ulid, call_seq: u64) -> Option<CallEvent> {
    call_rows(socket, chat, call_seq)
        .into_iter()
        .find(|event| event.kind == CallEventKind::End)
}

#[test]
fn a_chat_has_one_open_call_and_a_decline_ends_it_as_missed() {
    let mut pair = pair();
    let chat = pair.chat;
    let sara_device = pair.sara.device.record().device;
    let babak_device = pair.babak.device.record().device;

    // The call is the preview, and the chat's time stays on the message before it.
    seq_of(post(
        &mut pair.sara_socket,
        sealed(&pair.sara, chat, Kind::Content, None, b"hello"),
    ));
    let before_at = chat_item(&mut pair.sara_socket, chat).last_at;
    let call_seq = seq_of(post(&mut pair.sara_socket, call_frame(&pair.sara, chat)));
    let ring = pair.babak_socket.message(chat, call_seq);
    assert_eq!(ring.content_kind, Some(ContentKind::Call));
    let item = chat_item(&mut pair.sara_socket, chat);
    assert_eq!(item.last_content_seq, Some(call_seq));
    assert_eq!(item.last_at, before_at);
    assert_eq!(
        item.last_content.as_ref().map(|message| message.seq),
        Some(call_seq)
    );

    // Calling back while it rings meets the open call, and the refusal names it.
    match post(&mut pair.babak_socket, call_frame(&pair.babak, chat)) {
        ServerFrame::Refused {
            code,
            call_seq: open,
            ..
        } => {
            assert_eq!(code, "call_open");
            assert_eq!(open, Some(call_seq));
        }
        other => panic!("Wanted call_open, and got {other:?}"),
    }

    // Only the other account declines, only a decline comes over the socket, and only an open call.
    assert_refused(
        decline(&mut pair.sara_socket, chat, call_seq),
        "bad_message",
    );
    let (text, wanted) = call_event_frame(chat, call_seq, CallEventKind::Text);
    assert_refused(pair.babak_socket.request(&text, wanted), "bad_message");
    assert_refused(
        decline(&mut pair.babak_socket, chat, call_seq + 9),
        "call_ended",
    );
    let (stranger, wanted) =
        call_event_frame(Ulid::new().unwrap(), call_seq, CallEventKind::Decline);
    assert_refused(pair.babak_socket.request(&stranger, wanted), "not_member");

    done(decline(&mut pair.babak_socket, chat, call_seq));
    let declined = call_event(&mut pair.sara_socket);
    assert_eq!(declined.kind, CallEventKind::Decline);
    assert_eq!((declined.n, declined.device), (1, babak_device));
    assert_eq!(declined.call_seq, call_seq);
    let ended = call_event(&mut pair.sara_socket);
    assert_eq!(
        (ended.kind, ended.n, ended.device),
        (CallEventKind::End, 2, babak_device)
    );
    // The decliner's own device has its answer, so the end is the first event it gets.
    let ended = call_event(&mut pair.babak_socket);
    assert_eq!((ended.kind, ended.n), (CallEventKind::End, 2));

    let end = call_end(&mut pair.sara_socket, chat, call_seq).unwrap();
    assert_eq!(end.outcome, Some(CallOutcome::Missed));
    assert_refused(
        decline(&mut pair.babak_socket, chat, call_seq),
        "call_ended",
    );

    let row_list = call_rows(&mut pair.babak_socket, chat, call_seq);
    assert_eq!(row_list, vec![declined, ended]);
    assert_ne!(row_list[0].device, sara_device);

    // Once it ended, a new call can start.
    seq_of(post(&mut pair.babak_socket, call_frame(&pair.babak, chat)));
}

#[test]
fn a_call_nobody_answers_ends_as_missed_by_the_caller() {
    let server = Server::start(
        "",
        "[registration]\nallow = true\n[calls]\ntest_ring = \"1s\"\ntest_grace = \"1s\"\n",
    );
    let mut pair = pair_on(server);
    let chat = pair.chat;
    let sara_device = pair.sara.device.record().device;
    let call_seq = seq_of(post(&mut pair.sara_socket, call_frame(&pair.sara, chat)));
    for socket in [&mut pair.sara_socket, &mut pair.babak_socket] {
        let ended = call_event(socket);
        assert_eq!(
            (ended.kind, ended.n, ended.device, ended.call_seq),
            (CallEventKind::End, 1, sara_device, call_seq)
        );
    }
    let end = call_end(&mut pair.babak_socket, chat, call_seq).unwrap();
    assert_eq!(end.outcome, Some(CallOutcome::Missed));
}

#[test]
fn a_call_is_refused_outside_a_direct_chat() {
    let mut pair = pair();
    let self_chat = pair.sara.self_chat;
    assert_refused(
        post(&mut pair.sara_socket, call_frame(&pair.sara, self_chat)),
        "call_not_allowed",
    );
}

#[test]
fn deleting_an_open_call_ends_it_first() {
    let mut pair = pair();
    let chat = pair.chat;
    let call_seq = seq_of(post(&mut pair.sara_socket, call_frame(&pair.sara, chat)));
    pair.babak_socket.message(chat, call_seq);
    let delete_seq = seq_of(post(
        &mut pair.sara_socket,
        sealed(&pair.sara, chat, Kind::Delete, Some(call_seq), b""),
    ));
    let ended = call_event(&mut pair.babak_socket);
    assert_eq!((ended.kind, ended.call_seq), (CallEventKind::End, call_seq));
    let delete = pair.babak_socket.message(chat, delete_seq);
    assert_eq!(delete.pointer_seq, Some(call_seq));
    // The call and what happened inside it are gone.
    assert!(call_rows(&mut pair.babak_socket, chat, call_seq).is_empty());
    seq_of(post(&mut pair.babak_socket, call_frame(&pair.babak, chat)));
}

/// Joins a call's MoQ as this person's device, through keryx-moq, the tests' call client.
fn join_call(server: &Server, person: &Person, chat: Ulid, call_seq: u64) -> keryx_moq::Call {
    let (key, index_list) = person.device.call_key(chat).unwrap();
    let url = format!(
        "{}/api/v1/calls/{chat}/{call_seq}/moq",
        server.origin.replace("http://", "ws://")
    );
    keryx_moq::Call::connect(&url, Some(&person.token), key, index_list).unwrap()
}

/// The next of a call's events that `wanted` takes, skipping the rest, within 10 seconds.
fn on_call(call: &keryx_moq::Call, wanted: impl Fn(&keryx_moq::Event) -> bool) -> keryx_moq::Event {
    let until = std::time::Instant::now() + Duration::from_secs(10);
    while let Some(left) = until.checked_duration_since(std::time::Instant::now()) {
        match call.next(left) {
            Some(event) if wanted(&event) => return event,
            Some(keryx_moq::Event::Closed { code, reason }) => {
                panic!("The call socket closed with {code} {reason}")
            }
            Some(_) => {}
            None => break,
        }
    }
    panic!("Nothing wanted arrived on the call");
}

fn call_closed(call: &keryx_moq::Call) -> (u16, String) {
    let until = std::time::Instant::now() + Duration::from_secs(10);
    while let Some(left) = until.checked_duration_since(std::time::Instant::now()) {
        match call.next(left) {
            Some(keryx_moq::Event::Closed { code, reason }) => return (code, reason),
            Some(_) => {}
            None => break,
        }
    }
    panic!("The call socket did not close");
}

fn moq_event(event: keryx_moq::Event) -> CallEvent {
    match event {
        keryx_moq::Event::CallEvent(event) => event,
        other => panic!("Wanted a call event, and got {other:?}"),
    }
}

fn is_event(kind: CallEventKind, n: u64) -> impl Fn(&keryx_moq::Event) -> bool {
    move |event| matches!(event, keryx_moq::Event::CallEvent(event) if event.kind == kind && event.n == n)
}

#[test]
fn joining_answers_a_call_and_media_and_text_flow_through_the_relay() {
    let mut pair = pair();
    let chat = pair.chat;
    let sara_device = pair.sara.device.record().device;
    let babak_device = pair.babak.device.record().device;
    let call_seq = seq_of(post(&mut pair.sara_socket, call_frame(&pair.sara, chat)));
    pair.babak_socket.message(chat, call_seq);

    let sara_call = join_call(&pair.server, &pair.sara, chat, call_seq);
    let joined = call_event(&mut pair.babak_socket);
    assert_eq!(
        (joined.kind, joined.n, joined.device),
        (CallEventKind::Join, 1, sara_device)
    );
    let babak_call = join_call(&pair.server, &pair.babak, chat, call_seq);
    let joined = call_event(&mut pair.sara_socket);
    assert_eq!(
        (joined.kind, joined.n, joined.device),
        (CallEventKind::Join, 2, babak_device)
    );
    on_call(&sara_call, is_event(CallEventKind::Join, 2));
    // The other account joined, so the call is answered and stops ringing. It is still open.
    assert_eq!(call_end(&mut pair.babak_socket, chat, call_seq), None);
    assert_refused(
        decline(&mut pair.babak_socket, chat, call_seq),
        "call_answered",
    );

    // Each side sends until the other reads, since the other's subscription starts when it sees
    // the broadcast.
    let until = std::time::Instant::now() + Duration::from_secs(10);
    let heard = loop {
        assert!(
            std::time::Instant::now() < until,
            "No audio crossed the relay"
        );
        sara_call.audio(1_000, b"sara speaks").unwrap();
        if let Some(event @ keryx_moq::Event::Audio { .. }) =
            babak_call.next(Duration::from_millis(100))
        {
            break event;
        }
    };
    let keryx_moq::Event::Audio { device, ts, bytes } = heard else {
        unreachable!();
    };
    assert_eq!(
        (device, ts, bytes.as_slice()),
        (sara_device, 1_000, &b"sara speaks"[..])
    );
    let state = keryx_core::call::State {
        video: true,
        voice: false,
        width: Some(640),
        height: Some(360),
        rotation: Some(90),
    };
    let until = std::time::Instant::now() + Duration::from_secs(10);
    let (seen, shown) = loop {
        assert!(
            std::time::Instant::now() < until,
            "No video crossed the relay"
        );
        babak_call.video(2_000, b"a keyframe", true).unwrap();
        babak_call.state(&state).unwrap();
        if let Some(keryx_moq::Event::Video { device, ts, bytes }) =
            sara_call.next(Duration::from_millis(100))
        {
            break ((device, ts), bytes.to_vec());
        }
    };
    assert_eq!(
        (seen, shown.as_slice()),
        ((babak_device, 2_000), &b"a keyframe"[..])
    );
    let keryx_moq::Event::State {
        device,
        state: read,
    } = on_call(&sara_call, |event| {
        matches!(event, keryx_moq::Event::State { .. })
    })
    else {
        unreachable!();
    };
    assert_eq!((device, read), (babak_device, state));

    // Text goes up on Babak's own track, and comes back to both on `events`, saved inside the call.
    let epoch = pair.babak.device.epoch(chat).unwrap();
    let header = keryx_core::protocol::call_text_header(chat, babak_device, call_seq, epoch, 5);
    let (body, signature) = pair
        .babak
        .device
        .seal(&header, br#"{"text":"salam"}"#)
        .unwrap();
    babak_call
        .text(&keryx_core::protocol::CallText {
            client_at: 5,
            epoch,
            signature: Bytes(signature.to_vec()),
            body: Bytes(body.clone()),
        })
        .unwrap();
    let text = moq_event(on_call(&sara_call, is_event(CallEventKind::Text, 3)));
    assert_eq!(
        (text.device, text.client_at, text.epoch, text.body),
        (babak_device, Some(5), Some(epoch), Some(Bytes(body)))
    );
    on_call(&babak_call, is_event(CallEventKind::Text, 3));

    // A clean close is a leave at once, and the last one to leave ends the call.
    drop(sara_call);
    let left = moq_event(on_call(&babak_call, is_event(CallEventKind::Leave, 4)));
    assert_eq!(left.device, sara_device);
    drop(babak_call);
    // Sara's chat socket gets every event but her own leave.
    let ended = call_event(&mut pair.sara_socket);
    assert_eq!(
        (ended.kind, ended.n, ended.device),
        (CallEventKind::Leave, 5, babak_device)
    );
    let ended = call_event(&mut pair.sara_socket);
    assert_eq!((ended.kind, ended.n), (CallEventKind::End, 6));
    let end = call_end(&mut pair.babak_socket, chat, call_seq).unwrap();
    assert_eq!(end.outcome, Some(CallOutcome::Answered));
    let mut kind_list = Vec::new();
    for event in call_rows(&mut pair.babak_socket, chat, call_seq) {
        kind_list.push(event.kind);
    }
    assert_eq!(
        kind_list,
        [
            CallEventKind::Join,
            CallEventKind::Join,
            CallEventKind::Text,
            CallEventKind::Leave,
            CallEventKind::Leave,
            CallEventKind::End
        ]
    );
}

#[test]
fn a_call_socket_is_refused_after_its_upgrade() {
    let pair = pair();
    let call = join_call(&pair.server, &pair.sara, pair.chat, 99);
    assert_eq!(call_closed(&call), (4403, "call_ended".to_string()));
}

#[test]
fn a_call_that_rings_out_closes_its_sockets() {
    let server = Server::start(
        "",
        "[registration]\nallow = true\n[calls]\ntest_ring = \"1s\"\n",
    );
    let mut pair = pair_on(server);
    let chat = pair.chat;
    let call_seq = seq_of(post(&mut pair.sara_socket, call_frame(&pair.sara, chat)));
    let sara_call = join_call(&pair.server, &pair.sara, chat, call_seq);
    // The caller's own join does not answer it, so it rings out, and says so before the close.
    let ended = call_event(&mut pair.babak_socket);
    assert_eq!((ended.kind, ended.n), (CallEventKind::Join, 1));
    let ended = call_event(&mut pair.babak_socket);
    assert_eq!((ended.kind, ended.n), (CallEventKind::End, 2));
    assert_eq!(call_closed(&sara_call), (1000, "call_ended".to_string()));
    let end = call_end(&mut pair.babak_socket, chat, call_seq).unwrap();
    assert_eq!(end.outcome, Some(CallOutcome::Missed));
}

#[test]
fn a_new_call_socket_replaces_the_old_one_and_writes_nothing() {
    let mut pair = pair();
    let chat = pair.chat;
    let call_seq = seq_of(post(&mut pair.sara_socket, call_frame(&pair.sara, chat)));
    let first = join_call(&pair.server, &pair.sara, chat, call_seq);
    on_call(&first, is_event(CallEventKind::Join, 1));
    let second = join_call(&pair.server, &pair.sara, chat, call_seq);
    assert_eq!(call_closed(&first), (1000, "replaced".to_string()));
    let babak_call = join_call(&pair.server, &pair.babak, chat, call_seq);
    // Babak's join is the second event: the replacement wrote no leave and no join.
    on_call(&second, is_event(CallEventKind::Join, 2));
    drop(babak_call);
}

#[test]
fn a_frame_over_call_frame_closes_its_socket_as_a_leave() {
    let server = Server::start(
        "",
        "[registration]\nallow = true\n[limits]\ncall_frame = \"1KiB\"\n",
    );
    let mut pair = pair_on(server);
    let chat = pair.chat;
    let call_seq = seq_of(post(&mut pair.sara_socket, call_frame(&pair.sara, chat)));
    let babak_call = join_call(&pair.server, &pair.babak, chat, call_seq);
    let sara_call = join_call(&pair.server, &pair.sara, chat, call_seq);
    on_call(&babak_call, is_event(CallEventKind::Join, 2));
    let until = std::time::Instant::now() + Duration::from_secs(10);
    let closed = loop {
        assert!(
            std::time::Instant::now() < until,
            "The large frame was never refused"
        );
        let _ = sara_call.audio(1, &[7u8; 2048]);
        if let Some(keryx_moq::Event::Closed { code, reason }) =
            sara_call.next(Duration::from_millis(100))
        {
            break (code, reason);
        }
    };
    assert_eq!(closed, (4403, "frame_too_large".to_string()));
    let left = moq_event(on_call(&babak_call, is_event(CallEventKind::Leave, 3)));
    assert_eq!(left.device, pair.sara.device.record().device);
}

/// A call socket opened by hand and dropped without a close frame, as a phone losing its network.
fn drop_call_socket(server: &Server, person: &Person, chat: Ulid, call_seq: u64) {
    let url = format!(
        "{}/api/v1/calls/{chat}/{call_seq}/moq",
        server.origin.replace("http://", "ws://")
    );
    let builder = ClientRequestBuilder::new(url.parse().unwrap())
        .with_header("Authorization", format!("Bearer {}", person.token))
        .with_sub_protocol(keryx_moq::PROTOCOL);
    let (socket, _) = tungstenite::connect(builder).unwrap();
    drop(socket);
}

#[test]
fn a_dropped_call_socket_gets_the_grace() {
    let server = Server::start(
        "",
        "[registration]\nallow = true\n[calls]\ntest_grace = \"1s\"\n",
    );
    let mut pair = pair_on(server);
    let chat = pair.chat;
    let sara_device = pair.sara.device.record().device;
    let call_seq = seq_of(post(&mut pair.sara_socket, call_frame(&pair.sara, chat)));
    drop_call_socket(&pair.server, &pair.sara, chat, call_seq);
    let joined = call_event(&mut pair.babak_socket);
    assert_eq!((joined.kind, joined.n), (CallEventKind::Join, 1));
    // Back within the grace: no leave and no new join.
    let sara_call = join_call(&pair.server, &pair.sara, chat, call_seq);
    let babak_call = join_call(&pair.server, &pair.babak, chat, call_seq);
    on_call(&sara_call, is_event(CallEventKind::Join, 2));
    drop(sara_call);
    drop(babak_call);
    while call_event(&mut pair.sara_socket).kind != CallEventKind::End {}

    // Gone for longer than the grace: the leave comes, and the last one ends the call.
    let call_seq = seq_of(post(&mut pair.sara_socket, call_frame(&pair.sara, chat)));
    drop_call_socket(&pair.server, &pair.sara, chat, call_seq);
    let mut kind_list = Vec::new();
    while kind_list.len() < 3 {
        let event = call_event(&mut pair.babak_socket);
        if event.call_seq == call_seq {
            assert_eq!(event.device, sara_device);
            kind_list.push((event.kind, event.n));
        }
    }
    assert_eq!(
        kind_list,
        [
            (CallEventKind::Join, 1),
            (CallEventKind::Leave, 2),
            (CallEventKind::End, 3)
        ]
    );
}

/// A lossless WebP of this size: the RIFF header, then a VP8L chunk with its signature and size.
fn webp(width: u32, height: u32) -> Vec<u8> {
    let bits = (width - 1) | ((height - 1) << 14);
    let mut chunk = vec![0x2f];
    chunk.extend_from_slice(&bits.to_le_bytes());
    chunk.resize(12, 0);
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&u32::try_from(4 + 8 + chunk.len()).unwrap().to_le_bytes());
    bytes.extend_from_slice(b"WEBPVP8L");
    bytes.extend_from_slice(&u32::try_from(chunk.len()).unwrap().to_le_bytes());
    bytes.extend_from_slice(&chunk);
    bytes
}

#[test]
fn a_sticker_names_a_server_sticker_inside_its_encryption_and_cannot_be_edited() {
    let server = open_server();
    let (_, reply) = server.call(
        "POST",
        "/api/v1/sticker_packs",
        Some(OPERATOR_TOKEN),
        Some(&json!({"name": "Cats"})),
    );
    let pack = reply["data"]["pack"].as_str().unwrap().to_string();
    let (status, reply) = raw(
        &server,
        "POST",
        &format!("/api/v1/sticker_packs/{pack}/stickers"),
        OPERATOR_TOKEN,
        Some(&webp(64, 64)),
    );
    assert_eq!(status, 200);
    let reply: Value = serde_json::from_slice(&reply).unwrap();
    let sticker = ulid_of(&reply["data"]["sticker"]["id"]);

    // Core lists the packs and fetches the image.
    let (mut sara, mut babak, chat) = apps_on(&server);
    sara.command(&server, Command::Stickers);
    let Event::Stickers { pack_list } =
        sara.wait(&server, |event| matches!(event, Event::Stickers { .. }))
    else {
        unreachable!();
    };
    assert_eq!(pack_list.len(), 2);
    assert_eq!(pack_list[0].sticker_list[0].id, sticker);
    assert_eq!(pack_list[1].name, "Default");
    sara.command(&server, Command::StickerImage { sticker });
    let Event::StickerImage { bytes, .. } =
        sara.wait(&server, |event| matches!(event, Event::StickerImage { .. }))
    else {
        unreachable!();
    };
    assert_eq!(bytes.0, webp(64, 64));
    let missing = Ulid::new().unwrap();
    sara.command(&server, Command::StickerImage { sticker: missing });
    let Event::StickerMissing { sticker: gone } = sara.wait(&server, |event| {
        matches!(event, Event::StickerMissing { .. })
    }) else {
        unreachable!();
    };
    assert_eq!(gone, missing);

    // Sent as a sticker, a reply to nothing, and the other side reads its id.
    sara.command(
        &server,
        Command::SendSticker {
            thread: None,
            topic: None,
            chat,
            sticker,
            reply: None,
        },
    );
    // The pending message names the sticker, so the sender can draw it at once.
    let Event::Pending {
        content_kind,
        sticker: pending,
        ..
    } = sara.wait(&server, |event| matches!(event, Event::Pending { .. }))
    else {
        unreachable!();
    };
    assert_eq!(content_kind, Some(ContentKind::Sticker));
    assert_eq!(pending, Some(sticker));
    let Event::Sent { seq, .. } = sara.wait(&server, |event| matches!(event, Event::Sent { .. }))
    else {
        unreachable!();
    };
    let message = shown(babak.wait(&server, |event| is_message(event, chat, seq)));
    assert_eq!(message.content_kind, Some(ContentKind::Sticker));
    assert_eq!(message.sticker, Some(sticker));
    assert_eq!(message.text, None);
    let stored = server.call(
        "GET",
        "/api/v1/stickers",
        Some(sara.token.as_ref().unwrap()),
        None,
    );
    assert_eq!(stored.0, 200);

    // The server never sees which sticker, and refuses an edit of one.
    let mut pair = pair();
    let chat = pair.chat;
    let body = format!("{{\"sticker\":\"{sticker}\"}}");
    let frame = sealed_as(
        &pair.sara,
        chat,
        Kind::Content,
        Some(ContentKind::Sticker),
        1,
        None,
        body.as_bytes(),
    );
    let sticker_seq = seq_of(post(&mut pair.sara_socket, frame));
    let received = pair.babak_socket.message(chat, sticker_seq);
    assert!(
        !String::from_utf8_lossy(&received.body.unwrap().0).contains(&sticker.to_string()),
        "sealed"
    );
    let edit = sealed(&pair.sara, chat, Kind::Edit, Some(sticker_seq), b"{}");
    assert_refused(post(&mut pair.sara_socket, edit), "cannot_edit");
}

#[test]
fn reactions_go_through_core_with_their_target_inside_the_encryption_and_several_can_stand() {
    let server = open_server();
    let (mut sara, mut babak, chat) = apps_on(&server);
    sara.command(
        &server,
        Command::SendText {
            mentions: Vec::new(),
            thread: None,
            topic: None,
            chat,
            text: Secret("lunch?".to_string()),
            reply: None,
        },
    );
    let Event::Sent { seq: on, .. } =
        sara.wait(&server, |event| matches!(event, Event::Sent { .. }))
    else {
        unreachable!();
    };
    shown(babak.wait(&server, |event| is_message(event, chat, on)));
    // Two from one person on one message, each its own message.
    let mut sent = Vec::new();
    for emoji in ["👍", "👍🏽"] {
        babak.command(
            &server,
            Command::SendReaction {
                chat,
                emoji: emoji.to_string(),
                on: Pointer::Seq(on),
                topic: None,
                thread: None,
            },
        );
        let Event::Sent { seq, .. } =
            babak.wait(&server, |event| matches!(event, Event::Sent { .. }))
        else {
            unreachable!();
        };
        sent.push(seq);
    }
    for (seq, emoji) in sent.iter().zip(["👍", "👍🏽"]) {
        let message = shown(sara.wait(&server, |event| is_message(event, chat, *seq)));
        assert_eq!(message.content_kind, Some(ContentKind::Reaction));
        assert_eq!(message.emoji.as_deref(), Some(emoji));
        assert_eq!(message.on, Some(on));
        assert_eq!(message.text, None);
    }

    // The server holds no pointer: what it is on is only in the ciphertext.
    let mut socket = connect(&server, babak.token.as_deref().unwrap());
    let (rows, _) = sync_chat(&mut socket, chat);
    for row in rows {
        if sent.contains(&row.seq) {
            assert_eq!(row.pointer_seq, None);
        }
    }

    // Removing one is a delete pointing at it, and the other stays.
    babak.command(
        &server,
        Command::Delete {
            chat,
            target: Pointer::Seq(sent[0]),
            remove: Vec::new(),
        },
    );
    babak.wait(&server, |event| matches!(event, Event::Sent { .. }));
    let Event::Deleted { target_seq, .. } =
        sara.wait(&server, |event| matches!(event, Event::Deleted { .. }))
    else {
        unreachable!();
    };
    assert_eq!(target_seq, sent[0]);
}

#[test]
fn a_question_and_its_answer_are_sealed_checked_by_core_and_the_answer_can_change() {
    let server = open_server();
    let (mut sara, mut babak, chat) = apps_on(&server);
    let question: Question = serde_json::from_value(json!({
        "first": "size",
        "nodes": [
            {"id": "size", "text": "Which size?",
             "choice": {"multi": false, "options": [{"id": "s", "text": "Small"}, {"id": "l", "text": "Large"}]},
             "next": [{"picked": ["l"], "to": "much"}]},
            {"id": "much", "text": "How many kilos?", "number": {"min": 1, "max": 9, "unit": "kg"}}
        ]
    }))
    .unwrap();

    // Core refuses a tree that does not hold together, and sends nothing.
    let mut broken = question.clone();
    broken.first = "nowhere".to_string();
    sara.command(
        &server,
        Command::SendQuestion {
            chat,
            text: None,
            question: broken,
        },
    );
    let Event::Error { code, .. } =
        sara.wait(&server, |event| matches!(event, Event::Error { .. }))
    else {
        unreachable!();
    };
    assert_eq!(code, "bad_question");

    sara.command(
        &server,
        Command::SendQuestion {
            chat,
            text: Some(Secret("Lunch order".to_string())),
            question: question.clone(),
        },
    );
    let Event::Sent { seq, .. } = sara.wait(&server, |event| matches!(event, Event::Sent { .. }))
    else {
        unreachable!();
    };
    let asked = shown(babak.wait(&server, |event| is_message(event, chat, seq)));
    assert_eq!(asked.content_kind, Some(ContentKind::Question));
    assert_eq!(text_of(&asked), Some("Lunch order"));
    assert_eq!(asked.question.as_ref(), Some(&question));

    // An answer off the path is refused by core, and one along it reaches the asker.
    let off: Answer = serde_json::from_value(json!({"question": seq, "answers": [
        {"node": "size", "picked": ["s"]}, {"node": "much", "number": 2}]}))
    .unwrap();
    babak.command(
        &server,
        Command::SendAnswer {
            chat,
            question: question.clone(),
            answer: off,
        },
    );
    let Event::Error { code, .. } =
        babak.wait(&server, |event| matches!(event, Event::Error { .. }))
    else {
        unreachable!();
    };
    assert_eq!(code, "bad_answer");
    let large: Answer = serde_json::from_value(json!({"question": seq, "answers": [
        {"node": "size", "picked": ["l"]}, {"node": "much", "number": 3}]}))
    .unwrap();
    babak.command(
        &server,
        Command::SendAnswer {
            chat,
            question: question.clone(),
            answer: large.clone(),
        },
    );
    let Event::Pending {
        answer: pending, ..
    } = babak.wait(&server, |event| matches!(event, Event::Pending { .. }))
    else {
        unreachable!();
    };
    assert_eq!(pending.as_ref(), Some(&large));
    let Event::Sent { seq: answered, .. } =
        babak.wait(&server, |event| matches!(event, Event::Sent { .. }))
    else {
        unreachable!();
    };
    let answer = shown(sara.wait(&server, |event| is_message(event, chat, answered)));
    assert_eq!(answer.content_kind, Some(ContentKind::Answer));
    assert_eq!(answer.answer.as_ref(), Some(&large));

    // Changing it is an edit, and skipping the whole form is an answer too.
    let skipped: Answer =
        serde_json::from_value(json!({"question": seq, "skipped": true})).unwrap();
    babak.command(
        &server,
        Command::EditAnswer {
            chat,
            target: Pointer::Seq(answered),
            question,
            answer: skipped.clone(),
        },
    );
    let Event::Pending {
        kind,
        answer: pending,
        ..
    } = babak.wait(&server, |event| matches!(event, Event::Pending { .. }))
    else {
        unreachable!();
    };
    assert_eq!(kind, Kind::Edit);
    assert_eq!(pending.as_ref(), Some(&skipped));
    let Event::Edited {
        message,
        target_seq,
    } = sara.wait(&server, |event| matches!(event, Event::Edited { .. }))
    else {
        unreachable!();
    };
    assert_eq!(target_seq, answered);
    assert_eq!(message.answer.as_ref(), Some(&skipped));
}

#[test]
fn a_suggestion_and_its_ack_cannot_be_edited_and_an_ack_moves_neither_time_nor_preview() {
    let mut pair = pair();
    let chat = pair.chat;
    let text = sealed(&pair.sara, chat, Kind::Content, None, br#"{"text":"hi"}"#);
    let text_seq = seq_of(post(&mut pair.sara_socket, text));
    let text_at = chat_item(&mut pair.sara_socket, chat).last_at;
    let sara_epoch = pair.sara.device.epoch(chat).unwrap();
    let suggestion = sealed_as(
        &pair.sara,
        chat,
        Kind::Content,
        Some(ContentKind::Suggestion),
        sara_epoch,
        None,
        br#"{"folder":{"name":"Work","move":false}}"#,
    );
    let suggested = seq_of(post(&mut pair.sara_socket, suggestion));
    let item = chat_item(&mut pair.babak_socket, chat);
    assert_eq!(item.last_content_seq, Some(suggested));
    let suggested_at = item.last_at;
    assert!(suggested_at >= text_at);

    // An ack is quiet: the chat's time and preview stay on the suggestion.
    let babak_epoch = pair.babak.device.epoch(chat).unwrap();
    let body = format!(r#"{{"suggestion":{suggested},"accepted":false}}"#);
    let ack = sealed_as(
        &pair.babak,
        chat,
        Kind::Content,
        Some(ContentKind::Ack),
        babak_epoch,
        None,
        body.as_bytes(),
    );
    let acked = seq_of(post(&mut pair.babak_socket, ack));
    let item = chat_item(&mut pair.babak_socket, chat);
    assert_eq!(item.last_content_seq, Some(suggested));
    assert_eq!(item.last_at, suggested_at);
    assert_eq!(
        item.last_content.as_ref().map(|message| message.seq),
        Some(suggested)
    );

    // Neither can be edited.
    let edit = sealed(&pair.sara, chat, Kind::Edit, Some(suggested), b"{}");
    assert_refused(post(&mut pair.sara_socket, edit), "cannot_edit");
    let edit = sealed(&pair.babak, chat, Kind::Edit, Some(acked), b"{}");
    assert_refused(post(&mut pair.babak_socket, edit), "cannot_edit");

    // Deleting the suggestion walks the preview and the time back past the ack.
    let delete = sealed(&pair.sara, chat, Kind::Delete, Some(suggested), b"");
    seq_of(post(&mut pair.sara_socket, delete));
    let item = chat_item(&mut pair.sara_socket, chat);
    assert_eq!(item.last_content_seq, Some(text_seq));
    assert_eq!(item.last_at, text_at);
}

#[test]
fn a_folder_suggestion_is_accepted_by_core_saving_the_folder_before_its_ack() {
    let server = Server::start("", "[registration]\nallow = true\n[limits]\nfolders = 2\n");
    let (mut sara, mut babak, chat) = apps_on(&server);
    let (_, all) = folders_shown(&mut babak, &server, |_| true);
    let work = new_folder_id(&mut babak, &server);
    babak.command(
        &server,
        Command::Folders {
            folder_list: vec![
                Folder {
                    id: all,
                    name: None,
                    chat_list: None,
                    pin_list: Vec::new(),
                },
                Folder {
                    id: work,
                    name: Some(Secret("کاری".to_string())),
                    chat_list: Some(vec![chat]),
                    pin_list: vec![chat],
                },
            ],
            default: all,
        },
    );
    folders_shown(&mut babak, &server, |(list, _)| list.len() == 2);
    let suggest = |app: &mut App, text: Option<&str>, name: &str, r#move: bool| {
        app.command(
            &server,
            Command::SendSuggestion {
                chat,
                text: text.map(|text| Secret(text.to_string())),
                suggestion: Suggestion::Folder(FolderSuggestion {
                    name: name.to_string(),
                    r#move,
                }),
            },
        );
    };
    let error_of = |app: &mut App| {
        let Event::Error { code, .. } =
            app.wait(&server, |event| matches!(event, Event::Error { .. }))
        else {
            unreachable!();
        };
        code
    };
    let sent = |app: &mut App| {
        let Event::Sent { seq, .. } =
            app.wait(&server, |event| matches!(event, Event::Sent { .. }))
        else {
            unreachable!();
        };
        seq
    };

    // Core refuses a folder's name outside 1 to 32 characters, and sends nothing.
    suggest(&mut sara, None, "  ", false);
    assert_eq!(error_of(&mut sara), "bad_suggestion");
    suggest(&mut sara, None, &"x".repeat(33), false);
    assert_eq!(error_of(&mut sara), "bad_suggestion");

    // Arabic forms find the Persian folder, which already holds the chat: the ack goes at once and
    // says nothing changed.
    suggest(&mut sara, Some("Where this goes"), "كاري", false);
    let first = sent(&mut sara);
    let shown_first = shown(babak.wait(&server, |event| is_message(event, chat, first)));
    assert_eq!(shown_first.content_kind, Some(ContentKind::Suggestion));
    assert_eq!(text_of(&shown_first), Some("Where this goes"));
    let suggestion = *shown_first.suggestion.unwrap();
    assert_eq!(
        suggestion,
        Suggestion::Folder(FolderSuggestion {
            name: "كاري".to_string(),
            r#move: false,
        })
    );
    babak.command(
        &server,
        Command::AcceptSuggestion {
            chat,
            seq: first,
            suggestion,
        },
    );
    let Event::Pending { ack: pending, .. } =
        babak.wait(&server, |event| matches!(event, Event::Pending { .. }))
    else {
        unreachable!();
    };
    let unchanged = Ack {
        suggestion: first,
        accepted: true,
        created: Some(false),
        left: Some(0),
    };
    assert_eq!(pending, Some(unchanged.clone()));
    let first_ack = sent(&mut babak);
    let acked = shown(sara.wait(&server, |event| is_message(event, chat, first_ack)));
    assert_eq!(acked.content_kind, Some(ContentKind::Ack));
    assert_eq!(acked.ack.map(|ack| *ack), Some(unchanged));

    // A move to a folder that does not exist makes it last, takes the chat and its pin out of the
    // other, and acks once the server took the arrangement.
    suggest(&mut sara, None, "Lunch", true);
    let second = sent(&mut sara);
    let suggestion = *shown(babak.wait(&server, |event| is_message(event, chat, second)))
        .suggestion
        .unwrap();
    babak.command(
        &server,
        Command::AcceptSuggestion {
            chat,
            seq: second,
            suggestion,
        },
    );
    let (list, _) = folders_shown(&mut babak, &server, |(list, _)| list.len() == 3);
    assert_eq!(
        list[1],
        (work, Some("کاری".to_string()), Some(Vec::new()), Vec::new())
    );
    assert_eq!(list[2].1.as_deref(), Some("Lunch"));
    assert_eq!(list[2].2, Some(vec![chat]));
    let second_ack = sent(&mut babak);
    let acked = shown(sara.wait(&server, |event| is_message(event, chat, second_ack)));
    assert_eq!(
        acked.ack.map(|ack| *ack),
        Some(Ack {
            suggestion: second,
            accepted: true,
            created: Some(true),
            left: Some(1),
        })
    );

    // A change the server refuses sends no ack, and a decline is an ack alone.
    suggest(&mut sara, None, "Third", false);
    let third = sent(&mut sara);
    let suggestion = *shown(babak.wait(&server, |event| is_message(event, chat, third)))
        .suggestion
        .unwrap();
    babak.command(
        &server,
        Command::AcceptSuggestion {
            chat,
            seq: third,
            suggestion,
        },
    );
    assert_eq!(error_of(&mut babak), "too_many_folders");
    babak.command(&server, Command::DeclineSuggestion { chat, seq: third });
    let declined = sent(&mut babak);
    assert_eq!(declined, third + 1);
    let acked = shown(sara.wait(&server, |event| is_message(event, chat, declined)));
    assert_eq!(
        acked.ack.map(|ack| *ack),
        Some(Ack {
            suggestion: third,
            accepted: false,
            created: None,
            left: None,
        })
    );
}

/// A third person's words, for someone who shares no chat.
const NIMA_WORDS: &str =
    "letter advice cage absurd amount doctor acoustic avoid letter advice cage above";

fn is_profile(event: &Event, account: Ulid, wanted: impl Fn(&ShownProfile) -> bool) -> bool {
    matches!(event, Event::Profile { profile } if profile.account == account && wanted(profile))
}

fn profile_of(event: Event) -> ShownProfile {
    match event {
        Event::Profile { profile } => profile,
        other => panic!("Wanted a profile, and got {other:?}"),
    }
}

fn refusal(app: &mut App, server: &Server, command: Command) -> String {
    app.command(server, command);
    match app.wait(server, |event| matches!(event, Event::Error { .. })) {
        Event::Error { code, .. } => code,
        _ => unreachable!(),
    }
}

#[test]
fn a_profile_is_sealed_to_those_sharing_a_chat_and_changes_reach_them_live() {
    let server = open_server();
    let (mut sara, mut babak, _chat) = apps_on(&server);
    let (sara_id, babak_id) = (sara.id(), babak.id());
    sara.wait(&server, |event| is_profile(event, sara_id, |_| true));

    let link = ProfileLink {
        name: "Site".to_string(),
        url: "https://example.com".to_string(),
    };
    sara.command(
        &server,
        Command::SaveProfile {
            bio: Some(" Hello ".to_string()),
            link_list: vec![link.clone()],
        },
    );
    sara.wait(&server, |event| matches!(event, Event::ProfileSaved));
    let jpeg: Vec<u8> = (0..3000).map(|n| (n % 251) as u8).collect();
    sara.command(
        &server,
        Command::SetPhoto {
            jpeg: jpeg.clone(),
            thumbnail: Thumbnail {
                width: 2,
                height: 2,
                rgba: vec![200; 16],
            },
        },
    );
    sara.wait(&server, |event| matches!(event, Event::ProfileSaved));

    // Babak hears of it live, and opens it with the key Sara's commit shared in their chat.
    let profile = profile_of(babak.wait(&server, |event| {
        is_profile(event, sara_id, |profile| profile.photo.is_some())
    }));
    assert_eq!(profile.bio.as_deref(), Some("Hello"));
    assert_eq!(profile.link_list, vec![link]);
    let photo = profile.photo.unwrap();
    assert!(!photo.blurhash.is_empty());
    babak.command(&server, Command::Photo { account: sara_id });
    let Event::Photo {
        photo: got, bytes, ..
    } = babak.wait(&server, |event| matches!(event, Event::Photo { .. }))
    else {
        unreachable!();
    };
    assert_eq!(got, photo.id);
    assert_eq!(bytes.0, jpeg);

    // Babak's own key reached the chat by a frame, so Sara opens his profile too.
    babak.wait(&server, |event| is_profile(event, babak_id, |_| true));
    babak.command(
        &server,
        Command::SaveProfile {
            bio: Some("Hi".to_string()),
            link_list: Vec::new(),
        },
    );
    babak.wait(&server, |event| matches!(event, Event::ProfileSaved));
    sara.wait(&server, |event| {
        is_profile(event, babak_id, |profile| {
            profile.bio.as_deref() == Some("Hi")
        })
    });

    // Names are readable, and reach the other person live.
    babak.command(
        &server,
        Command::SetNames {
            display_name: Some("  Babak J ".to_string()),
            username: None,
            password: None,
        },
    );
    babak.wait(&server, |event| matches!(event, Event::ProfileSaved));
    sara.wait(&server, |event| {
        matches!(event, Event::Account { account }
            if account.id == babak_id && account.display_name.as_deref() == Some("Babak J"))
    });
    babak.command(
        &server,
        Command::SetNames {
            display_name: None,
            username: Some("babak_new".to_string()),
            password: Some(Secret(PASSWORD.to_string())),
        },
    );
    babak.wait(&server, |event| matches!(event, Event::ProfileSaved));
    sara.wait(&server, |event| {
        matches!(event, Event::Account { account }
            if account.id == babak_id && account.username == "babak_new")
    });
    let rename = |username: &str, password: &str| Command::SetNames {
        display_name: None,
        username: Some(username.to_string()),
        password: Some(Secret(password.to_string())),
    };
    assert_eq!(
        refusal(&mut babak, &server, rename("sara_x", PASSWORD)),
        "username_taken"
    );
    assert_eq!(
        refusal(
            &mut babak,
            &server,
            rename("babak_other", "not the password")
        ),
        "bad_password"
    );
    assert_eq!(
        refusal(
            &mut babak,
            &server,
            Command::SetNames {
                display_name: Some("   ".to_string()),
                username: None,
                password: None,
            }
        ),
        "bad_display_name"
    );

    // Core refuses what breaks the limits before anything goes.
    let save = |bio: &str, link_list: Vec<ProfileLink>| Command::SaveProfile {
        bio: Some(bio.to_string()),
        link_list,
    };
    let link_to = |url: &str| ProfileLink {
        name: "x".to_string(),
        url: url.to_string(),
    };
    for command in [
        save(&"a".repeat(141), Vec::new()),
        save("", vec![link_to("ftp://example.com")]),
        save("", vec![link_to("https://")]),
        save("", vec![link_to("https://example.com"); 4]),
    ] {
        assert_eq!(refusal(&mut sara, &server, command), "bad_profile");
    }

    // Someone sharing no chat gets neither the profile nor the photo.
    let stranger = App::sign_up(&server, "nima_x", NIMA_WORDS);
    let token = stranger.token.clone().unwrap();
    let (status, body) = raw(
        &server,
        "GET",
        &format!("/api/v1/accounts/{sara_id}/profile"),
        &token,
        None,
    );
    assert_eq!(status, 403, "{}", String::from_utf8_lossy(&body));
    let (status, _) = raw(
        &server,
        "GET",
        &format!("/api/v1/photos/{}", photo.id),
        &token,
        None,
    );
    assert_eq!(status, 404);

    // Only admin changes someone else's names or removes their photo, and never sees it.
    let babak_token = babak.token.clone().unwrap();
    let names = json!({"display_name": "Not Sara"});
    let route = format!("/api/v1/accounts/{sara_id}/names");
    let (status, _) = server.call("PUT", &route, Some(&babak_token), Some(&names));
    assert_eq!(status, 403);
    let route = format!("/api/v1/accounts/{sara_id}/photo");
    let (status, _) = server.call("DELETE", &route, Some(&babak_token), None);
    assert_eq!(status, 403);
    let (status, reply) = server.call("DELETE", &route, Some(OPERATOR_TOKEN), None);
    assert_eq!(status, 200, "{reply}");
    babak.wait(&server, |event| {
        is_profile(event, sara_id, |profile| {
            profile.photo.is_none() && profile.bio.as_deref() == Some("Hello")
        })
    });
    let (status, reply) = server.call("DELETE", &route, Some(OPERATOR_TOKEN), None);
    assert_eq!(status, 404);
    assert_eq!(reply["code"], "no_such_photo");
    let names = json!({"username": "b"});
    let route = format!("/api/v1/accounts/{babak_id}/names");
    let (status, reply) = server.call("PUT", &route, Some(OPERATOR_TOKEN), Some(&names));
    assert_eq!(status, 200, "{reply}");
    sara.wait(&server, |event| {
        matches!(event, Event::Account { account } if account.id == babak_id && account.username == "b")
    });

    // A new photo, then removing it: the old bytes go each time.
    sara.command(
        &server,
        Command::SetPhoto {
            jpeg: jpeg.clone(),
            thumbnail: Thumbnail {
                width: 1,
                height: 1,
                rgba: vec![9; 4],
            },
        },
    );
    sara.wait(&server, |event| matches!(event, Event::ProfileSaved));
    babak.wait(&server, |event| {
        is_profile(event, sara_id, |profile| profile.photo.is_some())
    });
    sara.command(&server, Command::RemovePhoto);
    sara.wait(&server, |event| matches!(event, Event::ProfileSaved));
    babak.wait(&server, |event| {
        is_profile(event, sara_id, |profile| {
            profile.photo.is_none() && profile.bio.as_deref() == Some("Hello")
        })
    });
    let (status, _) = raw(
        &server,
        "GET",
        &format!("/api/v1/photos/{}", photo.id),
        babak.token.as_deref().unwrap(),
        None,
    );
    assert_eq!(status, 404);
    let folder = server
        .path("files")
        .join("accounts")
        .join(sara_id.to_string());
    assert_eq!(std::fs::read_dir(folder).unwrap().count(), 0);

    // A new device of Babak's, with only his words, opens Sara's profile, though her key was
    // shared at an epoch before it joined.
    let mut new = App::new();
    new.command(
        &server,
        Command::LogIn {
            username: "b".to_string(),
            password: Secret(PASSWORD.to_string()),
            captcha: None,
        },
    );
    new.wait(&server, |event| {
        matches!(event, Event::NeedWords { restore: true })
    });
    new.command(
        &server,
        Command::Restore {
            words: Secret(BABAK_WORDS.to_string()),
        },
    );
    new.wait(&server, |event| {
        is_profile(event, sara_id, |profile| {
            profile.bio.as_deref() == Some("Hello")
        })
    });
}

/// The next `presence` about this account that `wanted` takes, keeping the rest.
fn presence_about(
    socket: &mut Socket,
    account: Ulid,
    wanted: impl Fn(&PresenceItem) -> bool,
) -> PresenceItem {
    let frame = socket.next_where(|frame| {
        matches!(frame, ServerFrame::Presence(item) if item.account == account && wanted(item))
    });
    let ServerFrame::Presence(item) = frame else {
        unreachable!();
    };
    *item
}

fn is_state(state: PresenceState) -> impl Fn(&PresenceItem) -> bool {
    move |item| item.state == Some(state)
}

/// This device's `presence`, an event with no answer.
fn announce(socket: &mut Socket, in_app: bool) {
    socket.send(&ClientFrame::Presence { in_app });
}

fn presence_asked(socket: &mut Socket, account: Ulid) -> ServerFrame {
    let wanted = reference();
    socket.request(
        &ClientFrame::PresenceOf {
            reference: wanted,
            account,
        },
        wanted,
    )
}

fn presence_answer(frame: ServerFrame) -> PresenceItem {
    match frame {
        ServerFrame::PresenceOf { presence, .. } => presence,
        other => panic!("Wanted a presence, and got {other:?}"),
    }
}

/// Takes a permission from every built-in role, straight in the database, since nothing manages
/// roles yet. The server reads the role tables on every operation.
fn take_permission(server: &Server, permission: &str) {
    let connection = rusqlite::Connection::open(server.path("keryx.db")).unwrap();
    connection.busy_timeout(Duration::from_secs(10)).unwrap();
    connection
        .execute(
            "DELETE FROM role_permission WHERE permission = ?1",
            [permission],
        )
        .unwrap();
}

#[test]
fn presence_counts_once_a_device_announces_itself_and_reaches_whoever_shares_a_chat() {
    let mut pair = pair();
    let (sara, babak, chat) = (pair.sara.id, pair.babak.id, pair.chat);
    let babak_device = pair.babak.device.record().device;
    let sara_device = pair.sara.device.record().device;

    // Sara's first `presence` brings everyone sharing a chat. Babak's socket has not announced
    // itself, so he is not there, with the time his client was last connected.
    announce(&mut pair.sara_socket, true);
    let seen = presence_about(&mut pair.sara_socket, babak, |_| true);
    assert_eq!(seen.state, Some(PresenceState::Offline));
    assert!(seen.connected_at.is_some());
    assert_eq!(seen.in_app_at, None);

    // Babak announces with his app in the background: he gets her online, and she gets him away.
    announce(&mut pair.babak_socket, false);
    let first = presence_about(
        &mut pair.babak_socket,
        sara,
        is_state(PresenceState::Online),
    );
    assert!(first.in_app_at.is_some() && first.connected_at.is_some());
    presence_about(&mut pair.sara_socket, babak, is_state(PresenceState::Away));

    // Her app leaves the screen: away, and her time in the app moves to now.
    announce(&mut pair.sara_socket, false);
    let away = presence_about(&mut pair.babak_socket, sara, is_state(PresenceState::Away));
    assert!(away.in_app_at >= first.in_app_at);
    let asked = presence_answer(presence_asked(&mut pair.babak_socket, sara));
    assert_eq!(asked.state, Some(PresenceState::Away));

    // In a call, red; the call ending brings her back.
    let call_seq = seq_of(post(&mut pair.sara_socket, call_frame(&pair.sara, chat)));
    pair.babak_socket.message(chat, call_seq);
    let call = join_call(&pair.server, &pair.sara, chat, call_seq);
    presence_about(&mut pair.babak_socket, sara, is_state(PresenceState::Call));
    done(decline(&mut pair.babak_socket, chat, call_seq));
    call_closed(&call);
    presence_about(&mut pair.babak_socket, sara, is_state(PresenceState::Away));

    // Someone sharing no chat is asked about once, and an unknown account is refused.
    let stranger = sign_up(&pair.server, "mina_x", SARA_WORDS);
    let asked = presence_answer(presence_asked(&mut pair.babak_socket, stranger.id));
    assert_eq!(asked.state, Some(PresenceState::Offline));
    assert!(asked.connected_at.is_some());
    assert_refused(
        presence_asked(&mut pair.babak_socket, Ulid::new().unwrap()),
        "no_such_account",
    );

    // The moment her connection drops she is offline, connected until then.
    pair.sara_socket.inner.close(None).unwrap();
    pair.sara_socket.closed();
    let gone = presence_about(
        &mut pair.babak_socket,
        sara,
        is_state(PresenceState::Offline),
    );
    assert!(gone.connected_at > first.connected_at);

    // A new connection counts for nothing until it announces itself.
    pair.sara_socket = connect(&pair.server, &pair.sara.token);
    let asked = presence_answer(presence_asked(&mut pair.babak_socket, sara));
    assert_eq!(asked.state, Some(PresenceState::Offline));
    announce(&mut pair.sara_socket, true);
    presence_about(
        &mut pair.babak_socket,
        sara,
        is_state(PresenceState::Online),
    );

    // Over REST: only the session's own device, answered with everyone sharing a chat.
    let route = format!("/api/v1/devices/{babak_device}/presence");
    let (status, reply) = pair.server.call(
        "PUT",
        &route,
        Some(&pair.babak.token),
        Some(&json!({"in_app": true})),
    );
    assert_eq!(status, 200, "{reply}");
    assert_eq!(
        reply["data"]["presence_list"],
        json!([{"account": sara.to_string(), "state": "online",
                "in_app_at": reply["data"]["presence_list"][0]["in_app_at"],
                "connected_at": reply["data"]["presence_list"][0]["connected_at"]}])
    );
    presence_about(
        &mut pair.sara_socket,
        babak,
        is_state(PresenceState::Online),
    );
    let (status, reply) = pair.server.call(
        "PUT",
        &format!("/api/v1/devices/{sara_device}/presence"),
        Some(&pair.babak.token),
        Some(&json!({"in_app": true})),
    );
    assert_eq!(
        (status, reply["code"].as_str()),
        (404, Some("no_such_device"))
    );
    let (status, reply) = pair.server.call(
        "GET",
        &format!("/api/v1/accounts/{sara}/presence"),
        Some(&pair.babak.token),
        None,
    );
    assert_eq!(
        (status, reply["data"]["state"].as_str()),
        (200, Some("online"))
    );

    // Without `presence.connected`, the Connected time is hidden; without `presence.in_app` too,
    // nothing is seen at all.
    take_permission(&pair.server, "presence.connected");
    announce(&mut pair.sara_socket, false);
    let hidden = presence_about(&mut pair.babak_socket, sara, is_state(PresenceState::Away));
    assert_eq!(hidden.connected_at, None);
    assert!(hidden.in_app_at.is_some());
    take_permission(&pair.server, "presence.in_app");
    assert_refused(
        presence_asked(&mut pair.babak_socket, sara),
        "no_permission",
    );
    let (status, reply) = pair.server.call(
        "PUT",
        &route,
        Some(&pair.babak.token),
        Some(&json!({"in_app": false})),
    );
    assert_eq!((status, &reply["data"]["presence_list"]), (200, &json!([])));
}

#[test]
fn a_new_chat_tells_each_side_where_the_other_is() {
    let mut pair = pair();
    announce(&mut pair.babak_socket, false);
    let mina = sign_up(&pair.server, "mina_x", SARA_WORDS);
    let mut mina_socket = connect(&pair.server, &mina.token);
    announce(&mut mina_socket, true);

    // Neither changes anything after the chat is made, and each still hears of the other at once.
    let (status, reply) = pair.server.call(
        "POST",
        "/api/v1/chats",
        Some(&pair.babak.token),
        Some(&json!({"account": mina.id.to_string()})),
    );
    assert_eq!(
        (status, reply["data"]["created"].as_bool()),
        (200, Some(true)),
        "{reply}"
    );
    presence_about(
        &mut pair.babak_socket,
        mina.id,
        is_state(PresenceState::Online),
    );
    presence_about(
        &mut mina_socket,
        pair.babak.id,
        is_state(PresenceState::Away),
    );
}

fn kv_item(key: &str, value: &[u8]) -> KvItem {
    KvItem {
        key: key.to_string(),
        value: Bytes(value.to_vec()),
    }
}

/// This account's settings over the socket: the version, and the whole set when it differs.
fn kv_asked(socket: &mut Socket, version: Option<u64>) -> (u64, Option<Vec<KvItem>>) {
    let wanted = reference();
    match socket.request(
        &ClientFrame::Kv {
            reference: wanted,
            version,
        },
        wanted,
    ) {
        ServerFrame::Kv {
            version, kv_list, ..
        } => (version, kv_list),
        other => panic!("Wanted the settings, and got {other:?}"),
    }
}

fn kv_change(socket: &mut Socket, version: u64, set: Vec<KvItem>, remove: &[&str]) -> ServerFrame {
    let wanted = reference();
    socket.request(
        &ClientFrame::KvSet {
            reference: wanted,
            change: KvChange {
                version,
                set,
                remove: remove.iter().map(|key| key.to_string()).collect(),
            },
        },
        wanted,
    )
}

fn kv_saved(frame: ServerFrame) -> u64 {
    match frame {
        ServerFrame::Kv {
            reference: Some(_),
            version,
            kv_list: None,
            ..
        } => version,
        other => panic!("Wanted it saved, and got {other:?}"),
    }
}

#[test]
fn settings_go_by_version_the_server_wins_and_other_devices_get_only_the_change() {
    let server = Server::start("", "[registration]\nallow = true\n[limits]\nkv_keys = 2\n");
    let mut pair = pair_on(server);
    let second_token = log_in(&pair.server, &pair.sara.username);
    add_device(&pair.server, pair.sara.id, &pair.sara.keys, &second_token);
    let mut second = connect(&pair.server, &second_token);

    // A new account has none, at version 0, and a device on the server's version gets only that.
    assert_eq!(kv_asked(&mut pair.sara_socket, None), (0, Some(Vec::new())));
    assert_eq!(kv_asked(&mut pair.sara_socket, Some(0)), (0, None));

    // A change is answered with the new version, and the account's other device gets the change.
    let lang = kv_item("lang", b"sealed fa");
    let version = kv_saved(kv_change(&mut pair.sara_socket, 0, vec![lang.clone()], &[]));
    assert_eq!(version, 1);
    let pushed = second.next_where(|frame| {
        matches!(
            frame,
            ServerFrame::Kv {
                reference: None,
                ..
            }
        )
    });
    let ServerFrame::Kv {
        version,
        from,
        set,
        remove,
        kv_list,
        ..
    } = pushed
    else {
        unreachable!();
    };
    assert_eq!(
        (version, from, set, remove, kv_list),
        (1, Some(0), Some(vec![lang.clone()]), Some(Vec::new()), None)
    );

    // One based on an older version is refused, with the server's whole set.
    let theme = kv_item("theme", b"sealed dark");
    match kv_change(&mut second, 0, vec![theme.clone()], &[]) {
        ServerFrame::Refused {
            code,
            version,
            kv_list,
            ..
        } => assert_eq!(
            (code.as_str(), version, kv_list),
            ("kv_stale", Some(1), Some(vec![lang.clone()]))
        ),
        other => panic!("Wanted it stale, and got {other:?}"),
    }
    assert_eq!(
        kv_saved(kv_change(&mut second, 1, vec![theme.clone()], &[])),
        2
    );

    // `kv_keys` counts what the change leaves, so a removal makes room in the same change.
    let stickers = kv_item("recent.stickers", b"sealed list");
    assert_refused(
        kv_change(&mut pair.sara_socket, 2, vec![stickers.clone()], &[]),
        "too_many_keys",
    );
    assert_eq!(
        kv_saved(kv_change(
            &mut pair.sara_socket,
            2,
            vec![stickers.clone()],
            &["lang"]
        )),
        3
    );
    assert_eq!(
        kv_asked(&mut pair.sara_socket, Some(1)),
        (3, Some(vec![stickers.clone(), theme.clone()]))
    );

    // Names are 1 to 64 of a-z, 0-9, _ and ., each once, and a value fits `kv_value`.
    let long = "a".repeat(65);
    for key in ["Lang", "", "lang-x", long.as_str()] {
        assert_refused(
            kv_change(&mut pair.sara_socket, 3, vec![kv_item(key, b"x")], &[]),
            "bad_body",
        );
    }
    assert_refused(
        kv_change(
            &mut pair.sara_socket,
            3,
            vec![kv_item("lang", b"x"), kv_item("lang", b"y")],
            &[],
        ),
        "bad_body",
    );
    assert_refused(
        kv_change(
            &mut pair.sara_socket,
            3,
            vec![kv_item("lang", b"x")],
            &["lang"],
        ),
        "bad_body",
    );
    assert_refused(
        kv_change(
            &mut pair.sara_socket,
            3,
            vec![kv_item("lang", &[0; 4097])],
            &[],
        ),
        "body_too_large",
    );

    // Each account has its own.
    assert_eq!(
        kv_asked(&mut pair.babak_socket, None),
        (0, Some(Vec::new()))
    );

    // The same over REST.
    let token = Some(pair.sara.token.as_str());
    let (status, reply) = pair.server.call("GET", "/api/v1/kv?version=3", token, None);
    assert_eq!((status, &reply["data"]), (200, &json!({"version": 3})));
    let (status, reply) = pair.server.call("GET", "/api/v1/kv", token, None);
    assert_eq!(status, 200);
    assert_eq!(reply["data"]["kv_list"][1]["key"], "theme");
    let body =
        json!({"version": 3, "set": [serde_json::to_value(&lang).unwrap()], "remove": ["theme"]});
    let (status, reply) = pair.server.call("PATCH", "/api/v1/kv", token, Some(&body));
    assert_eq!((status, &reply["data"]), (200, &json!({"version": 4})));
    second.next_where(|frame| {
        matches!(
            frame,
            ServerFrame::Kv {
                reference: None,
                from: Some(3),
                version: 4,
                ..
            }
        )
    });
    let (status, reply) = pair.server.call("PATCH", "/api/v1/kv", token, Some(&body));
    assert_eq!((status, reply["code"].as_str()), (409, Some("kv_stale")));
    assert_eq!(reply["version"], 4);
    assert_eq!(
        reply["kv_list"],
        json!([
            serde_json::to_value(&lang).unwrap(),
            serde_json::to_value(&stickers).unwrap()
        ])
    );
}

/// The next settings an app shows that `wanted` takes.
fn settings_shown(
    app: &mut App,
    server: &Server,
    wanted: impl Fn(&[Setting]) -> bool,
) -> Vec<Setting> {
    loop {
        let Event::Kv { setting_list } =
            app.wait(server, |event| matches!(event, Event::Kv { .. }))
        else {
            unreachable!();
        };
        if wanted(&setting_list) {
            return setting_list;
        }
    }
}

/// The same, reading `meanwhile` too, whose client has something to send first.
fn settings_shown_beside(
    app: &mut App,
    meanwhile: &mut App,
    server: &Server,
    wanted: impl Fn(&[Setting]) -> bool,
) -> Vec<Setting> {
    let started = std::time::Instant::now();
    loop {
        while let Some(index) = app
            .events
            .iter()
            .position(|event| matches!(event, Event::Kv { .. }))
        {
            let Some(Event::Kv { setting_list }) = app.events.remove(index) else {
                unreachable!();
            };
            if wanted(&setting_list) {
                return setting_list;
            }
        }
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "No settings came"
        );
        meanwhile.read_once(server);
        app.read_once(server);
    }
}

fn setting(key: &str, value: Value) -> Setting {
    Setting {
        key: key.to_string(),
        value,
    }
}

#[test]
fn core_seals_settings_shows_them_on_every_device_and_hands_presence_back() {
    let server = open_server();
    let (mut sara, mut babak, _chat) = apps_on(&server);

    // Presence: the platform says the app is on screen, and the other side's core shows it.
    babak.command(&server, Command::InApp { in_app: true });
    let babak_id = babak.id();
    sara.wait(&server, |event| {
        matches!(event, Event::Presence { presence }
            if presence.account == babak_id && presence.state == Some(PresenceState::Online))
    });
    let stranger = App::sign_up(&server, "mina_x", SARA_WORDS);
    let stranger_id = stranger.id();
    sara.command(
        &server,
        Command::PresenceOf {
            account: stranger_id,
        },
    );
    sara.wait(
        &server,
        |event| matches!(event, Event::Presence { presence } if presence.account == stranger_id),
    );

    // A setting shows at once, and only checked values are taken.
    sara.command(
        &server,
        Command::Kv {
            set: vec![setting("lang", json!("fa"))],
            remove: Vec::new(),
        },
    );
    settings_shown(&mut sara, &server, |list| {
        list == [setting("lang", json!("fa"))]
    });
    for bad in [
        setting("lang", json!("de")),
        setting("theme", json!("blue")),
        setting("recent.stickers", json!(["not an id"])),
        setting("recent.reactions", json!(vec!["👍"; 9])),
        setting("Lang", json!("fa")),
    ] {
        sara.command(
            &server,
            Command::Kv {
                set: vec![bad],
                remove: Vec::new(),
            },
        );
        sara.wait(
            &server,
            |event| matches!(event, Event::Error { code, .. } if code == "bad_setting"),
        );
    }

    // A second device, from the words alone, gets them on connecting, and the server holds them
    // sealed.
    let mut second = App::new();
    second.command(
        &server,
        Command::LogIn {
            username: "sara_x".to_string(),
            password: Secret(PASSWORD.to_string()),
            captcha: None,
        },
    );
    second.wait(&server, |event| {
        matches!(event, Event::NeedWords { restore: true })
    });
    second.command(
        &server,
        Command::Restore {
            words: Secret(SARA_WORDS.to_string()),
        },
    );
    settings_shown(&mut second, &server, |list| {
        list == [setting("lang", json!("fa"))]
    });
    let (status, reply) = server.call("GET", "/api/v1/kv", sara.token.as_deref(), None);
    assert_eq!(status, 200);
    let sealed: KvItem = serde_json::from_value(reply["data"]["kv_list"][0].clone()).unwrap();
    assert_eq!(sealed.key, "lang");
    assert!(!sealed.value.0.windows(2).any(|pair| pair == b"fa"));

    // A change on one device reaches the other live.
    let stickers = json!([Ulid::new().unwrap().to_string()]);
    second.command(
        &server,
        Command::Kv {
            set: vec![
                setting("theme", json!("dark")),
                setting("recent.stickers", stickers.clone()),
            ],
            remove: Vec::new(),
        },
    );
    let both = vec![
        setting("lang", json!("fa")),
        setting("recent.stickers", stickers.clone()),
        setting("theme", json!("dark")),
    ];
    settings_shown(&mut sara, &server, |list| list == both.as_slice());

    // A change made offline goes on reconnecting when nothing changed meanwhile.
    let mut sara = reopen_offline(&server, sara);
    sara.command(
        &server,
        Command::Kv {
            set: Vec::new(),
            remove: vec!["recent.stickers".to_string()],
        },
    );
    let removed = vec![
        setting("lang", json!("fa")),
        setting("theme", json!("dark")),
    ];
    settings_shown(&mut sara, &server, |list| list == removed.as_slice());
    sara.offline = false;
    sara.command(&server, Command::NetworkBack);
    settings_shown_beside(&mut second, &mut sara, &server, |list| {
        list == removed.as_slice()
    });

    // One made offline on a version another device moved on from gives way to the server's.
    let mut sara = reopen_offline(&server, sara);
    sara.command(
        &server,
        Command::Kv {
            set: vec![setting("lang", json!("en"))],
            remove: Vec::new(),
        },
    );
    settings_shown(&mut sara, &server, |list| {
        list.contains(&setting("lang", json!("en")))
    });
    second.command(
        &server,
        Command::Kv {
            set: vec![setting("theme", json!("light"))],
            remove: Vec::new(),
        },
    );
    let server_set = vec![
        setting("lang", json!("fa")),
        setting("theme", json!("light")),
    ];
    settings_shown(&mut second, &server, |list| list == server_set.as_slice());
    sara.offline = false;
    sara.command(&server, Command::NetworkBack);
    settings_shown(&mut sara, &server, |list| list == server_set.as_slice());
    let (status, reply) = server.call("GET", "/api/v1/kv", sara.token.as_deref(), None);
    assert_eq!((status, reply["data"]["version"].as_u64()), (200, Some(4)));
}

/// Sends text and gives back its sequence id once the other side has it.
fn say_to(server: &Server, from: &mut App, to: &mut App, chat: Ulid, text: &str) -> u64 {
    from.command(
        server,
        Command::SendText {
            mentions: Vec::new(),
            thread: None,
            topic: None,
            chat,
            text: Secret(text.to_string()),
            reply: None,
        },
    );
    let Event::Sent { seq, .. } = from.wait(server, |event| matches!(event, Event::Sent { .. }))
    else {
        unreachable!();
    };
    to.wait(server, |event| {
        matches!(event, Event::Message { message } | Event::Edited { message, .. }
            if message.chat == chat && message.seq == seq)
    });
    seq
}

/// A search's hits until its page is done, and whether older rows are left.
fn hits_of(server: &Server, app: &mut App) -> (Vec<Hit>, bool, u64, u64) {
    let mut list = Vec::new();
    loop {
        let Event::MessageHits {
            text: _,
            hit_list,
            done,
            more,
            scanned,
            total,
        } = app.wait(server, |event| matches!(event, Event::MessageHits { .. }))
        else {
            unreachable!();
        };
        list.extend(hit_list);
        if done {
            return (list, more, scanned, total);
        }
    }
}

fn search(server: &Server, app: &mut App, text: &str) -> (Vec<Hit>, bool, u64, u64) {
    app.command(
        server,
        Command::SearchMessages {
            text: Secret(text.to_string()),
        },
    );
    hits_of(server, app)
}

#[test]
fn a_search_opens_the_archive_newest_first_by_newest_text_a_page_at_a_time() {
    let server = open_server();
    let (mut sara, mut babak, chat) = apps_on(&server);

    // Edited text is found by its newest words only, under the message it edits.
    let edited = say_to(&server, &mut sara, &mut babak, chat, "the old apple pie");
    sara.command(
        &server,
        Command::Edit {
            chat,
            target: Pointer::Seq(edited),
            text: Secret("a Banana bread".to_string()),
            files: Vec::new(),
            remove: Vec::new(),
        },
    );
    sara.wait(&server, |event| matches!(event, Event::Sent { .. }));
    babak.wait(
        &server,
        |event| matches!(event, Event::Edited { target_seq, .. } if *target_seq == edited),
    );
    // Arabic yeh and kaf, a ZWNJ and Persian digits, as a Persian keyboard may not type them.
    let persian = say_to(
        &server,
        &mut sara,
        &mut babak,
        chat,
        "سلام علي كو\u{200C}چک ۱۲",
    );
    let gone = say_to(&server, &mut sara, &mut babak, chat, "drop the apple");
    sara.command(
        &server,
        Command::Edit {
            chat,
            target: Pointer::Seq(gone),
            text: Secret("drop the apple now".to_string()),
            files: Vec::new(),
            remove: Vec::new(),
        },
    );
    let Event::Sent { seq: gone_edit, .. } =
        sara.wait(&server, |event| matches!(event, Event::Sent { .. }))
    else {
        unreachable!();
    };
    babak.wait(
        &server,
        |event| matches!(event, Event::Edited { target_seq, .. } if *target_seq == gone),
    );
    sara.command(
        &server,
        Command::Delete {
            chat,
            target: Pointer::Seq(gone),
            remove: Vec::new(),
        },
    );
    sara.wait(&server, |event| matches!(event, Event::Sent { .. }));
    babak.wait(
        &server,
        |event| matches!(event, Event::Deleted { target_seq, .. } if *target_seq == gone),
    );
    // The server answered the delete, so both archives hold the target wiped, with no wait for a sync.
    for app in [&sara, &babak] {
        for seq in [gone, gone_edit] {
            let row = &app.archive[&(chat, seq)];
            assert_eq!((row.kind, row.body.as_ref()), (None, None));
        }
    }
    // A file is found by its name, and a voice note never is.
    sara.files.insert(1, pattern(10, 1));
    sara.files.insert(2, pattern(10, 2));
    sara.command(
        &server,
        Command::SendFiles {
            thread: None,
            topic: None,
            chat,
            files: vec![spec(1, ContentKind::Doc, 10)],
            text: None,
        },
    );
    sara.wait(&server, |event| matches!(event, Event::Sent { .. }));
    sara.command(
        &server,
        Command::SendFiles {
            thread: None,
            topic: None,
            chat,
            files: vec![spec(2, ContentKind::Voice, 10)],
            text: None,
        },
    );
    sara.wait(&server, |event| matches!(event, Event::Sent { .. }));
    let mut pears = Vec::new();
    for n in 1..=23 {
        pears.push(say_to(
            &server,
            &mut sara,
            &mut babak,
            chat,
            &format!("pear {n}"),
        ));
    }

    let (found, more, scanned, total) = search(&server, &mut babak, "BANANA");
    assert_eq!(found.len(), 1);
    assert_eq!((found[0].chat, found[0].seq), (chat, edited));
    assert!(!found[0].mine);
    assert_eq!(found[0].account, Some(sara.id()));
    assert_eq!(
        (
            &found[0].before.0[..],
            &found[0].matched.0[..],
            &found[0].after.0[..]
        ),
        ("a ", "Banana", " bread")
    );
    assert!(!more);
    assert_eq!(scanned, total);
    assert!(search(&server, &mut babak, "apple").0.is_empty());

    let (found, ..) = search(&server, &mut babak, "علی کوچک 12");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].seq, persian);
    assert_eq!(found[0].matched.0, "علي كو\u{200C}چک ۱۲");
    assert_eq!(found[0].before.0, "سلام ");

    let (found, ..) = search(&server, &mut babak, "file-");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].matched.0, "file-");
    assert_eq!(found[0].after.0, "1");

    // The sender finds their own, marked mine.
    let (found, ..) = search(&server, &mut sara, "pear 23");
    assert_eq!(found.len(), 1);
    assert!(found[0].mine);

    // A page is 20 hits, newest first, and the rest come when asked.
    let (first, more, scanned, total) = search(&server, &mut babak, "pear");
    let seqs: Vec<u64> = first.iter().map(|hit| hit.seq).collect();
    let newest: Vec<u64> = pears.iter().rev().take(20).copied().collect();
    assert_eq!(seqs, newest);
    assert!(more);
    assert!(scanned < total);
    babak.command(&server, Command::SearchOlder);
    let (rest, more, scanned, total) = hits_of(&server, &mut babak);
    let seqs: Vec<u64> = rest.iter().map(|hit| hit.seq).collect();
    assert_eq!(seqs, vec![pears[2], pears[1], pears[0]]);
    assert!(!more);
    assert_eq!(scanned, total);
    // Once through, asking for more does nothing, and an empty search looks for nothing.
    babak.command(&server, Command::SearchOlder);
    babak.command(
        &server,
        Command::SearchMessages {
            text: Secret(" \u{200C}".to_string()),
        },
    );
    babak.command(
        &server,
        Command::SearchMessages {
            text: Secret("nothing like this".to_string()),
        },
    );
    let (found, more, ..) = hits_of(&server, &mut babak);
    assert!(found.is_empty());
    assert!(!more);
}

// -------------------------------------------------------------------------------------------------
// Groups and channels
// -------------------------------------------------------------------------------------------------

const MINA_WORDS: &str = "zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo wrong";

/// Waits for a chat other than the self chat to be ready.
fn ready_chat(app: &mut App, server: &Server) -> Ulid {
    let self_chat = app.self_chat;
    let Event::ChatReady { chat } = app.wait(
        server,
        |event| matches!(event, Event::ChatReady { chat } if Some(*chat) != self_chat),
    ) else {
        unreachable!();
    };
    chat
}

fn ready_in(app: &mut App, server: &Server, chat: Ulid) {
    app.wait(
        server,
        |event| matches!(event, Event::ChatReady { chat: ready } if *ready == chat),
    );
}

/// Sara makes a group of this type with these people in it.
fn make_group(
    sara: &mut App,
    server: &Server,
    chat_type: ChatType,
    account_list: Vec<Ulid>,
) -> Ulid {
    sara.command(
        server,
        Command::CreateChat {
            chat_type,
            name: "  Apollo  ".to_string(),
            bio: Some("The moon team".to_string()),
            link_list: Vec::new(),
            account_list: account_list.clone(),
            topics: None,
            findable: false,
            options: ChatOptions::default(),
            photo: None,
            note: None,
        },
    );
    let chat = ready_chat(sara, server);
    if !account_list.is_empty() {
        // The add commit carries the profile sealed for the new people, and a text's `sent` comes
        // after the commit's on the same socket, so the add has landed.
        send_text(sara, server, chat, "made");
    }
    chat
}

/// Sends a text and waits until the server stored it, reading whatever came before its answer.
fn send_text(app: &mut App, server: &Server, chat: Ulid, text: &str) {
    app.command(
        server,
        Command::SendText {
            chat,
            text: Secret(text.to_string()),
            reply: None,
            topic: None,
            thread: None,
            mentions: Vec::new(),
        },
    );
    app.wait(
        server,
        |event| matches!(event, Event::Sent { chat: sent, .. } if *sent == chat),
    );
}

/// Waits for a message with this text in this chat.
fn read_text(app: &mut App, server: &Server, chat: Ulid, text: &str) -> keryx_core::client::Shown {
    shown(app.wait(server, |event| {
        matches!(event, Event::Message { message }
            if message.chat == chat && text_of(message) == Some(text))
    }))
}

fn chat_event_seen(app: &mut App, server: &Server, chat: Ulid, action: ChatAction) -> ChatEvent {
    let Event::ChatEvent { event, .. } = app.wait(
        server,
        |event| matches!(event, Event::ChatEvent { event, .. } if event.chat == chat && event.action == action),
    ) else {
        unreachable!();
    };
    event
}

#[test]
fn a_group_is_made_with_its_people_who_read_its_profile_and_each_other() {
    let server = open_server();
    let mut sara = App::sign_up(&server, "sara_x", SARA_WORDS);
    let mut babak = App::sign_up(&server, "babak_x", BABAK_WORDS);
    let mut mina = App::sign_up(&server, "mina_x", MINA_WORDS);
    let chat = make_group(
        &mut sara,
        &server,
        ChatType::Group,
        vec![babak.id(), mina.id()],
    );
    ready_in(&mut babak, &server, chat);
    ready_in(&mut mina, &server, chat);

    // The profile opens for the new members with its key, which the add shared at its epoch.
    let Event::ChatProfile { profile, .. } = mina.wait(
        &server,
        |event| matches!(event, Event::ChatProfile { chat: shown, .. } if *shown == chat),
    ) else {
        unreachable!();
    };
    assert_eq!(profile.name, "Apollo");
    assert_eq!(profile.bio.as_deref(), Some("The moon team"));

    send_text(&mut sara, &server, chat, "hello all");
    read_text(&mut babak, &server, chat, "hello all");
    read_text(&mut mina, &server, chat, "hello all");
    send_text(&mut mina, &server, chat, "hi from mina");
    read_text(&mut babak, &server, chat, "hi from mina");
    read_text(&mut sara, &server, chat, "hi from mina");
    let added = chat_event_seen(&mut babak, &server, chat, ChatAction::Added);
    assert_eq!(added.actor, sara.id());

    // A mention names members only, and travels inside the encryption.
    sara.command(
        &server,
        Command::SendText {
            chat,
            text: Secret("@mina_x look".to_string()),
            reply: None,
            topic: None,
            thread: None,
            mentions: vec![mina.id()],
        },
    );
    let message = read_text(&mut mina, &server, chat, "@mina_x look");
    assert_eq!(message.mentions, vec![mina.id()]);
    let stranger = Ulid::from_bytes([7; 16]);
    sara.command(
        &server,
        Command::SendText {
            chat,
            text: Secret("@nobody".to_string()),
            reply: None,
            topic: None,
            thread: None,
            mentions: vec![stranger],
        },
    );
    sara.wait(
        &server,
        |event| matches!(event, Event::Error { code, .. } if code == "not_member"),
    );
}

#[test]
fn leaving_is_at_once_and_a_member_takes_the_leaver_out_of_the_group() {
    let server = open_server();
    let mut sara = App::sign_up(&server, "sara_x", SARA_WORDS);
    let mut babak = App::sign_up(&server, "babak_x", BABAK_WORDS);
    let mut mina = App::sign_up(&server, "mina_x", MINA_WORDS);
    let chat = make_group(
        &mut sara,
        &server,
        ChatType::Group,
        vec![babak.id(), mina.id()],
    );
    ready_in(&mut babak, &server, chat);
    ready_in(&mut mina, &server, chat);

    mina.command(&server, Command::Leave { chat });
    mina.wait(
        &server,
        |event| matches!(event, Event::ChatLeft { chat: left } if *left == chat),
    );
    let left = chat_event_seen(&mut sara, &server, chat, ChatAction::Left);
    assert_eq!(left.target, Some(mina.id()));
    chat_event_seen(&mut babak, &server, chat, ChatAction::Left);

    // The group goes on without her: what is sent after reaches the others, on a key she lacks.
    send_text(&mut sara, &server, chat, "after mina");
    read_text(&mut babak, &server, chat, "after mina");

    // Removing someone is a commit by an owner, and the removed account drops the chat.
    sara.command(
        &server,
        Command::RemoveMembers {
            chat,
            account_list: vec![babak.id()],
        },
    );
    babak.wait(
        &server,
        |event| matches!(event, Event::ChatLeft { chat: left } if *left == chat),
    );
    let removed = chat_event_seen(&mut sara, &server, chat, ChatAction::Removed);
    assert_eq!(removed.target, Some(babak.id()));

    // The last owner leaving removes the whole chat.
    sara.command(&server, Command::Leave { chat });
    sara.wait(
        &server,
        |event| matches!(event, Event::ChatLeft { chat: left } if *left == chat),
    );
}

#[test]
fn roles_live_on_the_server_and_only_owners_change_them() {
    let server = open_server();
    let mut sara = App::sign_up(&server, "sara_x", SARA_WORDS);
    let mut babak = App::sign_up(&server, "babak_x", BABAK_WORDS);
    let chat = make_group(&mut sara, &server, ChatType::Group, vec![babak.id()]);
    ready_in(&mut babak, &server, chat);

    sara.command(
        &server,
        Command::SetRole {
            chat,
            account: babak.id(),
            role: ChatRole::Moderator,
        },
    );
    let made = chat_event_seen(&mut babak, &server, chat, ChatAction::MadeModerator);
    assert_eq!(made.target, Some(babak.id()));

    // A moderator may not change roles, and the last owner may not step down.
    babak.command(
        &server,
        Command::SetRole {
            chat,
            account: sara.id(),
            role: ChatRole::Member,
        },
    );
    babak.wait(
        &server,
        |event| matches!(event, Event::Error { code, .. } if code == "no_permission"),
    );
    sara.command(
        &server,
        Command::SetRole {
            chat,
            account: sara.id(),
            role: ChatRole::Member,
        },
    );
    sara.wait(
        &server,
        |event| matches!(event, Event::Error { code, .. } if code == "last_owner"),
    );

    // Over REST too, with the same answers.
    let token = sara.token.clone().unwrap();
    let route = format!("/api/v1/chats/{chat}/members/{}/role", babak.id());
    let (status, _) = raw(&server, "PUT", &route, &token, Some(br#"{"role":"owner"}"#));
    assert_eq!(status, 200);
    chat_event_seen(&mut babak, &server, chat, ChatAction::MadeOwner);
    let route = format!(
        "/api/v1/chats/{chat}/members/{}/role",
        Ulid::from_bytes([7; 16])
    );
    let (status, _) = raw(&server, "PUT", &route, &token, Some(br#"{"role":"owner"}"#));
    assert_eq!(status, 404);
}

/// Waits until `app` is told that `them` reached `seq` in this chat, received or seen. Their client
/// sends one position at a time, the next once the last is answered, so their socket is read too.
fn marked(app: &mut App, them: &mut App, server: &Server, chat: Ulid, seq: u64, seen: bool) {
    let account = them.id();
    let wanted = |event: &Event| {
        matches!(event, Event::Marks { chat: marked, mark } if *marked == chat
            && mark.account == account
            && if seen { mark.seen_seq >= Some(seq) } else { mark.received_seq >= seq })
    };
    let started = std::time::Instant::now();
    while !app.events.iter().any(wanted) {
        assert!(started.elapsed() < Duration::from_secs(20), "No mark came");
        them.read_once(server);
        app.read_once(server);
    }
    app.wait(server, wanted);
}

/// A message's times as this app may see them, by account.
fn times_of(app: &mut App, server: &Server, chat: Ulid, seq: u64) -> Vec<TimesItem> {
    app.command(
        server,
        Command::Times {
            chat,
            seq,
            after: None,
        },
    );
    let Event::Times { member_list, .. } = app.wait(
        server,
        |event| matches!(event, Event::Times { chat: asked, seq: of, .. } if *asked == chat && *of == seq),
    ) else {
        unreachable!();
    };
    member_list
}

fn times_for(member_list: &[TimesItem], account: Ulid) -> (bool, bool) {
    let Some(item) = member_list.iter().find(|item| item.account == account) else {
        panic!("{account} is not listed");
    };
    (item.received_at.is_some(), item.seen_at.is_some())
}

/// Sets the chat's options as `sara`, and waits until `other` is told.
fn options_set(sara: &mut App, other: &mut App, server: &Server, chat: Ulid, options: ChatOptions) {
    sara.command(
        server,
        Command::ChatOptions {
            chat,
            options: options.clone(),
        },
    );
    other.wait(
        server,
        |event| matches!(event, Event::ChatOptions { chat: set, options: got } if *set == chat && *got == options),
    );
}

#[test]
fn times_show_only_what_the_asker_may_see_on_their_own_messages_and_others() {
    let server = open_server();
    let mut sara = App::sign_up(&server, "sara_x", SARA_WORDS);
    let mut babak = App::sign_up(&server, "babak_x", BABAK_WORDS);
    let mut mina = App::sign_up(&server, "mina_x", MINA_WORDS);
    let chat = make_group(
        &mut sara,
        &server,
        ChatType::Group,
        vec![babak.id(), mina.id()],
    );
    ready_in(&mut babak, &server, chat);
    ready_in(&mut mina, &server, chat);
    let first = say_to(&server, &mut sara, &mut babak, chat, "from sara");
    read_text(&mut mina, &server, chat, "from sara");
    let second = say_to(&server, &mut babak, &mut sara, chat, "from babak");
    read_text(&mut mina, &server, chat, "from babak");
    mina.command(
        &server,
        Command::Seen {
            chat,
            up_to_seq: second,
            topic: None,
            thread: None,
        },
    );
    marked(&mut babak, &mut mina, &server, chat, second, true);
    marked(&mut babak, &mut mina, &server, chat, second, false);
    marked(&mut babak, &mut sara, &server, chat, second, false);

    // A plain member sees both times on their own message, and nothing on someone else's.
    let member_list = times_of(&mut babak, &server, chat, second);
    assert_eq!(member_list.len(), 2);
    assert_eq!(times_for(&member_list, mina.id()), (true, true));
    assert_eq!(times_for(&member_list, sara.id()), (true, false));
    assert_eq!(
        refusal(
            &mut babak,
            &server,
            Command::Times {
                chat,
                seq: first,
                after: None
            }
        ),
        "no_permission"
    );

    // An owner sees everyone's, and so does a moderator.
    let member_list = times_of(&mut sara, &server, chat, second);
    assert_eq!(times_for(&member_list, mina.id()), (true, true));
    sara.command(
        &server,
        Command::SetRole {
            chat,
            account: babak.id(),
            role: ChatRole::Moderator,
        },
    );
    chat_event_seen(&mut babak, &server, chat, ChatAction::MadeModerator);
    let member_list = times_of(&mut babak, &server, chat, first);
    assert_eq!(times_for(&member_list, mina.id()), (true, true));

    // Options take a member's own times away one by one, and can hand them others' too.
    let third = say_to(&server, &mut mina, &mut sara, chat, "from mina");
    sara.command(
        &server,
        Command::Seen {
            chat,
            up_to_seq: third,
            topic: None,
            thread: None,
        },
    );
    marked(&mut mina, &mut sara, &server, chat, third, true);
    marked(&mut mina, &mut sara, &server, chat, third, false);
    options_set(
        &mut sara,
        &mut mina,
        &server,
        chat,
        ChatOptions {
            deny: vec!["message.view_seen.own".to_string()],
            ..ChatOptions::default()
        },
    );
    let member_list = times_of(&mut mina, &server, chat, third);
    assert_eq!(times_for(&member_list, sara.id()), (true, false));
    options_set(
        &mut sara,
        &mut mina,
        &server,
        chat,
        ChatOptions {
            deny: vec![
                "message.view_seen.own".to_string(),
                "message.view_received.own".to_string(),
            ],
            ..ChatOptions::default()
        },
    );
    assert_eq!(
        refusal(
            &mut mina,
            &server,
            Command::Times {
                chat,
                seq: third,
                after: None
            }
        ),
        "no_permission"
    );
    options_set(
        &mut sara,
        &mut mina,
        &server,
        chat,
        ChatOptions {
            allow: vec!["message.view_seen.others".to_string()],
            ..ChatOptions::default()
        },
    );
    let member_list = times_of(&mut mina, &server, chat, first);
    assert_eq!(times_for(&member_list, mina.id()), (false, true));
}

/// How the server answered this app's next send or delete in this chat: its seq, or the refusal.
fn answered(app: &mut App, server: &Server, chat: Ulid) -> Result<u64, String> {
    match app.wait(server, |event| {
        matches!(event, Event::Sent { chat: of, .. } | Event::Failed { chat: of, .. } if *of == chat)
    }) {
        Event::Sent { seq, .. } => Ok(seq),
        Event::Failed { code, .. } => Err(code),
        _ => unreachable!(),
    }
}

/// Sends a text, in a thread or not, and says how the server answered.
fn try_text(
    app: &mut App,
    server: &Server,
    chat: Ulid,
    text: &str,
    thread: Option<u64>,
) -> Result<u64, String> {
    app.command(
        server,
        Command::SendText {
            chat,
            text: Secret(text.to_string()),
            reply: None,
            topic: None,
            thread,
            mentions: Vec::new(),
        },
    );
    answered(app, server, chat)
}

fn try_delete(app: &mut App, server: &Server, chat: Ulid, seq: u64) -> Result<u64, String> {
    app.command(
        server,
        Command::Delete {
            chat,
            target: Pointer::Seq(seq),
            remove: Vec::new(),
        },
    );
    answered(app, server, chat)
}

fn try_react(app: &mut App, server: &Server, chat: Ulid, on: u64) -> Result<u64, String> {
    app.command(
        server,
        Command::SendReaction {
            chat,
            emoji: "👍".to_string(),
            on: Pointer::Seq(on),
            topic: None,
            thread: None,
        },
    );
    answered(app, server, chat)
}

fn make_moderator(sara: &mut App, them: &mut App, server: &Server, chat: Ulid) {
    sara.command(
        server,
        Command::SetRole {
            chat,
            account: them.id(),
            role: ChatRole::Moderator,
        },
    );
    chat_event_seen(them, server, chat, ChatAction::MadeModerator);
}

#[test]
fn slow_mode_spaces_a_members_messages_and_moderators_delete_others_but_an_owners() {
    let server = open_server();
    let mut sara = App::sign_up(&server, "sara_x", SARA_WORDS);
    let mut babak = App::sign_up(&server, "babak_x", BABAK_WORDS);
    let mut mina = App::sign_up(&server, "mina_x", MINA_WORDS);
    let chat = make_group(
        &mut sara,
        &server,
        ChatType::Group,
        vec![babak.id(), mina.id()],
    );
    ready_in(&mut babak, &server, chat);
    ready_in(&mut mina, &server, chat);

    // Only the slow modes on the list are taken.
    sara.command(
        &server,
        Command::ChatOptions {
            chat,
            options: ChatOptions {
                slow_mode: Some(20),
                ..ChatOptions::default()
            },
        },
    );
    sara.wait(
        &server,
        |event| matches!(event, Event::Error { code, .. } if code == "bad_body"),
    );
    options_set(
        &mut sara,
        &mut babak,
        &server,
        chat,
        ChatOptions {
            slow_mode: Some(30),
            ..ChatOptions::default()
        },
    );
    let first = try_text(&mut babak, &server, chat, "one", None).unwrap();
    babak.command(
        &server,
        Command::SendText {
            chat,
            text: Secret("two".to_string()),
            reply: None,
            topic: None,
            thread: None,
            mentions: Vec::new(),
        },
    );
    let Event::Failed { code, seconds, .. } =
        babak.wait(&server, |event| matches!(event, Event::Failed { .. }))
    else {
        unreachable!();
    };
    assert_eq!(code, "slow_mode");
    assert!(seconds.is_some_and(|seconds| (1..=30).contains(&seconds)));
    // Threads, reactions and owners are not slowed.
    try_text(&mut babak, &server, chat, "in a thread", Some(first)).unwrap();
    try_react(&mut babak, &server, chat, first).unwrap();
    try_text(&mut sara, &server, chat, "a", None).unwrap();
    try_text(&mut sara, &server, chat, "b", None).unwrap();
    options_set(&mut sara, &mut babak, &server, chat, ChatOptions::default());
    try_text(&mut babak, &server, chat, "three", None).unwrap();

    // A plain member deletes only their own; a moderator deletes others', but never an owner's.
    let from_sara = try_text(&mut sara, &server, chat, "the owner's", None).unwrap();
    let from_mina = try_text(&mut mina, &server, chat, "mina's", None).unwrap();
    assert_eq!(
        try_delete(&mut babak, &server, chat, from_mina),
        Err("no_permission".to_string())
    );
    make_moderator(&mut sara, &mut babak, &server, chat);
    assert_eq!(
        try_delete(&mut babak, &server, chat, from_sara),
        Err("no_permission".to_string())
    );
    try_delete(&mut babak, &server, chat, from_mina).unwrap();
    mina.wait(
        &server,
        |event| matches!(event, Event::Deleted { target_seq, .. } if *target_seq == from_mina),
    );
    // An owner deletes anyone's.
    let from_babak = try_text(&mut babak, &server, chat, "babak's", None).unwrap();
    try_delete(&mut sara, &server, chat, from_babak).unwrap();

    // The history names who deleted what, for owners and moderators alone.
    let deleted = chat_event_seen(&mut sara, &server, chat, ChatAction::Deleted);
    assert_eq!(
        (deleted.actor, deleted.target, deleted.seq, deleted.audience),
        (
            babak.id(),
            Some(mina.id()),
            Some(from_mina),
            Audience::Moderators
        )
    );
    send_text(&mut sara, &server, chat, "after");
    read_text(&mut mina, &server, chat, "after");
    assert!(!mina.events.iter().any(|event| matches!(event,
        Event::ChatEvent { event, .. } if event.action == ChatAction::Deleted)));
}

#[test]
fn slow_mode_counts_each_topic_on_its_own_and_a_topics_zero_is_off() {
    let server = open_server();
    let mut sara = App::sign_up(&server, "sara_x", SARA_WORDS);
    let mut babak = App::sign_up(&server, "babak_x", BABAK_WORDS);
    let chat = make_group(&mut sara, &server, ChatType::Group, vec![babak.id()]);
    ready_in(&mut babak, &server, chat);
    let topic = topic_made(&mut sara, &server, chat, "Launch");
    options_set(
        &mut sara,
        &mut babak,
        &server,
        chat,
        ChatOptions {
            slow_mode: Some(30),
            ..ChatOptions::default()
        },
    );

    // A message in General does not slow the topic, and the topic's own then slows it.
    try_text(&mut babak, &server, chat, "in general", None).unwrap();
    try_topic_text(&mut babak, &server, chat, topic, "in launch", None).unwrap();
    assert_eq!(
        try_topic_text(&mut babak, &server, chat, topic, "again", None),
        Err("slow_mode".to_string())
    );

    // A topic's 0 is off while the group keeps its own.
    topic_options_set(
        &mut sara,
        &mut babak,
        &server,
        chat,
        topic,
        TopicOptionsBody {
            slow_mode: Some(0),
            ..TopicOptionsBody::default()
        },
    );
    try_topic_text(&mut babak, &server, chat, topic, "free", None).unwrap();
    try_topic_text(&mut babak, &server, chat, topic, "still free", None).unwrap();
    assert_eq!(
        try_text(&mut babak, &server, chat, "general again", None),
        Err("slow_mode".to_string())
    );
}

#[test]
fn a_channel_is_written_by_its_posters_and_talked_about_in_one_level_threads() {
    let server = open_server();
    let mut sara = App::sign_up(&server, "sara_x", SARA_WORDS);
    let mut babak = App::sign_up(&server, "babak_x", BABAK_WORDS);
    let mut mina = App::sign_up(&server, "mina_x", MINA_WORDS);
    let chat = make_group(
        &mut sara,
        &server,
        ChatType::Channel,
        vec![babak.id(), mina.id()],
    );
    ready_in(&mut babak, &server, chat);
    ready_in(&mut mina, &server, chat);

    // A subscriber writes in threads, and only reacts in the main list.
    assert_eq!(
        try_text(&mut babak, &server, chat, "main", None),
        Err("no_permission".to_string())
    );
    let post = try_text(&mut sara, &server, chat, "a post", None).unwrap();
    try_react(&mut babak, &server, chat, post).unwrap();
    let comment = try_text(&mut babak, &server, chat, "a comment", Some(post)).unwrap();
    let shown = read_text(&mut mina, &server, chat, "a comment");
    assert_eq!(shown.thread_seq, Some(post));
    assert_eq!(
        try_text(&mut babak, &server, chat, "deeper", Some(comment)),
        Err("bad_message".to_string())
    );
    assert_eq!(
        try_text(&mut babak, &server, chat, "nowhere", Some(999)),
        Err("no_such_message".to_string())
    );

    // Typing shows only inside a thread.
    for thread in [None, Some(post)] {
        mina.command(
            &server,
            Command::Typing {
                chat,
                on: true,
                topic: None,
                thread,
            },
        );
    }
    let Event::Typing {
        account, thread, ..
    } = sara.wait(&server, |event| matches!(event, Event::Typing { .. }))
    else {
        unreachable!();
    };
    assert_eq!((account, thread), (mina.id(), Some(post)));

    // A moderator posts in the main list.
    make_moderator(&mut sara, &mut babak, &server, chat);
    try_text(&mut babak, &server, chat, "a moderator's post", None).unwrap();
}

/// Waits for this account's own join request to show in this state.
fn asked_seen(app: &mut App, server: &Server, chat: Ulid, wanted: RequestState) -> Option<String> {
    let Event::Asked { name, .. } = app.wait(
        server,
        |event| matches!(event, Event::Asked { chat: of, state, .. } if *of == chat && *state == wanted),
    ) else {
        unreachable!();
    };
    name
}

fn found_chats(app: &mut App, server: &Server, text: &str) -> Vec<Ulid> {
    app.command(
        server,
        Command::Search {
            text: text.to_string(),
        },
    );
    let Event::SearchResults { chat_list, .. } =
        app.wait(server, |event| matches!(event, Event::SearchResults { .. }))
    else {
        unreachable!();
    };
    chat_list.into_iter().map(|found| found.chat).collect()
}

#[test]
fn a_join_request_is_declined_or_taken_back_and_its_asker_is_told_on_every_device() {
    let server = open_server();
    let mut sara = App::sign_up(&server, "sara_x", SARA_WORDS);
    let mut babak = App::sign_up(&server, "babak_x", BABAK_WORDS);
    let mut mina = App::sign_up(&server, "mina_x", MINA_WORDS);
    let chat = make_group(&mut sara, &server, ChatType::Group, vec![babak.id()]);
    ready_in(&mut babak, &server, chat);

    // Hidden, it is neither listed nor asked into.
    assert!(found_chats(&mut mina, &server, "apol").is_empty());
    assert_eq!(
        refusal(&mut mina, &server, Command::AskToJoin { chat, note: None }),
        "not_findable"
    );
    let findable = || Command::Findable {
        chat,
        findable: true,
        name: Some("Apollo".to_string()),
        bio: None,
    };
    assert_eq!(refusal(&mut babak, &server, findable()), "no_permission");
    sara.command(&server, findable());
    // The socket answers in order, so the text's `sent` comes after the findable's `done`.
    send_text(&mut sara, &server, chat, "findable now");
    assert_eq!(found_chats(&mut mina, &server, "apol"), vec![chat]);

    // The asking device is told too, with the chat's findable name.
    mina.command(
        &server,
        Command::AskToJoin {
            chat,
            note: Some("hi".to_string()),
        },
    );
    let name = asked_seen(&mut mina, &server, chat, RequestState::Waiting);
    assert_eq!(name.as_deref(), Some("Apollo"));
    sara.wait(
        &server,
        |event| matches!(event, Event::JoinRequest { request } if request.state == RequestState::Waiting),
    );
    assert_eq!(
        refusal(
            &mut babak,
            &server,
            Command::DeclineAsk {
                chat,
                account: mina.id()
            }
        ),
        "no_permission"
    );
    assert_eq!(
        refusal(&mut babak, &server, Command::Requests { chat }),
        "no_permission"
    );

    // Taken back, it goes from both sides, and cannot then be declined.
    mina.command(&server, Command::CancelAsk { chat });
    asked_seen(&mut mina, &server, chat, RequestState::Gone);
    sara.wait(
        &server,
        |event| matches!(event, Event::JoinRequest { request } if request.state == RequestState::Gone),
    );
    assert_eq!(
        refusal(
            &mut sara,
            &server,
            Command::DeclineAsk {
                chat,
                account: mina.id()
            }
        ),
        "no_request"
    );
    assert_eq!(
        refusal(&mut mina, &server, Command::CancelAsk { chat }),
        "no_request"
    );

    // Declined, the asker is told and may ask again.
    mina.command(&server, Command::AskToJoin { chat, note: None });
    asked_seen(&mut mina, &server, chat, RequestState::Waiting);
    sara.wait(
        &server,
        |event| matches!(event, Event::JoinRequest { request } if request.state == RequestState::Waiting),
    );
    sara.command(
        &server,
        Command::DeclineAsk {
            chat,
            account: mina.id(),
        },
    );
    asked_seen(&mut mina, &server, chat, RequestState::Declined);
    mina.command(&server, Command::AskToJoin { chat, note: None });
    asked_seen(&mut mina, &server, chat, RequestState::Waiting);

    // Every member reads the member list with roles.
    babak.command(&server, Command::Members { chat });
    let Event::Members { member_list, .. } =
        babak.wait(&server, |event| matches!(event, Event::Members { .. }))
    else {
        unreachable!();
    };
    let mut roles: Vec<_> = member_list
        .iter()
        .map(|member| (member.account, member.role))
        .collect();
    roles.sort_by_key(|(account, _)| *account);
    let mut wanted = vec![(sara.id(), ChatRole::Owner), (babak.id(), ChatRole::Member)];
    wanted.sort_by_key(|(account, _)| *account);
    assert_eq!(roles, wanted);
    assert_eq!(
        refusal(&mut mina, &server, Command::Members { chat }),
        "not_member"
    );

    // Hidden again, it leaves the directory.
    sara.command(
        &server,
        Command::Findable {
            chat,
            findable: false,
            name: None,
            bio: None,
        },
    );
    send_text(&mut sara, &server, chat, "hidden now");
    assert!(found_chats(&mut mina, &server, "apol").is_empty());

    // Only an owner removes the whole chat, and it goes for everyone.
    assert_eq!(
        refusal(&mut babak, &server, Command::RemoveChat { chat }),
        "no_permission"
    );
    sara.command(&server, Command::RemoveChat { chat });
    for app in [&mut sara, &mut babak] {
        app.wait(
            &server,
            |event| matches!(event, Event::ChatLeft { chat: left } if *left == chat),
        );
    }
    assert_eq!(
        refusal(&mut babak, &server, Command::Members { chat }),
        "not_member"
    );
}

/// This account lets nobody add it to a group straight in, so being added becomes an invitation.
fn invitation_only(app: &mut App, server: &Server) {
    app.command(
        server,
        Command::Kv {
            set: vec![setting("privacy.add.group", json!("nobody"))],
            remove: Vec::new(),
        },
    );
    settings_shown(app, server, |list| {
        list.iter().any(|item| item.key == "privacy.add.group")
    });
}

/// Waits for this account's invitation to the chat.
fn invited(app: &mut App, server: &Server, chat: Ulid) -> InvitationShown {
    let Event::Invitation { invitation, .. } = app.wait(
        server,
        |event| matches!(event, Event::Invitation { chat: invited, invitation: Some(_) } if *invited == chat),
    ) else {
        unreachable!();
    };
    invitation.unwrap()
}

#[test]
fn someone_added_by_invitation_only_is_invited_and_joins_by_itself() {
    let server = open_server();
    let mut sara = App::sign_up(&server, "sara_x", SARA_WORDS);
    let mut babak = App::sign_up(&server, "babak_x", BABAK_WORDS);
    let mut mina = App::sign_up(&server, "mina_x", MINA_WORDS);
    invitation_only(&mut mina, &server);

    // Babak goes straight in, and Mina is invited instead.
    let chat = make_group(
        &mut sara,
        &server,
        ChatType::Group,
        vec![babak.id(), mina.id()],
    );
    ready_in(&mut babak, &server, chat);
    let Event::Invitation { invitation, .. } = mina.wait(
        &server,
        |event| matches!(event, Event::Invitation { chat: invited, invitation: Some(_) } if *invited == chat),
    ) else {
        unreachable!();
    };
    // The group's name was sealed to Mina's invite key, so she reads it before joining.
    let invitation = invitation.unwrap();
    assert_eq!(invitation.inviter, sara.id());
    assert_eq!(invitation.chat_type, ChatType::Group);
    assert_eq!(invitation.name.as_deref(), Some("Apollo"));
    assert_eq!(invitation.blurhash, None);
    assert_eq!(invitation.note, None);

    // Accepting joins by external commit, carrying the inviter's signature, which members check.
    mina.command(&server, Command::AnswerInvite { chat, accept: true });
    ready_in(&mut mina, &server, chat);
    // With nobody else running, the profile opens with the key the invitation carried.
    let Event::ChatProfile { profile, .. } = mina.wait(
        &server,
        |event| matches!(event, Event::ChatProfile { chat: shown, .. } if *shown == chat),
    ) else {
        unreachable!();
    };
    assert_eq!(profile.name, "Apollo");
    assert_eq!(profile.bio.as_deref(), Some("The moon team"));
    // Reading Mina's text takes her join first, so Sara's answer is sealed under a key she holds.
    send_text(&mut mina, &server, chat, "thanks");
    read_text(&mut babak, &server, chat, "thanks");
    read_text(&mut sara, &server, chat, "thanks");
    send_text(&mut sara, &server, chat, "welcome mina");
    read_text(&mut mina, &server, chat, "welcome mina");

    // A change after her join reaches her too.
    sara.command(
        &server,
        Command::ChatProfile {
            chat,
            name: "Artemis".to_string(),
            bio: None,
            link_list: Vec::new(),
            rules: None,
        },
    );
    let Event::ChatProfile { profile, .. } = mina.wait(
        &server,
        |event| matches!(event, Event::ChatProfile { profile, .. } if profile.name == "Artemis"),
    ) else {
        unreachable!();
    };
    assert_eq!(profile.bio, None);
    // The device that saved it gets the server's copy too, with its new version.
    let own = chat_profile_seen(&mut sara, &server, chat, |profile| {
        profile.name == "Artemis"
    });
    assert_eq!(own.version, profile.version);
}

/// A group's photo of `size` bytes, which core seals without decoding, with a flat thumbnail.
fn group_photo(size: usize, seed: u8) -> PhotoInput {
    PhotoInput {
        jpeg: pattern(size, seed),
        thumbnail: Thumbnail {
            width: 2,
            height: 2,
            rgba: vec![seed; 16],
        },
    }
}

/// Sara makes a group with Babak in it, with this photo.
fn group_with_photo(sara: &mut App, server: &Server, babak: Ulid, photo: PhotoInput) {
    sara.command(
        server,
        Command::CreateChat {
            chat_type: ChatType::Group,
            name: "Apollo".to_string(),
            bio: None,
            link_list: Vec::new(),
            account_list: vec![babak],
            topics: None,
            findable: false,
            options: ChatOptions::default(),
            photo: Some(photo),
            note: None,
        },
    );
}

fn chat_profile_seen(
    app: &mut App,
    server: &Server,
    chat: Ulid,
    wanted: impl Fn(&ShownChatProfile) -> bool,
) -> ShownChatProfile {
    let Event::ChatProfile { profile, .. } = app.wait(
        server,
        |event| matches!(event, Event::ChatProfile { chat: shown, profile } if *shown == chat && wanted(profile)),
    ) else {
        unreachable!();
    };
    profile
}

fn chat_photo_bytes(app: &mut App, server: &Server, chat: Ulid) -> (Ulid, Vec<u8>) {
    app.command(server, Command::ChatPhotoGet { chat });
    let Event::ChatPhoto { photo, bytes, .. } = app.wait(
        server,
        |event| matches!(event, Event::ChatPhoto { chat: shown, .. } if *shown == chat),
    ) else {
        unreachable!();
    };
    (photo, bytes.0)
}

#[test]
fn a_group_photo_goes_up_before_its_people_and_a_failed_one_leaves_the_group_without_it() {
    let server = open_server();
    let mut sara = App::sign_up(&server, "sara_x", SARA_WORDS);
    let mut babak = App::sign_up(&server, "babak_x", BABAK_WORDS);

    // The photo went up before Babak was added, so the first profile he opens names it.
    let first = group_photo(3000, 1);
    group_with_photo(&mut sara, &server, babak.id(), first.clone());
    let chat = ready_chat(&mut sara, &server);
    ready_in(&mut babak, &server, chat);
    let profile = chat_profile_seen(&mut babak, &server, chat, |profile| profile.photo.is_some());
    let shown = profile.photo.unwrap();
    assert!(!shown.blurhash.is_empty());
    let (photo, bytes) = chat_photo_bytes(&mut babak, &server, chat);
    assert_eq!(photo, shown.id);
    assert_eq!(bytes, first.jpeg);

    // A new one replaces it for everyone, and the old one's bytes go.
    let second = group_photo(5000, 2);
    sara.command(
        &server,
        Command::ChatPhoto {
            chat,
            jpeg: second.jpeg.clone(),
            thumbnail: second.thumbnail,
        },
    );
    let profile = chat_profile_seen(&mut babak, &server, chat, |profile| {
        profile
            .photo
            .as_ref()
            .is_some_and(|photo| photo.id != shown.id)
    });
    assert_eq!(profile.name, "Apollo");
    let (_, bytes) = chat_photo_bytes(&mut babak, &server, chat);
    assert_eq!(bytes, second.jpeg);
    let token = babak.token.clone().unwrap();
    let route = format!("/api/v1/files/{}/chunks/0", shown.id);
    let (status, reply) = raw(&server, "GET", &route, &token, None);
    assert_eq!((status, code_of(&reply).as_str()), (404, "no_such_file"));

    // Larger than one chunk is refused by core, and the server refuses a sealed photo over
    // `[limits.upload] photo`, which a whole chunk is once sealed.
    let chunk = usize::try_from(file::CHUNK).unwrap();
    let too_large = group_photo(chunk + 1, 3);
    let refused = refusal(
        &mut sara,
        &server,
        Command::ChatPhoto {
            chat,
            jpeg: too_large.jpeg,
            thumbnail: too_large.thumbnail,
        },
    );
    assert_eq!(refused, "bad_files");
    let over_limit = group_photo(chunk, 4);
    let refused = refusal(
        &mut sara,
        &server,
        Command::ChatPhoto {
            chat,
            jpeg: over_limit.jpeg,
            thumbnail: over_limit.thumbnail,
        },
    );
    assert_eq!(refused, "no_such_file");

    // A photo that fails while a group is made, in core or on the server, is only reported: the
    // group is still made, and its people read a profile without one.
    let self_chat = sara.self_chat;
    let mut made_list = vec![chat];
    for (size, seed) in [(chunk + 1, 5), (chunk, 6)] {
        group_with_photo(&mut sara, &server, babak.id(), group_photo(size, seed));
        sara.wait(
            &server,
            |event| matches!(event, Event::Error { code, .. } if code == "photo_failed"),
        );
        let Event::ChatReady { chat: made } = sara.wait(
            &server,
            |event| matches!(event, Event::ChatReady { chat } if !made_list.contains(chat) && Some(*chat) != self_chat),
        ) else {
            unreachable!();
        };
        made_list.push(made);
        ready_in(&mut babak, &server, made);
        let profile = chat_profile_seen(&mut babak, &server, made, |_| true);
        assert_eq!(profile.name, "Apollo");
        assert_eq!(profile.photo, None);
    }
}

#[test]
fn a_findable_group_is_asked_to_join_and_its_asker_let_in() {
    let server = open_server();
    let mut sara = App::sign_up(&server, "sara_x", SARA_WORDS);
    let mut mina = App::sign_up(&server, "mina_x", MINA_WORDS);
    sara.command(
        &server,
        Command::CreateChat {
            chat_type: ChatType::Channel,
            name: "News".to_string(),
            bio: None,
            link_list: Vec::new(),
            account_list: Vec::new(),
            topics: None,
            findable: true,
            options: ChatOptions::default(),
            photo: None,
            note: None,
        },
    );
    let chat = ready_chat(&mut sara, &server);

    // Findable, its name is in the directory beside accounts.
    mina.command(
        &server,
        Command::Search {
            text: "new".to_string(),
        },
    );
    let Event::SearchResults { chat_list, .. } = mina.wait(&server, |event| {
        matches!(event, Event::SearchResults { .. })
    }) else {
        unreachable!();
    };
    assert_eq!(chat_list.len(), 1);
    assert_eq!(chat_list[0].chat, chat);
    assert_eq!(chat_list[0].name, "News");
    assert_eq!(chat_list[0].chat_type, ChatType::Channel);

    mina.command(
        &server,
        Command::AskToJoin {
            chat,
            note: Some("Let me read".to_string()),
        },
    );
    let Event::JoinRequest { request } = sara.wait(
        &server,
        |event| matches!(event, Event::JoinRequest { request } if request.chat == chat),
    ) else {
        unreachable!();
    };
    assert_eq!(request.account, mina.id());
    assert_eq!(request.note.as_deref(), Some("Let me read"));
    sara.command(&server, Command::Requests { chat });
    let Event::Requests { request_list, .. } =
        sara.wait(&server, |event| matches!(event, Event::Requests { .. }))
    else {
        unreachable!();
    };
    assert_eq!(request_list.len(), 1);

    sara.command(
        &server,
        Command::LetIn {
            chat,
            account: mina.id(),
        },
    );
    ready_in(&mut mina, &server, chat);
    let let_in = chat_event_seen(&mut sara, &server, chat, ChatAction::LetIn);
    assert_eq!(let_in.target, Some(mina.id()));
    send_text(&mut sara, &server, chat, "first post");
    read_text(&mut mina, &server, chat, "first post");

    // Hiding it, and listing it again under another name, reach its members live.
    for (findable, name) in [(false, None), (true, Some("Moon news"))] {
        sara.command(
            &server,
            Command::Findable {
                chat,
                findable,
                name: name.map(str::to_string),
                bio: None,
            },
        );
        let Event::Findable {
            findable: shown, ..
        } = mina.wait(
            &server,
            |event| matches!(event, Event::Findable { chat: of, .. } if *of == chat),
        )
        else {
            unreachable!();
        };
        assert_eq!(shown.map(|item| item.name).as_deref(), name);
    }
}

/// This account's waiting join requests and invitations, from the first page of its chats list.
fn asked_and_invited(server: &Server, token: &str) -> (Vec<RequestItem>, Vec<InvitationItem>) {
    let mut socket = connect(server, token);
    let wanted = reference();
    let ServerFrame::Chats {
        asked_list,
        invitation_list,
        ..
    } = socket.request(&sync_frame(wanted), wanted)
    else {
        panic!("Wanted the chats");
    };
    (asked_list, invitation_list)
}

#[test]
fn removing_a_chat_takes_its_join_requests_and_invitations_from_those_waiting() {
    let server = open_server();
    let mut sara = App::sign_up(&server, "sara_x", SARA_WORDS);
    let mut babak = App::sign_up(&server, "babak_x", BABAK_WORDS);
    let mut mina = App::sign_up(&server, "mina_x", MINA_WORDS);
    invitation_only(&mut mina, &server);
    sara.command(
        &server,
        Command::CreateChat {
            chat_type: ChatType::Group,
            name: "Apollo".to_string(),
            bio: None,
            link_list: Vec::new(),
            account_list: vec![mina.id()],
            topics: None,
            findable: true,
            options: ChatOptions::default(),
            photo: None,
            note: None,
        },
    );
    let chat = ready_chat(&mut sara, &server);
    mina.wait(
        &server,
        |event| matches!(event, Event::Invitation { chat: invited, invitation: Some(_) } if *invited == chat),
    );
    babak.command(&server, Command::AskToJoin { chat, note: None });
    sara.wait(
        &server,
        |event| matches!(event, Event::JoinRequest { request } if request.chat == chat),
    );

    // Removing the chat tells the one asking and the one invited live, and their lists lose them.
    sara.command(&server, Command::RemoveChat { chat });
    babak.wait(
        &server,
        |event| matches!(event, Event::Asked { chat: of, state: RequestState::Gone, .. } if *of == chat),
    );
    mina.wait(
        &server,
        |event| matches!(event, Event::Invitation { chat: of, invitation: None } if *of == chat),
    );
    // A socket of their own replaces each app's, so the lists are read only once the pushes came.
    assert!(
        asked_and_invited(&server, &babak.token.clone().unwrap())
            .0
            .is_empty()
    );
    assert!(
        asked_and_invited(&server, &mina.token.clone().unwrap())
            .1
            .is_empty()
    );
}

#[test]
fn declining_an_invitation_tells_the_inviter_alone_and_keeps_the_chat_closed() {
    let server = open_server();
    let mut sara = App::sign_up(&server, "sara_x", SARA_WORDS);
    let mut babak = App::sign_up(&server, "babak_x", BABAK_WORDS);
    let mut mina = App::sign_up(&server, "mina_x", MINA_WORDS);
    invitation_only(&mut mina, &server);
    let chat = make_group(
        &mut sara,
        &server,
        ChatType::Group,
        vec![babak.id(), mina.id()],
    );
    ready_in(&mut babak, &server, chat);
    invited(&mut mina, &server, chat);

    // The declining device drops it, and the inviter alone gets a line.
    mina.command(
        &server,
        Command::AnswerInvite {
            chat,
            accept: false,
        },
    );
    mina.wait(
        &server,
        |event| matches!(event, Event::Invitation { chat: of, invitation: None } if *of == chat),
    );
    let line = chat_event_seen(&mut sara, &server, chat, ChatAction::InvitationDeclined);
    assert_eq!(
        (line.actor, line.target, line.audience),
        (mina.id(), Some(mina.id()), Audience::One)
    );
    send_text(&mut sara, &server, chat, "after");
    read_text(&mut babak, &server, chat, "after");
    let declined = |event: &ChatEvent| event.action == ChatAction::InvitationDeclined;
    assert!(!babak.archive_events.values().any(declined));
    assert!(!babak.events.iter().any(|event| matches!(event,
        Event::ChatEvent { event, .. } if declined(event))));

    // Declined, it is gone: it cannot be accepted after all, and Mina is not in.
    assert_eq!(
        refusal(
            &mut mina,
            &server,
            Command::AnswerInvite { chat, accept: true }
        ),
        "no_invitation"
    );
    assert_eq!(
        refusal(&mut mina, &server, Command::Members { chat }),
        "not_member"
    );

    // She can be invited again.
    sara.command(
        &server,
        Command::AddMembers {
            chat,
            account_list: vec![mina.id()],
            note: None,
        },
    );
    invited(&mut mina, &server, chat);
}

#[test]
fn an_invitation_and_a_join_request_expire_from_both_sides_and_can_come_again() {
    let server = Server::start(
        "",
        "[registration]\nallow = true\n[limits]\ninvitation_lifetime = \"2s\"\n",
    );
    let lifetime = Duration::from_millis(2500);
    let mut sara = App::sign_up(&server, "sara_x", SARA_WORDS);
    let mut babak = App::sign_up(&server, "babak_x", BABAK_WORDS);
    let mut mina = App::sign_up(&server, "mina_x", MINA_WORDS);
    invitation_only(&mut mina, &server);
    sara.command(
        &server,
        Command::CreateChat {
            chat_type: ChatType::Group,
            name: "Apollo".to_string(),
            bio: None,
            link_list: Vec::new(),
            account_list: vec![mina.id()],
            topics: None,
            findable: true,
            options: ChatOptions::default(),
            photo: None,
            note: None,
        },
    );
    let chat = ready_chat(&mut sara, &server);
    invited(&mut mina, &server, chat);
    babak.command(&server, Command::AskToJoin { chat, note: None });
    asked_seen(&mut babak, &server, chat, RequestState::Waiting);
    sara.wait(
        &server,
        |event| matches!(event, Event::JoinRequest { request } if request.chat == chat),
    );
    let requests = |sara: &mut App| {
        sara.command(&server, Command::Requests { chat });
        let Event::Requests { request_list, .. } =
            sara.wait(&server, |event| matches!(event, Event::Requests { .. }))
        else {
            unreachable!();
        };
        request_list
    };

    // Past its lifetime, the chat no longer lists the request, and the invitation is not there to
    // accept or in the invited account's list.
    std::thread::sleep(lifetime);
    assert!(requests(&mut sara).is_empty());
    assert_eq!(
        refusal(
            &mut sara,
            &server,
            Command::DeclineAsk {
                chat,
                account: babak.id()
            }
        ),
        "no_request"
    );
    assert_eq!(
        refusal(
            &mut mina,
            &server,
            Command::AnswerInvite { chat, accept: true }
        ),
        "no_invitation"
    );
    // A socket of its own replaces the app's, which reconnects, and core shows the invitation gone,
    // as the new list leaves it out.
    assert!(
        asked_and_invited(&server, &mina.token.clone().unwrap())
            .1
            .is_empty()
    );
    mina.wait(
        &server,
        |event| matches!(event, Event::Invitation { chat: of, invitation: None } if *of == chat),
    );

    // Invited again, it is as new and can be accepted.
    sara.command(
        &server,
        Command::AddMembers {
            chat,
            account_list: vec![mina.id()],
            note: None,
        },
    );
    invited(&mut mina, &server, chat);
    mina.command(&server, Command::AnswerInvite { chat, accept: true });
    ready_in(&mut mina, &server, chat);

    // Asked again, it waits as new, then goes from the asker's own list once it expires too.
    babak.command(&server, Command::AskToJoin { chat, note: None });
    asked_seen(&mut babak, &server, chat, RequestState::Waiting);
    assert_eq!(requests(&mut sara).len(), 1);
    std::thread::sleep(lifetime);
    assert!(requests(&mut sara).is_empty());
    assert!(
        asked_and_invited(&server, &babak.token.clone().unwrap())
            .0
            .is_empty()
    );
    asked_seen(&mut babak, &server, chat, RequestState::Gone);
}

#[test]
fn topics_are_named_in_the_sealed_profile_and_a_message_carries_its_topic() {
    let server = open_server();
    let mut sara = App::sign_up(&server, "sara_x", SARA_WORDS);
    let mut babak = App::sign_up(&server, "babak_x", BABAK_WORDS);
    let chat = make_group(&mut sara, &server, ChatType::Group, vec![babak.id()]);
    ready_in(&mut babak, &server, chat);

    sara.command(&server, Command::TopicId { chat });
    let Event::TopicId { topic, .. } =
        sara.wait(&server, |event| matches!(event, Event::TopicId { .. }))
    else {
        unreachable!();
    };
    sara.command(
        &server,
        Command::Topics {
            chat,
            on: true,
            topic_list: vec![TopicName {
                id: topic,
                name: " Launch ".to_string(),
                place: 0,
            }],
        },
    );
    let Event::ChatProfile { profile, .. } = babak.wait(
        &server,
        |event| matches!(event, Event::ChatProfile { profile, .. } if !profile.topic_list.is_empty()),
    ) else {
        unreachable!();
    };
    assert_eq!(profile.topic_list[0].name, "Launch");

    sara.command(
        &server,
        Command::SendText {
            chat,
            text: Secret("in launch".to_string()),
            reply: None,
            topic: Some(topic),
            thread: None,
            mentions: Vec::new(),
        },
    );
    let message = read_text(&mut babak, &server, chat, "in launch");
    assert_eq!(message.topic, Some(topic));

    // A thread under that message, and seen positions in the topic and the thread.
    babak.command(
        &server,
        Command::SendText {
            chat,
            text: Secret("a reply in the thread".to_string()),
            reply: None,
            topic: Some(topic),
            thread: Some(message.seq),
            mentions: Vec::new(),
        },
    );
    let reply = read_text(&mut sara, &server, chat, "a reply in the thread");
    assert_eq!(reply.thread_seq, Some(message.seq));
    sara.command(
        &server,
        Command::Seen {
            chat,
            up_to_seq: reply.seq,
            topic: Some(topic),
            thread: Some(message.seq),
        },
    );
    send_text(&mut sara, &server, chat, "in general");
    read_text(&mut babak, &server, chat, "in general");
}

/// A topic of this chat, named, with topics on.
fn topic_made(sara: &mut App, server: &Server, chat: Ulid, name: &str) -> Ulid {
    sara.command(server, Command::TopicId { chat });
    let Event::TopicId { topic, .. } =
        sara.wait(server, |event| matches!(event, Event::TopicId { .. }))
    else {
        unreachable!();
    };
    sara.command(
        server,
        Command::Topics {
            chat,
            on: true,
            topic_list: vec![TopicName {
                id: topic,
                name: name.to_string(),
                place: 0,
            }],
        },
    );
    chat_event_seen(sara, server, chat, ChatAction::TopicAdded);
    topic
}

#[test]
fn removing_a_topic_wipes_its_messages_and_threads_and_later_sends_into_it_are_refused() {
    let server = open_server();
    let mut sara = App::sign_up(&server, "sara_x", SARA_WORDS);
    let mut babak = App::sign_up(&server, "babak_x", BABAK_WORDS);
    let chat = make_group(&mut sara, &server, ChatType::Group, vec![babak.id()]);
    ready_in(&mut babak, &server, chat);
    let topic = topic_made(&mut sara, &server, chat, "Launch");

    sara.command(
        &server,
        Command::SendText {
            chat,
            text: Secret("in launch".to_string()),
            reply: None,
            topic: Some(topic),
            thread: None,
            mentions: Vec::new(),
        },
    );
    let post = read_text(&mut babak, &server, chat, "in launch");
    babak.command(
        &server,
        Command::SendText {
            chat,
            text: Secret("under it".to_string()),
            reply: None,
            topic: Some(topic),
            thread: Some(post.seq),
            mentions: Vec::new(),
        },
    );
    let comment = read_text(&mut sara, &server, chat, "under it");
    send_text(&mut sara, &server, chat, "in general");
    read_text(&mut babak, &server, chat, "in general");

    // A plain member may not change topics, and an id the server never issued is refused.
    let route = format!("/api/v1/chats/{chat}/topics");
    let body = json!({"on": true, "topic_list": []}).to_string();
    let babak_token = babak.token.clone().unwrap();
    let (status, reply) = raw(&server, "PUT", &route, &babak_token, Some(body.as_bytes()));
    assert_eq!((status, code_of(&reply).as_str()), (403, "no_permission"));
    let sara_token = sara.token.clone().unwrap();
    let body = json!({"on": true, "topic_list": [topic, Ulid::from_bytes([7; 16])]}).to_string();
    let (status, reply) = raw(&server, "PUT", &route, &sara_token, Some(body.as_bytes()));
    assert_eq!((status, code_of(&reply).as_str()), (404, "unknown_topic"));

    // Leaving it out removes it for everyone, and the history says so to owners and moderators.
    sara.command(
        &server,
        Command::Topics {
            chat,
            on: true,
            topic_list: Vec::new(),
        },
    );
    babak.wait(
        &server,
        |event| matches!(event, Event::Topics { chat: of, on: true, topic_list } if *of == chat && topic_list.is_empty()),
    );
    let removed = chat_event_seen(&mut sara, &server, chat, ChatAction::TopicRemoved);
    assert_eq!(removed.target, Some(topic));
    assert_eq!(removed.audience, Audience::Moderators);

    // No delete is sent for them, so both online devices sync and drop them from their archives at
    // once: Babak's on the pushed list, and Sara's, which made the change, once it was taken.
    for app in [&mut babak, &mut sara] {
        archive_wiped(app, &server, chat, post.seq);
        archive_wiped(app, &server, chat, comment.seq);
    }
    // A topic's events name the topic as their target, which is no account to ask about.
    for app in [&mut babak, &mut sara] {
        app.read_once(&server);
        assert!(
            !app.events.iter().any(
                |event| matches!(event, Event::Error { code, .. } if code == "no_such_account")
            ),
            "A topic was asked for as an account"
        );
    }

    // Sending into it now is refused, in the main list and in a thread. His thread comment's `sent`
    // was never waited for, so it goes first.
    babak.events.clear();
    babak.command(
        &server,
        Command::SendText {
            chat,
            text: Secret("too late".to_string()),
            reply: None,
            topic: Some(topic),
            thread: None,
            mentions: Vec::new(),
        },
    );
    assert_eq!(
        answered(&mut babak, &server, chat),
        Err("topic_gone".to_string())
    );
    babak.command(
        &server,
        Command::SendText {
            chat,
            text: Secret("too late here too".to_string()),
            reply: None,
            topic: Some(topic),
            thread: Some(post.seq),
            mentions: Vec::new(),
        },
    );
    // Its post was wiped with it, so a thread under it has nothing to hang from.
    assert_eq!(
        answered(&mut babak, &server, chat),
        Err("no_such_message".to_string())
    );
    send_text(&mut babak, &server, chat, "general still works");
    read_text(&mut sara, &server, chat, "general still works");

    // Its post and the thread under it keep only their seq, and General keeps its text.
    let mut socket = connect(&server, &sara_token);
    let (message_list, _) = sync_chat(&mut socket, chat);
    let wiped = |seq: u64| {
        let message = message_list
            .iter()
            .find(|message| message.seq == seq)
            .unwrap();
        message.body.is_none() && message.signature.is_none() && message.topic.is_none()
    };
    assert!(wiped(post.seq));
    assert!(wiped(comment.seq));
    assert!(
        message_list
            .iter()
            .any(|message| message.body.is_some() && message.topic.is_none())
    );
}

/// Reads until this app's archive holds the message at `seq` wiped to its seq.
fn archive_wiped(app: &mut App, server: &Server, chat: Ulid, seq: u64) {
    let started = std::time::Instant::now();
    while app
        .archive
        .get(&(chat, seq))
        .is_none_or(|message| message.body.is_some())
    {
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "The archive still holds {seq}"
        );
        app.read_once(server);
    }
}

/// Another device of this account, from the words alone.
fn second_device(server: &Server, username: &str, words: &str) -> App {
    let mut second = App::new();
    second.command(
        server,
        Command::LogIn {
            username: username.to_string(),
            password: Secret(PASSWORD.to_string()),
            captcha: None,
        },
    );
    second.wait(server, |event| {
        matches!(event, Event::NeedWords { restore: true })
    });
    second.command(
        server,
        Command::Restore {
            words: Secret(words.to_string()),
        },
    );
    second
}

/// The chat's events this app's archive holds, by action.
fn archived_actions(app: &App, chat: Ulid) -> Vec<ChatAction> {
    app.archive_events
        .range((chat, 0)..=(chat, u64::MAX))
        .map(|(_, event)| event.action)
        .collect()
}

#[test]
fn a_new_device_reads_the_chat_events_its_account_may_see_through_sync() {
    let server = open_server();
    let mut sara = App::sign_up(&server, "sara_x", SARA_WORDS);
    let mut babak = App::sign_up(&server, "babak_x", BABAK_WORDS);
    let mut mina = App::sign_up(&server, "mina_x", MINA_WORDS);
    let chat = make_group(
        &mut sara,
        &server,
        ChatType::Group,
        vec![babak.id(), mina.id()],
    );
    ready_in(&mut babak, &server, chat);
    ready_in(&mut mina, &server, chat);
    make_moderator(&mut sara, &mut babak, &server, chat);
    let doomed = try_text(&mut mina, &server, chat, "remove me", None).unwrap();
    read_text(&mut babak, &server, chat, "remove me");
    assert!(try_delete(&mut babak, &server, chat, doomed).is_ok());
    let deleted = chat_event_seen(&mut sara, &server, chat, ChatAction::Deleted);
    assert_eq!(deleted.seq, Some(doomed));
    send_text(&mut sara, &server, chat, "last");
    read_text(&mut mina, &server, chat, "last");

    // An owner's new device gets the membership lines and the history, in order.
    let mut owner = second_device(&server, "sara_x", SARA_WORDS);
    ready_in(&mut owner, &server, chat);
    owner.command(&server, Command::OpenChat { chat });
    read_text(&mut owner, &server, chat, "last");
    let owner_actions = archived_actions(&owner, chat);
    assert!(owner_actions.contains(&ChatAction::Added));
    assert!(owner_actions.contains(&ChatAction::MadeModerator));
    assert!(owner_actions.contains(&ChatAction::Deleted));
    let mut n_list: Vec<u64> = owner
        .archive_events
        .range((chat, 0)..=(chat, u64::MAX))
        .map(|((_, n), _)| *n)
        .collect();
    let in_order = n_list.clone();
    n_list.sort_unstable();
    assert_eq!(n_list, in_order);
    // Opening the chat shows them as lines, from the archive.
    let line = chat_event_seen(&mut owner, &server, chat, ChatAction::Deleted);
    assert_eq!(line.actor, babak.id());

    // A plain member's new device gets the membership lines, which are messages, and no history.
    let mut member = second_device(&server, "mina_x", MINA_WORDS);
    ready_in(&mut member, &server, chat);
    member.command(&server, Command::OpenChat { chat });
    read_text(&mut member, &server, chat, "last");
    assert!(archived_actions(&member, chat).is_empty());
    member.wait(&server, |event| {
        matches!(event, Event::Line { chat: of, action: LineAction::MadeModerator, .. } if *of == chat)
    });
}

#[test]
fn content_sealed_under_an_older_key_is_refused_and_core_seals_it_again() {
    let server = open_server();
    let mut sara = App::sign_up(&server, "sara_x", SARA_WORDS);
    let mut babak = App::sign_up(&server, "babak_x", BABAK_WORDS);
    let mut mina = App::sign_up(&server, "mina_x", MINA_WORDS);
    let chat = make_group(&mut sara, &server, ChatType::Group, vec![babak.id()]);
    ready_in(&mut babak, &server, chat);
    send_text(&mut sara, &server, chat, "before mina");
    read_text(&mut babak, &server, chat, "before mina");

    // Mina is added while Babak reads nothing, so the add commit waits unread on his socket.
    sara.command(
        &server,
        Command::AddMembers {
            chat,
            account_list: vec![mina.id()],
            note: None,
        },
    );
    ready_in(&mut mina, &server, chat);
    chat_event_seen(&mut sara, &server, chat, ChatAction::Added);

    // Babak's text is sealed under the old key at once and refused with `stale_epoch`. Core applies
    // the commit and seals it again, so Mina, who holds only the new key, reads it.
    let seq = try_text(&mut babak, &server, chat, "sealed again", None).unwrap();
    let message = read_text(&mut mina, &server, chat, "sealed again");
    assert_eq!(message.seq, seq);
    read_text(&mut sara, &server, chat, "sealed again");
}

/// How many key packages this device keeps on the server.
fn key_packages_kept(server: &Server, token: &str) -> u64 {
    let (status, reply) = raw(
        server,
        "POST",
        "/api/v1/key_packages",
        token,
        Some(br#"{"body_list":[]}"#),
    );
    assert_eq!(status, 200);
    let reply: Value = serde_json::from_slice(&reply).unwrap();
    reply["data"]["count"].as_u64().unwrap()
}

#[test]
fn claiming_for_an_add_refuses_someone_added_by_invitation_only_before_any_package_goes() {
    let server = open_server();
    let mut sara = App::sign_up(&server, "sara_x", SARA_WORDS);
    let mut babak = App::sign_up(&server, "babak_x", BABAK_WORDS);
    let mut mina = App::sign_up(&server, "mina_x", MINA_WORDS);
    invitation_only(&mut mina, &server);
    let chat = make_group(&mut sara, &server, ChatType::Group, vec![babak.id()]);
    ready_in(&mut babak, &server, chat);
    let mina_token = mina.token.clone().unwrap();
    let kept = key_packages_kept(&server, &mina_token);
    assert!(kept > 0);

    let route = format!("/api/v1/accounts/{}/key_packages/claim", mina.id());
    let body = json!({ "chat": chat }).to_string();
    let sara_token = sara.token.clone().unwrap();
    let (status, reply) = raw(&server, "POST", &route, &sara_token, Some(body.as_bytes()));
    assert_eq!((status, code_of(&reply).as_str()), (403, "invite_needed"));
    let reply: Value = serde_json::from_slice(&reply).unwrap();
    assert_eq!(reply["account_list"], json!([mina.id()]));
    // A member who may not add people is refused before that, and so is a stranger.
    let babak_token = babak.token.clone().unwrap();
    let (status, reply) = raw(&server, "POST", &route, &babak_token, Some(body.as_bytes()));
    assert_eq!((status, code_of(&reply).as_str()), (403, "no_permission"));
    let (status, reply) = raw(&server, "POST", &route, &mina_token, Some(body.as_bytes()));
    assert_eq!((status, code_of(&reply).as_str()), (404, "not_member"));
    assert_eq!(key_packages_kept(&server, &mina_token), kept);

    // A note over 500 characters is refused before anything is asked.
    let code = refusal(
        &mut sara,
        &server,
        Command::AddMembers {
            chat,
            account_list: vec![mina.id()],
            note: Some("a".repeat(501)),
        },
    );
    assert_eq!(code, "bad_note");

    // Core's add turns into an invitation carrying the note sealed, and still none of her packages
    // went.
    sara.command(
        &server,
        Command::AddMembers {
            chat,
            account_list: vec![mina.id()],
            note: Some("  Come and see the moon  ".to_string()),
        },
    );
    let Event::Invitation { invitation, .. } = mina.wait(
        &server,
        |event| matches!(event, Event::Invitation { chat: invited, invitation: Some(_) } if *invited == chat),
    ) else {
        unreachable!();
    };
    assert_eq!(
        invitation.unwrap().note.as_deref(),
        Some("Come and see the moon")
    );
    assert_eq!(key_packages_kept(&server, &mina_token), kept);
}

#[test]
fn a_new_device_of_someone_who_joined_by_invitation_reads_the_profile_from_the_words() {
    let server = open_server();
    let mut sara = App::sign_up(&server, "sara_x", SARA_WORDS);
    let mut mina = App::sign_up(&server, "mina_x", MINA_WORDS);
    invitation_only(&mut mina, &server);
    sara.command(
        &server,
        Command::CreateChat {
            chat_type: ChatType::Group,
            name: "Apollo".to_string(),
            bio: Some("The moon team".to_string()),
            link_list: Vec::new(),
            account_list: vec![mina.id()],
            topics: None,
            findable: false,
            options: ChatOptions::default(),
            photo: None,
            note: None,
        },
    );
    let chat = ready_chat(&mut sara, &server);
    mina.wait(
        &server,
        |event| matches!(event, Event::Invitation { chat: invited, invitation: Some(_) } if *invited == chat),
    );
    mina.command(&server, Command::AnswerInvite { chat, accept: true });
    ready_in(&mut mina, &server, chat);
    send_text(&mut mina, &server, chat, "joined");
    read_text(&mut sara, &server, chat, "joined");

    // Sara is gone, and Mina's first device too: the new one has only the words, and the share
    // Mina's join carried is what opens the profile.
    drop(sara);
    drop(mina);
    let mut second = second_device(&server, "mina_x", MINA_WORDS);
    ready_in(&mut second, &server, chat);
    let profile = chat_profile_seen(&mut second, &server, chat, |_| true);
    assert_eq!(profile.name, "Apollo");
    assert_eq!(profile.bio.as_deref(), Some("The moon team"));
    second.command(&server, Command::OpenChat { chat });
    read_text(&mut second, &server, chat, "joined");
}

/// Sends a text into a topic, in its main list or a thread, and says how the server answered.
fn try_topic_text(
    app: &mut App,
    server: &Server,
    chat: Ulid,
    topic: Ulid,
    text: &str,
    thread: Option<u64>,
) -> Result<u64, String> {
    app.command(
        server,
        Command::SendText {
            chat,
            text: Secret(text.to_string()),
            reply: None,
            topic: Some(topic),
            thread,
            mentions: Vec::new(),
        },
    );
    answered(app, server, chat)
}

fn try_topic_react(
    app: &mut App,
    server: &Server,
    chat: Ulid,
    topic: Ulid,
    on: u64,
) -> Result<u64, String> {
    app.command(
        server,
        Command::SendReaction {
            chat,
            emoji: "👍".to_string(),
            on: Pointer::Seq(on),
            topic: Some(topic),
            thread: None,
        },
    );
    answered(app, server, chat)
}

/// Sets a topic's options and waits until another member's device has them.
fn topic_options_set(
    app: &mut App,
    other: &mut App,
    server: &Server,
    chat: Ulid,
    topic: Ulid,
    options: TopicOptionsBody,
) {
    app.command(
        server,
        Command::TopicOptions {
            chat,
            topic,
            options: options.clone(),
        },
    );
    let hidden = options.hidden;
    other.wait(server, |event| {
        matches!(event, Event::TopicOptions { topic: of, options: got, .. }
            if *of == topic && got.hidden == hidden && (hidden || *got == options))
    });
    // The device that changed them is shown the server's copy too.
    app.wait(server, |event| {
        matches!(event, Event::TopicOptions { topic: of, options: got, .. }
            if *of == topic && *got == options)
    });
}

#[test]
fn a_topic_has_its_own_options_moderators_and_profile_and_a_hidden_one_is_withheld() {
    let server = open_server();
    let mut sara = App::sign_up(&server, "sara_x", SARA_WORDS);
    let mut babak = App::sign_up(&server, "babak_x", BABAK_WORDS);
    let mut mina = App::sign_up(&server, "mina_x", MINA_WORDS);
    let chat = make_group(
        &mut sara,
        &server,
        ChatType::Group,
        vec![babak.id(), mina.id()],
    );
    ready_in(&mut babak, &server, chat);
    ready_in(&mut mina, &server, chat);
    let topic = topic_made(&mut sara, &server, chat, "Launch");
    let first = try_topic_text(&mut mina, &server, chat, topic, "hello", None).unwrap();

    // Closed: only moderators write or react there.
    topic_options_set(
        &mut sara,
        &mut mina,
        &server,
        chat,
        topic,
        TopicOptionsBody {
            closed: true,
            ..TopicOptionsBody::default()
        },
    );
    assert_eq!(
        try_topic_text(&mut babak, &server, chat, topic, "no", None),
        Err("topic_closed".to_string())
    );
    assert_eq!(
        try_topic_react(&mut mina, &server, chat, topic, first),
        Err("topic_closed".to_string())
    );
    try_topic_text(&mut sara, &server, chat, topic, "the owner writes", None).unwrap();

    // A topic's own moderator writes when it is closed and changes its settings; a member cannot.
    sara.command(
        &server,
        Command::TopicRole {
            chat,
            topic,
            account: babak.id(),
            role: ChatRole::Moderator,
        },
    );
    mina.wait(&server, |event| {
        matches!(event, Event::TopicOptions { mod_list, .. } if mod_list.contains(&babak.id()))
    });
    sara.wait(&server, |event| {
        matches!(event, Event::TopicOptions { mod_list, .. } if mod_list.contains(&babak.id()))
    });
    try_topic_text(
        &mut babak,
        &server,
        chat,
        topic,
        "the topic's moderator",
        None,
    )
    .unwrap();
    mina.command(
        &server,
        Command::TopicOptions {
            chat,
            topic,
            options: TopicOptionsBody::default(),
        },
    );
    mina.wait(
        &server,
        |event| matches!(event, Event::Error { code, .. } if code == "no_permission"),
    );

    // Moderators only in the main list: members still react and talk in threads.
    topic_options_set(
        &mut babak,
        &mut mina,
        &server,
        chat,
        topic,
        TopicOptionsBody {
            write: TopicWrite::Moderators,
            ..TopicOptionsBody::default()
        },
    );
    assert_eq!(
        try_topic_text(&mut mina, &server, chat, topic, "main", None),
        Err("moderators_only".to_string())
    );
    try_topic_text(&mut mina, &server, chat, topic, "in a thread", Some(first)).unwrap();
    try_topic_react(&mut mina, &server, chat, topic, first).unwrap();

    // Its own permissions: reactions refused there, still allowed in General.
    topic_options_set(
        &mut babak,
        &mut mina,
        &server,
        chat,
        topic,
        TopicOptionsBody {
            deny: vec!["message.send.reaction".to_string()],
            ..TopicOptionsBody::default()
        },
    );
    assert_eq!(
        try_topic_react(&mut mina, &server, chat, topic, first),
        Err("no_permission".to_string())
    );
    let general = try_text(&mut mina, &server, chat, "in general", None).unwrap();
    try_react(&mut mina, &server, chat, general).unwrap();

    // Its own profile, with rules, opened by every member.
    babak.command(
        &server,
        Command::TopicProfile {
            chat,
            topic,
            bio: Some(" About the launch ".to_string()),
            link_list: Vec::new(),
            rules: Some("Be kind".to_string()),
        },
    );
    let Event::TopicProfile { profile, .. } = mina.wait(
        &server,
        |event| matches!(event, Event::TopicProfile { topic: of, .. } if *of == topic),
    ) else {
        unreachable!();
    };
    assert_eq!(
        (profile.bio.as_deref(), profile.rules.as_deref()),
        (Some("About the launch"), Some("Be kind"))
    );
    babak.command(
        &server,
        Command::TopicProfile {
            chat,
            topic,
            bio: None,
            link_list: Vec::new(),
            rules: Some("x".repeat(2001)),
        },
    );
    babak.wait(
        &server,
        |event| matches!(event, Event::Error { code, .. } if code == "bad_profile"),
    );

    // General cannot be hidden.
    sara.command(
        &server,
        Command::TopicOptions {
            chat,
            topic: GENERAL_TOPIC,
            options: TopicOptionsBody {
                hidden: true,
                ..TopicOptionsBody::default()
            },
        },
    );
    sara.wait(
        &server,
        |event| matches!(event, Event::Error { code, .. } if code == "bad_body"),
    );

    // Hidden: a member is told only that, cannot write there, and its messages are withheld.
    topic_options_set(
        &mut babak,
        &mut mina,
        &server,
        chat,
        topic,
        TopicOptionsBody {
            hidden: true,
            ..TopicOptionsBody::default()
        },
    );
    assert_eq!(
        try_topic_text(&mut mina, &server, chat, topic, "nobody hears", None),
        Err("topic_gone".to_string())
    );
    try_topic_text(&mut sara, &server, chat, topic, "a secret", None).unwrap();
    read_text(&mut babak, &server, chat, "a secret");
    send_text(&mut sara, &server, chat, "after");
    read_text(&mut mina, &server, chat, "after");
    assert!(!mina.events.iter().any(|event| matches!(event,
        Event::Message { message } if text_of(message) == Some("a secret"))));

    // Shown again, the member's device syncs the chat and reads what was withheld.
    topic_options_set(
        &mut babak,
        &mut mina,
        &server,
        chat,
        topic,
        TopicOptionsBody::default(),
    );
    read_text(&mut mina, &server, chat, "a secret");

    // Each change is in the topic's history, which its moderator and the owners read.
    let edited = chat_event_seen(&mut babak, &server, chat, ChatAction::TopicEdited);
    assert_eq!((edited.actor, edited.topic), (babak.id(), Some(topic)));
    let made = chat_event_seen(&mut babak, &server, chat, ChatAction::MadeTopicModerator);
    assert_eq!(
        (made.actor, made.target, made.topic),
        (sara.id(), Some(babak.id()), Some(topic))
    );
    chat_event_seen(&mut sara, &server, chat, ChatAction::TopicOptions);
    assert!(!mina.events.iter().any(|event| matches!(event,
        Event::ChatEvent { event, .. } if event.topic == Some(topic))));

    // A topic's first profile, set by whoever added it, is part of creating it: no "edited" row.
    // A later one is. Gamma goes beside Launch, since the list sent is the whole list.
    sara.command(&server, Command::TopicId { chat });
    let Event::TopicId { topic: gamma, .. } =
        sara.wait(&server, |event| matches!(event, Event::TopicId { .. }))
    else {
        unreachable!();
    };
    sara.command(
        &server,
        Command::Topics {
            chat,
            on: true,
            topic_list: vec![
                TopicName {
                    id: topic,
                    name: "Launch".to_string(),
                    place: 0,
                },
                TopicName {
                    id: gamma,
                    name: "Gamma".to_string(),
                    place: 1,
                },
            ],
        },
    );
    let Event::ChatEvent { event: added, .. } = sara.wait(&server, |event| {
        matches!(event, Event::ChatEvent { event, .. }
            if event.action == ChatAction::TopicAdded && event.target == Some(gamma))
    }) else {
        unreachable!();
    };
    assert_eq!(added.actor, sara.id());
    let profile_set = |sara: &mut App, mina: &mut App, bio: &str| {
        sara.command(
            &server,
            Command::TopicProfile {
                chat,
                topic: gamma,
                bio: Some(bio.to_string()),
                link_list: Vec::new(),
                rules: None,
            },
        );
        mina.wait(&server, |event| {
            matches!(event, Event::TopicProfile { topic: of, profile, .. }
                if *of == gamma && profile.bio.as_deref() == Some(bio))
        });
    };
    profile_set(&mut sara, &mut mina, "first");
    // Sara's own copy of a later change comes after any row the first one wrote.
    topic_options_set(
        &mut sara,
        &mut mina,
        &server,
        chat,
        gamma,
        TopicOptionsBody::default(),
    );
    assert!(!sara.events.iter().any(|event| matches!(event,
        Event::ChatEvent { event, .. } if event.action == ChatAction::TopicEdited
            && event.topic == Some(gamma))));
    profile_set(&mut sara, &mut mina, "second");
    let Event::ChatEvent { event: edited, .. } = sara.wait(&server, |event| {
        matches!(event, Event::ChatEvent { event, .. }
            if event.action == ChatAction::TopicEdited && event.topic == Some(gamma))
    }) else {
        unreachable!();
    };
    assert_eq!(edited.actor, sara.id());
}

#[test]
fn every_delete_and_edit_in_a_group_keeps_what_it_changed_for_owners_and_moderators() {
    let server = open_server();
    let mut sara = App::sign_up(&server, "sara_x", SARA_WORDS);
    let mut babak = App::sign_up(&server, "babak_x", BABAK_WORDS);
    let mut mina = App::sign_up(&server, "mina_x", MINA_WORDS);
    let chat = make_group(
        &mut sara,
        &server,
        ChatType::Group,
        vec![babak.id(), mina.id()],
    );
    ready_in(&mut babak, &server, chat);
    ready_in(&mut mina, &server, chat);

    // Mina edits her own message, then deletes it: owners see both, with the texts as they were.
    let seq = try_text(&mut mina, &server, chat, "first", None).unwrap();
    mina.command(
        &server,
        Command::Edit {
            chat,
            target: Pointer::Seq(seq),
            text: Secret("second".to_string()),
            files: Vec::new(),
            remove: Vec::new(),
        },
    );
    answered(&mut mina, &server, chat).unwrap();
    try_delete(&mut mina, &server, chat, seq).unwrap();
    let texts = |event: &Event| match event {
        Event::ChatEvent { copy_list, .. } => copy_list
            .iter()
            .map(|copy| text_of(copy).unwrap_or("").to_string())
            .collect::<Vec<_>>(),
        _ => Vec::new(),
    };
    let edited = sara.wait(&server, |event| {
        matches!(event, Event::ChatEvent { event, .. } if event.action == ChatAction::Edited)
    });
    assert_eq!(texts(&edited), ["first", "second"]);
    let deleted = sara.wait(&server, |event| {
        matches!(event, Event::ChatEvent { event, .. } if event.action == ChatAction::Deleted)
    });
    assert_eq!(texts(&deleted), ["first", "second"]);
    let Event::ChatEvent { event, .. } = deleted else {
        unreachable!();
    };
    assert_eq!(
        (event.actor, event.target, event.seq),
        (mina.id(), Some(mina.id()), Some(seq))
    );
    // The archive keeps the copies sealed, and a History page read from it has core open them.
    let kept: Vec<ChatEvent> = sara
        .archive_events
        .values()
        .filter(|kept| kept.chat == chat && !kept.copy_list.is_empty())
        .cloned()
        .collect();
    assert_eq!(kept.len(), 2);
    sara.events.clear();
    sara.command(
        &server,
        Command::OpenHistory {
            chat,
            event_list: kept,
        },
    );
    let opened: Vec<Vec<String>> = sara
        .events
        .iter()
        .filter(|event| matches!(event, Event::ChatEvent { .. }))
        .map(texts)
        .collect();
    assert_eq!(opened, [["first", "second"], ["first", "second"]]);
    // A plain member reads no history.
    send_text(&mut sara, &server, chat, "after");
    read_text(&mut mina, &server, chat, "after");
    assert!(!mina.events.iter().any(|event| matches!(event,
        Event::ChatEvent { event, .. }
            if matches!(event.action, ChatAction::Edited | ChatAction::Deleted))));

    // In a topic, its own moderator deletes a member's message and reads that topic's history.
    let topic = topic_made(&mut sara, &server, chat, "Launch");
    sara.command(
        &server,
        Command::TopicRole {
            chat,
            topic,
            account: babak.id(),
            role: ChatRole::Moderator,
        },
    );
    let babak_id = babak.id();
    babak.wait(&server, |event| {
        matches!(event, Event::TopicOptions { mod_list, .. } if mod_list.contains(&babak_id))
    });
    let in_topic = try_topic_text(&mut mina, &server, chat, topic, "in launch", None).unwrap();
    try_delete(&mut babak, &server, chat, in_topic).unwrap();
    let deleted = babak.wait(&server, |event| {
        matches!(event, Event::ChatEvent { event, .. }
            if event.action == ChatAction::Deleted && event.topic == Some(topic))
    });
    assert_eq!(texts(&deleted), ["in launch"]);
}
