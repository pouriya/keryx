//! A person: `keryx-core`'s [`Client`] driven against the real server the way an app drives it, and
//! the raw pieces the tests that break rules sign with by hand, once the client has set things up.
//! Apart from `mod.rs`, because a test file that has no use for it would fail clippy on dead code,
//! and `#[allow]` is not used here.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use keryx_core::client::{Auth, Client, Command, Event, Input, Output, Platform, Secret};
use keryx_core::mls::{AccountKey, Device, stamp_account_key};
use keryx_core::protocol::{Bytes, CallEvent, ChatType, MessageFrame};
use keryx_core::seed::{AccountKeys, derive};
use keryx_core::ulid::Ulid;
use serde_json::{Value, json};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{ClientRequestBuilder, Message, WebSocket};

use crate::common::Server;

pub const PASSWORD: &str = "a long enough password";
pub const SARA_WORDS: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
pub const BABAK_WORDS: &str =
    "legal winner thank year wave sausage worth useful legal winner thank yellow";

/// How long a test waits for something before failing.
const PATIENCE: Duration = Duration::from_secs(20);

/// The client, with what an app does around it: the requests, the socket, the two slots and the timer.
pub struct App {
    pub client: Client,
    socket: Option<WebSocket<MaybeTlsStream<TcpStream>>>,
    pub slots: [Option<Vec<u8>>; 2],
    wake_at: Option<i64>,
    pub token: Option<String>,
    pub self_chat: Option<Ulid>,
    /// Shown and not yet waited for.
    pub events: VecDeque<Event>,
    /// The files this app holds, by handle: what it sends, and what it downloads into.
    pub files: HashMap<u64, Vec<u8>>,
    /// What the client asked of the platform's call code, in order.
    pub call_outputs: VecDeque<Output>,
    /// The archive an app keeps on disk: every message and call row as the server sent it.
    pub archive: BTreeMap<(Ulid, u64), MessageFrame>,
    pub archive_calls: BTreeMap<(Ulid, u64, u64), CallEvent>,
}

/// One request with raw bytes each way, as file chunks go: the status and the reply's bytes.
pub fn raw(
    server: &Server,
    method: &str,
    route: &str,
    token: &str,
    body: Option<&[u8]>,
) -> (u16, Vec<u8>) {
    let agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .new_agent();
    let url = format!("{}{route}", server.origin);
    let authorization = format!("Bearer {token}");
    let mut response = match body {
        Some(body) => {
            let request = match method {
                "PUT" => agent.put(&url),
                _ => agent.post(&url),
            };
            request
                .header("Authorization", &authorization)
                .send(body)
                .unwrap()
        }
        None => agent
            .get(&url)
            .header("Authorization", &authorization)
            .call()
            .unwrap(),
    };
    let status = response.status().as_u16();
    let reply = response
        .body_mut()
        .with_config()
        .limit(4 * 1024 * 1024)
        .read_to_vec()
        .unwrap();
    (status, reply)
}

impl App {
    pub fn new() -> Self {
        let platform = Platform {
            platform: 0,
            os: "linux".to_string(),
            os_version: "7".to_string(),
            web_platform: None,
            web_platform_version: None,
        };
        Self {
            client: Client::new(Auth::Token, platform),
            socket: None,
            slots: [None, None],
            wake_at: None,
            token: None,
            self_chat: None,
            events: VecDeque::new(),
            files: HashMap::new(),
            call_outputs: VecDeque::new(),
            archive: BTreeMap::new(),
            archive_calls: BTreeMap::new(),
        }
    }

    /// Registers, logs in, sets up the words, and waits for the self chat to be ready.
    pub fn sign_up(server: &Server, username: &str, words: &str) -> Self {
        let mut app = Self::new();
        app.command(
            server,
            Command::SignUp {
                username: username.to_string(),
                password: Secret(PASSWORD.to_string()),
                display_name: None,
                captcha: None,
            },
        );
        app.wait(server, |event| {
            matches!(event, Event::NeedWords { restore: false })
        });
        app.command(
            server,
            Command::SetWords {
                words: Secret(words.to_string()),
            },
        );
        app.wait(server, |event| matches!(event, Event::ChatReady { .. }));
        app
    }

    pub fn id(&self) -> Ulid {
        self.client.account().unwrap()
    }

    pub fn command(&mut self, server: &Server, command: Command) {
        self.feed(server, Input::Command { command });
    }

    pub fn feed(&mut self, server: &Server, input: Input) {
        let outputs = self.client.step(input, Ulid::now());
        self.run(server, outputs);
    }

    /// Does what the client asked, feeding each answer back, until it asks for nothing more.
    fn run(&mut self, server: &Server, outputs: Vec<Output>) {
        let mut todo: VecDeque<Output> = outputs.into();
        while let Some(output) = todo.pop_front() {
            let input = match output {
                Output::Send { text } => {
                    if let Some(socket) = self.socket.as_mut() {
                        let _ = socket.send(Message::text(text.0.clone()));
                    }
                    None
                }
                Output::Request {
                    id,
                    method,
                    path,
                    body,
                    bearer,
                } => {
                    if let Some(bearer) = &bearer {
                        self.token = Some(bearer.0.clone());
                    }
                    let body: Option<Value> = body
                        .as_ref()
                        .map(|body| serde_json::from_str(&body.0).unwrap());
                    let bearer = bearer.as_ref().map(|bearer| bearer.0.as_str());
                    let (status, reply) = server.call(&method, &path, bearer, body.as_ref());
                    if path == "/api/v1/login/token" && status == 200 {
                        self.token = Some(reply["data"]["token"].as_str().unwrap().to_string());
                    }
                    Some(Input::Reply {
                        id,
                        status,
                        body: reply.to_string(),
                    })
                }
                Output::Connect { bearer } => {
                    let url = format!("{}/api/v1/ws", server.origin.replace("http://", "ws://"));
                    let mut request = ClientRequestBuilder::new(url.parse().unwrap())
                        .with_sub_protocol("keryx.v1");
                    if let Some(bearer) = &bearer {
                        request =
                            request.with_header("Authorization", format!("Bearer {}", bearer.0));
                    }
                    match tungstenite::connect(request) {
                        Ok((socket, _)) => {
                            if let MaybeTlsStream::Plain(stream) = socket.get_ref() {
                                stream
                                    .set_read_timeout(Some(Duration::from_millis(20)))
                                    .unwrap();
                            }
                            self.socket = Some(socket);
                            Some(Input::Connected)
                        }
                        Err(_) => Some(Input::Closed {
                            code: 1006,
                            reason: String::new(),
                        }),
                    }
                }
                Output::Disconnect => {
                    self.socket = None;
                    None
                }
                Output::Save { id, slot, bytes } => {
                    self.slots[usize::from(slot)] = Some(bytes);
                    Some(Input::Saved { id })
                }
                Output::Wipe => {
                    self.slots = [None, None];
                    None
                }
                Output::WakeAt { at } => {
                    self.wake_at = Some(at);
                    None
                }
                Output::ReadChunk {
                    handle,
                    n,
                    offset,
                    length,
                } => {
                    let held = &self.files[&handle];
                    let start = usize::try_from(offset).unwrap();
                    let end = start + usize::try_from(length).unwrap();
                    Some(Input::ChunkRead {
                        handle,
                        n,
                        bytes: held[start..end].to_vec(),
                    })
                }
                Output::PutChunk {
                    id,
                    method,
                    path,
                    bearer,
                    bytes,
                } => {
                    let token = bearer.map(|bearer| bearer.0.clone()).unwrap_or_default();
                    let (status, reply) = raw(server, &method, &path, &token, Some(&bytes));
                    Some(Input::Reply {
                        id,
                        status,
                        body: String::from_utf8(reply).unwrap(),
                    })
                }
                Output::GetChunk { id, path, bearer } => {
                    let token = bearer.map(|bearer| bearer.0.clone()).unwrap_or_default();
                    let (status, bytes) = raw(server, "GET", &path, &token, None);
                    Some(Input::Fetched { id, status, bytes })
                }
                Output::WriteChunk {
                    handle,
                    offset,
                    bytes,
                    ..
                } => {
                    let held = self.files.entry(handle).or_default();
                    let start = usize::try_from(offset).unwrap();
                    if held.len() < start + bytes.len() {
                        held.resize(start + bytes.len(), 0);
                    }
                    held[start..start + bytes.len()].copy_from_slice(&bytes);
                    None
                }
                call @ (Output::JoinCall { .. }
                | Output::CallKey { .. }
                | Output::LeaveCall { .. }
                | Output::CallText { .. }) => {
                    self.call_outputs.push_back(call);
                    None
                }
                Output::SaveRows {
                    id,
                    message_list,
                    call_list,
                } => {
                    for message in message_list {
                        let key = (message.chat, message.seq);
                        if message.kind.is_none() {
                            self.archive_calls
                                .retain(|(chat, call_seq, _), _| (*chat, *call_seq) != key);
                            self.archive.insert(key, message);
                        } else {
                            self.archive.entry(key).or_insert(message);
                        }
                    }
                    for event in call_list {
                        self.archive_calls
                            .entry((event.chat, event.call_seq, event.n))
                            .or_insert(event);
                    }
                    Some(Input::RowsSaved { id })
                }
                Output::ReadRows {
                    id,
                    chat,
                    before_seq,
                    limit,
                } => {
                    let before = before_seq.unwrap_or(u64::MAX);
                    let mut message_list: Vec<MessageFrame> = self
                        .archive
                        .range((chat, 0)..(chat, before))
                        .rev()
                        .take(usize::try_from(limit).unwrap())
                        .map(|(_, message)| message.clone())
                        .collect();
                    message_list.reverse();
                    let call_list = self
                        .archive_calls
                        .iter()
                        .filter(|((c, call_seq, _), _)| {
                            *c == chat && message_list.iter().any(|m| m.seq == *call_seq)
                        })
                        .map(|(_, event)| event.clone())
                        .collect();
                    Some(Input::RowsRead {
                        id,
                        message_list,
                        call_list,
                    })
                }
                Output::Event { event } => {
                    if let Event::Chats { chat_list } = &event {
                        for item in chat_list {
                            if item.chat_type == ChatType::SelfChat {
                                self.self_chat = Some(item.chat);
                            }
                        }
                    }
                    self.events.push_back(event);
                    None
                }
            };
            if let Some(input) = input {
                let more = self.client.step(input, Ulid::now());
                todo.extend(more);
            }
        }
    }

    /// Reads one frame if one comes soon, and fires the timer when it is due.
    pub fn read_once(&mut self, server: &Server) {
        if let Some(at) = self.wake_at
            && Ulid::now() >= at
        {
            self.wake_at = None;
            self.feed(server, Input::Timer);
        }
        let Some(socket) = self.socket.as_mut() else {
            std::thread::sleep(Duration::from_millis(20));
            return;
        };
        let input = match socket.read() {
            Ok(Message::Text(text)) => Input::Frame {
                text: text.as_str().to_string(),
            },
            Ok(Message::Close(frame)) => {
                self.socket = None;
                match frame {
                    Some(frame) => Input::Closed {
                        code: u16::from(frame.code),
                        reason: frame.reason.to_string(),
                    },
                    None => Input::Closed {
                        code: 1005,
                        reason: String::new(),
                    },
                }
            }
            Ok(_) => return,
            Err(tungstenite::Error::Io(error))
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                return;
            }
            Err(_) => {
                self.socket = None;
                Input::Closed {
                    code: 1006,
                    reason: String::new(),
                }
            }
        };
        self.feed(server, input);
    }

    /// The first event that matches, reading the socket until it comes. Others are kept for later,
    /// and an error event fails the test unless it is what was wanted.
    pub fn wait(&mut self, server: &Server, wanted: impl Fn(&Event) -> bool) -> Event {
        let started = Instant::now();
        loop {
            for index in 0..self.events.len() {
                if wanted(&self.events[index]) {
                    return self.events.remove(index).unwrap();
                }
                if let Event::Error { code, message } = &self.events[index] {
                    panic!("The client failed: {code}: {message}");
                }
            }
            assert!(
                started.elapsed() < PATIENCE,
                "Waited too long, holding {:?}",
                self.events
            );
            self.read_once(server);
        }
    }

    /// The person behind this app, holding the device the client set up, for signing by hand.
    pub fn into_person(self, words: &str) -> Person {
        assert!(
            self.slots.iter().any(Option::is_some),
            "The client saved nothing"
        );
        let id = self.id();
        let username = String::new();
        let token = self.token.clone().unwrap();
        let self_chat = self.self_chat.unwrap();
        let device = self.client.into_device().unwrap();
        let keys = derive(words).unwrap();
        let key = AccountKey {
            account: id,
            account_public: *keys.account_public(),
            master_public: *keys.master_public(),
            stamp: stamp_account_key(&keys, &id),
        };
        Person {
            id,
            username,
            keys,
            key,
            self_chat,
            token,
            device,
        }
    }
}

pub struct Person {
    pub id: Ulid,
    pub username: String,
    pub keys: AccountKeys,
    pub key: AccountKey,
    pub self_chat: Ulid,
    pub token: String,
    pub device: Device,
}

pub fn ulid_of(value: &Value) -> Ulid {
    Ulid::parse(value.as_str().unwrap()).unwrap()
}

/// A password login, with no device.
pub fn log_in(server: &Server, username: &str) -> String {
    let (status, reply) = server.call(
        "POST",
        "/api/v1/login/token",
        None,
        Some(&json!({"username": username, "password": PASSWORD})),
    );
    assert_eq!(status, 200, "{reply}");
    reply["data"]["token"].as_str().unwrap().to_string()
}

/// Asks for an id, stamps it, and registers it on this session, by hand.
pub fn add_device(server: &Server, account: Ulid, keys: &AccountKeys, token: &str) -> Device {
    let (status, reply) = server.call("POST", "/api/v1/devices/id", Some(token), None);
    assert_eq!(status, 200, "{reply}");
    let id = ulid_of(&reply["data"]["device"]);
    let device = Device::create(keys, account, id).unwrap();
    let record = device.record();
    let (status, reply) = server.call(
        "POST",
        "/api/v1/devices",
        Some(token),
        Some(&json!({
            "device": id,
            "public_key": Bytes(record.device_public.to_vec()),
            "stamp": Bytes(record.stamp.to_vec()),
            "platform": 0,
            "os": "linux",
            "os_version": "7",
        })),
    );
    assert_eq!(status, 200, "{reply}");
    device
}

/// Signs up through the client, then hands back the person for the tests that sign by hand.
pub fn sign_up(server: &Server, username: &str, words: &str) -> Person {
    let app = App::sign_up(server, username, words);
    let mut person = app.into_person(words);
    person.username = username.to_string();
    person
}
