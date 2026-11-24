//! Drives core as a real client against a running server, for the phone test: it sends what a phone
//! cannot, such as a question. Not part of the server, and not a test. Its one argument is the server,
//! `http://127.0.0.1:7778` when left out, which agent/tests give their own. Reads one command per line on
//! stdin: `signup` or `login` with a username, password and words, `find <username>`, `say <text>`,
//! `many <n>`, `seen`, `call`, `question`, `bio <text>`, `link <name> <url>`, `photo <file.jpg>`,
//! `app <on|off>`, `kv <key> <json>`, `react <seq> <emoji>`, `delete <seq>`, `sticker [n]`,
//! `suggest move|copy <folder> [text]`, `pump <ms>` and `quit`.
//!
//! Groups and channels, on the chat in use, which `group`, `channel` and `use` set:
//! `group <name> [username ...] [-- note]`, `channel <name> [username ...] [-- note]`, `use <chat id>`,
//! `add <username ...> [-- note]`, `remove <username ...>`, `role <username> owner|moderator|member`, `leave`,
//! `removechat`, `findchat <text>`, `ask <chat id> [note]`, `cancelask <chat id>`, `requests`,
//! `letin <username>`, `decline <username>`, `invite yes|no`, `topics on|off`, `topic <name>`,
//! `in <topic name>|main`, `thread <seq>|off`, `mention <username> <text>`, `chatprofile <name>`,
//! `options <json>`, `findable on <name>|off`, and on the topic in use, General when none,
//! `topicbio <text> [-- rules]`, `topicphoto <file.jpg>`, `topicopts <json>` and
//! `topicmod <username> on|off`. `chatprofile <name> [-- rules]` sets the chat's rules too. `say`, `sticker` and `seen` go to the topic and
//! thread in use. A note after a lone `--` goes in the invitation of anyone added by invitation only.
//! What arrives for groups is printed as it comes: `ready`, `left`, `line`, `request`, `invited`,
//! `topics`, `profile` and `findable`, with ids and counts only.
//!
//! As an agent's owner: `agent add <username> [--admin] [permission ...]`, which prints its token and
//! words once, `agent list`, `agent perms <username> [permission ...]`, `agent token renew|revoke
//! <username> -- <words>`, `agent profile <username> [bio] [-- photo.jpg]` and `agent disable
//! <username> -- <words>`. A renew, revoke or disable signs the agent's device's end with its words.

use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::net::TcpStream;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use keryx_core::client::{
    Auth, Client, Command, Event, Input, Output, Platform, Pointer, Secret, Setting, Thumbnail,
};
use keryx_core::protocol::{
    ChatOptions, ChatRole, ChatType, FolderSuggestion, GENERAL_TOPIC, MessageFrame, ProfileLink,
    Suggestion, TopicName,
};
use keryx_core::ulid::Ulid;
use serde_json::{Value, json};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{ClientRequestBuilder, Message, WebSocket};

/// The server: the first argument, or the phone test's, which `adb reverse` reaches.
static ORIGIN: OnceLock<String> = OnceLock::new();

fn origin() -> &'static str {
    ORIGIN.get_or_init(|| {
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "http://127.0.0.1:7778".to_string())
    })
}

struct App {
    client: Client,
    socket: Option<WebSocket<MaybeTlsStream<TcpStream>>>,
    slots: [Option<Vec<u8>>; 2],
    wake_at: Option<i64>,
    token: Option<String>,
    events: Vec<Event>,
    archive: BTreeMap<(Ulid, u64), MessageFrame>,
    chat: Option<Ulid>,
    /// This account's bio and links as last saved here, since saving one sends both.
    bio: Option<String>,
    link_list: Vec<ProfileLink>,
    /// Each group's topics as last shown, and whether they are on, since `topics` sends them all.
    topics: BTreeMap<Ulid, (bool, Vec<TopicName>)>,
    /// Where `say`, `sticker` and `seen` go inside the chat in use.
    topic: Option<Ulid>,
    thread: Option<u64>,
    /// The newest invitation, which `invite yes|no` answers.
    invited: Option<Ulid>,
}

/// The owner's agent with this username, from a fresh list.
fn agent_named(app: &mut App, username: &str) -> Option<Ulid> {
    app.events
        .retain(|event| !matches!(event, Event::Agents { .. }));
    app.command(Command::Agents);
    let Event::Agents { agent_list, .. } = app.wait(|event| matches!(event, Event::Agents { .. }))
    else {
        unreachable!()
    };
    let found = agent_list
        .iter()
        .find(|item| item.username == username)
        .map(|item| item.id);
    if found.is_none() {
        println!("missing {username}");
    }
    found
}

/// `agent ...`: what an agent's owner does.
fn agent_command(app: &mut App, rest: &str) {
    let (verb, rest) = rest.split_once(' ').unwrap_or((rest, ""));
    match verb {
        "add" => {
            let mut bits = rest.split_whitespace();
            let Some(username) = bits.next() else {
                return println!("agent add <username> [--admin] [permission ...]");
            };
            let mut admin = false;
            let mut permission_list = Vec::new();
            for bit in bits {
                if bit == "--admin" {
                    admin = true;
                } else {
                    permission_list.push(bit.to_string());
                }
            }
            app.command(Command::CreateAgent {
                username: username.to_string(),
                display_name: username.to_string(),
                permission_list,
                admin,
            });
            app.pump_until(|event| matches!(event, Event::AgentCreated { .. }));
            for event in std::mem::take(&mut app.events) {
                match event {
                    // Shown once, for the agent's own agent.toml: nothing here keeps them.
                    Event::AgentCreated {
                        account,
                        token,
                        words,
                        ..
                    } => {
                        println!("agent {account}");
                        println!("token {}", token.0);
                        println!("words {}", words.0);
                    }
                    Event::Error { code, .. } => println!("refused {code}"),
                    other => app.events.push(other),
                }
            }
        }
        "list" => {
            app.command(Command::Agents);
            let Event::Agents { agent_list, .. } =
                app.wait(|event| matches!(event, Event::Agents { .. }))
            else {
                unreachable!()
            };
            for item in agent_list {
                println!(
                    "agent {} {} admin={} disabled={} token={} device={} perms={}",
                    item.id,
                    item.username,
                    item.admin,
                    item.disabled,
                    item.token_at.is_some(),
                    item.device.is_some(),
                    item.permission_list.join(",")
                );
            }
        }
        "perms" => {
            let mut bits = rest.split_whitespace();
            let Some(agent) = bits.next().and_then(|name| agent_named(app, name)) else {
                return;
            };
            app.command(Command::UpdateAgent {
                agent,
                display_name: None,
                permission_list: Some(bits.map(str::to_string).collect()),
                admin: None,
            });
            app.settle();
            println!("saved");
        }
        "token" | "disable" => {
            let (head, words) = match rest.split_once(" -- ") {
                Some((head, words)) => (head, words.to_string()),
                None => return println!("agent token|disable ... -- <words>"),
            };
            let (action, username) = if verb == "disable" {
                ("disable", head.trim())
            } else {
                head.split_once(' ').unwrap_or((head, ""))
            };
            let Some(agent) = agent_named(app, username.trim()) else {
                return;
            };
            let words = Secret(words);
            match action {
                "renew" => {
                    app.command(Command::RenewAgentToken { agent, words });
                    app.pump_until(|event| matches!(event, Event::AgentToken { .. }));
                    for event in std::mem::take(&mut app.events) {
                        match event {
                            Event::AgentToken { token, .. } => println!("token {}", token.0),
                            Event::Error { code, .. } => println!("refused {code}"),
                            other => app.events.push(other),
                        }
                    }
                }
                "revoke" => {
                    app.command(Command::RevokeAgentToken { agent, words });
                    app.settle();
                    println!("revoked");
                }
                "disable" => {
                    app.command(Command::DisableAgent { agent, words });
                    app.settle();
                    println!("disabled");
                }
                other => println!("unknown {other}"),
            }
        }
        "profile" => {
            let (head, photo) = match rest.split_once(" -- ") {
                Some((head, photo)) => (head, Some(photo.trim().to_string())),
                None => (rest, None),
            };
            let (username, bio) = head.split_once(' ').unwrap_or((head, ""));
            let Some(agent) = agent_named(app, username.trim()) else {
                return;
            };
            app.pump(1000);
            if let Some(path) = photo {
                let jpeg = std::fs::read(path).unwrap();
                let mut rgba = Vec::new();
                for _ in 0..16 {
                    rgba.extend_from_slice(&[0x45, 0x76, 0xC4, 0xFF]);
                }
                app.command(Command::AgentPhoto {
                    agent,
                    jpeg,
                    thumbnail: Thumbnail {
                        width: 4,
                        height: 4,
                        rgba,
                    },
                });
                app.pump_until(|event| matches!(event, Event::ProfileSaved));
            }
            let bio = bio.trim();
            if !bio.is_empty() {
                app.command(Command::AgentProfile {
                    agent,
                    bio: Some(bio.to_string()),
                    link_list: Vec::new(),
                });
                app.pump_until(|event| matches!(event, Event::ProfileSaved));
            }
            app.settle();
            println!("saved");
        }
        other => println!("unknown agent {other}"),
    }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .new_agent()
}

fn call(method: &str, route: &str, token: Option<&str>, body: Option<&Value>) -> (u16, String) {
    let url = format!("{}{route}", origin());
    let authorization = token.map(|token| format!("Bearer {token}"));
    let mut response = if method == "GET" || (method == "DELETE" && body.is_none()) {
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
        // A revoke's DELETE carries the agent's device's end.
        let mut request = match method {
            "PUT" => agent().put(&url),
            "DELETE" => agent().delete(&url).force_send_body(),
            _ => agent().post(&url),
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
            bio: None,
            link_list: Vec::new(),
            topics: BTreeMap::new(),
            topic: None,
            thread: None,
            invited: None,
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
                    let url = format!("{}/api/v1/ws", origin().replace("http://", "ws://"));
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
                // One account per client here, so its kept slots are that account's.
                Output::Open { .. } => {
                    let [first, second] = self.slots.clone();
                    Some(Input::Opened { first, second })
                }
                Output::WakeAt { at } => {
                    self.wake_at = Some(at);
                    None
                }
                // Only a photo goes up from here, as the raw sealed bytes.
                Output::PutChunk {
                    id,
                    method,
                    path,
                    bearer,
                    bytes,
                } => {
                    let url = format!("{}{path}", origin());
                    let mut request = if method == "PUT" {
                        agent().put(&url)
                    } else {
                        agent().post(&url)
                    };
                    request = request.header("Content-Type", "application/octet-stream");
                    if let Some(bearer) = &bearer {
                        request = request.header("Authorization", format!("Bearer {}", bearer.0));
                    }
                    let mut response = request.send(&bytes[..]).unwrap();
                    let status = response.status().as_u16();
                    let body = response.body_mut().read_to_string().unwrap();
                    Some(Input::Reply { id, status, body })
                }
                Output::ReadChunk { .. } | Output::GetChunk { .. } | Output::WriteChunk { .. } => {
                    None
                }
                Output::JoinCall { .. }
                | Output::CallKey { .. }
                | Output::LeaveCall { .. }
                | Output::CallText { .. } => None,
                // Nothing here shows a chat's history, so its call rows and events are not kept.
                Output::SaveRows {
                    id, message_list, ..
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
                        chat_event_list: Vec::new(),
                    })
                }
                // Nothing here searches its archive.
                Output::ScanRows { id, .. } => Some(Input::RowsScanned {
                    id,
                    message_list: Vec::new(),
                    total: 0,
                }),
                Output::Event { event } => {
                    let event = *event;
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
                    // An ack to a suggestion this account sent, printed as it arrives.
                    if let Event::Message { message } = &event
                        && !message.mine
                        && let Some(ack) = &message.ack
                    {
                        println!(
                            "ack {} {} created={:?} left={:?}",
                            ack.suggestion,
                            if ack.accepted { "accepted" } else { "declined" },
                            ack.created,
                            ack.left
                        );
                    }
                    // Another account's message, by seq and where it sits, never its words.
                    if let Event::Message { message } = &event
                        && !message.mine
                    {
                        println!(
                            "msg {} {} kind={:?} thread={:?} topic={:?} reply={:?} on={:?} mentions={}",
                            message.chat,
                            message.seq,
                            message.content_kind,
                            message.thread_seq,
                            message.topic,
                            message.reply,
                            message.on,
                            message.mentions.len()
                        );
                    }
                    self.print_group(&event);
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

    /// Reads until an event matches or a refusal arrives, leaving both in `events`.
    fn pump_until(&mut self, wanted: impl Fn(&Event) -> bool) {
        let started = Instant::now();
        while !self
            .events
            .iter()
            .any(|event| wanted(event) || matches!(event, Event::Error { .. }))
        {
            if started.elapsed() > Duration::from_secs(30) {
                return println!("timed out");
            }
            self.read_once();
        }
    }

    /// Prints what arrives for groups and channels, with ids and counts and never their content.
    fn print_group(&mut self, event: &Event) {
        match event {
            Event::ChatReady { chat } => println!("ready {chat}"),
            Event::ChatLeft { chat } => println!("left {chat}"),
            Event::ChatEvent { event, copy_list } => {
                let target = event.target.map(|target| target.to_string());
                let topic = event.topic.map(|topic| topic.to_string());
                println!(
                    "event {} {:?} actor={} target={} topic={} copies={}",
                    event.chat,
                    event.action,
                    event.actor,
                    target.as_deref().unwrap_or("-"),
                    topic.as_deref().unwrap_or("-"),
                    copy_list.len()
                );
            }
            Event::TopicOptions {
                chat,
                topic,
                options,
                mod_list,
            } => println!(
                "topicopts {chat} {topic} closed={} hidden={} write={:?} mods={}",
                options.closed,
                options.hidden,
                options.write,
                mod_list.len()
            ),
            Event::TopicProfile {
                chat,
                topic,
                profile,
            } => println!(
                "topicprofile {chat} {topic} version={} photo={} rules={}",
                profile.version,
                profile.photo.is_some(),
                profile.rules.is_some()
            ),
            Event::Line {
                chat,
                seq,
                account,
                action,
                account_list,
                ..
            } => {
                let list: Vec<String> = account_list.iter().map(Ulid::to_string).collect();
                println!(
                    "line {chat} {seq} {action:?} actor={account} accounts={}",
                    list.join(",")
                );
            }
            Event::JoinRequest { request } => {
                println!(
                    "request {} {} {:?}",
                    request.chat, request.account, request.state
                );
            }
            Event::Asked { chat, state, .. } => println!("asked {chat} {state:?}"),
            Event::Invitation {
                chat,
                invitation: Some(invitation),
            } => {
                self.invited = Some(*chat);
                // Whether its sealed name opened and it has a note, never either itself.
                println!(
                    "invited {chat} by {} named={} note={}",
                    invitation.inviter,
                    invitation.name.is_some(),
                    invitation.note.is_some()
                );
            }
            Event::Invitation {
                chat,
                invitation: None,
            } => println!("invitation gone {chat}"),
            Event::Findable { chat, findable } => {
                println!("findable {chat} on={}", findable.is_some());
            }
            Event::Marks { chat, mark } => println!(
                "marks {chat} {} received={} seen={}",
                mark.account,
                mark.received_seq,
                mark.seen_seq.unwrap_or(0)
            ),
            // An agent's trouble reason starts with its code, which is all printed here.
            Event::Presence { presence, text } => {
                let code = text
                    .as_deref()
                    .map(|text| text.split(':').next().unwrap_or_default().to_string());
                println!(
                    "presence {} {:?} code={}",
                    presence.account,
                    presence.state,
                    code.as_deref().unwrap_or("-")
                );
            }
            Event::Topics {
                chat,
                on,
                topic_list,
            } => {
                self.topics.insert(*chat, (*on, topic_list.clone()));
                println!("topics {chat} on={on} count={}", topic_list.len());
            }
            Event::ChatProfile { chat, profile } => {
                // Topic names live in the profile, which a new device opens only after joining.
                if let Some((_, list)) = self.topics.get_mut(chat) {
                    for topic in list.iter_mut() {
                        if let Some(named) = profile.topic_list.iter().find(|t| t.id == topic.id) {
                            topic.name = named.name.clone();
                        }
                    }
                }
                println!(
                    "profile {chat} version={} photo={}",
                    profile.version,
                    profile.photo.is_some()
                );
            }
            _ => {}
        }
        let _ = std::io::stdout().flush();
    }

    /// Lets what a group command started finish, printing any refusal instead of stopping.
    fn settle(&mut self) {
        self.pump(1500);
        let mut kept = Vec::new();
        for event in std::mem::take(&mut self.events) {
            match event {
                Event::Error { code, .. } => println!("refused {code}"),
                other => kept.push(other),
            }
        }
        self.events = kept;
    }

    /// The account with exactly this username, from the directory.
    fn account(&mut self, username: &str) -> Option<Ulid> {
        self.events
            .retain(|event| !matches!(event, Event::SearchResults { .. }));
        self.command(Command::Search {
            text: username.to_string(),
        });
        let Event::SearchResults { account_list, .. } =
            self.wait(|event| matches!(event, Event::SearchResults { .. }))
        else {
            unreachable!()
        };
        let found = account_list
            .iter()
            .find(|item| item.username == username)
            .map(|item| item.id);
        if found.is_none() {
            println!("missing {username}");
        }
        found
    }

    fn accounts(&mut self, usernames: &str) -> Vec<Ulid> {
        usernames
            .split_whitespace()
            .filter_map(|username| self.account(username))
            .collect()
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
                let Event::SearchResults { account_list, .. } = event else {
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
                    mentions: Vec::new(),
                    thread: app.thread,
                    topic: app.topic,
                    chat,
                    text: Secret(rest.to_string()),
                    reply: None,
                });
                match app.wait(|event| matches!(event, Event::Sent { .. } | Event::Failed { .. })) {
                    Event::Failed { code, seconds, .. } => match seconds {
                        Some(seconds) => println!("refused {code} {seconds}s"),
                        None => println!("refused {code}"),
                    },
                    _ => println!("sent"),
                }
            }
            "many" => {
                let n: u32 = rest.parse().unwrap();
                let chat = app.chat.expect("no chat");
                for i in 1..=n {
                    app.command(Command::SendText {
                        mentions: Vec::new(),
                        thread: None,
                        topic: None,
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
                    thread: app.thread,
                    topic: app.topic,
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
                    reply: None,
                    topic: None,
                    thread: None,
                });
                let Event::Sent { seq, .. } = app.wait(|event| matches!(event, Event::Sent { .. }))
                else {
                    unreachable!()
                };
                println!("asked {seq}");
            }
            // A folder suggestion for this chat, printing its sequence id; its ack is printed when it
            // arrives, during this or a later command such as `pump`.
            "suggest" => {
                let chat = app.chat.expect("no chat");
                let mut words = rest.splitn(3, ' ');
                let how = words.next().unwrap_or_default();
                let name = words.next().expect("suggest move|copy <folder> [text]");
                let text = words.next().map(|text| Secret(text.to_string()));
                app.command(Command::SendSuggestion {
                    chat,
                    text,
                    suggestion: Suggestion::Folder(FolderSuggestion {
                        name: name.to_string(),
                        r#move: how == "move",
                    }),
                    reply: None,
                    topic: None,
                    thread: None,
                });
                let Event::Sent { seq, .. } = app.wait(|event| matches!(event, Event::Sent { .. }))
                else {
                    unreachable!()
                };
                println!("suggested {seq}");
            }
            "bio" => {
                app.bio = Some(rest.to_string());
                app.command(Command::SaveProfile {
                    bio: app.bio.clone(),
                    link_list: app.link_list.clone(),
                });
                app.wait(|event| matches!(event, Event::ProfileSaved));
                println!("saved");
            }
            "link" => {
                let (name, url) = rest.rsplit_once(' ').expect("link <name> <url>");
                app.link_list.push(ProfileLink {
                    name: name.to_string(),
                    url: url.to_string(),
                });
                app.command(Command::SaveProfile {
                    bio: app.bio.clone(),
                    link_list: app.link_list.clone(),
                });
                app.wait(|event| matches!(event, Event::ProfileSaved));
                println!("saved");
            }
            // The JPEG goes as it is, and its BlurHash is a flat blue, as nothing here decodes it.
            "photo" => {
                let jpeg = std::fs::read(rest).unwrap();
                let mut rgba = Vec::new();
                for _ in 0..16 {
                    rgba.extend_from_slice(&[0x45, 0x76, 0xC4, 0xFF]);
                }
                app.command(Command::SetPhoto {
                    jpeg,
                    thumbnail: Thumbnail {
                        width: 4,
                        height: 4,
                        rgba,
                    },
                });
                app.wait(|event| matches!(event, Event::ProfileSaved));
                println!("saved");
            }
            // Whether this account's app is open, so others see it online, then away.
            "app" => {
                app.command(Command::InApp {
                    in_app: rest == "on",
                    trouble: false,
                    text: None,
                });
                app.pump(500);
                println!("app {rest}");
            }
            // Sets one setting, so logged in as the phone's own account it acts as a second device.
            "kv" => {
                let (key, value) = rest.split_once(' ').expect("kv <key> <json>");
                app.events
                    .retain(|event| !matches!(event, Event::Kv { .. }));
                app.command(Command::Kv {
                    set: vec![Setting {
                        key: key.to_string(),
                        value: serde_json::from_str(value).unwrap(),
                    }],
                    remove: Vec::new(),
                });
                app.wait(|event| matches!(event, Event::Kv { .. }));
                app.pump(800);
                println!("set {key}");
            }
            // A reaction on the message at `seq`, printing its own sequence id for `delete`.
            "react" => {
                let chat = app.chat.expect("no chat");
                let (on, emoji) = rest.split_once(' ').expect("react <seq> <emoji>");
                app.command(Command::SendReaction {
                    chat,
                    emoji: emoji.to_string(),
                    on: Pointer::Seq(on.parse().unwrap()),
                    topic: app.topic,
                    thread: app.thread,
                });
                let Event::Sent { seq, .. } = app.wait(|event| matches!(event, Event::Sent { .. }))
                else {
                    unreachable!()
                };
                println!("reacted {seq}");
            }
            // Removes one of this account's own messages or reactions, for everyone.
            "delete" => {
                let chat = app.chat.expect("no chat");
                app.command(Command::Delete {
                    chat,
                    target: Pointer::Seq(rest.parse().unwrap()),
                    remove: Vec::new(),
                });
                app.wait(|event| matches!(event, Event::Sent { .. }));
                println!("deleted {rest}");
            }
            // The nth sticker of the bundled Default pack, from 1, the first when none is given,
            // chosen by the author. Prints its sequence id, as `react` does.
            "sticker" => {
                let chat = app.chat.expect("no chat");
                let n: usize = if rest.is_empty() {
                    1
                } else {
                    rest.parse().unwrap()
                };
                app.events
                    .retain(|event| !matches!(event, Event::Stickers { .. }));
                app.command(Command::Stickers);
                let Event::Stickers { pack_list } =
                    app.wait(|event| matches!(event, Event::Stickers { .. }))
                else {
                    unreachable!()
                };
                let pack = pack_list
                    .iter()
                    .find(|pack| pack.name == "Default")
                    .expect("no Default pack");
                let Some(sticker) = n.checked_sub(1).and_then(|at| pack.sticker_list.get(at))
                else {
                    println!("no sticker {n}");
                    continue;
                };
                app.command(Command::SendSticker {
                    thread: app.thread,
                    topic: app.topic,
                    chat,
                    sticker: sticker.id,
                    reply: None,
                });
                let Event::Sent { seq, .. } = app.wait(|event| matches!(event, Event::Sent { .. }))
                else {
                    unreachable!()
                };
                println!("sticker {seq}");
            }
            // A group or channel with this name and these people, which becomes the chat in use.
            "group" | "channel" => {
                let (rest, note) = with_note(rest);
                let mut words = rest.split_whitespace();
                let name = words
                    .next()
                    .expect("group <name> [username ...]")
                    .to_string();
                let usernames = words.collect::<Vec<_>>().join(" ");
                let account_list = app.accounts(&usernames);
                app.command(Command::CreateChat {
                    chat_type: if verb == "group" {
                        ChatType::Group
                    } else {
                        ChatType::Channel
                    },
                    name,
                    bio: None,
                    link_list: Vec::new(),
                    account_list,
                    topics: None,
                    findable: false,
                    options: ChatOptions::default(),
                    photo: None,
                    note,
                });
                let Event::Started { chat, .. } =
                    app.wait(|event| matches!(event, Event::Started { .. }))
                else {
                    unreachable!()
                };
                app.chat = Some(chat);
                app.settle();
                println!("chat {chat}");
            }
            "use" => {
                app.chat = Some(Ulid::parse(rest).expect("use <chat id>"));
                app.topic = None;
                app.thread = None;
                println!("using {rest}");
            }
            "add" | "remove" => {
                let chat = app.chat.expect("no chat");
                let (rest, note) = with_note(rest);
                let account_list = app.accounts(rest);
                app.command(if verb == "add" {
                    Command::AddMembers {
                        chat,
                        account_list,
                        note,
                    }
                } else {
                    Command::RemoveMembers { chat, account_list }
                });
                app.settle();
                println!("{verb} asked");
            }
            "role" => {
                let chat = app.chat.expect("no chat");
                let (username, role) = rest.split_once(' ').expect("role <username> <role>");
                let role = match role {
                    "owner" => ChatRole::Owner,
                    "moderator" => ChatRole::Moderator,
                    _ => ChatRole::Member,
                };
                if let Some(account) = app.account(username) {
                    app.command(Command::SetRole {
                        chat,
                        account,
                        role,
                    });
                    app.settle();
                    println!("role asked");
                }
            }
            "leave" => {
                let chat = app.chat.expect("no chat");
                app.command(Command::Leave { chat });
                app.settle();
            }
            "removechat" => {
                let chat = app.chat.expect("no chat");
                app.command(Command::RemoveChat { chat });
                app.settle();
            }
            // Findable groups and channels whose name holds the text.
            "findchat" => {
                app.events
                    .retain(|event| !matches!(event, Event::SearchResults { .. }));
                app.command(Command::Search {
                    text: rest.to_string(),
                });
                let Event::SearchResults { chat_list, .. } =
                    app.wait(|event| matches!(event, Event::SearchResults { .. }))
                else {
                    unreachable!()
                };
                for found in &chat_list {
                    println!(
                        "found chat {} {:?} members={}",
                        found.chat, found.chat_type, found.member_count
                    );
                }
                println!("found {}", chat_list.len());
            }
            "ask" => {
                let (chat, note) = match rest.split_once(' ') {
                    Some((chat, note)) => (chat, Some(note.to_string())),
                    None => (rest, None),
                };
                app.command(Command::AskToJoin {
                    chat: Ulid::parse(chat).expect("ask <chat id> [note]"),
                    note,
                });
                app.settle();
                println!("asked");
            }
            "cancelask" => {
                app.command(Command::CancelAsk {
                    chat: Ulid::parse(rest).expect("cancelask <chat id>"),
                });
                app.settle();
                println!("cancelled");
            }
            "requests" => {
                let chat = app.chat.expect("no chat");
                app.command(Command::Requests { chat });
                let Event::Requests { request_list, .. } =
                    app.wait(|event| matches!(event, Event::Requests { .. }))
                else {
                    unreachable!()
                };
                for request in &request_list {
                    println!("waiting {}", request.account);
                }
                println!("requests {}", request_list.len());
            }
            "letin" | "decline" => {
                let chat = app.chat.expect("no chat");
                if let Some(account) = app.account(rest) {
                    app.command(if verb == "letin" {
                        Command::LetIn { chat, account }
                    } else {
                        Command::DeclineAsk { chat, account }
                    });
                    app.settle();
                    println!("{verb} done");
                }
            }
            // Answers the newest invitation; accepting joins it and makes it the chat in use.
            "invite" => {
                let chat = app.invited.take().expect("no invitation");
                let accept = rest == "yes";
                app.command(Command::AnswerInvite { chat, accept });
                if accept {
                    app.wait(
                        |event| matches!(event, Event::ChatReady { chat: ready } if *ready == chat),
                    );
                    app.chat = Some(chat);
                }
                app.settle();
                println!("answered {chat}");
            }
            "topics" => {
                let chat = app.chat.expect("no chat");
                let topic_list = app
                    .topics
                    .get(&chat)
                    .map(|(_, list)| list.clone())
                    .unwrap_or_default();
                app.command(Command::Topics {
                    chat,
                    on: rest == "on",
                    topic_list,
                });
                app.settle();
                println!("topics {rest}");
            }
            // A new topic after the others, keeping whether topics are on.
            "topic" => {
                let chat = app.chat.expect("no chat");
                app.command(Command::TopicId { chat });
                let Event::TopicId { topic, .. } =
                    app.wait(|event| matches!(event, Event::TopicId { .. }))
                else {
                    unreachable!()
                };
                let (on, mut topic_list) = app.topics.get(&chat).cloned().unwrap_or_default();
                let place = topic_list.len() as u64;
                topic_list.push(TopicName {
                    id: topic,
                    name: rest.to_string(),
                    place,
                });
                app.command(Command::Topics {
                    chat,
                    on,
                    topic_list,
                });
                app.settle();
                println!("topic {topic}");
            }
            "in" => {
                let chat = app.chat.expect("no chat");
                app.topic = if rest == "main" {
                    None
                } else {
                    app.topics.get(&chat).and_then(|(_, list)| {
                        list.iter()
                            .find(|topic| topic.name == rest)
                            .map(|topic| topic.id)
                    })
                };
                match app.topic {
                    Some(topic) => println!("in {topic}"),
                    None => println!("in main"),
                }
            }
            "thread" => {
                app.thread = rest.parse().ok();
                println!("thread {rest}");
            }
            "mention" => {
                let chat = app.chat.expect("no chat");
                let (username, text) = rest.split_once(' ').expect("mention <username> <text>");
                let Some(account) = app.account(username) else {
                    continue;
                };
                app.command(Command::SendText {
                    chat,
                    text: Secret(format!("@{username} {text}")),
                    reply: None,
                    topic: app.topic,
                    thread: app.thread,
                    mentions: vec![account],
                });
                app.wait(|event| matches!(event, Event::Sent { .. }));
                println!("sent");
            }
            "chatprofile" => {
                let chat = app.chat.expect("no chat");
                // A name, then rules after a lone `--`.
                let (name, rules) = match rest.split_once(" -- ") {
                    Some((name, rules)) => (name, Some(rules.to_string())),
                    None => (rest, None),
                };
                app.command(Command::ChatProfile {
                    chat,
                    name: name.to_string(),
                    bio: None,
                    link_list: Vec::new(),
                    rules,
                });
                app.settle();
                println!("saved");
            }
            // A photo for the chat in use, from a JPEG file. Its BlurHash is a flat colour, as the
            // account's is, since this program decodes no images.
            "chatphoto" => {
                let chat = app.chat.expect("no chat");
                let jpeg = std::fs::read(rest).unwrap();
                let mut rgba = Vec::new();
                for _ in 0..16 {
                    rgba.extend_from_slice(&[0x45, 0x76, 0xC4, 0xFF]);
                }
                app.command(Command::ChatPhoto {
                    chat,
                    jpeg,
                    thumbnail: Thumbnail {
                        width: 4,
                        height: 4,
                        rgba,
                    },
                });
                app.settle();
                println!("saved");
            }
            // The topic in use's bio, then rules after a lone `--`; General when none is in use.
            "topicbio" => {
                let chat = app.chat.expect("no chat");
                let (bio, rules) = match rest.split_once(" -- ") {
                    Some((bio, rules)) => (bio, Some(rules.to_string())),
                    None => (rest, None),
                };
                app.command(Command::TopicProfile {
                    chat,
                    topic: app.topic.unwrap_or(GENERAL_TOPIC),
                    bio: Some(bio.to_string()),
                    link_list: Vec::new(),
                    rules,
                });
                app.settle();
                println!("saved");
            }
            // A photo for the topic in use, from a JPEG file, with a flat BlurHash as `chatphoto`.
            "topicphoto" => {
                let chat = app.chat.expect("no chat");
                let jpeg = std::fs::read(rest).unwrap();
                let mut rgba = Vec::new();
                for _ in 0..16 {
                    rgba.extend_from_slice(&[0xC4, 0x76, 0x45, 0xFF]);
                }
                app.command(Command::TopicPhoto {
                    chat,
                    topic: app.topic.unwrap_or(GENERAL_TOPIC),
                    jpeg,
                    thumbnail: Thumbnail {
                        width: 4,
                        height: 4,
                        rgba,
                    },
                });
                app.settle();
                println!("saved");
            }
            // The topic in use's own options as JSON, such as {"closed": true, "write": "moderators",
            // "deny": ["message.send.reaction"]}.
            "topicopts" => {
                let chat = app.chat.expect("no chat");
                app.command(Command::TopicOptions {
                    chat,
                    topic: app.topic.unwrap_or(GENERAL_TOPIC),
                    options: serde_json::from_str(rest).expect("topicopts <json>"),
                });
                app.settle();
                println!("saved");
            }
            // Makes someone the topic in use's moderator, or a member there again.
            "topicmod" => {
                let chat = app.chat.expect("no chat");
                let (username, on) = rest.split_once(' ').expect("topicmod <username> on|off");
                if let Some(account) = app.account(username) {
                    app.command(Command::TopicRole {
                        chat,
                        topic: app.topic.unwrap_or(GENERAL_TOPIC),
                        account,
                        role: if on == "on" {
                            ChatRole::Moderator
                        } else {
                            ChatRole::Member
                        },
                    });
                    app.settle();
                    println!("role asked");
                }
            }
            // The chat's options as JSON, such as {"deny": ["message.send.sticker"], "slow_mode": 10}.
            "options" => {
                let chat = app.chat.expect("no chat");
                app.command(Command::ChatOptions {
                    chat,
                    options: serde_json::from_str(rest).expect("options <json>"),
                });
                app.settle();
                println!("saved");
            }
            "findable" => {
                let chat = app.chat.expect("no chat");
                let name = rest.strip_prefix("on ").map(str::to_string);
                app.command(Command::Findable {
                    chat,
                    findable: name.is_some(),
                    name,
                    bio: None,
                });
                app.settle();
                println!("saved");
            }
            "agent" => agent_command(&mut app, rest),
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
/// The usernames before a lone `--`, and the invitation's note after it.
fn with_note(rest: &str) -> (&str, Option<String>) {
    match rest.split_once(" -- ") {
        Some((names, note)) => (names, Some(note.to_string())),
        None => (rest.strip_suffix(" --").unwrap_or(rest), None),
    }
}

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
