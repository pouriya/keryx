//! The loop, against the real keryx server and a fake LLM, with `drive.rs` as the owner.

pub mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use common::{
    Agent, Drive, FakeLlm, OWNER_WORDS, SARA_WORDS, Server, World, chat_of, configuration,
    events_of, scripted, step_of, system_of, text, tool_call, tool_output, wake_on,
};
use serde_json::{Value, json};
use tempfile::TempDir;

#[test]
fn a_message_wakes_the_agent_which_answers_through_a_script() {
    let mut world = World::start(
        |number, request| match number {
            0 => {
                let chat = chat_of(request);
                tool_call(
                    &format!(
                        "value, err = keryx.send({chat:?}, \"hello owner\")\nprint(value, err)"
                    ),
                    json!([{"intention": "Saying hello", "chat_list": [chat]}]),
                )
            }
            _ => text("done"),
        },
        "",
        "",
    );
    world.owner.send("say hello agent");
    world.owner.expect("sent");
    let requests = world.llm.wait_for(2);
    let events = events_of(&requests[0]).unwrap();
    let group = &events["group_list"][0];
    assert_eq!(group["chat"], world.chat);
    assert_eq!(group["chat_type"], "direct");
    assert_eq!(group["event_list"][0]["event"], "message");
    assert_eq!(group["event_list"][0]["message"]["text"], "hello agent");
    assert_eq!(
        group["event_list"][0]["message"]["sender"]["username"],
        "owner"
    );
    let system = system_of(&requests[0]);
    assert!(system.starts_with("You are a test agent."), "{system}");
    assert!(system.contains("helper_agent"), "{system}");
    assert_eq!(requests[0]["tools"][0]["function"]["name"], "starlark");
    let output = tool_output(&requests[1]);
    assert!(output.contains("\"action\":\"keryx.send\""), "{output}");
    assert!(output.contains("\"seq\":"), "{output}");
    let activity = world.owner.pump_until(&format!("msg {} ", world.chat));
    assert!(activity.contains("kind=Some(Activity)"), "{activity}");
    let reply = world.owner.pump_until(&format!("msg {} ", world.chat));
    assert!(reply.contains("kind=Some(Text)"), "{reply}");
    world.agent.wait_log("\"message\":\"a wake ended\"", 1);
    assert!(world.agent.log().contains("\"state\":\"done\""));
}

/// How many LLM requests arrived, after waiting `ms` for more that should not come.
fn settled(world: &World, ms: u64) -> usize {
    std::thread::sleep(Duration::from_millis(ms));
    world.llm.requests().len()
}

/// The events of every request as one text, to look for what was shown.
fn shown(requests: &[Value]) -> Vec<Value> {
    let mut list = Vec::new();
    for request in requests {
        if let Some(events) = events_of(request) {
            list.push(events);
        }
    }
    list
}

fn event_texts(events: &Value) -> Vec<String> {
    let mut texts = Vec::new();
    for group in events["group_list"].as_array().unwrap() {
        for event in group["event_list"].as_array().unwrap() {
            texts.push(
                event["message"]["text"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            );
        }
    }
    texts
}

fn context_texts(group: &Value) -> Vec<String> {
    let mut texts = Vec::new();
    for message in group["context"].as_array().unwrap() {
        texts.push(message["text"].as_str().unwrap_or_default().to_string());
    }
    texts
}

#[test]
fn only_what_a_rule_wakes_on_reaches_the_loop_with_its_context_and_seen() {
    let rules = TempDir::new().unwrap();
    let mut world = World::start(scripted(vec![None]), "", &wake_on(rules.path(), "go", ""));
    for said in ["one", "two", "three"] {
        world.owner.send(&format!("say {said}"));
        world.owner.expect("sent");
    }
    world.owner.send("pump 2000");
    world.owner.expect("pumped");
    assert_eq!(
        settled(&world, 0),
        0,
        "a message no rule wakes on woke the loop"
    );
    let agent = world.agent_id.clone();
    for line in &world.owner.seen {
        assert!(
            !(line.starts_with("marks ") && line.contains(&agent) && !line.ends_with("seen=0")),
            "an ignored message was marked seen: {line}"
        );
    }
    world.owner.send("say go");
    world.owner.expect("sent");
    let requests = world.llm.wait_for(1);
    let events = events_of(&requests[0]).unwrap();
    assert_eq!(event_texts(&events), ["go"]);
    let group = &events["group_list"][0];
    assert_eq!(
        context_texts(group),
        ["one", "two", "three"],
        "ignored messages are context"
    );
    // Shown is taken: the owner sees the agent saw up to "go", seq 5.
    let started = std::time::Instant::now();
    loop {
        let line = world
            .owner
            .pump_until(&format!("marks {} {agent} ", world.chat));
        if line.ends_with("seen=5") {
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(30), "{line}");
    }
}

#[test]
fn a_rule_overrides_the_prompt_and_the_context() {
    let rules = TempDir::new().unwrap();
    let mut world = World::start(
        scripted(vec![None]),
        "[context]\naround = 20\n",
        &wake_on(
            rules.path(),
            "go",
            "[wake.prompt]\nsystem = \"Override prompt.\"\n[wake.context]\naround = 1\n",
        ),
    );
    for said in ["one", "two", "three", "go"] {
        world.owner.send(&format!("say {said}"));
        world.owner.expect("sent");
    }
    let requests = world.llm.wait_for(1);
    let system = system_of(&requests[0]);
    assert!(system.starts_with("Override prompt."), "{system}");
    assert!(!system.contains("You are a test agent."), "{system}");
    let events = events_of(&requests[0]).unwrap();
    assert_eq!(context_texts(&events["group_list"][0]), ["three"]);
}

#[test]
fn a_thread_root_is_kept_even_if_it_exceeds_the_background_budget() {
    let rules = TempDir::new().unwrap();
    let rule = rules.path().join("thread.star");
    std::fs::write(
        &rule,
        r#"def wake(event):
    text = event["message"]["text"]
    if text.startswith("root "):
        return True
    if text != "go":
        return False
    context = event["context"]
    return len(context) == 2 and context[0]["text"].startswith("root ") and context[1]["text"] == "near"
"#,
    )
    .unwrap();
    let mut world = World::start(
        scripted(vec![None, None, None]),
        "[context]\naround = 3\naround_bytes = \"512B\"\n\
         [context_limits.wake_script]\ncount = 2\nbytes = \"4KiB\"\n",
        &format!(
            "[[wake]]\nevent = [\"added\"]\naction = \"wake\"\n\
             [[wake]]\nevent = [\"message\"]\nscript = {rule:?}\naction = \"wake\"\n"
        ),
    );
    world.owner.send("group Crew helper_agent");
    let requests = world.llm.wait_for(1);
    let chat = chat_of(&requests[0]);
    assert_ne!(chat, world.chat);
    world.owner.expect("chat ");
    world.agent.wait_log("\"message\":\"a wake ended\"", 1);
    let root = format!("root {}", "x".repeat(1024));
    world.owner.send(&format!("use {chat}"));
    world.owner.expect("using ");
    world.owner.send(&format!("say {root}"));
    world.owner.expect("sent");
    let requests = world.llm.wait_for(2);
    let events = events_of(&requests[1]).unwrap();
    let seq = events["group_list"][0]["event_list"][0]["message"]["seq"]
        .as_u64()
        .unwrap();
    world.agent.wait_log("\"message\":\"a wake ended\"", 2);
    for text in ["far", "near", "go"] {
        world.owner.send(&format!("use {chat}"));
        world.owner.expect("using ");
        world.owner.send(&format!("thread {seq}"));
        world.owner.expect("thread ");
        world.owner.send(&format!("say {text}"));
        world.owner.expect("sent");
    }
    let requests = world.llm.wait_for(3);
    let events = events_of(&requests[2]).unwrap();
    let group = &events["group_list"][0];
    assert_eq!(group["thread"], seq);
    assert_eq!(event_texts(&events), ["go"]);
    assert_eq!(context_texts(group), [root]);
}

#[test]
fn rules_go_in_order_and_match_the_sender() {
    let mut world = World::start(
        scripted(vec![None, None]),
        "",
        "[[wake]]\nsender = [\"owner\"]\naction = \"ignore\"\n[[wake]]\naction = \"wake\"\n",
    );
    world.owner.send("say ignored");
    world.owner.expect("sent");
    assert_eq!(settled(&world, 2000), 0);
    let (mut sara, chat) = world.sara();
    sara.send("say hi from sara");
    sara.expect("sent");
    let requests = world.llm.wait_for(1);
    let events = events_of(&requests[0]).unwrap();
    let groups = events["group_list"].as_array().unwrap();
    assert_eq!(groups.len(), 1, "{events}");
    assert_eq!(groups[0]["chat"], chat);
    assert_eq!(groups[0]["chat_type"], "direct");
    assert_eq!(event_texts(&events), ["hi from sara"]);
    assert_eq!(
        groups[0]["event_list"][0]["message"]["sender"]["username"],
        "sara"
    );
}

#[test]
fn a_rule_without_a_match_and_a_failing_wake_script_do_not_wake() {
    let rules = TempDir::new().unwrap();
    let crash = rules.path().join("crash.star");
    std::fs::write(&crash, "def wake(event):\n    return 1 // 0\n").unwrap();
    let writes = rules.path().join("writes.star");
    std::fs::write(
        &writes,
        "def wake(event):\n    keryx.send(event[\"chat\"], \"from a wake script\")\n    return True\n",
    )
    .unwrap();
    let mut world = World::start(
        scripted(vec![None]),
        "",
        &format!(
            "[[wake]]\nscript = {crash:?}\nevent = [\"message\"]\naction = \"wake\"\n\
             [[wake]]\nchat_type = [\"group\"]\naction = \"wake\"\n\
             [[wake]]\nscript = {writes:?}\naction = \"wake\"\n"
        ),
    );
    world.owner.send("say anything");
    world.owner.expect("sent");
    world
        .agent
        .wait_log("\"msg\":\"Skipped a failing wake rule\"", 2);
    world.owner.send("pump 3000");
    world.owner.expect("pumped");
    assert_eq!(settled(&world, 0), 0);
    let chat = world.chat.clone();
    assert!(
        !world
            .owner
            .seen
            .iter()
            .any(|line| line.starts_with(&format!("msg {chat} "))),
        "a wake script sent a message: {:?}",
        world.owner.seen
    );
}

#[test]
fn a_failing_wake_rule_can_open_the_owners_chat_to_report_each_failure() {
    let home = TempDir::new().unwrap();
    let server = Server::start();
    let mut owner = Drive::sign_up(&server, "owner", OWNER_WORDS);
    owner.send("agent add helper_agent direct.create");
    owner.expect("agent ");
    let token = owner.expect("token ");
    let words = owner.expect("words ");
    let llm = FakeLlm::start(scripted(vec![None]));
    let script = home.path().join("crash.star");
    std::fs::write(&script, "def wake(event):\n    return 1 // 0\n").unwrap();
    let wake = format!(
        "[[wake]]\nsender = [\"sara\"]\nscript = {script:?}\nreport_failure = true\n\
         action = \"wake\"\n[[wake]]\nsender = [\"sara\"]\naction = \"wake\"\n"
    );
    let toml = configuration(
        &server,
        &llm,
        home.path(),
        token.strip_prefix("token ").unwrap(),
        words.strip_prefix("words ").unwrap(),
        "",
        &wake,
    );
    let agent = Agent::start(home.path(), &toml);
    agent.wait_ready();

    let mut sara = Drive::sign_up(&server, "sara", SARA_WORDS);
    sara.send("find helper_agent");
    let sara_chat = sara.expect("chat ");
    let sara_chat = sara_chat.strip_prefix("chat ").unwrap();
    let mut owner_chat = String::new();
    for (n, message) in ["first", "second"].into_iter().enumerate() {
        sara.send(&format!("say {message}"));
        sara.expect("sent");
        let line = owner.pump_until("msg ");
        assert!(line.contains("kind=Some(Text)"), "{line}");
        let chat = line.split_whitespace().nth(1).unwrap();
        assert_ne!(chat, sara_chat);
        if n == 0 {
            owner_chat = chat.to_string();
        }
        assert_eq!(chat, owner_chat);
        let requests = llm.wait_for(n + 1);
        let events = events_of(&requests[n]).unwrap();
        assert_eq!(event_texts(&events), [message]);
        agent.wait_log("\"message\":\"a wake ended\"", n + 1);
    }
    agent.wait_log("\"msg\":\"Skipped a failing wake rule\"", 2);
    assert!(owner.pump(500, "msg ").is_empty());
}

#[test]
fn group_events_wake_on_being_added_and_on_mentions() {
    let mut world = World::start(
        scripted(vec![None, None]),
        "",
        "[[wake]]\nevent = [\"added\"]\naction = \"wake\"\n\
         [[wake]]\nchat_type = [\"group\"]\nmentions_me = true\naction = \"wake\"\n",
    );
    world.owner.send("group Crew helper_agent");
    let requests = world.llm.wait_for(1);
    let events = events_of(&requests[0]).unwrap();
    let group = events["group_list"][0]["chat"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(group, world.chat, "the wake is still for the direct chat");
    let first = &events["group_list"][0];
    assert_eq!(first["chat"], group);
    assert_eq!(first["chat_type"], "group");
    assert_eq!(first["chat_name"], "Crew");
    assert_eq!(first["event_list"][0]["event"], "added");
    world.agent.wait_log("\"message\":\"a wake ended\"", 1);
    world.owner.send(&format!("use {group}"));
    world.owner.expect(&format!("using {group}"));
    world.owner.send("say just talking");
    world.owner.expect("sent");
    assert_eq!(
        settled(&world, 2000),
        1,
        "a plain group message woke the loop"
    );
    world.owner.send(&format!("use {group}"));
    world.owner.expect(&format!("using {group}"));
    world.owner.send("mention helper_agent please look");
    world.owner.expect("sent");
    let requests = world.llm.wait_for(2);
    let events = events_of(&requests[1]).unwrap();
    let event = &events["group_list"][0]["event_list"][0];
    assert_eq!(event["mentions_me"], true, "{events}");
    assert_eq!(event["message"]["text"], "@helper_agent please look");
    assert_eq!(event_texts(&events).len(), 1, "{events}");
}

#[test]
fn reactions_and_deletes_are_events_of_their_own() {
    let mut world = World::start(
        |number, request| match number {
            0 => tool_call(
                &format!(
                    "print(json.encode(keryx.send({:?}, \"pick me\")))",
                    chat_of(request)
                ),
                json!([{"intention": "Asking", "chat_list": []}]),
            ),
            _ => text("done"),
        },
        "",
        "[[wake]]\nevent = [\"reaction\", \"delete\"]\naction = \"wake\"\n\
         [[wake]]\nevent = [\"message\"]\ncontent_kind = [\"text\"]\naction = \"wake\"\n",
    );
    world.owner.send("say start");
    world.owner.expect("sent");
    world.llm.wait_for(2);
    let reply = world.owner.pump_until(&format!("msg {} ", world.chat));
    let seq: u64 = reply.split_whitespace().nth(2).unwrap().parse().unwrap();
    world.agent.wait_log("\"message\":\"a wake ended\"", 1);
    world.owner.send(&format!("react {seq} 👍"));
    let reaction: u64 = world.owner.expect("reacted ")["reacted ".len()..]
        .parse()
        .unwrap();
    let requests = world.llm.wait_for(3);
    let events = events_of(&requests[2]).unwrap();
    let event = &events["group_list"][0]["event_list"][0];
    assert_eq!(event["event"], "reaction", "{events}");
    assert!(event.to_string().contains("👍"), "{events}");
    world.agent.wait_log("\"message\":\"a wake ended\"", 2);
    world.owner.send(&format!("delete {reaction}"));
    world.owner.expect("deleted");
    let requests = world.llm.wait_for(4);
    let events = events_of(&requests[3]).unwrap();
    assert_eq!(
        events["group_list"][0]["event_list"][0]["event"], "delete",
        "{events}"
    );
}

#[test]
fn send_final_sends_the_last_text_as_a_reply() {
    let mut world = World::start(|_, _| text("all done here"), "send_final = true\n", "");
    world.owner.send("say finish up");
    world.owner.expect("sent");
    let reply = world.owner.pump_until(&format!("msg {} ", world.chat));
    assert!(reply.contains("kind=Some(Text)"), "{reply}");
    assert!(reply.contains("reply=Some(2)"), "{reply}");
}

#[test]
fn the_final_text_is_not_sent_by_default() {
    let mut world = World::start(|_, _| text("nobody reads this"), "", "");
    world.owner.send("say hello");
    world.owner.expect("sent");
    world.llm.wait_for(1);
    world.agent.wait_log("\"message\":\"a wake ended\"", 1);
    world.owner.send("pump 1500");
    world.owner.expect("pumped");
    let chat = world.chat.clone();
    assert!(
        !world
            .owner
            .seen
            .iter()
            .any(|line| line.starts_with(&format!("msg {chat} "))),
        "{:?}",
        world.owner.seen
    );
}

#[test]
fn a_bad_intention_list_runs_nothing() {
    let mut world = World::start(
        |number, request| {
            let chat = chat_of(request);
            let script = format!("keryx.send({chat:?}, \"should not go\")\nprint(\"ran\")");
            match number {
                0 => tool_call(&script, json!([])),
                1 => tool_call(
                    &script,
                    json!([{"intention": "x".repeat(141), "chat_list": [chat]}]),
                ),
                2 => tool_call(
                    &script,
                    json!([
                        {"intention": "One", "chat_list": [chat]},
                        {"intention": "Two", "chat_list": [chat]}
                    ]),
                ),
                _ => text("done"),
            }
        },
        "",
        "",
    );
    world.owner.send("say try");
    world.owner.expect("sent");
    let requests = world.llm.wait_for(4);
    for request in &requests[1..4] {
        let output = tool_output(request);
        assert!(!output.contains("ran"), "{output}");
        assert!(output.contains("intention"), "{output}");
    }
    world.owner.send("pump 1500");
    world.owner.expect("pumped");
    let chat = world.chat.clone();
    assert!(
        !world
            .owner
            .seen
            .iter()
            .any(|line| line.starts_with(&format!("msg {chat} "))),
        "{:?}",
        world.owner.seen
    );
}

#[test]
fn new_events_join_before_the_next_call_and_context_is_not_sent_again() {
    let release = Arc::new(AtomicBool::new(false));
    let held = release.clone();
    let mut world = World::start(
        move |number, request| match number {
            0 => {
                while !held.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(20));
                }
                tool_call(
                    "print(\"step one\")",
                    json!([{"intention": "Waiting", "chat_list": [chat_of(request)]}]),
                )
            }
            _ => text("done"),
        },
        "",
        "",
    );
    world.owner.send("say first");
    world.owner.expect("sent");
    world.llm.wait_for(1);
    world.owner.send("say second");
    world.owner.expect("sent");
    world.owner.send("pump 1000");
    world.owner.expect("pumped");
    release.store(true, Ordering::SeqCst);
    let requests = world.llm.wait_for(2);
    let every = shown(&requests[1..2]);
    assert_eq!(
        event_texts(&every[0]),
        ["second"],
        "the newest events message"
    );
    let all = common::request_text(&requests[1]);
    assert_eq!(all.matches("\"text\":\"first\"").count(), 1, "{all}");
    assert_eq!(all.matches("\"text\":\"second\"").count(), 1, "{all}");
}

#[test]
fn deferred_events_ride_along_with_the_next_wake() {
    let rules = TempDir::new().unwrap();
    let wake = format!(
        "{}{}",
        wake_on(rules.path(), "later please", ""),
        wake_on(
            rules.path(),
            "now",
            "[wake.prompt]\nsystem = \"Fresh rule.\""
        ),
    );
    let mut world = World::start(
        |number, request| match number {
            0 => {
                let events = events_of(request).unwrap();
                let group = events["group_list"][0]["group"]
                    .as_str()
                    .unwrap()
                    .to_string();
                tool_call(
                    &format!("print(json.encode(inbox.defer([{group:?}])))"),
                    json!([{"intention": "Later", "chat_list": []}]),
                )
            }
            _ => text("done"),
        },
        "",
        &wake,
    );
    world.owner.send("say later please");
    world.owner.expect("sent");
    let requests = world.llm.wait_for(2);
    assert!(tool_output(&requests[1]).contains("{\"count\":1}"));
    world.agent.wait_log("\"message\":\"a wake ended\"", 1);
    assert_eq!(
        settled(&world, 2500),
        2,
        "a deferred event woke the loop by itself"
    );
    world.owner.send("say now");
    world.owner.expect("sent");
    let requests = world.llm.wait_for(3);
    let events = events_of(&requests[2]).unwrap();
    assert_eq!(event_texts(&events), ["later please", "now"]);
    assert!(system_of(&requests[2]).starts_with("Fresh rule."));
}

#[test]
fn the_tool_call_limit_ends_the_wake_through_the_limit_prompt() {
    let mut world = World::start(
        |_, _| {
            tool_call(
                "print(\"working\")",
                json!([{"intention": "Working", "chat_list": []}]),
            )
        },
        "limit = \"LIMIT TEST PROMPT\"\n[limits]\ntool_calls = 2\n",
        "",
    );
    world.owner.send("say work forever");
    world.owner.expect("sent");
    world.agent.wait_log("\"message\":\"a wake ended\"", 1);
    assert!(
        world.agent.log().contains("\"state\":\"limit\""),
        "{}",
        world.agent.log()
    );
    let count = settled(&world, 1500);
    assert!((3..=4).contains(&count), "{count} calls for a limit of 2");
    let requests = world.llm.requests();
    let last = common::request_text(requests.last().unwrap());
    assert!(last.contains("LIMIT TEST PROMPT"), "{last}");
    assert!(
        !common::request_text(&requests[1]).contains("LIMIT TEST PROMPT"),
        "the limit prompt came early"
    );
    // The owner sees the agent stuck, until a person tells it to go on.
    let agent = world.agent_id.clone();
    loop {
        let line = world.owner.pump_until(&format!("presence {agent} "));
        if line.contains("Some(Trouble)") {
            assert!(line.ends_with("code=stuck"), "{line}");
            break;
        }
    }
    world.owner.send("say continue");
    world.owner.expect("sent");
    world.llm.wait_for(count + 1);
    loop {
        let line = world.owner.pump_until(&format!("presence {agent} "));
        if line.contains("Some(Online)") {
            break;
        }
    }
}

#[test]
fn llm_failures_back_off_show_trouble_and_recover() {
    let mut world = World::start(
        |number, request| match number {
            0 => (500, json!({"error": {"message": "down"}})),
            1 => (429, json!({"error": {"message": "slow down"}})),
            2 => (503, json!({"error": {"message": "still down"}})),
            3 => tool_call(
                &format!("keryx.send({:?}, \"made it\")", chat_of(request)),
                json!([{"intention": "Recovering", "chat_list": []}]),
            ),
            _ => text("done"),
        },
        "",
        "",
    );
    world.owner.send("say are you there");
    world.owner.expect("sent");
    let agent = world.agent_id.clone();
    loop {
        let line = world.owner.pump_until(&format!("presence {agent} "));
        if line.contains("Some(Trouble)") {
            assert!(line.ends_with("code=llm_failing"), "{line}");
            break;
        }
    }
    let requests = world.llm.wait_for(5);
    // Each retry is the same call.
    for request in &requests[1..4] {
        assert_eq!(request["messages"], requests[0]["messages"]);
    }
    world.owner.pump_until(&format!("msg {} ", world.chat));
    loop {
        let line = world.owner.pump_until(&format!("presence {agent} "));
        if line.contains("Some(Online)") {
            break;
        }
    }
    world.agent.wait_log("\"message\":\"a wake ended\"", 1);
    assert!(world.agent.log().contains("\"state\":\"done\""));
}

/// Whether a request is a compaction call, by its prompt.
fn compacting(request: &Value) -> bool {
    common::request_text(request).contains("COMPACT TEST PROMPT")
}

#[test]
fn a_long_wake_is_compacted_when_its_input_grows_too_large() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    let mut world = World::start(
        move |_, request| {
            if compacting(request) {
                return text("SUMMARY-OF-EARLIER-STEPS");
            }
            match counted.fetch_add(1, Ordering::SeqCst) {
                0..=3 => tool_call(
                    "print(\"y\" * 1500)",
                    json!([{"intention": "Filling", "chat_list": []}]),
                ),
                _ => text("done"),
            }
        },
        "compact = \"COMPACT TEST PROMPT\"\n[context]\nmax = \"4KiB\"\n",
        "",
    );
    world.owner.send("say fill it");
    world.owner.expect("sent");
    world.agent.wait_log("\"message\":\"a wake ended\"", 1);
    let requests = world.llm.requests();
    let at = requests
        .iter()
        .position(compacting)
        .unwrap_or_else(|| panic!("no compaction in {} calls", requests.len()));
    let after = &requests[at + 1];
    let text = common::request_text(after);
    assert!(text.contains("SUMMARY-OF-EARLIER-STEPS"), "{text}");
    assert!(
        text.matches(&"y".repeat(1500)).count()
            < common::request_text(&requests[at])
                .matches(&"y".repeat(1500))
                .count(),
        "the compacted input still holds the old outputs"
    );
    assert!(world.agent.log().contains("\"state\":\"done\""));
}

#[test]
fn a_context_length_error_compacts_too() {
    let mut world = World::start(
        |number, request| {
            if compacting(request) {
                return text("SUMMARY-AFTER-ERROR");
            }
            match number {
                0 => tool_call(
                    "print(\"step\")",
                    json!([{"intention": "Working", "chat_list": []}]),
                ),
                1 => (
                    400,
                    json!({"error": {
                        "message": "This model's maximum context length is 8192 tokens.",
                        "type": "invalid_request_error",
                        "code": "context_length_exceeded",
                    }}),
                ),
                _ => text("done"),
            }
        },
        "compact = \"COMPACT TEST PROMPT\"\n",
        "",
    );
    world.owner.send("say go on");
    world.owner.expect("sent");
    let requests = world.llm.wait_for(4);
    assert!(
        compacting(&requests[2]),
        "{}",
        common::request_text(&requests[2])
    );
    assert!(common::request_text(&requests[3]).contains("SUMMARY-AFTER-ERROR"));
}

#[test]
fn a_restart_during_the_llm_call_sends_it_again() {
    let mut world = World::start(
        |number, request| match number {
            0 => {
                std::thread::sleep(Duration::from_secs(4));
                text("too late")
            }
            1 => tool_call(
                &format!("keryx.send({:?}, \"after the restart\")", chat_of(request)),
                json!([{"intention": "Resuming", "chat_list": []}]),
            ),
            _ => text("done"),
        },
        "",
        "",
    );
    world.owner.send("say survive this");
    world.owner.expect("sent");
    let first = world.llm.wait_for(1)[0].clone();
    world.agent.kill();
    world.agent.restart();
    let requests = world.llm.wait_for(3);
    assert_eq!(
        events_of(&requests[1]),
        events_of(&first),
        "the same step is sent again"
    );
    world.owner.pump_until(&format!("msg {} ", world.chat));
    world.agent.wait_log("\"message\":\"a wake ended\"", 1);
    assert_eq!(settled(&world, 1500), 3);
}

#[test]
fn a_restart_mid_script_never_runs_it_again() {
    let mut world = World::start(
        |number, request| match number {
            0 => tool_call(
                &format!(
                    "print(json.encode(keryx.send({:?}, \"before the cut\")))\n\
                     print(\"printed before\")\n\
                     def spin():\n    for i in range(1000000000):\n        pass\n\
                     spin()",
                    chat_of(request)
                ),
                json!([{"intention": "Spinning", "chat_list": []}]),
            ),
            _ => text("done"),
        },
        "",
        "",
    );
    world.owner.send("say spin");
    world.owner.expect("sent");
    let chat = world.chat.clone();
    let sent = world.owner.pump_until(&format!("msg {chat} "));
    assert!(sent.contains("kind=Some(Text)"), "{sent}");
    std::thread::sleep(Duration::from_millis(500));
    world.agent.kill();
    world.agent.restart();
    let requests = world.llm.wait_for(2);
    let output = tool_output(&requests[1]);
    assert!(
        output.starts_with("A restart cut this script short"),
        "{output}"
    );
    assert!(output.contains("printed before"), "{output}");
    assert!(output.contains("\"action\":\"keryx.send\""), "{output}");
    world.agent.wait_log("\"message\":\"a wake ended\"", 1);
    world.owner.send("pump 2000");
    world.owner.expect("pumped");
    assert!(
        !world
            .owner
            .seen
            .iter()
            .any(|line| line.starts_with(&format!("msg {chat} ")) && line.contains("Text")),
        "the cut script's send went twice: {:?}",
        world.owner.seen
    );
}

#[test]
fn a_restart_after_a_wake_does_not_wake_again() {
    let mut world = World::start(scripted(vec![None]), "", "");
    world.owner.send("say once");
    world.owner.expect("sent");
    world.llm.wait_for(1);
    world.agent.wait_log("\"message\":\"a wake ended\"", 1);
    world.agent.kill();
    world.agent.restart();
    world.agent.wait_log("\"message\":\"connected\"", 2);
    assert_eq!(settled(&world, 2500), 1);
    world.owner.send("say twice");
    world.owner.expect("sent");
    let requests = world.llm.wait_for(2);
    assert_eq!(event_texts(&events_of(&requests[1]).unwrap()), ["twice"]);
}

#[test]
fn sigterm_ends_it_at_once() {
    let mut world = World::start(scripted(vec![]), "", "");
    assert!(world.agent.terminate(Duration::from_secs(3)));
}

/// The schedule event among a request's events.
fn schedule_event(events: &Value) -> Value {
    for group in events["group_list"].as_array().unwrap() {
        for event in group["event_list"].as_array().unwrap() {
            if event["event"] == "schedule" {
                let mut found = event.clone();
                found["group"] = group.clone();
                return found;
            }
        }
    }
    panic!("no schedule event in {events}");
}

#[test]
fn a_schedule_fires_with_its_note_and_its_chats_context() {
    let mut world = World::start(
        scripted(vec![
            Some(
                "show(schedule.at(time.add(time.now()[0], \"2s\")[0], \"check the oven\", chat_list=[\"$CHAT\"]))\n\
                 show(schedule.every(\"2s\", \"tick\"))"
                    .to_string(),
            ),
            None,
        ]),
        "[context]\naround = 0\n[context_limits.schedule]\ncount = 1\n",
        "",
    );
    world.owner.send("say bake");
    world.owner.expect("sent");
    let requests = world.llm.wait_for(3);
    let mut notes = Vec::new();
    let requests = {
        let mut requests = requests;
        while notes.len() < 3 {
            notes.clear();
            for events in shown(&requests[2..]) {
                for group in events["group_list"].as_array().unwrap() {
                    for event in group["event_list"].as_array().unwrap() {
                        if event["event"] == "schedule" {
                            notes.push(event.to_string());
                        }
                    }
                }
            }
            requests = world.llm.wait_for(requests.len() + 1);
        }
        requests
    };
    let oven = notes
        .iter()
        .find(|note| note.contains("check the oven"))
        .unwrap_or_else(|| panic!("{notes:?}"));
    assert!(
        notes.iter().filter(|note| note.contains("tick")).count() >= 2,
        "{notes:?}"
    );
    assert_eq!(
        notes
            .iter()
            .filter(|note| note.contains("check the oven"))
            .count(),
        1
    );
    let _ = requests;
    let oven: Value = serde_json::from_str(oven).unwrap();
    let planned = oven["planned_at"].as_str().unwrap();
    let fired = oven["fired_at"].as_str().unwrap();
    assert!(fired >= planned, "{oven}");
    let messages = oven["chat_context"][0]["message_list"].as_array().unwrap();
    assert_eq!(messages.len(), 1, "{oven}");
    assert_eq!(messages[0]["text"], "bake", "{oven}");
}

#[test]
fn a_schedule_missed_while_down_fires_late_and_says_why() {
    let mut world = World::start(
        scripted(vec![
            Some(
                "show(schedule.at(time.add(time.now()[0], \"2s\")[0], \"while away\"))".to_string(),
            ),
            None,
            None,
        ]),
        "",
        "",
    );
    world.owner.send("say plan");
    world.owner.expect("sent");
    world.llm.wait_for(2);
    world.agent.wait_log("\"message\":\"a wake ended\"", 1);
    world.agent.kill();
    std::thread::sleep(Duration::from_secs(5));
    world.agent.restart();
    let requests = world.llm.wait_for(3);
    let events = events_of(&requests[2]).unwrap();
    let event = schedule_event(&events);
    let text = event.to_string();
    assert!(text.contains("while away"), "{text}");
    assert!(text.contains("down"), "it says the agent was down: {text}");
}

#[test]
fn a_joining_schedule_enters_the_running_loop_and_a_waiting_one_does_not() {
    let mut world = World::start(
        move |number, request| match number {
            0 => tool_call(
                "print(json.encode(schedule.at(time.add(time.now()[0], \"1s\")[0], \"joining\", join=True)))\n\
                 print(json.encode(schedule.at(time.add(time.now()[0], \"1s\")[0], \"waiting\")))",
                json!([{"intention": "Planning", "chat_list": []}]),
            ),
            1 => {
                std::thread::sleep(Duration::from_millis(2500));
                tool_call(
                    "print(\"busy\")",
                    json!([{"intention": "Busy", "chat_list": []}]),
                )
            }
            _ => {
                let _ = request;
                text("done")
            }
        },
        "",
        "",
    );
    world.owner.send("say schedule both");
    world.owner.expect("sent");
    let requests = world.llm.wait_for(4);
    let in_run = common::request_text(&requests[2]);
    assert!(in_run.contains("joining"), "{in_run}");
    let newest = events_of(&requests[2]).unwrap().to_string();
    assert!(!newest.contains("\"waiting\""), "{newest}");
    let waited = events_of(&requests[3]).unwrap().to_string();
    let next = if waited.contains("waiting") {
        waited
    } else {
        events_of(&world.llm.wait_for(5)[4]).unwrap().to_string()
    };
    assert!(next.contains("waiting"), "{next}");
}

/// A world whose agent's storage cap is `storage`, warning at `warn`, answering every wake with
/// `script` and reporting each request's events.
fn storage_world(storage: &str, warn: &str, script: &'static str) -> World {
    World::start(
        move |number, _| match number {
            0 => tool_call(script, json!([{"intention": "Storage", "chat_list": []}])),
            _ => text("done"),
        },
        &format!("[limits]\nstorage = {storage:?}\nstorage_warn = {warn}\n"),
        "",
    )
}

#[test]
fn passing_storage_warn_wakes_the_loop_with_usage_and_shows_trouble() {
    let mut world = storage_world("2MiB", "0.05", "print(json.encode(storage.usage()))");
    let requests = world.llm.wait_for(2);
    let events = events_of(&requests[0]).unwrap();
    let text = events.to_string();
    assert!(text.contains("\"storage\""), "{text}");
    for kind in ["runs", "archive", "index"] {
        assert!(text.contains(kind), "{kind} missing from {text}");
    }
    let usage = step_of(&tool_output(&requests[1]));
    assert_eq!(usage.printed[0][0]["limit"], 2 * 1024 * 1024);
    let agent = world.agent_id.clone();
    loop {
        let line = world.owner.pump_until(&format!("presence {agent} "));
        if line.contains("Some(Trouble)") {
            assert!(line.ends_with("code=storage_warn"), "{line}");
            break;
        }
    }
}

#[test]
fn at_the_storage_cap_writes_stop_but_the_archive_keeps_going() {
    let mut world = storage_world(
        "64KiB",
        "0.8",
        "print(json.encode(memory.add(\"one more note\")))\n\
         print(json.encode(file.write(\"tmp/a.txt\", \"more\")))",
    );
    let requests = world.llm.wait_for(2);
    let step = step_of(&tool_output(&requests[1]));
    assert_eq!(step.code(0), "storage_full");
    assert_eq!(step.code(1), "storage_full");
    let agent = world.agent_id.clone();
    loop {
        let line = world.owner.pump_until(&format!("presence {agent} "));
        if line.contains("Some(Trouble)") {
            assert!(line.ends_with("code=storage_full"), "{line}");
            break;
        }
    }
    world.agent.wait_log("\"message\":\"a wake ended\"", 1);
    world.owner.send("say still archived");
    world.owner.expect("sent");
    let requests = world.llm.wait_for(3);
    assert!(
        common::request_text(requests.last().unwrap()).contains("still archived"),
        "a message reached the loop at the cap"
    );
}
