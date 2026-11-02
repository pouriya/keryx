mod common;
#[path = "common/person.rs"]
mod person;

use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

use common::{OPERATOR_TOKEN, Server};
use keryx_core::mls::{AccountWrap, Commit, Device, sign_revocation};
use keryx_core::protocol::{
    Bytes, ChatType, ClientFrame, ContentKind, Header, Kind, MessageFrame, SendFrame, ServerFrame,
    WelcomeItem, WrapItem,
};
use keryx_core::ulid::Ulid;
use person::{BABAK_WORDS, PASSWORD, Person, SARA_WORDS, add_device, log_in, sign_up, ulid_of};
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

fn pair_on(server: Server) -> Pair {
    let mut sara = sign_up(&server, "sara_x", SARA_WORDS);
    let mut babak = sign_up(&server, "babak_x", BABAK_WORDS);
    let mut sara_socket = connect(&server, &sara.token);
    let mut babak_socket = connect(&server, &babak.token);

    let (status, reply) = server.call(
        "POST",
        "/api/v1/chats",
        Some(&sara.token),
        Some(&json!({"account": babak.id})),
    );
    assert_eq!(status, 200, "{reply}");
    let chat = ulid_of(&reply["data"]["chat"]);

    let (status, reply) = server.call(
        "POST",
        &format!("/api/v1/accounts/{}/key_packages/claim", babak.id),
        Some(&sara.token),
        None,
    );
    assert_eq!(status, 200, "{reply}");
    let mut packages = Vec::new();
    let mut welcome_devices = Vec::new();
    for item in reply["data"]["key_package_list"].as_array().unwrap() {
        let body: Bytes = serde_json::from_value(item["body"].clone()).unwrap();
        packages.push(body.0);
        welcome_devices.push(ulid_of(&item["device"]));
    }

    let created = sara
        .device
        .create_chat(chat, Ulid::now(), std::slice::from_ref(&sara.key))
        .unwrap();
    done(upload_group_info(
        &mut sara_socket,
        chat,
        0,
        created.group_info,
    ));
    let wanted = reference();
    done(sara_socket.request(
        &ClientFrame::Wraps {
            reference: wanted,
            chat,
            epoch: 0,
            wrap_list: wrap_items(&created.wrap_list),
        },
        wanted,
    ));

    let accounts = [sara.key.clone(), babak.key.clone()];
    let commit = sara.device.add_devices(chat, &packages, &accounts).unwrap();
    let mut welcome_list = Vec::new();
    for device in welcome_devices {
        welcome_list.push(WelcomeItem {
            device,
            body: Bytes(commit.welcome.clone().unwrap()),
        });
    }
    let frame = commit_frame(&sara.device, chat, &commit, welcome_list);
    let seq = seq_of(post(&mut sara_socket, frame));
    let group_info = sara.device.commit_accepted(chat, Ulid::now()).unwrap();
    done(upload_group_info(
        &mut sara_socket,
        chat,
        commit.epoch,
        group_info,
    ));

    let welcome = babak_socket.next_where(|frame| matches!(frame, ServerFrame::Welcome { .. }));
    let ServerFrame::Welcome { chat: joined, body } = welcome else {
        unreachable!();
    };
    assert_eq!(joined, chat);
    babak
        .device
        .join_welcome(chat, &body.0, Ulid::now())
        .unwrap();
    assert_eq!(babak_socket.message(chat, seq).kind, Some(Kind::Commit));

    Pair {
        server,
        sara,
        babak,
        sara_socket,
        babak_socket,
        chat,
    }
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
    let mut pair = pair();
    let chat = pair.chat;
    let frame = sealed(&pair.sara, chat, Kind::Content, None, "سلام".as_bytes());
    let sent = post(&mut pair.sara_socket, frame.clone());
    let ServerFrame::Sent {
        reference: answered,
        seq,
        server_saved_at,
    } = sent
    else {
        panic!("{sent:?}");
    };
    assert_eq!(answered, frame.reference);
    assert!(server_saved_at > 0);

    // The other device gets it with its sequence id, and it opens. The sender got `sent` instead.
    let received = pair.babak_socket.message(chat, seq);
    assert_eq!(
        open_message(&pair.babak.device, &pair.sara.device, &received),
        "سلام".as_bytes()
    );
    assert_eq!(received.client_sent_at, Some(frame.client_sent_at));

    // The reply goes the other way, and sequence ids keep growing.
    let reply = sealed(&pair.babak, chat, Kind::Content, None, b"hello");
    let reply_seq = seq_of(post(&mut pair.babak_socket, reply));
    assert_eq!(reply_seq, seq + 1);
    let received = pair.sara_socket.message(chat, reply_seq);
    assert_eq!(
        open_message(&pair.sara.device, &pair.babak.device, &received),
        b"hello"
    );

    // History and the list agree, from either side.
    let page = history(&mut pair.babak_socket, chat, seq - 1, 200);
    assert_eq!(page.len(), 2);
    assert_eq!(
        open_message(&pair.babak.device, &pair.sara.device, &page[0]),
        "سلام".as_bytes()
    );
    let item = chat_item(&mut pair.sara_socket, chat);
    assert_eq!(item.chat_type, ChatType::Direct);
    assert_eq!(item.last_seq, Some(reply_seq));
    assert_eq!(item.last_content_seq, Some(reply_seq));
    assert_eq!(item.last_kind, Some(Kind::Content));
    assert!(item.last_at.is_some());
    assert_eq!(history(&mut pair.sara_socket, chat, 0, 1).len(), 1);

    // The wraps went up with the commit, and the words open them.
    let (status, wraps) = pair.server.call(
        "GET",
        &format!("/api/v1/chats/{chat}/wraps"),
        Some(&pair.sara.token),
        None,
    );
    assert_eq!(status, 200);
    let wrap_list = wraps["data"]["wrap_list"].as_array().unwrap();
    assert_eq!(wrap_list.len(), 3, "sara at epoch 0, and both people at 1");

    // The ciphertext the server stores does not hold the text.
    let database = std::fs::read(pair.server.path("keryx.db")).unwrap();
    assert!(
        !database
            .windows("سلام".len())
            .any(|bytes| bytes == "سلام".as_bytes())
    );
}

#[test]
fn a_new_device_gets_everything_with_only_the_words() {
    let mut pair = pair();
    let chat = pair.chat;
    let before = sealed(&pair.babak, chat, Kind::Content, None, b"before");
    let seq = seq_of(post(&mut pair.babak_socket, before));
    let old_message = pair.sara_socket.message(chat, seq);

    // Sara arrives on a new device, with nobody approving anything.
    let token = log_in(&pair.server, &pair.sara.username);
    let mut new_device = add_device(&pair.server, pair.sara.id, &pair.sara.keys, &token);
    let new_id = new_device.record().device;

    // Her old device sees the notice in her self chat, signed with the device's stamp.
    let notice = pair.sara_socket.next_where(|frame| {
        matches!(frame, ServerFrame::Message(message) if message.chat == pair.sara.self_chat)
    });
    let ServerFrame::Message(notice) = notice else {
        unreachable!();
    };
    assert_eq!(notice.kind, Some(Kind::DeviceNotice));
    assert_eq!(notice.device, Some(new_id));
    assert_eq!(notice.encrypted, Some(false));
    assert_eq!(
        notice.signature.clone().unwrap().0,
        new_device.record().stamp.to_vec()
    );
    assert!(notice.body.clone().unwrap().0.is_empty());
    let mut new_socket = connect(&pair.server, &token);

    // It sees the list and members, joins by itself, and catches up through the wraps.
    let (status, reply) = pair.server.call(
        "GET",
        &format!("/api/v1/chats/{chat}/group_info"),
        Some(&token),
        None,
    );
    assert_eq!(status, 200, "{reply}");
    let group_info: Bytes = serde_json::from_value(reply["data"]["group_info"].clone()).unwrap();
    let accounts = [pair.sara.key.clone(), pair.babak.key.clone()];
    let join = new_device
        .join_external(chat, &group_info.0, Ulid::now(), &accounts)
        .unwrap();
    let frame = commit_frame(&new_device, chat, &join, Vec::new());
    let join_seq = seq_of(post(&mut new_socket, frame));
    let group_info = new_device.commit_accepted(chat, Ulid::now()).unwrap();
    done(upload_group_info(
        &mut new_socket,
        chat,
        join.epoch,
        group_info,
    ));

    // Existing members apply it.
    let commit = pair.babak_socket.message(chat, join_seq);
    pair.babak
        .device
        .process_commit(chat, &commit.body.unwrap().0, Ulid::now())
        .unwrap();

    let (_, wraps) = pair.server.call(
        "GET",
        &format!("/api/v1/chats/{chat}/wraps"),
        Some(&token),
        None,
    );
    for wrap in wraps["data"]["wrap_list"].as_array().unwrap() {
        if wrap["account"] == json!(pair.sara.id) && wrap["epoch"] == json!(1) {
            let wrapped: Bytes = serde_json::from_value(wrap["wrapped"].clone()).unwrap();
            new_device
                .unwrap_history(&pair.sara.keys, chat, 1, &wrapped.0)
                .unwrap();
        }
    }
    let page = history(&mut new_socket, chat, seq - 1, 1);
    assert_eq!(page[0], old_message);
    assert_eq!(
        open_message(&new_device, &pair.babak.device, &page[0]),
        b"before"
    );
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
    let group_info = pair.sara.device.commit_accepted(chat, Ulid::now()).unwrap();
    done(upload_group_info(
        &mut pair.babak_socket,
        chat,
        sara_commit.epoch,
        group_info,
    ));

    // Before epoch 0 is uploaded, no commit or later epoch can come in.
    let (_, reply) = pair.server.call(
        "POST",
        "/api/v1/chats",
        Some(&pair.sara.token),
        Some(&json!({"account": pair.sara.id})),
    );
    let self_chat = ulid_of(&reply["data"]["chat"]);
    assert_eq!(self_chat, pair.sara.self_chat);
    assert_refused(
        upload_group_info(&mut pair.sara_socket, self_chat, 3, vec![1]),
        "stale_epoch",
    );
    let mut lost = commit_frame(&pair.sara.device, self_chat, &sara_commit, Vec::new());
    lost.epoch = Some(1);
    lost.wrap_list.clear();
    assert_refused(post(&mut pair.sara_socket, lost), "commit_lost");
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
fn seen_keeps_the_highest_and_typing_is_not_saved() {
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
    done(seen(&mut pair.babak_socket, seq));
    done(seen(&mut pair.babak_socket, 1));
    done(seen(&mut pair.babak_socket, seq));
    assert_refused(seen(&mut pair.babak_socket, seq + 1), "bad_seen");

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
    assert_eq!(chat_item(&mut pair.sara_socket, chat).last_seq, Some(seq));
}

#[test]
fn an_ended_device_is_cut_off_and_everyone_sharing_a_chat_is_told() {
    let mut pair = pair();
    let chat = pair.chat;
    let id = pair.babak.device.record().device;

    let signature = sign_revocation(&pair.babak.keys, &pair.babak.id, &id);
    let token = log_in(&pair.server, "babak_x");
    let (status, reply) = pair.server.call(
        "POST",
        &format!("/api/v1/devices/{id}/terminate"),
        Some(&token),
        Some(&json!({"signature": Bytes(signature.to_vec())})),
    );
    assert_eq!(status, 200, "{reply}");

    let revoked = pair
        .sara_socket
        .next_where(|frame| matches!(frame, ServerFrame::Revoked { .. }));
    let ServerFrame::Revoked {
        account,
        device,
        statement,
    } = revoked
    else {
        unreachable!();
    };
    assert_eq!(account, pair.babak.id);
    assert_eq!(device, id);
    let fixed: [u8; 64] = statement.0.clone().try_into().unwrap();
    keryx_core::mls::verify_revocation(pair.babak.keys.master_public(), &account, &device, &fixed)
        .unwrap();
    assert_eq!(
        pair.babak_socket.closed(),
        (4403, "device_ended".to_string())
    );

    // A client that was offline reads it from the chat's device list.
    let (_, listed) = pair.server.call(
        "GET",
        &format!("/api/v1/chats/{chat}/devices"),
        Some(&pair.sara.token),
        None,
    );
    let mut with_statement = 0;
    for device in listed["data"]["device_list"].as_array().unwrap() {
        if device["device"] == json!(id) {
            assert_eq!(device["statement"], json!(Bytes(statement.0.clone())));
            with_statement += 1;
        }
    }
    assert_eq!(with_statement, 1);

    // A member commits its removal.
    let accounts = [pair.sara.key.clone(), pair.babak.key.clone()];
    let removal = pair
        .sara
        .device
        .remove_devices(chat, &[id], &accounts)
        .unwrap();
    let frame = commit_frame(&pair.sara.device, chat, &removal, Vec::new());
    seq_of(post(&mut pair.sara_socket, frame));

    // It cannot come back, by login or by Welcome.
    let (_, asked) = pair.server.call(
        "POST",
        "/api/v1/challenge",
        None,
        Some(&json!({"username": "babak_x", "purpose": "device_login"})),
    );
    let challenge: Bytes = serde_json::from_value(asked["data"]["challenge"].clone()).unwrap();
    let fixed: [u8; 32] = challenge.0.clone().try_into().unwrap();
    let signature = pair.babak.device.sign_device_login(&fixed).unwrap();
    let (status, reply) = pair.server.call(
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
    let mut welcome = sealed(&pair.sara, chat, Kind::Content, None, b"");
    welcome.reference = reference();
    welcome.kind = Kind::Commit;
    welcome.content_kind = None;
    welcome.epoch = Some(99);
    welcome.welcome_list = vec![WelcomeItem {
        device: id,
        body: Bytes(vec![1]),
    }];
    assert_refused(post(&mut pair.sara_socket, welcome), "no_such_device");
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
    let mut sara = sign_up(&server, "sara_x", SARA_WORDS);
    let mut babak = sign_up(&server, "babak_x", BABAK_WORDS);
    let mut sara_socket = connect(&server, &sara.token);
    let (_, reply) = server.call(
        "POST",
        "/api/v1/chats",
        Some(&sara.token),
        Some(&json!({"account": babak.id})),
    );
    let chat = ulid_of(&reply["data"]["chat"]);
    let (_, reply) = server.call(
        "POST",
        &format!("/api/v1/accounts/{}/key_packages/claim", babak.id),
        Some(&sara.token),
        None,
    );
    let item: &Value = &reply["data"]["key_package_list"][0];
    let package: Bytes = serde_json::from_value(item["body"].clone()).unwrap();
    let created = sara
        .device
        .create_chat(chat, Ulid::now(), std::slice::from_ref(&sara.key))
        .unwrap();
    done(upload_group_info(
        &mut sara_socket,
        chat,
        0,
        created.group_info,
    ));
    let accounts = [sara.key.clone(), babak.key.clone()];
    let commit = sara
        .device
        .add_devices(chat, &[package.0], &accounts)
        .unwrap();
    let frame = commit_frame(
        &sara.device,
        chat,
        &commit,
        vec![WelcomeItem {
            device: babak.device.record().device,
            body: Bytes(commit.welcome.clone().unwrap()),
        }],
    );
    seq_of(post(&mut sara_socket, frame));

    // Babak comes online afterwards and gets it, once.
    let mut socket = connect(&server, &babak.token);
    let welcome = socket.next_where(|frame| matches!(frame, ServerFrame::Welcome { .. }));
    let ServerFrame::Welcome { body, .. } = welcome else {
        unreachable!();
    };
    babak
        .device
        .join_welcome(chat, &body.0, Ulid::now())
        .unwrap();
    drop(socket);
    let mut again = connect(&server, &babak.token);
    assert_eq!(chat_item(&mut again, chat).chat, chat);
    assert!(again.waiting.is_empty());
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
    let server = Server::start("", "[registration]\nallow = true\n# tls\n");
    let sara = sign_up(&server, "sara_x", SARA_WORDS);
    let url = format!("{}/api/v1/ws", server.origin.replace("https://", "wss://"));
    let request = || {
        ClientRequestBuilder::new(url.parse().unwrap())
            .with_header("Authorization", format!("Bearer {}", sara.token))
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
                assert_eq!(chat_list[0].chat, sara.self_chat);
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
