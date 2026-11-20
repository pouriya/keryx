use std::fs;
use std::process::{Command, Output};

use tempfile::TempDir;

const TEMPLATE: &str = include_str!("../src/agent.toml");

fn agent(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_keryx-agent"))
        .args(args)
        .output()
        .unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).to_string()
}

/// Writes `body` as agent.toml in a new directory and runs `check` on it.
fn check(body: &str) -> Output {
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("agent.toml");
    fs::write(&path, body).unwrap();
    agent(&["--config", path.to_str().unwrap(), "check"])
}

#[test]
fn bare_command_prints_its_long_help_and_exits_zero() {
    let bare = agent(&[]);
    assert!(bare.status.success());
    assert!(text(&bare.stdout).contains("Every command needs --config"));
    assert!(text(&bare.stdout).contains("check"));
}

#[test]
fn config_is_required_with_no_default() {
    let output = agent(&["check"]);
    assert!(!output.status.success());
    assert!(text(&output.stderr).contains("--config is required"));
}

#[test]
fn the_template_is_fine() {
    let output = check(TEMPLATE);
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "agent.toml is fine: 1 wake rule, 0 skills\n"
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn the_template_with_every_commented_part_on_is_fine() {
    let mut body = String::new();
    for line in TEMPLATE.lines() {
        // Uncomment `# key = ...` and `# [table]` lines, but not the prose ones.
        let Some(uncommented) = line.strip_prefix("# ") else {
            body.push_str(line);
            body.push('\n');
            continue;
        };
        let is_setting = match uncommented.split_once(" = ") {
            Some((key, _)) => !key.contains(' '),
            None => uncommented.starts_with('['),
        };
        if is_setting {
            body.push_str(uncommented);
        } else {
            body.push_str(line);
        }
        body.push('\n');
    }
    let directory = TempDir::new().unwrap();
    let script = directory.path().join("sara.star");
    let skills = directory.path().join("skills");
    fs::write(&script, "def wake(event):\n    return True\n").unwrap();
    fs::create_dir(&skills).unwrap();
    let body = body
        .replace("\"01J...\"", "\"01JC0000000000000000000000\"")
        .replace(
            "\"/var/lib/keryx-agent/wake/sara.star\"",
            &format!("{script:?}"),
        )
        .replace("\"/var/lib/keryx-agent/skills\"", &format!("{skills:?}"));
    let output = check(&body);
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "agent.toml is fine: 2 wake rules, 0 skills\n"
    );
}

#[test]
fn settings_left_out_take_their_defaults() {
    let output = check(
        "url = \"https://keryx.example\"\ntoken = \"t\"\nwords = \"w\"\ndirectory = \"/agent\"\n\
         [llm]\nurl = \"https://llm.example\"\nkey = \"k\"\nmodel = \"m\"\n\
         [prompt]\nsystem = \"You help.\"\n",
    );
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "agent.toml is fine: 0 wake rules, 0 skills\n"
    );
}

#[test]
fn an_unknown_key_stops_and_says_where() {
    let output = check(&TEMPLATE.replace("[search]\n", "[search]\nfuzzy = true\n"));
    assert!(!output.status.success());
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("Could not read the configuration"),
        "{stderr}"
    );
    assert!(stderr.contains("fuzzy"), "{stderr}");
}

#[test]
fn a_missing_required_key_stops() {
    let output = check(&TEMPLATE.replace("words = \"...\"", ""));
    assert!(!output.status.success());
    assert!(text(&output.stderr).contains("words"));
}

#[test]
fn every_problem_is_listed_at_once() {
    let body = TEMPLATE
        .replace(
            "url = \"http://127.0.0.1:7777\"",
            "url = \"ws://127.0.0.1:7777\"",
        )
        .replace("https://api.openai.com/v1", "api.openai.com")
        .replace(
            "protocol = \"chat_completions\"",
            "protocol = \"responses\"",
        )
        .replace("timezone = \"UTC\"", "timezone = \"Mars/Olympus\"")
        .replace("storage_warn = 0.8", "storage_warn = 1.5")
        .replace(
            "directory = \"/var/lib/keryx-agent\"",
            "directory = \"keryx-agent\"",
        )
        .replace(
            "# [skills]\n# directory = \"/var/lib/keryx-agent/skills\"",
            "[skills]\ndirectory = \"skills\"",
        )
        .replace(
            "[[wake]]\nchat_type = [\"direct\"]\naction = \"wake\"",
            "[[wake]]\nchat_type = [\"room\"]\ncontent_kind = [\"gif\"]\nevent = [\"poke\"]\n\
             chat = [\"nope\"]\ntopic = [\"01JC0000000000000000000000\", \"bad\"]\n\
             script = \"wake.star\"\naction = \"maybe\"",
        );
    let output = check(&body);
    assert!(!output.status.success());
    let stderr = text(&output.stderr);
    for expected in [
        "is not fine:",
        "url must start with http:// or https://",
        "llm.url must start with http:// or https://",
        "llm.protocol is \"responses\"",
        "timezone \"Mars/Olympus\" is not a known IANA zone",
        "limits.storage_warn is 1.5",
        "directory must be an absolute path",
        "skills.directory must be an absolute path",
        "wake #1 script must be an absolute path",
        "wake #1 action is \"maybe\"",
        "wake #1 chat_type has an unknown \"room\"",
        "wake #1 content_kind has an unknown \"gif\"",
        "wake #1 event has an unknown \"poke\"",
        "wake #1 chat has \"nope\", which is not an id",
        "wake #1 topic has \"bad\", which is not an id",
    ] {
        assert!(
            stderr.contains(expected),
            "{expected} missing from {stderr}"
        );
    }
    assert!(!stderr.contains("01JC0000000000000000000000"), "{stderr}");
}

#[test]
fn an_empty_wake_list_is_refused() {
    let output = check(&TEMPLATE.replace("chat_type = [\"direct\"]", "chat_type = []"));
    assert!(!output.status.success());
    assert!(text(&output.stderr).contains("chat_type"));
}

#[test]
fn a_secret_needs_its_hosts() {
    let output = check(&format!(
        "{TEMPLATE}\n[secrets]\ngithub = {{ value = \"x\", hosts = [] }}\n"
    ));
    assert!(!output.status.success());
    assert!(text(&output.stderr).contains("hosts"));
}

#[test]
fn logging_flags_are_accepted_before_or_after_the_command() {
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("agent.toml");
    fs::write(&path, TEMPLATE).unwrap();
    let path = path.to_str().unwrap();
    for flag in ["--quiet", "--info", "--debug", "--trace"] {
        let before = agent(&[flag, "--config", path, "check"]);
        assert!(before.status.success(), "{flag}");
        let after = agent(&["check", "--config", path, flag]);
        assert!(after.status.success(), "{flag}");
    }
}
