mod common;

use std::fs;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use common::{OPERATOR_TOKEN, Server};
use serde_json::{Value, json};
use tempfile::TempDir;

fn keryx(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_keryx"))
        .args(args)
        .output()
        .unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).to_string()
}

fn config_of(server: &Server) -> String {
    server.path("server.toml").to_string_lossy().to_string()
}

#[test]
fn bare_commands_print_their_long_help_and_exit_zero() {
    let bare = keryx(&[]);
    assert!(bare.status.success());
    assert!(text(&bare.stdout).contains("Every command needs --config"));
    let group = keryx(&["user"]);
    assert!(group.status.success());
    assert!(text(&group.stdout).contains("add"));
}

#[test]
fn config_is_required_with_no_default() {
    for args in [vec!["serve"], vec!["user", "add", "sara_x"]] {
        let output = keryx(&args);
        assert!(!output.status.success());
        assert!(text(&output.stderr).contains("--config is required"));
    }
}

#[test]
fn a_bad_configuration_stops_startup_and_says_where() {
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("server.toml");
    let database = directory.path().join("keryx.db");
    let www = directory.path().join("www");
    let good = format!(
        "listen = \"127.0.0.1:1\"\npublic_url = \"http://127.0.0.1:1\"\n\
         [database]\ntype = \"sqlite\"\npath = {database:?}\n[www]\ndirectory = {www:?}\n"
    );
    let bad_list = [
        (good.replace("sqlite", "postgres"), "database.type"),
        (
            good.replace(database.to_str().unwrap(), "relative/keryx.db"),
            "database.path",
        ),
        (good.replace(www.to_str().unwrap(), "www"), "www.directory"),
        (good.replace("http://127.0.0.1:1", "ftp://x"), "public_url"),
        (
            format!("{good}\n[tls]\ncertificate = \"cert.pem\"\nkey = \"/key.pem\"\n"),
            "tls.certificate",
        ),
        (
            format!("{good}\n[tls]\ncertificate = \"/cert.pem\"\nkey = \"key.pem\"\n"),
            "tls.key",
        ),
        (format!("{good}\nnot_a_key = 1\n"), "not_a_key"),
        (
            format!("{good}\n[limits]\nping_after = \"0s\"\n"),
            "limits.ping_after",
        ),
        (
            format!("{good}\n[limits]\nping_every = \"0s\"\n"),
            "limits.ping_every",
        ),
        (
            format!("{good}\n[limits]\nping_timeout = \"0s\"\n"),
            "limits.ping_timeout",
        ),
        (
            format!("{good}\n[tcp]\nsend_timeout = \"0s\"\n"),
            "tcp.send_timeout",
        ),
        (
            format!("{good}\n[tcp]\nuser_timeout = \"0s\"\n"),
            "tcp.user_timeout",
        ),
        (
            format!("{good}\n[tcp]\nkeepalive_idle = \"0s\"\n"),
            "tcp.keepalive_idle",
        ),
        (
            format!("{good}\n[tcp]\nkeepalive_interval = \"0s\"\n"),
            "tcp.keepalive_interval",
        ),
        (format!("{good}\n[tcp]\nnot_a_key = 1\n"), "not_a_key"),
        (
            format!("{good}\n[chat]\nidle_after = \"soon\"\n"),
            "idle_after",
        ),
        (
            format!("{good}\n[limits]\nrest_body = \"lots\"\n"),
            "rest_body",
        ),
    ];
    for (bad, naming) in bad_list {
        fs::write(&path, bad).unwrap();
        let output = keryx(&["--config", path.to_str().unwrap(), "serve"]);
        assert!(!output.status.success(), "{naming}");
        assert!(
            text(&output.stderr).contains(naming),
            "{naming}: {}",
            text(&output.stderr)
        );
    }

    let output = keryx(&["--config", "/nothing/here.toml", "serve"]);
    assert!(!output.status.success());
    assert!(text(&output.stderr).contains("Could not read the configuration"));
}

#[test]
fn a_database_that_cannot_open_or_a_taken_port_stops_startup() {
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("server.toml");
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = taken.local_addr().unwrap().port();
    let www = directory.path().join("www");

    fs::write(
        &path,
        format!(
            "listen = \"127.0.0.1:{port}\"\npublic_url = \"http://127.0.0.1:1\"\n\
             [database]\ntype = \"sqlite\"\npath = \"/nothing/here/keryx.db\"\n[www]\ndirectory = {www:?}\n"
        ),
    )
    .unwrap();
    let output = keryx(&["--config", path.to_str().unwrap(), "--quiet", "serve"]);
    assert!(!output.status.success());
    assert!(text(&output.stderr).contains("Could not open the database"));

    let database = directory.path().join("keryx.db");
    fs::write(
        &path,
        format!(
            "listen = \"127.0.0.1:{port}\"\npublic_url = \"http://127.0.0.1:1\"\n\
             [database]\ntype = \"sqlite\"\npath = {database:?}\n[www]\ndirectory = {www:?}\n"
        ),
    )
    .unwrap();
    let output = keryx(&["--config", path.to_str().unwrap(), "--quiet", "serve"]);
    assert!(!output.status.success());
    assert!(text(&output.stderr).contains("Could not listen"));

    // TLS files that are missing stop it too, before it listens.
    fs::write(
        &path,
        format!(
            "listen = \"127.0.0.1:{port}\"\npublic_url = \"https://127.0.0.1:1\"\n\
             [database]\ntype = \"sqlite\"\npath = {database:?}\n[www]\ndirectory = {www:?}\n\
             [tls]\ncertificate = \"/nothing/cert.pem\"\nkey = \"/nothing/key.pem\"\n"
        ),
    )
    .unwrap();
    let output = keryx(&["--config", path.to_str().unwrap(), "--quiet", "serve"]);
    assert!(!output.status.success());
    assert!(text(&output.stderr).contains("TLS"));
}

#[test]
fn the_commented_example_is_a_valid_configuration() {
    let directory = TempDir::new().unwrap();
    let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = free.local_addr().unwrap().port();
    drop(free);
    let example = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/server.toml")).unwrap();
    let mut filled = example
        .replace("/var/lib/keryx", directory.path().to_str().unwrap())
        .replace("127.0.0.1:7777", &format!("127.0.0.1:{port}"));

    // The commented-out sections have to be valid too.
    let mut uncommented = String::new();
    let mut in_captcha = false;
    for line in filled.lines() {
        if line.starts_with("# [captcha]") {
            in_captcha = true;
        }
        if in_captcha && !line.starts_with("# For the tests only") {
            uncommented.push_str(line.trim_start_matches("# "));
        } else {
            uncommented.push_str(line);
        }
        uncommented.push('\n');
    }
    filled = uncommented;
    let path = directory.path().join("server.toml");
    fs::write(&path, filled).unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_keryx"))
        .args(["--config", path.to_str().unwrap(), "--quiet", "serve"])
        .spawn()
        .unwrap();
    let url = format!("http://127.0.0.1:{port}/api/v1");
    let mut up = false;
    for _ in 0..500 {
        if ureq::get(&url).call().is_ok() {
            up = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let killed = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(killed.success());
    assert!(child.wait().unwrap().success());
    assert!(up, "the example did not start");
    assert!(directory.path().join("keryx.db").exists());
}

#[test]
fn user_add_tells_the_server_and_shows_its_reply() {
    let server = Server::start("", "");
    let config = config_of(&server);

    let added = keryx(&["--config", &config, "user", "add", "sara_x", "--admin"]);
    assert!(added.status.success(), "{}", text(&added.stderr));
    let link = text(&added.stdout);
    let link = link.trim();
    assert!(link.starts_with(&format!("{}/setup#", server.origin)));
    assert!(
        !text(&added.stderr).contains('#'),
        "only stdout has the link"
    );

    // --json prints the same wrapped object the API returns.
    let added = keryx(&[
        "--config",
        &config,
        "user",
        "add",
        "babak_x",
        "--json",
        "--display-name",
        "بابک",
    ]);
    assert!(added.status.success());
    let reply: Value = serde_json::from_slice(&added.stdout).unwrap();
    assert_eq!(reply["ok"], true);
    assert!(reply["data"]["link"].as_str().is_some());

    // A refusal fails the command, and --json still shows it.
    let again = keryx(&["--config", &config, "user", "add", "sara_x"]);
    assert!(!again.status.success());
    assert!(text(&again.stderr).contains("That username is taken"));
    let again = keryx(&["--config", &config, "user", "add", "sara_x", "--json"]);
    assert!(!again.status.success());
    let reply: Value = serde_json::from_slice(&again.stdout).unwrap();
    assert_eq!(reply["code"], "username_taken");

    // The link really works, so the admin can log in.
    let token = link.split_once('#').unwrap().1;
    let (status, reply) = server.call(
        "POST",
        "/api/v1/setup",
        None,
        Some(&json!({"token": token, "password": "a long enough password"})),
    );
    assert_eq!(status, 200, "{reply}");
}

#[test]
fn remote_takes_a_host_a_port_or_a_scheme() {
    let server = Server::start("", "");
    let config = config_of(&server);
    let port = server.origin.rsplit(':').next().unwrap().to_string();

    let forms = [
        "127.0.0.1".to_string(),
        "localhost".to_string(),
        format!("127.0.0.1:{port}"),
        format!("http://127.0.0.1:{port}/"),
        "http://127.0.0.1".to_string(),
    ];
    for (index, remote) in forms.iter().enumerate() {
        let username = format!("user_{index}");
        let added = keryx(&[
            "--config", &config, "user", "add", &username, "--remote", remote,
        ]);
        assert!(added.status.success(), "{remote}: {}", text(&added.stderr));
    }

    // A port of its own wins over the configuration's, and nothing listens there.
    let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let unused = free.local_addr().unwrap().port();
    drop(free);
    let added = keryx(&[
        "--config",
        &config,
        "user",
        "add",
        "nobody_x",
        "--remote",
        &format!("127.0.0.1:{unused}"),
    ]);
    assert!(!added.status.success());
    assert!(text(&added.stderr).contains("Could not reach"));

    // https to a plain server fails, with or without checking the certificate.
    for extra in [vec![], vec!["--insecure"]] {
        let mut args = vec![
            "--config",
            &config,
            "user",
            "add",
            "nobody_x",
            "--remote",
            "https://127.0.0.1",
        ];
        args.extend(extra);
        assert!(!keryx(&args).status.success());
    }
    let (status, _) = server.call(
        "GET",
        "/api/v1/accounts?search=nobody",
        Some(OPERATOR_TOKEN),
        None,
    );
    assert_eq!(status, 200);
}

#[test]
fn without_an_operator_token_there_is_no_operator_access() {
    let server = Server::start("", "# no operator\n");
    let added = keryx(&["--config", &config_of(&server), "user", "add", "sara_x"]);
    assert!(!added.status.success());
    assert!(text(&added.stderr).contains("no [operator] token"));
    let (status, reply) = server.call(
        "POST",
        "/api/v1/accounts",
        Some(OPERATOR_TOKEN),
        Some(&json!({"username": "sara_x"})),
    );
    assert_eq!(status, 401, "{reply}");
}

#[test]
fn sigterm_shuts_down_cleanly_and_data_survives_a_restart() {
    let mut server = Server::start("", "");
    let config = config_of(&server);
    assert!(
        keryx(&["--config", &config, "user", "add", "sara_x"])
            .status
            .success()
    );
    let started = Instant::now();
    assert!(server.stop().success());
    assert!(started.elapsed() < Duration::from_secs(8));
    let log = fs::read_to_string(server.path("server.log")).unwrap();
    assert!(log.contains("Shut down"));

    // One database file with nothing beside it.
    let mut name_list = Vec::new();
    for entry in fs::read_dir(server.path("")).unwrap() {
        name_list.push(entry.unwrap().file_name().to_string_lossy().to_string());
    }
    assert!(name_list.contains(&"keryx.db".to_string()));
    for name in &name_list {
        assert!(
            !name.contains("-wal") && !name.contains("-shm") && !name.contains("-journal"),
            "{name}"
        );
    }

    let mut child = Command::new(env!("CARGO_BIN_EXE_keryx"))
        .args(["--config", &config, "--quiet", "serve"])
        .spawn()
        .unwrap();
    let mut answer = None;
    for _ in 0..500 {
        let output = keryx(&["--config", &config, "user", "add", "sara_x", "--json"]);
        if !output.stdout.is_empty() {
            answer = Some(output);
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let reply: Value = serde_json::from_slice(&answer.unwrap().stdout).unwrap();
    assert_eq!(reply["code"], "username_taken");
    let killed = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(killed.success());
    assert!(child.wait().unwrap().success());
}

#[test]
fn logging_flags_work_before_or_after_a_subcommand() {
    let server = Server::start("", "");
    let config = config_of(&server);
    let quiet = keryx(&[
        "--config", &config, "user", "add", "a_one", "--quiet", "--trace",
    ]);
    assert!(quiet.status.success());
    assert!(text(&quiet.stderr).contains("One-time link"));
    assert!(!text(&quiet.stderr).contains("HTTP client request"));
    let traced = keryx(&["--trace", "--config", &config, "user", "add", "a_two"]);
    assert!(traced.status.success());
    let log = text(&traced.stderr);
    assert!(log.contains("HTTP client request"));
    assert!(log.contains("\"line_number\""));
    assert!(!log.contains(OPERATOR_TOKEN));
    let debugged = keryx(&["user", "add", "a_three", "--config", &config, "--debug"]);
    assert!(debugged.status.success());
    assert!(text(&debugged.stderr).contains("HTTP client request"));
    let informed = keryx(&["--info", "--config", &config, "user", "add", "a_four"]);
    assert!(informed.status.success());
    assert!(text(&informed.stderr).contains("HTTP client request"));
    let plain = keryx(&["--config", &config, "user", "add", "a_five"]);
    assert!(!text(&plain.stderr).contains("HTTP client request"));
}

#[test]
fn a_tls_server_serves_and_its_client_checks_only_when_it_should() {
    let mut server = Server::start("", "# tls\n");
    assert!(server.origin.starts_with("https://"));
    let config = config_of(&server);
    let port = server.origin.rsplit(':').next().unwrap().to_string();

    let (status, reply) = server.call(
        "POST",
        "/api/v1/accounts",
        Some(OPERATOR_TOKEN),
        Some(&json!({"username": "by_rest"})),
    );
    assert_eq!(status, 200, "{reply}");
    assert!(
        reply["data"]["link"]
            .as_str()
            .unwrap()
            .starts_with("https://")
    );

    // On the default 127.0.0.1, the scheme comes from the configuration and nothing is checked.
    let added = keryx(&["--config", &config, "user", "add", "by_loopback"]);
    assert!(added.status.success(), "{}", text(&added.stderr));

    // With --remote it is checked, and this certificate is in nobody's trust store.
    let remote_forms = [
        "localhost".to_string(),
        format!("localhost:{port}"),
        format!("https://localhost:{port}"),
        format!("https://127.0.0.1:{port}/"),
    ];
    for (index, remote) in remote_forms.iter().enumerate() {
        let username = format!("remote_{index}");
        let refused = keryx(&[
            "--config", &config, "user", "add", &username, "--remote", remote,
        ]);
        assert!(!refused.status.success(), "{remote}");
        assert!(
            text(&refused.stderr).contains("Could not reach"),
            "{remote}: {}",
            text(&refused.stderr)
        );
        let added = keryx(&[
            "--config",
            &config,
            "user",
            "add",
            &username,
            "--remote",
            remote,
            "--insecure",
        ]);
        assert!(added.status.success(), "{remote}: {}", text(&added.stderr));
    }

    // Plain HTTP gets nothing from a TLS port.
    assert!(
        ureq::get(format!("http://127.0.0.1:{port}/api/v1"))
            .call()
            .is_err()
    );
    assert!(server.stop().success());
}

#[test]
fn every_command_accepts_insecure_anywhere_on_its_line() {
    for sections in ["", "# tls\n"] {
        let server = Server::start("", sections);
        let config = config_of(&server);
        let port = server.origin.rsplit(':').next().unwrap().to_string();
        let remote = format!(
            "{}://127.0.0.1:{port}",
            server.origin.split(':').next().unwrap()
        );
        let line_list = [
            vec!["--insecure", "--config", &config, "user", "add", "first_x"],
            vec!["--config", &config, "user", "--insecure", "add", "second_x"],
            vec!["--config", &config, "user", "add", "third_x", "--insecure"],
            vec![
                "--config",
                &config,
                "user",
                "add",
                "fourth_x",
                "--insecure",
                "--remote",
                &remote,
            ],
        ];
        for line in line_list {
            let added = keryx(&line);
            assert!(added.status.success(), "{line:?}: {}", text(&added.stderr));
        }
        let help = keryx(&["--insecure"]);
        assert!(help.status.success());
    }
}
