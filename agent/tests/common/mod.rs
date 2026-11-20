//! What the agent's tests share: the real keryx server, `drive.rs` as the owner and the people the
//! agent talks to, a fake LLM answering from a handler per test, and the agent's own process. The
//! server and `drive.rs` are built by `make test` and named by `KERYX_BIN` and `KERYX_DRIVE`.

use std::collections::VecDeque;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tempfile::TempDir;

/// The owner's words, a valid BIP39 phrase.
pub const OWNER_WORDS: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

/// Another person's words.
pub const SARA_WORDS: &str =
    "legal winner thank year wave sausage worth useful legal winner thank yellow";

/// How long a test waits for anything before it fails.
pub const PATIENCE: Duration = Duration::from_secs(90);

fn binary(name: &str) -> String {
    match std::env::var(name) {
        Ok(path) => path,
        Err(_) => panic!("{name} is not set: run the tests with make test"),
    }
}

fn free_port() -> u16 {
    let free = TcpListener::bind("127.0.0.1:0").unwrap();
    free.local_addr().unwrap().port()
}

/// The real keryx server, with registration open and agents on.
pub struct Server {
    pub directory: TempDir,
    child: Child,
    pub origin: String,
}

impl Server {
    pub fn start() -> Self {
        let directory = TempDir::new().unwrap();
        let port = free_port();
        let www = directory.path().join("www");
        fs::create_dir(&www).unwrap();
        let origin = format!("http://127.0.0.1:{port}");
        let configuration = format!(
            "listen = \"127.0.0.1:{port}\"\npublic_url = \"{origin}\"\n\
             [database]\ntype = \"sqlite\"\npath = {:?}\n[www]\ndirectory = {:?}\n\
             [files]\ndirectory = {:?}\n[registration]\nallow = true\n\
             [agents]\nsecret = \"a test secret of at least thirty-two characters\"\n",
            directory.path().join("keryx.db"),
            www,
            directory.path().join("files"),
        );
        let config = directory.path().join("server.toml");
        fs::write(&config, configuration).unwrap();
        let log = File::create(directory.path().join("server.log")).unwrap();
        let mut child = Command::new(binary("KERYX_BIN"))
            .arg("--config")
            .arg(&config)
            .arg("serve")
            .stdout(Stdio::null())
            .stderr(log)
            .spawn()
            .unwrap();
        let started = Instant::now();
        while TcpStream::connect(("127.0.0.1", port)).is_err() {
            if let Some(status) = child.try_wait().unwrap() {
                let log = fs::read_to_string(directory.path().join("server.log")).unwrap();
                panic!("the server exited with {status}: {log}");
            }
            assert!(started.elapsed() < PATIENCE, "the server did not come up");
            std::thread::sleep(Duration::from_millis(20));
        }
        Self {
            directory,
            child,
            origin,
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// `drive.rs`, one account, read line by line.
pub struct Drive {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    /// Lines read while waiting for another, oldest first.
    pub seen: VecDeque<String>,
}

impl Drive {
    pub fn start(server: &Server) -> Self {
        let mut child = Command::new(binary("KERYX_DRIVE"))
            .arg(&server.origin)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (sender, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else {
                    return;
                };
                if sender.send(line).is_err() {
                    return;
                }
            }
        });
        Self {
            child,
            stdin,
            lines,
            seen: VecDeque::new(),
        }
    }

    /// Signs up with these words and waits until ready.
    pub fn sign_up(server: &Server, username: &str, words: &str) -> Self {
        let mut drive = Self::start(server);
        drive.send(&format!("signup {username} {username}-password-1 {words}"));
        drive.expect(&format!("ready {username}"));
        drive
    }

    pub fn send(&mut self, line: &str) {
        writeln!(self.stdin, "{line}").unwrap();
        self.stdin.flush().unwrap();
    }

    /// The first line starting with `prefix`, from those already seen or still to come.
    pub fn expect(&mut self, prefix: &str) -> String {
        if let Some(at) = self.seen.iter().position(|line| line.starts_with(prefix)) {
            return self.seen.remove(at).unwrap_or_default();
        }
        let started = Instant::now();
        loop {
            let left = PATIENCE.saturating_sub(started.elapsed());
            match self.lines.recv_timeout(left) {
                Ok(line) if line.starts_with(prefix) => return line,
                Ok(line) => self.seen.push_back(line),
                Err(_) => panic!("drive never printed {prefix:?}; it printed {:?}", self.seen),
            }
        }
    }

    /// Reads the socket for `ms`, then answers every line starting with `prefix` it printed.
    pub fn pump(&mut self, ms: u64, prefix: &str) -> Vec<String> {
        self.send(&format!("pump {ms}"));
        self.expect("pumped");
        let mut found = Vec::new();
        self.seen.retain(|line| {
            if line.starts_with(prefix) {
                found.push(line.clone());
                false
            } else {
                true
            }
        });
        found
    }

    /// Pumps until a line starting with `prefix` was printed, and answers the first, leaving any
    /// later one for the next call.
    pub fn pump_until(&mut self, prefix: &str) -> String {
        let started = Instant::now();
        loop {
            if let Some(at) = self.seen.iter().position(|line| line.starts_with(prefix)) {
                return self.seen.remove(at).unwrap_or_default();
            }
            self.send("pump 1000");
            self.expect("pumped");
            assert!(
                started.elapsed() < PATIENCE,
                "drive never printed {prefix:?}; it printed {:?}",
                self.seen
            );
        }
    }
}

impl Drop for Drive {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// What the fake LLM answers: a status and a JSON body.
pub type Answer = (u16, Value);

type Handler = dyn Fn(usize, &Value) -> Answer + Send + Sync;

/// A fake OpenAI-compatible server: each request to `/chat/completions` is kept and answered by the
/// test's handler, given its number from 0.
pub struct FakeLlm {
    pub url: String,
    requests: Arc<Mutex<Vec<Value>>>,
}

impl FakeLlm {
    pub fn start(handler: impl Fn(usize, &Value) -> Answer + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let handler: Arc<Handler> = Arc::new(handler);
        let kept = requests.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else {
                    continue;
                };
                let handler = handler.clone();
                let kept = kept.clone();
                std::thread::spawn(move || serve(stream, &*handler, &kept));
            }
        });
        Self { url, requests }
    }

    pub fn requests(&self) -> Vec<Value> {
        self.requests.lock().unwrap().clone()
    }

    /// Waits until at least `count` requests arrived.
    pub fn wait_for(&self, count: usize) -> Vec<Value> {
        let started = Instant::now();
        loop {
            let requests = self.requests();
            if requests.len() >= count {
                return requests;
            }
            assert!(
                started.elapsed() < PATIENCE,
                "the LLM got {} requests, not {count}",
                requests.len()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// One connection, kept alive for as many requests as the client sends on it.
fn serve(stream: TcpStream, handler: &Handler, kept: &Mutex<Vec<Value>>) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut writer = stream;
    loop {
        let mut length = 0;
        let mut first = true;
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            if first {
                first = false;
                continue;
            }
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some((name, value)) = line.split_once(':')
                && name.eq_ignore_ascii_case("content-length")
            {
                length = value.trim().parse().unwrap_or(0);
            }
        }
        let mut body = vec![0; length];
        if reader.read_exact(&mut body).is_err() {
            return;
        }
        let request: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        let number = {
            let mut list = kept.lock().unwrap();
            list.push(request.clone());
            list.len() - 1
        };
        let (status, answer) = handler(number, &request);
        let text = answer.to_string();
        let retry = if status == 429 {
            "Retry-After: 1\r\n"
        } else {
            ""
        };
        let reply = format!(
            "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\n{retry}Content-Length: {}\r\n\r\n{text}",
            text.len()
        );
        if writer.write_all(reply.as_bytes()).is_err() {
            return;
        }
    }
}

/// An answer calling `starlark` with this script and intention list.
pub fn tool_call(script: &str, intention_list: Value) -> Answer {
    let arguments = json!({"script": script, "intention_list": intention_list}).to_string();
    (
        200,
        json!({
            "choices": [{"message": {
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {"name": "starlark", "arguments": arguments},
                }],
            }}],
            "usage": {"prompt_tokens": 100, "completion_tokens": 10},
        }),
    )
}

/// An answer in text, which ends the wake.
pub fn text(text: &str) -> Answer {
    (
        200,
        json!({
            "choices": [{"message": {"role": "assistant", "content": text}}],
            "usage": {"prompt_tokens": 100, "completion_tokens": 10},
        }),
    )
}

/// The events of a request's newest events message, as the loop wrote them.
pub fn events_of(request: &Value) -> Option<Value> {
    let messages = request["messages"].as_array()?;
    for message in messages.iter().rev() {
        if message["role"] == "user"
            && let Some(text) = message["content"].as_str()
            && let Some(body) = text.strip_prefix("Events:\n")
        {
            return serde_json::from_str(body).ok();
        }
    }
    None
}

/// The chat of a request's first event group.
pub fn chat_of(request: &Value) -> String {
    let events = events_of(request).expect("no events in the request");
    events["group_list"][0]["chat"]
        .as_str()
        .expect("the first group has no chat")
        .to_string()
}

/// The newest tool output in a request.
pub fn tool_output(request: &Value) -> String {
    let messages = request["messages"].as_array().unwrap();
    for message in messages.iter().rev() {
        if message["role"] == "tool" {
            return message["content"].as_str().unwrap_or_default().to_string();
        }
    }
    String::new()
}

/// The system message of a request.
pub fn system_of(request: &Value) -> String {
    request["messages"][0]["content"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The keryx-agent process.
pub struct Agent {
    pub directory: PathBuf,
    config: PathBuf,
    log: PathBuf,
    child: Option<Child>,
}

impl Agent {
    /// Writes `agent.toml` in `home` and starts the agent on it.
    pub fn start(home: &Path, configuration: &str) -> Self {
        let directory = home.join("agent");
        let config = home.join("agent.toml");
        fs::write(&config, configuration).unwrap();
        let log = home.join("agent.log");
        let mut agent = Self {
            directory,
            config,
            log,
            child: None,
        };
        agent.restart();
        agent
    }

    /// Starts it again on the same `agent.toml`, its log appended.
    pub fn restart(&mut self) {
        let log = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log)
            .unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_keryx-agent"))
            .arg("--config")
            .arg(&self.config)
            .arg("--debug")
            .arg("run")
            .stdout(Stdio::null())
            .stderr(log)
            .spawn()
            .unwrap();
        self.child = Some(child);
    }

    /// Kills it at once, as a crash would.
    pub fn kill(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    pub fn log(&self) -> String {
        fs::read_to_string(&self.log).unwrap_or_default()
    }

    /// Waits until the log holds `needle` at least `count` times.
    pub fn wait_log(&self, needle: &str, count: usize) {
        let started = Instant::now();
        while self.log().matches(needle).count() < count {
            assert!(
                started.elapsed() < PATIENCE,
                "the agent never logged {needle:?} {count} times:\n{}",
                self.log()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Waits until it is connected with its key packages up, so a chat can be started with it.
    pub fn wait_ready(&self) {
        self.wait_log("\"message\":\"connected\"", 1);
        self.wait_log("\"path\":\"/api/v1/key_packages\"", 2);
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        self.kill();
    }
}

/// Everything a loop test needs: the server, the owner, the agent made by the owner, the fake LLM,
/// and a direct chat between the owner and the agent.
pub struct World {
    pub home: TempDir,
    pub server: Server,
    pub owner: Drive,
    pub llm: FakeLlm,
    pub agent: Agent,
    pub agent_id: String,
    pub chat: String,
}

/// `agent.toml` for a test: `sections` go after `[prompt]`, and `wake` holds the `[[wake]]` rules,
/// the template's one when empty.
pub fn configuration(
    server: &Server,
    llm: &FakeLlm,
    home: &Path,
    token: &str,
    words: &str,
    sections: &str,
    wake: &str,
) -> String {
    let wake = if wake.is_empty() {
        "[[wake]]\nchat_type = [\"direct\"]\naction = \"wake\"\n"
    } else {
        wake
    };
    format!(
        "url = \"{}\"\ntoken = \"{token}\"\nwords = \"{words}\"\ndirectory = {:?}\n\
         [llm]\nurl = \"{}\"\nkey = \"k\"\nmodel = \"m\"\nbackoff_max = \"1s\"\n\
         [prompt]\nsystem = \"You are a test agent.\"\n{sections}\n{wake}",
        server.origin,
        home.join("agent"),
        llm.url,
    )
}

impl World {
    pub fn start(
        handler: impl Fn(usize, &Value) -> Answer + Send + Sync + 'static,
        sections: &str,
        wake: &str,
    ) -> Self {
        Self::start_with(handler, sections, wake, "")
    }

    /// As [`World::start`], the owner ticking these server-wide permissions for the agent.
    pub fn start_with(
        handler: impl Fn(usize, &Value) -> Answer + Send + Sync + 'static,
        sections: &str,
        wake: &str,
        permissions: &str,
    ) -> Self {
        let home = TempDir::new().unwrap();
        let server = Server::start();
        let mut owner = Drive::sign_up(&server, "owner", OWNER_WORDS);
        owner.send(&format!("agent add helper_agent {permissions}"));
        let agent_id = owner.expect("agent ")["agent ".len()..].to_string();
        let token = owner.expect("token ")["token ".len()..].to_string();
        let words = owner.expect("words ")["words ".len()..].to_string();
        let llm = FakeLlm::start(handler);
        let toml = configuration(&server, &llm, home.path(), &token, &words, sections, wake);
        let agent = Agent::start(home.path(), &toml);
        agent.wait_ready();
        owner.send("find helper_agent");
        let chat = owner.expect("chat ")["chat ".len()..].to_string();
        Self {
            home,
            server,
            owner,
            llm,
            agent,
            agent_id,
            chat,
        }
    }
}

impl World {
    /// Another person, `sara`, signed up with a direct chat to the agent: her drive and the chat.
    pub fn sara(&self) -> (Drive, String) {
        let mut sara = Drive::sign_up(&self.server, "sara", SARA_WORDS);
        sara.send("find helper_agent");
        let chat = sara.expect("chat ")["chat ".len()..].to_string();
        (sara, chat)
    }

    /// The owner says `trigger`, and the wake runs `count` scripts, one per step: their outputs.
    pub fn steps(&mut self, trigger: &str, count: usize) -> Vec<Step> {
        self.owner.send(&format!("say {trigger}"));
        self.owner.expect("sent");
        let requests = self.llm.wait_for(count + 1);
        let mut step_list = Vec::new();
        for request in &requests[1..=count] {
            step_list.push(step_of(&tool_output(request)));
            eprintln!("DEBUGSTEP {}", tool_output(request));
        }
        step_list
    }
}

/// Where a script names the direct chat its wake is about: replaced before it runs.
pub const CHAT: &str = "$CHAT";

/// Defined before every script given to [`scripted`]: prints a built-in's `(value, err)` as JSON.
const SHOW: &str = "def show(result):\n    print(json.encode(result))\n\n";

/// A handler answering request n with the nth script, `None` being a text answer that ends the wake.
/// Each script gets [`SHOW`] before it and [`CHAT`] replaced by its wake's first chat, and names no
/// chat in its intention list, so no activity message is sent.
pub fn scripted(
    scripts: Vec<Option<String>>,
) -> impl Fn(usize, &Value) -> Answer + Send + Sync + 'static {
    move |number, request| match scripts.get(number) {
        Some(Some(script)) => {
            let script = format!("{SHOW}{}", script.replace(CHAT, &chat_of(request)));
            tool_call(&script, json!([{"intention": "Testing", "chat_list": []}]))
        }
        _ => text("done"),
    }
}

/// One step's output read back: each printed line as JSON, or as a string when it is not, and each
/// action.
#[derive(Debug)]
pub struct Step {
    pub text: String,
    pub printed: Vec<Value>,
    pub actions: Vec<Value>,
}

impl Step {
    /// The value of the nth `show`, failing if it was an err.
    pub fn value(&self, n: usize) -> &Value {
        let shown = &self.printed[n];
        assert!(
            shown[1].is_null(),
            "show #{n} failed: {shown}\n{}",
            self.text
        );
        &shown[0]
    }

    /// The err code of the nth `show`, failing if it succeeded.
    pub fn code(&self, n: usize) -> &str {
        let shown = &self.printed[n];
        assert!(
            shown[0].is_null(),
            "show #{n} did not fail: {shown}\n{}",
            self.text
        );
        shown[1]["code"].as_str().unwrap_or_default()
    }

    /// The actions named `name`.
    pub fn actions_of(&self, name: &str) -> Vec<&Value> {
        let mut found = Vec::new();
        for action in &self.actions {
            if action["action"] == name {
                found.push(action);
            }
        }
        found
    }
}

pub fn step_of(text: &str) -> Step {
    let (head, actions) = text.split_once("actions:\n").unwrap_or((text, ""));
    let printed_text = match head.split_once("output:\n") {
        Some((_, printed)) => printed,
        None => "",
    };
    let mut printed = Vec::new();
    for line in printed_text.lines() {
        if line == "(nothing printed)" {
            continue;
        }
        match serde_json::from_str(line) {
            Ok(value) => printed.push(value),
            Err(_) => printed.push(Value::String(line.to_string())),
        }
    }
    let mut action_list = Vec::new();
    for line in actions.lines() {
        if let Ok(value) = serde_json::from_str(line) {
            action_list.push(value);
        }
    }
    Step {
        text: text.to_string(),
        printed,
        actions: action_list,
    }
}

/// Every text of a request's messages, tool outputs and user content included, joined.
pub fn request_text(request: &Value) -> String {
    let mut text = String::new();
    if let Some(messages) = request["messages"].as_array() {
        for message in messages {
            match &message["content"] {
                Value::String(content) => text.push_str(content),
                Value::Null => {}
                other => text.push_str(&other.to_string()),
            }
            text.push_str(&message["tool_calls"].to_string());
            text.push('\n');
        }
    }
    text
}

/// A `[[wake]]` rule waking only on a message whose text is `word`, its script written in
/// `directory`, followed by `extra` lines of the same rule.
pub fn wake_on(directory: &Path, word: &str, extra: &str) -> String {
    let path = directory.join(format!("wake-{word}.star"));
    fs::write(
        &path,
        format!(
            "def wake(event):\n    message = event.get(\"message\") or {{}}\n    \
             return message.get(\"text\") == {word:?}\n"
        ),
    )
    .unwrap();
    format!("[[wake]]\nscript = {path:?}\naction = \"wake\"\n{extra}\n")
}

/// One request a [`FakeHttp`] got.
#[derive(Debug, Clone)]
pub struct HttpRequest {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl HttpRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        for (key, value) in &self.headers {
            if key.eq_ignore_ascii_case(name) {
                return Some(value);
            }
        }
        None
    }
}

/// A plain HTTP server for scripts' `http`, answering each request from the test's handler with a
/// status and a JSON or text body, one connection per request.
pub struct FakeHttp {
    pub origin: String,
    requests: Arc<Mutex<Vec<HttpRequest>>>,
}

type HttpHandler = dyn Fn(&HttpRequest) -> (u16, String) + Send + Sync;

impl FakeHttp {
    pub fn start(handler: impl Fn(&HttpRequest) -> (u16, String) + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let handler: Arc<HttpHandler> = Arc::new(handler);
        let kept = requests.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else {
                    continue;
                };
                let handler = handler.clone();
                let kept = kept.clone();
                std::thread::spawn(move || answer_http(stream, &*handler, &kept));
            }
        });
        Self { origin, requests }
    }

    pub fn requests(&self) -> Vec<HttpRequest> {
        self.requests.lock().unwrap().clone()
    }
}

fn answer_http(stream: TcpStream, handler: &HttpHandler, kept: &Mutex<Vec<HttpRequest>>) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut writer = stream;
    let mut first = String::new();
    if reader.read_line(&mut first).is_err() {
        return;
    }
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();
    let mut headers = Vec::new();
    let mut length = 0;
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                length = value.trim().parse().unwrap_or(0);
            }
            headers.push((name.to_string(), value.trim().to_string()));
        }
    }
    let mut body = vec![0; length];
    if reader.read_exact(&mut body).is_err() {
        return;
    }
    let request = HttpRequest {
        method,
        path,
        headers,
        body: String::from_utf8_lossy(&body).to_string(),
    };
    kept.lock().unwrap().push(request.clone());
    let (status, text) = handler(&request);
    let kind = if text.starts_with('{') {
        "application/json"
    } else {
        "text/plain"
    };
    let reply = format!(
        "HTTP/1.1 {status} X\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
        text.len()
    );
    let _ = writer.write_all(reply.as_bytes());
}

impl Agent {
    /// Sends SIGTERM and answers whether it exited within `within`.
    pub fn terminate(&mut self, within: Duration) -> bool {
        let Some(child) = self.child.as_mut() else {
            return true;
        };
        let status = Command::new("kill")
            .arg("-TERM")
            .arg(child.id().to_string())
            .status()
            .unwrap();
        assert!(status.success());
        let started = Instant::now();
        while started.elapsed() < within {
            if child.try_wait().unwrap().is_some() {
                self.child = None;
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }
}
