//! Drives core as a real client against a running server, for the phone test: it sends what a phone
//! cannot, such as a question. Not part of the server, and not a test. Reads one command per line on
//! stdin: `signup` or `login` with a username, password and words, `find <username>`, `say <text>`,
//! `many <n>`, `seen`, `call`, `question`, `pump <ms>` and `quit`.

use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use keryx_core::client::{Auth, Client, Command, Event, Input, Output, Platform, Secret};
use keryx_core::protocol::{ChatType, MessageFrame};
use keryx_core::ulid::Ulid;
use serde_json::{Value, json};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{ClientRequestBuilder, Message, WebSocket};

const ORIGIN: &str = "http://127.0.0.1:7778";

struct App {
    client: Client,
    socket: Option<WebSocket<MaybeTlsStream<TcpStream>>>,
    slots: [Option<Vec<u8>>; 2],
    wake_at: Option<i64>,
    token: Option<String>,
    events: Vec<Event>,
    archive: BTreeMap<(Ulid, u64), MessageFrame>,
    chat: Option<Ulid>,
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .new_agent()
}

fn call(method: &str, route: &str, token: Option<&str>, body: Option<&Value>) -> (u16, String) {
    let url = format!("{ORIGIN}{route}");
    let authorization = token.map(|token| format!("Bearer {token}"));
    let mut response = if method == "GET" || method == "DELETE" {
        let mut request = if method == "DELETE" {
            agent().delete(&url)
        } else {
            agent().get(&url)
        };
        if let Some(authorization) = &authorization {
            request = request.header("Authorization", authorization);
        }
        request.call().unwrap()
    } else {
        let mut request = if method == "PUT" {
            agent().put(&url)
        } else {
            agent().post(&url)
        };
        if let Some(authorization) = &authorization {
            request = request.header("Authorization", authorization);
        }
        match body {
            Some(body) => request.send_json(body).unwrap(),
            None => request.send_empty().unwrap(),
        }
    };
    let status = response.status().as_u16();
    let reply = response.body_mut().read_to_string().unwrap();
    (status, reply)
}

impl App {
    fn new() -> Self {
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
            events: Vec::new(),
            archive: BTreeMap::new(),
            chat: None,
        }
    }

    fn command(&mut self, command: Command) {
        self.feed(Input::Command { command });
    }

    fn feed(&mut self, input: Input) {
        let outputs = self.client.step(input, Ulid::now());
        self.run(outputs);
    }

    fn run(&mut self, outputs: Vec<Output>) {
        let mut todo: std::collections::VecDeque<Output> = outputs.into();
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
                    let parsed = body
                        .as_ref()
                        .map(|body| serde_json::from_str(&body.0).unwrap());
                    let (status, reply) = call(
                        &method,
                        &path,
                        bearer.as_ref().map(|b| b.0.as_str()),
                        parsed.as_ref(),
                    );
                    if path == "/api/v1/login/token" && status == 200 {
                        let value: Value = serde_json::from_str(&reply).unwrap();
                        self.token = Some(value["data"]["token"].as_str().unwrap().to_string());
                    }
                    Some(Input::Reply {
                        id,
                        status,
                        body: reply,
                    })
                }
                Output::Connect { bearer } => {
                    let url = format!("{}/api/v1/ws", ORIGIN.replace("http://", "ws://"));
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
                        Err(error) => {
                            eprintln!("connect failed: {error}");
                            Some(Input::Closed {
                                code: 1006,
                                reason: String::new(),
                            })
                        }
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
                Output::ReadChunk { .. }
                | Output::PutChunk { .. }
                | Output::GetChunk { .. }
                | Output::WriteChunk { .. } => None,
                Output::JoinCall { .. }
                | Output::CallKey { .. }
                | Output::LeaveCall { .. }
                | Output::CallText { .. } => None,
                Output::SaveRows {
                    id,
                    message_list,
                    call_list: _,
                } => {
                    for message in message_list {
                        let key = (message.chat, message.seq);
                        self.archive.entry(key).or_insert(message);
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
                    Some(Input::RowsRead {
                        id,
                        message_list,
                        call_list: Vec::new(),
                    })
                }
                Output::Event { event } => {
                    if let Event::Chats { chat_list } = &event {
                        for item in chat_list {
                            if item.chat_type == ChatType::Direct {
                                self.chat = Some(item.chat);
                            }
                        }
                    }
                    if let Event::Started { chat, .. } = &event {
                        self.chat = Some(*chat);
                    }
                    self.events.push(event);
                    None
                }
            };
            if let Some(input) = input {
                let more = self.client.step(input, Ulid::now());
                todo.extend(more);
            }
        }
    }

    fn read_once(&mut self) {
        if let Some(at) = self.wake_at
            && Ulid::now() >= at
        {
            self.wake_at = None;
            self.feed(Input::Timer);
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
        self.feed(input);
    }

    fn wait(&mut self, wanted: impl Fn(&Event) -> bool) -> Event {
        let started = Instant::now();
        loop {
            for index in 0..self.events.len() {
                if wanted(&self.events[index]) {
                    return self.events.remove(index);
                }
                if let Event::Error { code, message } = &self.events[index] {
                    panic!("client failed: {code}: {message}");
                }
            }
            if started.elapsed() > Duration::from_secs(30) {
                panic!("timed out, holding {} events", self.events.len());
            }
            self.read_once();
        }
    }

    fn pump(&mut self, ms: u64) {
        let started = Instant::now();
        while started.elapsed() < Duration::from_millis(ms) {
            self.read_once();
        }
    }
}

fn main() {
    let mut app = App::new();
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let line = line.unwrap();
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(2, ' ');
        let verb = parts.next().unwrap();
        let rest = parts.next().unwrap_or("");
        match verb {
            "signup" => {
                let mut bits = rest.splitn(3, ' ');
                let username = bits.next().unwrap();
                let password = bits.next().unwrap();
                let words = bits.next().unwrap();
                app.command(Command::SignUp {
                    username: username.to_string(),
                    password: Secret(password.to_string()),
                    display_name: None,
                    captcha: None,
                });
                app.wait(|event| matches!(event, Event::NeedWords { restore: false }));
                app.command(Command::SetWords {
                    words: Secret(words.to_string()),
                });
                app.wait(|event| matches!(event, Event::ChatReady { .. }));
                println!("ready {}", username);
            }
            "login" => {
                let mut bits = rest.splitn(3, ' ');
                let username = bits.next().unwrap();
                let password = bits.next().unwrap();
                let words = bits.next().unwrap();
                app.command(Command::LogIn {
                    username: username.to_string(),
                    password: Secret(password.to_string()),
                    captcha: None,
                });
                app.wait(|event| matches!(event, Event::NeedWords { .. }));
                app.command(Command::Restore {
                    words: Secret(words.to_string()),
                });
                app.wait(|event| matches!(event, Event::Online));
                app.pump(1500);
                println!("online {}", username);
            }
            "find" => {
                app.command(Command::Search {
                    text: rest.to_string(),
                });
                let event = app.wait(|event| matches!(event, Event::SearchResults { .. }));
                let Event::SearchResults { account_list } = event else {
                    unreachable!()
                };
                let Some(account) = account_list.iter().find(|item| item.username == rest) else {
                    println!("missing {}", rest);
                    continue;
                };
                println!("found {} {}", account.username, account.id);
                app.command(Command::StartChat {
                    account: account.id,
                });
                app.wait(|event| matches!(event, Event::ChatReady { .. }));
                println!("chat {}", app.chat.unwrap());
            }
            "say" => {
                let chat = app.chat.expect("no chat");
                app.command(Command::SendText {
                    chat,
                    text: Secret(rest.to_string()),
                    reply: None,
                });
                app.wait(|event| matches!(event, Event::Sent { .. }));
                println!("sent");
            }
            "many" => {
                let n: u32 = rest.parse().unwrap();
                let chat = app.chat.expect("no chat");
                for i in 1..=n {
                    app.command(Command::SendText {
                        chat,
                        text: Secret(format!("note {i}")),
                        reply: None,
                    });
                    app.wait(|event| matches!(event, Event::Sent { .. }));
                    if i % 25 == 0 {
                        println!("sent {i}");
                        let _ = std::io::stdout().flush();
                    }
                }
                println!("sent {n}");
            }
            "seen" => {
                let chat = app.chat.expect("no chat");
                app.pump(500);
                let up_to = app
                    .archive
                    .range((chat, 0)..(chat, u64::MAX))
                    .next_back()
                    .map(|(_, message)| message.seq)
                    .unwrap_or(1);
                app.command(Command::Seen {
                    chat,
                    up_to_seq: up_to,
                });
                app.pump(800);
                println!("seen {up_to}");
            }
            "call" => {
                let chat = app.chat.expect("no chat");
                app.command(Command::StartCall {
                    chat,
                    video: false,
                    voice: true,
                });
                app.wait(|event| matches!(event, Event::Sent { .. } | Event::Message { .. }));
                println!("ringing");
            }
            "question" => {
                let chat = app.chat.expect("no chat");
                app.command(Command::SendQuestion {
                    chat,
                    text: Some(Secret("A few questions about lunch".to_string())),
                    question: serde_json::from_value(sample_question()).unwrap(),
                });
                let Event::Sent { seq, .. } = app.wait(|event| matches!(event, Event::Sent { .. }))
                else {
                    unreachable!()
                };
                println!("asked {seq}");
            }
            "pump" => {
                let ms: u64 = rest.parse().unwrap_or(1000);
                app.pump(ms);
                println!("pumped");
            }
            "quit" => break,
            other => println!("unknown {other}"),
        }
        let _ = std::io::stdout().flush();
    }
}

/// A tree that takes every branch a form can: a single choice whose pick, written-in text or skip each
/// lead somewhere else, a multi-select, and a number in halves whose range decides what comes next.
fn sample_question() -> Value {
    json!({
        "first": "where",
        "nodes": [
            {"id": "where", "text": "Where should we eat?",
             "choice": {"multi": false, "options": [
                 {"id": "in", "text": "In the office"},
                 {"id": "out", "text": "Somewhere nearby"}]},
             "next": [
                 {"picked": ["in"], "to": "what"},
                 {"picked": ["out"], "to": "walk"},
                 {"own": true, "to": "what"},
                 {"skipped": true, "to": null}]},
            {"id": "walk", "text": "How far would you walk?",
             "number": {"min": 0, "max": 5, "step": 0.5, "unit": "km"},
             "next": [{"range": [0, 1], "to": "what"}, {"to": "sure"}]},
            {"id": "sure", "text": "That is far. Still want to go?",
             "choice": {"multi": false, "options": [
                 {"id": "yes", "text": "Yes"}, {"id": "no", "text": "No"}]},
             "next": [{"picked": ["yes"], "to": "what"}]},
            {"id": "what", "text": "What would you like?",
             "choice": {"multi": true, "options": [
                 {"id": "rice", "text": "Rice"},
                 {"id": "soup", "text": "Soup"},
                 {"id": "salad", "text": "Salad"},
                 {"id": "bread", "text": "Bread"}]}}
        ]
    })
}
