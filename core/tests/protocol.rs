use keryx_core::mls::{Device, verify_message};
use keryx_core::protocol::{
    Answer, Bytes, ChatItem, ChatMode, ChatType, ClientFrame, ContentKind, FormError, Header, Kind,
    MarkItem, MessageFrame, Question, SendFrame, ServerFrame, SyncPosition, WelcomeItem, WrapItem,
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
        client_sent_at: 1800,
        body: Bytes(vec![1, 2, 3]),
        signature: Bytes(vec![255]),
        wrap_list: Vec::new(),
        welcome_list: Vec::new(),
        group_info: None,
        persist: Vec::new(),
        remove: Vec::new(),
        profile_key_list: Vec::new(),
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
        ContentKind::Music,
        ContentKind::Video,
        ContentKind::Doc,
        ContentKind::File,
        ContentKind::Call,
        ContentKind::MediaBundle,
        ContentKind::Question,
        ContentKind::Answer,
        ContentKind::Sticker,
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
            ClientFrame::Pins {
                reference: ulid(REF),
                chat_list: vec![ulid(CHAT)],
            },
            json!({"type": "pins", "ref": REF, "chat_list": [CHAT]}),
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
                since: None,
                after: None,
                chat: None,
            },
            json!({"type": "sync", "ref": REF}),
        ),
        (
            ClientFrame::Sync {
                reference: ulid(REF),
                since: Some(1800),
                after: None,
                chat: Some(ulid(CHAT)),
            },
            json!({"type": "sync", "ref": REF, "since": 1800, "chat": CHAT}),
        ),
        (
            ClientFrame::Sync {
                reference: ulid(REF),
                since: None,
                after: Some(SyncPosition {
                    at: 1800,
                    chat: ulid(CHAT),
                    seq: 42,
                    n: 0,
                }),
                chat: None,
            },
            json!({"type": "sync", "ref": REF,
                   "after": {"at": 1800, "chat": CHAT, "seq": 42, "n": 0}}),
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
        client_sent_at: None,
        server_saved_at: Some(1900),
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
        draft: None,
        profile_key_list: Vec::new(),
        pin: None,
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
        pin: Some(0),
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
                call_seq: None,
            },
            json!({"type": "refused", "ref": REF, "code": "too_many_devices", "message": "No"}),
        ),
        (
            ServerFrame::Message(Box::new(removed.clone())),
            json!({"type": "message", "chat": CHAT, "seq": 5, "server_saved_at": 1900}),
        ),
        (
            ServerFrame::Chats {
                reference: ulid(REF),
                chat_list: vec![chat, marked],
                message_list: vec![removed.clone()],
                call_list: Vec::new(),
                more: true,
            },
            json!({"type": "chats", "ref": REF, "message_list": [{"chat": CHAT, "seq": 5,
                   "server_saved_at": 1900}], "call_list": [], "more": true, "chat_list": [{
                "chat": CHAT, "type": 1, "mode": 0, "last_seq": 42, "last_kind": 0, "last_at": 1800,
            }, {
                "chat": CHAT, "type": 1, "mode": 0, "last_seq": 42, "last_kind": 0, "last_at": 1800,
                "last_content_seq": 5, "epoch": 3, "last_content": {"chat": CHAT, "seq": 5,
                "server_saved_at": 1900},
                "mark_list": [{"account": ID, "received_seq": 42, "seen_seq": 41}], "pin": 0,
            }]}),
        ),
        (
            ServerFrame::Pins {
                chat_list: vec![ulid(CHAT)],
            },
            json!({"type": "pins", "chat_list": [CHAT]}),
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
            reference: ulid(REF),
            since: None,
            after: None,
            chat: None,
        }
    );
}

#[test]
fn bad_frames_are_refused() {
    let good = to_value(ClientFrame::Send(Box::new(send_frame()))).unwrap();
    for (field, value) in [
        ("kind", json!(99)),
        ("content_kind", json!(13)),
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

/// Two questions: how many, which leads on for 0 to 3 or a skip, then pick from three, where `a`
/// leads to a last question and anything else ends.
fn tree() -> serde_json::Value {
    json!({
        "first": "count",
        "nodes": [
            {"id": "count", "text": "How many?",
             "number": {"min": 0, "max": 10, "step": 0.5, "unit": "kg"},
             "next": [{"range": [0, 3], "to": "pick"}, {"skipped": true, "to": "pick"}]},
            {"id": "pick", "text": "Which?",
             "choice": {"multi": true, "options": [
                 {"id": "a", "text": "A"}, {"id": "b", "text": "B"}, {"id": "c", "text": "C"}]},
             "next": [{"picked": ["a"], "to": "last"}, {"own": true, "to": null}]},
            {"id": "last", "text": "Anything else?",
             "choice": {"options": [{"id": "no", "text": "No"}]}}
        ]
    })
}

fn question(value: serde_json::Value) -> Question {
    serde_json::from_value(value).unwrap()
}

fn answer(value: serde_json::Value) -> Answer {
    serde_json::from_value(value).unwrap()
}

#[test]
fn a_question_tree_holds_together_or_is_refused() {
    let good = question(tree());
    assert_eq!(good.check(), Ok(()));
    assert_eq!(
        to_value(&good).unwrap()["nodes"][0]["number"]["step"],
        json!(0.5)
    );

    let mut broken = tree();
    broken["first"] = json!("nowhere");
    assert_eq!(question(broken).check(), Err(FormError::Unknown));
    let mut broken = tree();
    broken["nodes"][1]["id"] = json!("count");
    assert_eq!(question(broken).check(), Err(FormError::Duplicate));
    let mut broken = tree();
    broken["nodes"][1]["choice"]["options"][1]["id"] = json!("a");
    assert_eq!(question(broken).check(), Err(FormError::Duplicate));
    let mut broken = tree();
    broken["nodes"][0]["choice"] = json!({"options": [{"id": "x", "text": "X"}]});
    assert_eq!(question(broken).check(), Err(FormError::Shape));
    let mut broken = tree();
    broken["nodes"][2]["choice"]["options"] = json!([]);
    assert_eq!(question(broken).check(), Err(FormError::Options));
    let mut broken = tree();
    let nine: Vec<_> = (0..9)
        .map(|n| json!({"id": n.to_string(), "text": "x"}))
        .collect();
    broken["nodes"][2]["choice"]["options"] = json!(nine);
    assert_eq!(question(broken).check(), Err(FormError::Options));
    let mut broken = tree();
    broken["nodes"][0]["text"] = json!("x".repeat(501));
    assert_eq!(question(broken).check(), Err(FormError::TooLong));
    let mut broken = tree();
    broken["nodes"][1]["choice"]["options"][0]["text"] = json!("x".repeat(501));
    assert_eq!(question(broken).check(), Err(FormError::TooLong));
    let mut broken = tree();
    broken["nodes"][0]["number"]["unit"] = json!("x".repeat(501));
    assert_eq!(question(broken).check(), Err(FormError::TooLong));
    let mut broken = tree();
    broken["nodes"][0]["number"]["min"] = json!(11);
    assert_eq!(question(broken).check(), Err(FormError::Range));
    let mut broken = tree();
    broken["nodes"][0]["number"]["step"] = json!(0);
    assert_eq!(question(broken).check(), Err(FormError::Range));
    let mut broken = tree();
    broken["nodes"][0]["next"][0] = json!({"range": [0, 3], "own": true, "to": "pick"});
    assert_eq!(question(broken).check(), Err(FormError::Rule));
    let mut broken = tree();
    broken["nodes"][0]["next"][0] = json!({"picked": ["a"], "to": "pick"});
    assert_eq!(question(broken).check(), Err(FormError::Rule));
    let mut broken = tree();
    broken["nodes"][1]["next"][0] = json!({"picked": ["z"], "to": "last"});
    assert_eq!(question(broken).check(), Err(FormError::Rule));
    let mut broken = tree();
    broken["nodes"][1]["next"][0] = json!({"range": [0, 1], "to": "last"});
    assert_eq!(question(broken).check(), Err(FormError::Rule));
    let mut broken = tree();
    broken["nodes"][0]["next"][0] = json!({"range": [3, 0], "to": "pick"});
    assert_eq!(question(broken).check(), Err(FormError::Rule));
    let mut broken = tree();
    broken["nodes"][1]["next"][1]["to"] = json!("gone");
    assert_eq!(question(broken).check(), Err(FormError::Unknown));
    let mut looped = tree();
    looped["nodes"][2]["next"] = json!([{"to": "count"}]);
    assert_eq!(question(looped).check(), Err(FormError::Path));
    let mut unreached = tree();
    unreached["nodes"][1]["next"][0]["to"] = json!(null);
    assert_eq!(question(unreached).check(), Err(FormError::Path));
    assert_eq!(
        question(json!({"first": "x", "nodes": []})).check(),
        Err(FormError::Nodes)
    );

    // 32 questions at most, and no path longer than 16.
    let chain = |length: usize| {
        let mut nodes = Vec::new();
        for n in 0..length {
            let to = if n + 1 < length {
                json!(format!("q{}", n + 1))
            } else {
                json!(null)
            };
            nodes.push(json!({"id": format!("q{n}"), "text": "?",
                "choice": {"options": [{"id": "y", "text": "Y"}]}, "next": [{"to": to}]}));
        }
        question(json!({"first": "q0", "nodes": nodes}))
    };
    assert_eq!(chain(16).check(), Ok(()));
    assert_eq!(chain(17).check(), Err(FormError::Deep));
    let mut wide = Vec::new();
    let mut rules = Vec::new();
    for n in 1..33 {
        wide.push(json!({"id": format!("q{n}"), "text": "?",
            "choice": {"options": [{"id": "y", "text": "Y"}]}}));
        rules.push(json!({"to": format!("q{n}")}));
    }
    wide.push(
        json!({"id": "q0", "text": "?", "choice": {"options": [{"id": "y", "text": "Y"}]},
        "next": rules}),
    );
    assert_eq!(
        question(json!({"first": "q0", "nodes": wide})).check(),
        Err(FormError::Nodes)
    );
}

#[test]
fn an_answer_follows_its_question_along_the_path_taken() {
    let tree = question(tree());
    let check = |value| tree.check_answer(&answer(value));

    // In range, then a pick that leads on, then the last question.
    assert_eq!(
        check(json!({"question": 5, "answers": [
            {"node": "count", "number": 2.5},
            {"node": "pick", "picked": ["a", "b"], "own": "and d"},
            {"node": "last", "picked": ["no"]}]})),
        Ok(())
    );
    // Written-in text alone ends the form.
    assert_eq!(
        check(json!({"question": 5, "answers": [
            {"node": "count", "number": 0},
            {"node": "pick", "own": "none of them"}]})),
        Ok(())
    );
    // A skipped question goes on by the skip rule, and the whole form can be skipped.
    assert_eq!(
        check(json!({"question": 5, "answers": [
            {"node": "count", "skipped": true},
            {"node": "pick", "skipped": true}]})),
        Ok(())
    );
    assert_eq!(check(json!({"question": 5, "skipped": true})), Ok(()));
    // Out of range or off the step, and a number past every rule ends the form.
    for number in [json!(11), json!(-1), json!(1.25)] {
        assert_eq!(
            check(json!({"question": 5, "answers": [{"node": "count", "number": number}]})),
            Err(FormError::Answer)
        );
    }
    assert_eq!(
        check(json!({"question": 5, "answers": [{"node": "count", "number": 7}]})),
        Ok(())
    );
    assert_eq!(
        check(json!({"question": 5, "answers": [{"node": "count", "own": "about seven"}]})),
        Ok(())
    );
    let wrong = [
        // Not where the path is, or stopping before it ends.
        json!([{"node": "pick", "picked": ["a"]}]),
        json!([{"node": "count", "number": 1}]),
        json!([{"node": "count", "number": 7}, {"node": "pick", "picked": ["a"]}]),
        // Nothing given, a number on a choice, a pick on a number, an unknown option.
        json!([{"node": "count"}]),
        json!([{"node": "count", "picked": ["a"]}]),
        json!([{"node": "count", "number": 1}, {"node": "pick", "number": 1}]),
        json!([{"node": "count", "number": 1}, {"node": "pick", "picked": ["z"]}]),
        json!([{"node": "count", "number": 1}, {"node": "pick"}]),
        // A skip that carries an answer.
        json!([{"node": "count", "skipped": true, "number": 1}]),
        // One pick only where the choice is not multi.
        json!([{"node": "count", "number": 1}, {"node": "pick", "picked": ["a"]},
               {"node": "last", "picked": ["no", "no"]}]),
    ];
    for answers in wrong {
        assert_eq!(
            check(json!({"question": 5, "answers": answers})),
            Err(FormError::Answer),
            "{answers}"
        );
    }
    assert_eq!(
        check(json!({"question": 5, "answers": [{"node": "count", "own": "x".repeat(2001)}]})),
        Err(FormError::TooLong)
    );
    assert_eq!(
        check(json!({"question": 5, "skipped": true, "answers": [{"node": "count", "number": 1}]})),
        Err(FormError::Answer)
    );
}
