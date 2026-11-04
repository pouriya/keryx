mod common;
#[path = "common/person.rs"]
mod person;

use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

use common::{OPERATOR_TOKEN, Server};
use keryx_core::client::{Command, Event, Secret};
use keryx_core::mls::{AccountWrap, Commit, Device};
use keryx_core::protocol::{
    Bytes, ChatType, ClientFrame, ContentKind, Header, Kind, MessageFrame, SendFrame, ServerFrame,
    WelcomeItem, WrapItem,
};
use keryx_core::ulid::Ulid;
use person::{
    App, BABAK_WORDS, PASSWORD, Person, SARA_WORDS, add_device, log_in, sign_up, ulid_of,
};
use rustls::pki_types::pem::PemObject;
use serde_json::json;
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
        | ServerFrame::Page { reference, .. } => Some(*reference),
        _ => None,
    }
}

impl Socket {
    fn send(&mut self, frame: &ClientFrame) {
        let text = serde_json::to_string(frame).unwrap();
        self.inner.send(Message::text(text)).unwrap();
    }

    fn read(&mut self) -> ServerFrame {
        loop {
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
        loop {
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
        call_seq: None,
        client_sent_at,
        body: Bytes(body),
        signature: Bytes(signature.to_vec()),
        wrap_list: Vec::new(),
        welcome_list: Vec::new(),
        group_info: None,
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
        call_seq: None,
        client_sent_at,
        body: Bytes(commit.commit.clone()),
        signature: Bytes(signature.to_vec()),
        wrap_list: wrap_items(&commit.wrap_list),
        welcome_list,
        group_info: Some(Bytes(commit.group_info.clone())),
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

fn history(socket: &mut Socket, chat: Ulid, after_seq: u64, limit: u32) -> Vec<MessageFrame> {
    let wanted = reference();
    let answer = socket.request(
        &ClientFrame::History {
            reference: wanted,
            chat,
            after_seq,
            before_seq: None,
            limit,
        },
        wanted,
    );
    match answer {
        ServerFrame::Page { message_list, .. } => message_list,
        other => panic!("Wanted a page, and got {other:?}"),
    }
}

fn chat_item(socket: &mut Socket, chat: Ulid) -> keryx_core::protocol::ChatItem {
    let wanted = reference();
    let answer = socket.request(&ClientFrame::Sync { reference: wanted }, wanted);
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
            chat,
            text: Secret("سلام".to_string()),
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
            chat,
            text: Secret("hello".to_string()),
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
            chat,
            text: Secret("before".to_string()),
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
            chat,
            text: Secret("from the new one".to_string()),
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
    let page = history(&mut new_socket, self_chat, 0, 200);
    assert_eq!(page.len(), 2, "one notice per device so far");
    let notice_seq = page[1].seq;
    assert_eq!(page[1].device, Some(new_device.record().device));

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
            call_seq: None,
            client_sent_at,
            body: Bytes(Vec::new()),
            signature: Bytes(signature.to_vec()),
            wrap_list: Vec::new(),
            welcome_list: Vec::new(),
            group_info: None,
        }
    };

    assert_refused(
        post(&mut new_socket, delete(&new_device)),
        "notice_protected",
    );
    let seq = seq_of(post(&mut pair.sara_socket, delete(&pair.sara.device)));
    let page = history(&mut pair.sara_socket, self_chat, notice_seq - 1, 200);
    assert_eq!(page[0].kind, None, "it is wiped to its sequence id");
    assert_eq!(page[1].seq, seq);

    // Walks the preview's time back to the notice before it, as the list is not moved by a delete.
    let item = chat_item(&mut pair.sara_socket, self_chat);
    assert_eq!(item.chat_type, ChatType::SelfChat);
    assert_eq!(item.last_kind, Some(Kind::Delete));
    assert_eq!(item.last_content_seq, None);
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
    let page = history(&mut pair.sara_socket, chat, first_seq - 1, 200);
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

    // The same epoch 0 again is a resend whose answer was lost, and anything else lost the start.
    done(upload_group_info(
        &mut pair.sara_socket,
        unstarted,
        0,
        vec![1],
    ));
    done(upload_group_info(
        &mut pair.sara_socket,
        unstarted,
        0,
        vec![1],
    ));
    assert_refused(
        upload_group_info(&mut pair.sara_socket, unstarted, 0, vec![2]),
        "chat_started",
    );
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
            Box::new(|frame: &mut SendFrame| frame.call_seq = Some(1)),
            "commit_or_ok",
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
            "unencrypted_refused",
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
                &ClientFrame::History {
                    reference: wanted,
                    chat,
                    after_seq: 0,
                    before_seq: None,
                    limit: 10,
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
    socket.send(&ClientFrame::Typing { chat, on: true });

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
                &ClientFrame::History {
                    reference: wanted,
                    chat,
                    after_seq: 0,
                    before_seq: None,
                    limit: 10,
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

    pair.sara_socket
        .send(&ClientFrame::Typing { chat, on: true });
    let typing = pair
        .babak_socket
        .next_where(|frame| matches!(frame, ServerFrame::Typing { .. }));
    let ServerFrame::Typing { account, on, .. } = typing else {
        unreachable!();
    };
    assert_eq!(account, pair.sara.id);
    assert!(on);
    assert_eq!(history(&mut pair.sara_socket, chat, seq, 10).len(), 0);
}

#[test]
fn history_pages_backward_from_before_seq() {
    let mut pair = pair();
    let chat = pair.chat;
    let mut seq_list = Vec::new();
    for text in [b"a", b"b", b"c", b"d"] {
        let frame = sealed(&pair.sara, chat, Kind::Content, None, text);
        seq_list.push(seq_of(post(&mut pair.sara_socket, frame)));
    }
    let last = *seq_list.last().unwrap();
    let page = |socket: &mut Socket, before_seq| {
        let wanted = reference();
        match socket.request(
            &ClientFrame::History {
                reference: wanted,
                chat,
                after_seq: 0,
                before_seq: Some(before_seq),
                limit: 2,
            },
            wanted,
        ) {
            ServerFrame::Page { message_list, .. } => {
                let mut found = Vec::new();
                for message in message_list {
                    found.push(message.seq);
                }
                found
            }
            other => panic!("Wanted a page, and got {other:?}"),
        }
    };
    assert_eq!(page(&mut pair.babak_socket, last + 1), vec![last - 1, last]);
    assert_eq!(
        page(&mut pair.babak_socket, last - 1),
        vec![last - 3, last - 2]
    );
    assert_eq!(page(&mut pair.babak_socket, 1), Vec::<u64>::new());
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
            chat,
            text: Secret("you are back".to_string()),
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
            chat,
            text: Secret("x".to_string()),
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

    sara.command(&server, Command::Typing { chat, on: true });
    let Event::Typing { account, on, .. } =
        babak.wait(&server, |event| matches!(event, Event::Typing { .. }))
    else {
        unreachable!();
    };
    assert_eq!(account, sara.id());
    assert!(on);
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
                chat,
                text: Secret(format!("{number}")),
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
            chat,
            text: Secret("frist".to_string()),
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
            chat,
            text: Secret("still me".to_string()),
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
        let frame = serde_json::to_string(&ClientFrame::Sync { reference: wanted }).unwrap();
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
        let page = history(&mut back, chat, after_seq, 5);
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
        socket.send(&ClientFrame::Sync {
            reference: reference(),
        });
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
