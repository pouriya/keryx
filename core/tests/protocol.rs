use keryx_core::mls::{Device, verify_message};
use keryx_core::protocol::{
    Bytes, ChatItem, ChatMode, ChatType, ClientFrame, ContentKind, Header, Kind, MarkItem,
    MessageFrame, SendFrame, ServerFrame, WelcomeItem, WrapItem,
};
use keryx_core::seed::derive;
use keryx_core::ulid::Ulid;
use serde_json::{from_str, json, to_value};

const CHAT: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const ID: &str = "01BX5ZZKBKACTAV9WEVGEMMVRZ";
const REF: &str = "01H8XGJWBWBAQ4Z5Y6N3V7K2MD";

fn ulid(text: &str) -> Ulid {
    Ulid::parse(text).unwrap()
}

fn send_frame() -> SendFrame {
    SendFrame {
        reference: ulid(REF),
        chat: ulid(CHAT),
        kind: Kind::Content,
        content_kind: Some(ContentKind::Text),
        encrypted: true,
        epoch: Some(3),
        pointer_seq: None,
        call_seq: None,
        client_sent_at: 1800,
        body: Bytes(vec![1, 2, 3]),
        signature: Bytes(vec![255]),
        wrap_list: Vec::new(),
        welcome_list: Vec::new(),
        group_info: None,
        persist: Vec::new(),
        remove: Vec::new(),
    }
}

fn header() -> Header {
    Header {
        chat: Ulid::from_bytes([1; 16]),
        device: Ulid::from_bytes([2; 16]),
        kind: Kind::Content,
        content_kind: Some(ContentKind::Text),
        encrypted: true,
        epoch: Some(4),
        pointer_seq: None,
        call_seq: None,
        client_sent_at: 5,
    }
}

/// **If this fails, no stored signature verifies any more.** Never update it to make the test pass.
#[test]
fn layout_is_pinned() {
    let mut expected = b"keryx v1 message".to_vec();
    expected.extend_from_slice(&[1; 16]);
    expected.extend_from_slice(&[2; 16]);
    expected.extend_from_slice(&[0, 1, 0, 1]);
    expected.extend_from_slice(&[1, 0, 0, 0, 0, 0, 0, 0, 4]);
    expected.extend_from_slice(&[0; 18]);
    expected.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 5]);
    assert_eq!(header().bytes(), expected);
}

#[test]
fn every_field_changes_the_bytes() {
    let base = header().bytes();
    let mut changes = vec![header(); 9];
    changes[0].chat = Ulid::from_bytes([9; 16]);
    changes[1].device = Ulid::from_bytes([9; 16]);
    changes[2].kind = Kind::Edit;
    changes[3].content_kind = Some(ContentKind::Reaction);
    changes[4].encrypted = false;
    changes[5].epoch = None;
    changes[6].pointer_seq = Some(0);
    changes[7].call_seq = Some(0);
    changes[8].client_sent_at = 6;
    for change in &changes {
        assert_ne!(change.bytes(), base, "{change:?}");
        assert_eq!(change.bytes().len(), base.len());
    }
}

#[test]
fn absent_differs_from_zero() {
    let mut absent = header();
    absent.content_kind = None;
    assert_ne!(absent.bytes(), header().bytes());
    let mut zero = header();
    zero.epoch = Some(0);
    let mut none = header();
    none.epoch = None;
    assert_ne!(zero.bytes(), none.bytes());
}

#[test]
fn kinds_match_the_schema() {
    assert_eq!(Kind::Content as u8, 0);
    assert_eq!(Kind::Edit as u8, 1);
    assert_eq!(Kind::Delete as u8, 2);
    assert_eq!(Kind::Commit as u8, 3);
    assert_eq!(Kind::DeviceNotice as u8, 4);
    let content_kinds = [
        ContentKind::Text,
        ContentKind::Voice,
        ContentKind::Reaction,
        ContentKind::Image,
        ContentKind::Video,
        ContentKind::Doc,
        ContentKind::File,
        ContentKind::Call,
        ContentKind::MediaBundle,
    ];
    for (number, content_kind) in content_kinds.iter().enumerate() {
        assert_eq!(*content_kind as usize, number);
    }
}

#[test]
fn a_send_frame_has_the_agreed_shape() {
    let frame = ClientFrame::Send(Box::new(send_frame()));
    let expected = json!({
        "type": "send", "ref": REF, "chat": CHAT, "kind": 0, "content_kind": 0,
        "encrypted": true, "epoch": 3, "client_sent_at": 1800, "body": "AQID", "signature": "/w==",
    });
    assert_eq!(to_value(&frame).unwrap(), expected);
    assert_eq!(
        from_str::<ClientFrame>(&expected.to_string()).unwrap(),
        frame
    );
}

#[test]
fn a_commit_is_a_send_carrying_its_wraps_welcomes_and_group_info() {
    let mut commit = send_frame();
    commit.kind = Kind::Commit;
    commit.content_kind = None;
    commit.wrap_list = vec![WrapItem {
        account: ulid(ID),
        wrapped: Bytes(vec![1]),
    }];
    commit.welcome_list = vec![WelcomeItem {
        device: ulid(CHAT),
        body: Bytes(vec![2]),
    }];
    commit.group_info = Some(Bytes(vec![3]));
    let value = to_value(ClientFrame::Send(Box::new(commit))).unwrap();
    assert_eq!(value["type"], "send");
    assert_eq!(value["kind"], 3);
    assert!(value.get("content_kind").is_none());
    assert_eq!(
        value["wrap_list"],
        json!([{"account": ID, "wrapped": "AQ=="}])
    );
    assert_eq!(
        value["welcome_list"],
        json!([{"device": CHAT, "body": "Ag=="}])
    );
    assert_eq!(value["group_info"], "Aw==");
}

#[test]
fn every_client_frame_has_the_agreed_shape() {
    let cases = [
        (
            ClientFrame::Seen {
                reference: ulid(REF),
                chat: ulid(CHAT),
                up_to_seq: 41,
                client_seen_at: 1800,
            },
            json!({"type": "seen", "ref": REF, "chat": CHAT, "up_to_seq": 41, "client_seen_at": 1800}),
        ),
        (
            ClientFrame::Ping {
                reference: ulid(REF),
            },
            json!({"type": "ping", "ref": REF}),
        ),
        (
            ClientFrame::Typing {
                chat: ulid(CHAT),
                on: true,
            },
            json!({"type": "typing", "chat": CHAT, "on": true}),
        ),
        (
            ClientFrame::Sync {
                reference: ulid(REF),
            },
            json!({"type": "sync", "ref": REF}),
        ),
        (
            ClientFrame::History {
                reference: ulid(REF),
                chat: ulid(CHAT),
                after_seq: 41,
                before_seq: None,
                limit: 100,
            },
            json!({"type": "history", "ref": REF, "chat": CHAT, "after_seq": 41, "limit": 100}),
        ),
        (
            ClientFrame::History {
                reference: ulid(REF),
                chat: ulid(CHAT),
                after_seq: 0,
                before_seq: Some(43),
                limit: 100,
            },
            json!({"type": "history", "ref": REF, "chat": CHAT, "after_seq": 0, "before_seq": 43,
                   "limit": 100}),
        ),
        (
            ClientFrame::Received {
                reference: ulid(REF),
                chat: ulid(CHAT),
                up_to_seq: 42,
                client_received_at: 1800,
            },
            json!({"type": "received", "ref": REF, "chat": CHAT, "up_to_seq": 42,
                   "client_received_at": 1800}),
        ),
        (
            ClientFrame::GroupInfo {
                reference: ulid(REF),
                chat: ulid(CHAT),
                epoch: 4,
                body: Bytes(vec![1, 2, 3]),
            },
            json!({"type": "group_info", "ref": REF, "chat": CHAT, "epoch": 4, "body": "AQID"}),
        ),
        (
            ClientFrame::Wraps {
                reference: ulid(REF),
                chat: ulid(CHAT),
                epoch: 4,
                wrap_list: vec![WrapItem {
                    account: ulid(ID),
                    wrapped: Bytes(vec![1]),
                }],
            },
            json!({"type": "wraps", "ref": REF, "chat": CHAT, "epoch": 4,
                   "wrap_list": [{"account": ID, "wrapped": "AQ=="}]}),
        ),
    ];
    for (frame, expected) in cases {
        assert_eq!(to_value(&frame).unwrap(), expected);
        assert_eq!(
            from_str::<ClientFrame>(&expected.to_string()).unwrap(),
            frame
        );
    }
}

#[test]
fn every_server_frame_has_the_agreed_shape() {
    let removed = MessageFrame {
        chat: ulid(CHAT),
        seq: 5,
        device: None,
        kind: None,
        content_kind: None,
        encrypted: None,
        epoch: None,
        pointer_seq: None,
        call_seq: None,
        client_sent_at: None,
        server_saved_at: None,
        body: None,
        signature: None,
    };
    let chat = ChatItem {
        chat: ulid(CHAT),
        chat_type: ChatType::Direct,
        mode: ChatMode::Private,
        last_seq: Some(42),
        last_kind: Some(Kind::Content),
        last_at: Some(1800),
        last_content_seq: None,
        epoch: None,
        last_content: None,
        mark_list: Vec::new(),
        seen_seq: None,
    };
    let marked = ChatItem {
        last_content_seq: Some(5),
        epoch: Some(3),
        last_content: Some(Box::new(removed.clone())),
        mark_list: vec![MarkItem {
            account: ulid(ID),
            received_seq: 42,
            seen_seq: Some(41),
        }],
        ..chat.clone()
    };
    let cases = [
        (
            ServerFrame::Sent {
                reference: ulid(REF),
                seq: 42,
                server_saved_at: 1800,
            },
            json!({"type": "sent", "ref": REF, "seq": 42, "server_saved_at": 1800}),
        ),
        (
            ServerFrame::Done {
                reference: ulid(REF),
            },
            json!({"type": "done", "ref": REF}),
        ),
        (
            ServerFrame::Refused {
                reference: ulid(REF),
                code: "too_many_devices".to_string(),
                message: "No".to_string(),
            },
            json!({"type": "refused", "ref": REF, "code": "too_many_devices", "message": "No"}),
        ),
        (
            ServerFrame::Message(Box::new(removed.clone())),
            json!({"type": "message", "chat": CHAT, "seq": 5}),
        ),
        (
            ServerFrame::Chats {
                reference: ulid(REF),
                chat_list: vec![chat, marked],
            },
            json!({"type": "chats", "ref": REF, "chat_list": [{
                "chat": CHAT, "type": 1, "mode": 0, "last_seq": 42, "last_kind": 0, "last_at": 1800,
            }, {
                "chat": CHAT, "type": 1, "mode": 0, "last_seq": 42, "last_kind": 0, "last_at": 1800,
                "last_content_seq": 5, "epoch": 3, "last_content": {"chat": CHAT, "seq": 5},
                "mark_list": [{"account": ID, "received_seq": 42, "seen_seq": 41}],
            }]}),
        ),
        (
            ServerFrame::Marks {
                chat: ulid(CHAT),
                mark: MarkItem {
                    account: ulid(ID),
                    received_seq: 42,
                    seen_seq: None,
                },
            },
            json!({"type": "marks", "chat": CHAT, "account": ID, "received_seq": 42}),
        ),
        (
            ServerFrame::Page {
                reference: ulid(REF),
                message_list: vec![removed],
            },
            json!({"type": "page", "ref": REF, "message_list": [{"chat": CHAT, "seq": 5}]}),
        ),
        (
            ServerFrame::Features {
                ping_after: 5,
                ping_every: 15,
                ping_timeout: 5,
            },
            json!({"type": "features", "ping_after": 5, "ping_every": 15, "ping_timeout": 5}),
        ),
        (
            ServerFrame::Revoked {
                account: ulid(ID),
                device: ulid(CHAT),
                statement: Bytes(vec![1, 2, 3]),
            },
            json!({"type": "revoked", "account": ID, "device": CHAT, "statement": "AQID"}),
        ),
        (
            ServerFrame::Welcome {
                chat: ulid(CHAT),
                body: Bytes(vec![1]),
            },
            json!({"type": "welcome", "chat": CHAT, "body": "AQ=="}),
        ),
        (
            ServerFrame::Typing {
                chat: ulid(CHAT),
                account: ulid(ID),
                on: false,
            },
            json!({"type": "typing", "chat": CHAT, "account": ID, "on": false}),
        ),
    ];
    for (frame, expected) in cases {
        assert_eq!(to_value(&frame).unwrap(), expected);
        assert_eq!(
            from_str::<ServerFrame>(&expected.to_string()).unwrap(),
            frame
        );
    }
}

#[test]
fn unknown_fields_are_ignored_because_they_do_not_bump_the_version() {
    let text = json!({"type": "sync", "ref": REF, "from_the_future": true}).to_string();
    assert_eq!(
        from_str::<ClientFrame>(&text).unwrap(),
        ClientFrame::Sync {
            reference: ulid(REF)
        }
    );
}

#[test]
fn bad_frames_are_refused() {
    let good = to_value(ClientFrame::Send(Box::new(send_frame()))).unwrap();
    for (field, value) in [
        ("kind", json!(99)),
        ("content_kind", json!(11)),
        ("body", json!("not base64!")),
        ("chat", json!("not a ulid")),
        ("type", json!("no_such_type")),
    ] {
        let mut bad = good.clone();
        bad[field] = value;
        assert!(
            from_str::<ClientFrame>(&bad.to_string()).is_err(),
            "{field}"
        );
    }
}

#[test]
fn bytes_never_show_their_contents_when_debug_printed() {
    let printed = format!("{:?}", send_frame());
    assert!(printed.contains("Bytes(3 bytes)"));
    assert!(!printed.contains("[1, 2, 3]"));
}

#[test]
fn a_removed_message_has_no_header_and_a_stored_one_verifies_after_the_wire() {
    let keys = derive(
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
    )
    .unwrap();
    let device = Device::create(&keys, ulid(ID), ulid(CHAT)).unwrap();
    let mut header = header();
    header.device = device.record().device;
    header.encrypted = false;
    header.epoch = None;
    let signature = device.sign(&header, b"hello").unwrap();

    let frame = MessageFrame {
        chat: header.chat,
        seq: 1,
        device: Some(header.device),
        kind: Some(header.kind),
        content_kind: header.content_kind,
        encrypted: Some(header.encrypted),
        epoch: header.epoch,
        pointer_seq: header.pointer_seq,
        call_seq: header.call_seq,
        client_sent_at: Some(header.client_sent_at),
        server_saved_at: Some(6),
        body: Some(Bytes(b"hello".to_vec())),
        signature: Some(Bytes(signature.to_vec())),
    };
    let text = serde_json::to_string(&ServerFrame::Message(Box::new(frame))).unwrap();
    let ServerFrame::Message(received) = from_str::<ServerFrame>(&text).unwrap() else {
        panic!("this is not a message");
    };
    assert_eq!(received.header().unwrap(), header);
    let mut bytes = [0u8; 64];
    bytes.copy_from_slice(&received.signature.as_ref().unwrap().0);
    let body = &received.body.as_ref().unwrap().0;
    verify_message(&header, body, &bytes, &device.record().device_public).unwrap();

    let removed: MessageFrame = from_str(&json!({"chat": CHAT, "seq": 5}).to_string()).unwrap();
    assert!(removed.header().is_none());
}
