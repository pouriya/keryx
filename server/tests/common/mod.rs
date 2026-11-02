//! The real binary on a free loopback port, with its own temporary directory and `--config`.

use std::fs::{self, File};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;
use tempfile::TempDir;

pub const OPERATOR_TOKEN: &str = "operator-token-for-the-tests";

/// Reads a status as an answer, and does not check a certificate. The tests of checking use their own
/// client, in `chat.rs` and through the command line in `main.rs`.
fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .disable_verification(true)
                .build(),
        )
        .build()
        .new_agent()
}

pub struct Server {
    directory: TempDir,
    child: Child,
    status: Option<ExitStatus>,
    pub origin: String,
}

impl Server {
    /// `top` holds keys that come before the first section, and `sections` holds whole sections.
    /// The operator's section is here unless `sections` starts with "# no operator".
    /// `sections` holding "# tls" turns on TLS, with the certificate in `tests/tls/`.
    pub fn start(top: &str, sections: &str) -> Self {
        let directory = TempDir::new().unwrap();
        let free = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = free.local_addr().unwrap().port();
        drop(free);
        let www = directory.path().join("www");
        fs::create_dir(&www).unwrap();
        fs::write(www.join("index.html"), "the web client").unwrap();
        fs::write(www.join("page.txt"), "a file").unwrap();
        let (scheme, tls) = if sections.contains("# tls") {
            let certificates = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/tls");
            (
                "https",
                format!(
                    "[tls]\ncertificate = \"{certificates}/cert.pem\"\nkey = \"{certificates}/key.pem\"\n"
                ),
            )
        } else {
            ("http", String::new())
        };
        let origin = format!("{scheme}://127.0.0.1:{port}");
        let operator = if sections.starts_with("# no operator") {
            String::new()
        } else {
            format!("[operator]\ntoken = \"{OPERATOR_TOKEN}\"\n")
        };
        let configuration = format!(
            "listen = \"127.0.0.1:{port}\"\npublic_url = \"{origin}\"\n{top}\n\
             [database]\ntype = \"sqlite\"\npath = {:?}\n[www]\ndirectory = {:?}\n{operator}{tls}{sections}\n",
            directory.path().join("keryx.db"),
            www,
        );
        let config = directory.path().join("server.toml");
        fs::write(&config, configuration).unwrap();
        let log = File::create(directory.path().join("server.log")).unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_keryx"))
            .arg("--config")
            .arg(&config)
            .arg("--debug")
            .arg("serve")
            .stdout(Stdio::null())
            .stderr(log)
            .spawn()
            .unwrap();
        let mut server = Self {
            directory,
            child,
            status: None,
            origin,
        };
        server.wait_until_up();
        server
    }

    fn wait_until_up(&mut self) {
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(30) {
            if agent()
                .get(format!("{}/api/v1", self.origin))
                .call()
                .is_ok()
            {
                return;
            }
            if let Some(status) = self.child.try_wait().unwrap() {
                let log = fs::read_to_string(self.path("server.log")).unwrap();
                panic!("The server exited with {status}: {log}");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("The server did not come up");
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.directory.path().join(name)
    }

    /// One request: the status and the JSON reply.
    pub fn call(
        &self,
        method: &str,
        route: &str,
        token: Option<&str>,
        body: Option<&Value>,
    ) -> (u16, Value) {
        let agent = agent();
        let url = format!("{}{route}", self.origin);
        let authorization = match token {
            Some(token) => format!("Bearer {token}"),
            None => String::new(),
        };
        let mut response = match (method, body) {
            ("GET", _) => {
                let mut request = agent.get(&url);
                if token.is_some() {
                    request = request.header("Authorization", &authorization);
                }
                request.call().unwrap()
            }
            ("DELETE", _) => {
                let mut request = agent.delete(&url);
                if token.is_some() {
                    request = request.header("Authorization", &authorization);
                }
                request.call().unwrap()
            }
            (method, body) => {
                let mut request = match method {
                    "PUT" => agent.put(&url),
                    _ => agent.post(&url),
                };
                if token.is_some() {
                    request = request.header("Authorization", &authorization);
                }
                match body {
                    Some(body) => request.send_json(body).unwrap(),
                    None => request.send_empty().unwrap(),
                }
            }
        };
        let status = response.status().as_u16();
        let reply: Value = response.body_mut().read_json().unwrap();
        (status, reply)
    }

    /// SIGTERM, then the exit status. Coverage is only written by a process that exits by itself.
    pub fn stop(&mut self) -> ExitStatus {
        if let Some(status) = self.status {
            return status;
        }
        let killed = Command::new("kill")
            .arg("-TERM")
            .arg(self.child.id().to_string())
            .status()
            .unwrap();
        assert!(killed.success());
        let status = self.child.wait().unwrap();
        self.status = Some(status);
        status
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop();
    }
}
