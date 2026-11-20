//! The Starlark tool's built-ins, through the running agent: each test's fake LLM hands the agent
//! scripts, and the test reads back what each step printed and the actions it took.

pub mod common;

use std::fs;
use std::time::Duration;

use common::{FakeHttp, World, request_text, scripted, step_of, system_of, tool_output, wake_on};
use serde_json::{Value, json};
use tempfile::TempDir;

fn some(scripts: &[&str]) -> Vec<Option<String>> {
    let mut list = Vec::new();
    for script in scripts {
        list.push(Some(script.to_string()));
    }
    list
}

/// The ids in a page's `<name>_list`.
fn ids(page: &Value, name: &str) -> Vec<String> {
    let mut list = Vec::new();
    for item in page[name].as_array().unwrap() {
        list.push(item["id"].as_str().unwrap_or_default().to_string());
    }
    list
}

#[test]
fn globals_and_time() {
    let mut world = World::start(
        scripted(some(&[r#"
show([ME["username"], ME["id"], OWNER["username"], CONNECTED])
show(time.now())
show(time.add("2026-11-15T09:12:00Z", "90m"))
show(time.add("2026-11-15T09:12:00Z", "7d"))
show(time.diff("2026-11-15T09:00:00Z", "2026-11-15T09:30:00Z"))
show(time.add("not a time", "5m"))
show(time.add("2026-11-15T09:12:00Z", "soon"))
"#])),
        "",
        "",
    );
    let step = &world.steps("hello", 1)[0];
    assert_eq!(
        step.printed[0],
        json!(["helper_agent", world.agent_id, "owner", true])
    );
    let now = step.value(1).as_str().unwrap();
    assert!(
        now.len() == 20 && now.ends_with('Z') && now.contains('T'),
        "{now}"
    );
    assert_eq!(step.value(2), "2026-11-15T10:42:00Z");
    assert_eq!(step.value(3), "2026-11-22T09:12:00Z");
    assert_eq!(step.value(4), "30m");
    assert_eq!(step.code(5), "bad_argument");
    assert_eq!(step.code(6), "bad_argument");
    assert!(step.actions.is_empty(), "{:?}", step.actions);
}

#[test]
fn reading_and_writing_a_direct_chat() {
    let mut world = World::start_with(
        scripted(some(&[
            r#"
c = "$CHAT"
show(keryx.chat_list())
show(keryx.chat(c))
show(keryx.message_list(c))
first = keryx.send(c, "first reply")
show(first)
show(keryx.send(c, "a reply", reply=2))
show(keryx.react(c, 2, "👍"))
show(keryx.edit(c, first[0]["seq"], "edited reply"))
show(keryx.react(c, 2, "not an emoji"))
show(keryx.chat("01JC0000000000000000000000"))
show(keryx.message(c, 999))
show(keryx.send("not a chat", "x"))
show(keryx.member_list(c))
show(keryx.profile("owner"))
show(keryx.directory_list("owner"))
show(keryx.pack_list())
"#,
            r#"
c = "$CHAT"
page = keryx.message_list(c, limit=50)[0]
show(page)
mine = [m for m in page["message_list"] if m["sender"]["username"] == "helper_agent"]
show(keryx.message(c, mine[0]["seq"]))
show(keryx.message(c, 2))
show(keryx.unreact(c, 2, "👍"))
show(keryx.delete(c, mine[1]["seq"]))
show(keryx.message_list(c, after=2, limit=1))
show(keryx.message_list(c, before=3))
show(keryx.direct("owner"))
"#,
            r#"
c = "$CHAT"
page = keryx.message_list(c, limit=50)[0]
show([[m["seq"], m["deleted"]] for m in page["message_list"]])
"#,
        ])),
        "",
        "",
        "directory.search direct.create",
    );
    let chat = world.chat.clone();
    let steps = world.steps("hello agent", 3);
    let step = &steps[0];
    let chat_list = step.value(0);
    assert_eq!(chat_list["more"], false);
    let listed = chat_list["chat_list"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == chat)
        .unwrap_or_else(|| panic!("{chat_list}"));
    assert_eq!(listed["type"], "direct");
    assert_eq!(listed["name"], "owner");
    let one = step.value(1);
    assert_eq!(one["id"], chat);
    assert_eq!(one["type"], "direct");
    let page = step.value(2);
    let last = page["message_list"].as_array().unwrap().last().unwrap();
    assert_eq!(last["text"], "hello agent");
    assert_eq!(last["sender"]["username"], "owner");
    assert_eq!(last["kind"], "text");
    let first_seq = step.value(3)["seq"].as_u64().unwrap();
    let reply_seq = step.value(4)["seq"].as_u64().unwrap();
    assert!(reply_seq > first_seq);
    let reaction_seq = step.value(5)["seq"].as_u64().unwrap();
    assert!(step.value(6)["seq"].as_u64().unwrap() > reaction_seq);
    assert_eq!(step.code(7), "bad_emoji", "core's own code");
    assert_eq!(step.code(8), "not_found");
    assert_eq!(step.code(9), "not_found");
    assert_eq!(step.code(10), "bad_argument");
    assert_eq!(step.code(11), "not_group");
    let profile = step.value(12);
    assert_eq!(profile["username"], "owner");
    assert_eq!(profile["agent"], false);
    let directory = step.value(13);
    assert!(
        directory["directory_list"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["kind"] == "account" && item["name"] == "owner"
                || item["username"] == "owner"),
        "{directory}"
    );
    let packs = step.value(14);
    assert!(
        packs["pack_list"]
            .as_array()
            .unwrap()
            .iter()
            .any(|pack| pack["name"] == "Default"),
        "{packs}"
    );
    assert_eq!(step.actions_of("keryx.send").len(), 2);
    let reactions = step.actions_of("keryx.react");
    assert_eq!(reactions.len(), 2);
    assert_eq!(reactions[0]["seq"], reaction_seq);
    assert_eq!(reactions[1]["err"], "bad_emoji");
    assert_eq!(step.actions_of("keryx.edit").len(), 1);
    for action in &step.actions {
        assert!(!action.to_string().contains("first reply"), "{action}");
    }

    // The owner gets the reply pointing at their message and the reaction on it.
    let mut kinds = Vec::new();
    while kinds.len() < 3 {
        let line = world.owner.pump_until(&format!("msg {chat} "));
        kinds.push(line);
    }
    assert!(kinds[0].contains("kind=Some(Text)") && kinds[0].contains("reply=None"));
    assert!(kinds[1].contains("kind=Some(Text)") && kinds[1].contains("reply=Some(2)"));
    assert!(kinds[2].contains("kind=Some(Reaction)") && kinds[2].contains("on=Some(2)"));

    let step = &steps[1];
    let mine = step.value(1);
    assert_eq!(mine["seq"], first_seq);
    assert_eq!(mine["text"], "edited reply");
    assert_eq!(mine["edited"], true);
    assert_eq!(mine["sender"]["username"], "helper_agent");
    assert!(mine["edit_list"].as_array().unwrap().len() == 1, "{mine}");
    let theirs = step.value(2);
    let reactions = theirs["reaction_list"].as_array().unwrap();
    assert_eq!(reactions.len(), 1, "{theirs}");
    assert!(reactions[0].to_string().contains("👍"), "{theirs}");
    assert!(step.value(3)["seq"].as_u64().is_some());
    assert!(step.value(4)["seq"].as_u64().is_some());
    let after = step.value(5);
    assert_eq!(after["message_list"].as_array().unwrap().len(), 1);
    assert_eq!(after["message_list"][0]["seq"], 3);
    assert_eq!(after["more"], true);
    let before = step.value(6);
    for message in before["message_list"].as_array().unwrap() {
        assert!(message["seq"].as_u64().unwrap() < 3, "{before}");
    }
    assert_eq!(step.value(7)["chat"], chat);
    assert_eq!(step.actions_of("keryx.unreact").len(), 1);
    assert_eq!(step.actions_of("keryx.delete").len(), 1);

    let listed = steps[2].printed[0].as_array().unwrap();
    assert!(
        listed.iter().all(|pair| pair[0] != reply_seq),
        "the deleted reply {reply_seq} is still listed: {listed:?}"
    );
    assert!(listed.iter().any(|pair| pair[0] == first_seq));
}

#[test]
fn files_stay_inside_tmp_and_chats() {
    let mut world = World::start(
        scripted(some(&[r#"
show(file.write("tmp/notes/a.txt", "hello"))
show(file.write("tmp/notes/a.txt", " world", append=True))
show(file.read("tmp/notes/a.txt"))
show(file.read("tmp/notes/a.txt", offset=6))
show(file.path_list())
show(file.write("a.txt", "outside"))
show(file.write("chats/x.txt", "no"))
show(file.write("tmp/../x.txt", "no"))
show(file.write("/etc/x.txt", "no"))
show(file.read("tmp/../agent.db"))
show(file.read("tmp/missing.txt"))
show(file.write("tmp/b.txt", "bee"))
show(file.remove("tmp/notes/a.txt"))
show(file.read("tmp/notes/a.txt"))
show(storage.remove_files(["tmp/b.txt"]))
show(file.read("tmp/b.txt"))
show(file.remove("tmp/missing.txt"))
"#])),
        "",
        "",
    );
    let step = &world.steps("files", 1)[0];
    assert_eq!(
        step.value(0),
        &json!({"path": "tmp/notes/a.txt", "size": 5})
    );
    assert_eq!(step.value(1)["size"], 11);
    assert_eq!(
        step.value(2),
        &json!({"text": "hello world", "more": false})
    );
    assert_eq!(step.value(3)["text"], "world");
    let listed = step.value(4);
    assert!(
        listed["path_list"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["path"] == "tmp/notes/a.txt" && item["size"] == 11),
        "{listed}"
    );
    for n in 5..=9 {
        assert_eq!(step.code(n), "path_not_allowed", "show #{n}");
    }
    assert_eq!(step.code(10), "not_found");
    assert_eq!(step.value(11)["size"], 3);
    assert_eq!(step.value(12)["path"], "tmp/notes/a.txt");
    assert_eq!(step.code(13), "not_found");
    assert_eq!(step.value(14)["count"], 1);
    assert_eq!(step.code(15), "not_found");
    assert_eq!(step.code(16), "not_found");
    assert!(!world.agent.directory.join("a.txt").exists());
    assert!(!world.agent.directory.join("x.txt").exists());
    // Every file change is an action, and the file's text never is.
    assert!(step.actions.len() >= 5, "{:?}", step.actions);
    for action in &step.actions {
        assert!(!action.to_string().contains("hello"), "{action}");
    }
}

#[test]
fn memory_is_kept_per_chat_searched_and_capped() {
    let mut world = World::start(
        scripted(vec![
            Some(
                r#"
show(memory.add("The deploy key rotates every Friday"))
show(memory.add("Sara prefers green tea", chat="$CHAT"))
show(memory.add("x" * 20000))
"#
                .to_string(),
            ),
            Some(
                r#"
c = "$CHAT"
show(memory.note_list())
show(memory.note_list(chat=c))
first = memory.note_list()[0]["note_list"][0]["id"]
show(memory.note(first))
show(memory.replace(first, "The deploy key rotates every Monday"))
show(memory.add("a third note"))
show(memory.add("a fourth note"))
show(memory.note("01JC0000000000000000000000"))
third = memory.note_list()[0]["note_list"][0]["id"]
show(memory.delete(third))
show(memory.note(third))
show(memory.add("room again"))
show(memory.note(first))
"#
                .to_string(),
            ),
            Some(
                r#"
c = "$CHAT"
show(memory.note_list(query="deploy"))
show(memory.note_list(query="Friday"))
show(memory.note_list(query="tea"))
show(memory.note_list(query="tea", chat=c))
"#
                .to_string(),
            ),
            None,
            None,
        ]),
        "[limits]\nmemory_notes = 3\n",
        "",
    );
    let chat = world.chat.clone();
    let steps = world.steps("remember", 3);
    let step = &steps[0];
    let first = step.value(0)["id"].as_str().unwrap().to_string();
    let second = step.value(1)["id"].as_str().unwrap().to_string();
    assert_ne!(first, second);
    assert_eq!(step.code(2), "too_large");
    assert_eq!(step.actions_of("memory.add").len(), 2, "{:?}", step.actions);

    let step = &steps[1];
    assert_eq!(
        ids(step.value(0), "note_list"),
        std::slice::from_ref(&first)
    );
    let note = &step.value(0)["note_list"][0];
    assert_eq!(note["text"], "The deploy key rotates every Friday");
    assert!(note["chat"].is_null(), "{note}");
    assert!(!note["source"]["run"].is_null(), "{note}");
    assert_eq!(note["source"]["chat"], chat);
    assert_eq!(
        ids(step.value(1), "note_list"),
        std::slice::from_ref(&second)
    );
    assert_eq!(step.value(1)["note_list"][0]["chat"], chat);
    assert_eq!(step.value(2)["text"], "The deploy key rotates every Friday");
    assert_eq!(step.value(3)["id"], first);
    assert!(step.value(4)["id"].is_string());
    assert_eq!(
        step.code(5),
        "memory_full",
        "memory_notes counts every memory together"
    );
    assert_eq!(step.code(6), "not_found");
    assert!(step.value(7)["id"].is_string());
    assert_eq!(step.code(8), "not_found");
    assert!(step.value(9)["id"].is_string(), "a deleted note makes room");
    assert_eq!(
        step.value(10)["text"],
        "The deploy key rotates every Monday"
    );
    assert_eq!(step.actions_of("memory.replace").len(), 1);
    assert_eq!(step.actions_of("memory.delete").len(), 1);

    // The next wake's preamble counts what each memory holds, and never shows a note.
    world.owner.send("say again");
    world.owner.expect("sent");
    let requests = world.llm.wait_for(5);
    let system = system_of(&requests[4]);
    assert!(system.contains("the global memory: 2 notes"), "{system}");
    assert!(
        system.contains(&format!("chat {chat} (owner): 1 note")),
        "{system}"
    );
    assert!(
        !system.contains("rotates every Monday"),
        "a note's text reached the preamble: {system}"
    );

    // Notes are found by search, each in its own memory.
    let step = &steps[2];
    assert_eq!(
        ids(step.value(0), "note_list"),
        std::slice::from_ref(&first),
        "{}",
        step.text
    );
    assert!(step.value(1)["note_list"].as_array().unwrap().is_empty());
    assert!(
        step.value(2)["note_list"].as_array().unwrap().is_empty(),
        "a chat's note showed in the global memory"
    );
    assert_eq!(
        ids(step.value(3), "note_list"),
        std::slice::from_ref(&second)
    );
    assert!(system.contains("deploy"), "the hint words: {system}");
}

#[test]
fn a_stale_index_is_rebuilt_from_the_saved_notes() {
    let mut world = World::start(
        scripted(vec![
            Some("show(memory.add(\"The moon orchid blooms\"))".to_string()),
            None,
            Some("show(memory.note_list(query=\"orchid\"))\nshow(search.message_list(\"remember\", chat=\"$CHAT\"))".to_string()),
        ]),
        "",
        "",
    );
    let step = &world.steps("remember", 1)[0];
    let id = step.value(0)["id"].clone();
    world.agent.wait_log("\"message\":\"a wake ended\"", 1);
    assert!(world.agent.terminate(Duration::from_secs(3)));

    // The old on-disk schema indexed the document key without storing it.
    let path = world.agent.directory.join("index/meta.json");
    let mut meta: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let key = meta["schema"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|field| field["name"] == "key")
        .unwrap();
    key["options"]["stored"] = json!(false);
    fs::write(&path, serde_json::to_vec(&meta).unwrap()).unwrap();

    world.agent.restart();
    world.agent.wait_log("\"message\":\"connected\"", 2);
    world.owner.send("say look up the note");
    world.owner.expect("sent");
    let requests = world.llm.wait_for(4);
    let step = step_of(&tool_output(&requests[3]));
    let notes = step.value(0)["note_list"].as_array().unwrap();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0]["id"], id);
    assert_eq!(notes[0]["text"], "The moon orchid blooms");
    assert_eq!(texts(step.value(1)), ["remember"]);
}

#[test]
fn past_runs_are_read_and_removed() {
    let mut world = World::start(
        scripted(vec![
            Some("print(\"marker-alpha\")".to_string()),
            None,
            Some(
                r#"
show(log.run_list())
marker = "marker-" + "alpha"
show(log.run_list(query=marker))
show(log.run_list(query="nothing-" + "like-this"))
first = log.run_list(query=marker)[0]["run_list"][0]["id"]
show(log.run(first))
show(log.run("01JC0000000000000000000000"))
show(log.run("bad"))
show(storage.usage())
show(storage.remove_runs("2100-01-01T00:00:00Z"))
show(log.run_list())
show(log.run(first))
"#
                .to_string(),
            ),
            None,
        ]),
        "",
        "",
    );
    world.owner.send("say first");
    world.owner.expect("sent");
    world.llm.wait_for(2);
    world.agent.wait_log("\"message\":\"a wake ended\"", 1);
    let step = &world.steps("second", 3)[2];
    let runs = step.value(0);
    assert_eq!(
        runs["run_list"].as_array().unwrap().len(),
        2,
        "this run and the first: {runs}"
    );
    let marked = step.value(1);
    assert_eq!(marked["run_list"].as_array().unwrap().len(), 1, "{marked}");
    let first = &marked["run_list"][0];
    assert_eq!(first["outcome"], "done");
    assert!(first["ended_at"].is_string());
    assert!(step.value(2)["run_list"].as_array().unwrap().is_empty());
    let run = step.value(3);
    assert_eq!(run["id"], first["id"]);
    let steps = run["step_list"].as_array().unwrap();
    assert!(
        steps[0]["script"]
            .as_str()
            .unwrap_or_default()
            .contains("marker-alpha"),
        "{run}"
    );
    assert!(
        steps[0]["output"].to_string().contains("marker-alpha"),
        "{run}"
    );
    assert_eq!(steps[0]["intention_list"][0]["intention"], "Testing");
    let events = run["event_list"].as_array().unwrap();
    assert_eq!(events.len(), 1, "{run}");
    assert_eq!(events[0]["message"]["text"], "first");
    assert_eq!(step.code(4), "not_found");
    assert_eq!(step.code(5), "bad_argument");
    let usage = step.value(6);
    assert_eq!(usage["limit"], 10u64 * 1024 * 1024 * 1024);
    assert!(usage["total"].as_u64().unwrap() > 0);
    assert!(usage["runs"].as_u64().unwrap() > 0);
    assert!(usage["archive"].as_u64().unwrap() > 0);
    assert_eq!(
        step.value(7)["count"],
        1,
        "the finished run goes, the running one stays"
    );
    assert_eq!(step.value(8)["run_list"].as_array().unwrap().len(), 1);
    assert_eq!(step.code(9), "not_found");
    assert!(!step.actions_of("storage.remove_runs").is_empty());
}

#[test]
fn schedules_are_listed_and_removed() {
    let mut world = World::start(
        scripted(some(&[r#"
c = "$CHAT"
show(schedule.at("2030-01-01T00:00:00Z", "new year", chat_list=[c]))
every = schedule.every("1h", "hourly", join=True)
show(every)
show(schedule.cron("0 9 * * MON", "monday", tz="Europe/Berlin"))
show(schedule.cron("61 * * * *", "bad minute"))
show(schedule.cron("0 9 * * *", "bad zone", tz="Mars/Olympus"))
show(schedule.every("often", "bad duration"))
show(schedule.at("tomorrow", "bad time"))
show(schedule.schedule_list())
show(schedule.remove(every[0]["id"]))
show(schedule.remove(every[0]["id"]))
show(schedule.schedule_list())
"#])),
        "",
        "",
    );
    let chat = world.chat.clone();
    let step = &world.steps("plan", 1)[0];
    let at = step.value(0)["id"].as_str().unwrap().to_string();
    let every = step.value(1)["id"].as_str().unwrap().to_string();
    let cron = step.value(2)["id"].as_str().unwrap().to_string();
    for n in 3..=6 {
        assert_eq!(step.code(n), "bad_argument", "show #{n}");
    }
    let listed = step.value(7);
    let list = listed["schedule_list"].as_array().unwrap();
    assert_eq!(list.len(), 3, "{listed}");
    let find = |id: &str| list.iter().find(|item| item["id"] == id).unwrap().clone();
    let at_item = find(&at);
    assert_eq!(at_item["kind"], "at");
    assert_eq!(at_item["next_at"], "2030-01-01T00:00:00Z");
    assert_eq!(at_item["note"], "new year");
    assert_eq!(at_item["chat_list"], json!([chat]));
    assert_eq!(at_item["join"], false);
    let every_item = find(&every);
    assert_eq!(every_item["kind"], "every");
    assert_eq!(every_item["join"], true);
    let cron_item = find(&cron);
    assert_eq!(cron_item["kind"], "cron");
    // 09:00 in Berlin in winter is 08:00 UTC, on a Monday.
    let next = cron_item["next_at"].as_str().unwrap();
    assert!(next.ends_with("T08:00:00Z"), "{next}");
    assert_eq!(list[0]["id"], every, "the next to fire comes first");
    assert_eq!(step.value(8)["id"], every);
    assert_eq!(step.code(9), "not_found");
    assert_eq!(step.value(10)["schedule_list"].as_array().unwrap().len(), 2);
    assert_eq!(step.actions_of("schedule.remove").len(), 1);
}

#[test]
fn http_reports_status_and_keeps_secrets_out() {
    let http = FakeHttp::start(|request| match request.path.as_str() {
        "/ok" => (200, "{\"fine\": true}".to_string()),
        "/missing" => (404, "no such page".to_string()),
        "/big" => (200, "b".repeat(4096)),
        "/slow" => {
            std::thread::sleep(Duration::from_secs(3));
            (200, "late".to_string())
        }
        _ => (200, "{\"echo\": true}".to_string()),
    });
    let host = http.origin.trim_start_matches("http://").to_string();
    let port = host.split(':').nth(1).unwrap().to_string();
    let origin = http.origin.clone();
    let sections = "[limits]\nhttp_body = \"1KiB\"\n\
                    [http]\nallow = [\"127.0.0.1\"]\n\
                    [secrets]\nkey = { value = \"s3cret-value\", hosts = [\"127.0.0.1\"] }\n\
                    elsewhere = { value = \"other-value\", hosts = [\"example.com\"] }\n";
    let mut world = World::start(
        scripted(vec![Some(
            r#"
u = "ORIGIN"
show(http.get(u + "/ok"))
show(http.get(u + "/missing"))
show(http.post(u + "/echo", json={"a": 1}))
show(http.request("PUT", u + "/form", form={"b": "2"}))
key = secret("key")
print(key)
show(http.get(u + "/auth", headers={"Authorization": "Bearer " + key}))
show(http.get(u + "/query?token=" + key))
show(http.post(u + "/body", body="token " + key))
show(http.get(u + "/auth", headers={"X-Key": secret("elsewhere")}))
show(http.get("http://localhost:PORT/ok"))
show(http.get(u + "/big"))
show(http.get(u + "/slow", timeout="1s"))
show(http.get("not a url"))
"#
            .replace("ORIGIN", &origin)
            .replace("PORT", &port),
        )]),
        sections,
        "",
    );
    let step = &world.steps("fetch", 1)[0];
    let ok = step.value(0);
    assert_eq!(ok["status"], 200);
    assert_eq!(ok["json"], json!({"fine": true}));
    let missing = step.value(1);
    assert_eq!(missing["status"], 404, "a non-2xx answer is a value");
    assert_eq!(missing["body"], "no such page");
    assert!(missing["json"].is_null());
    assert_eq!(step.value(2)["status"], 200);
    assert_eq!(step.value(3)["status"], 200);
    assert_eq!(step.printed[4], "<secret key>");
    assert_eq!(step.value(5)["status"], 200);
    assert_eq!(step.value(6)["status"], 200);
    let in_body = &step.printed[7];
    assert_eq!(step.code(8), "secret_not_allowed");
    assert_eq!(step.code(9), "host_not_allowed");
    assert_eq!(step.code(10), "too_large");
    assert!(!step.code(12).is_empty());

    let requests = http.requests();
    let find = |path: &str| {
        requests
            .iter()
            .find(|request| request.path.starts_with(path))
            .unwrap_or_else(|| panic!("no request to {path}: {requests:?}"))
            .clone()
    };
    let echo = find("/echo");
    assert_eq!(echo.method, "POST");
    assert_eq!(
        serde_json::from_str::<Value>(&echo.body).unwrap(),
        json!({"a": 1})
    );
    let form = find("/form");
    assert_eq!(form.method, "PUT");
    assert_eq!(form.body, "b=2");
    assert_eq!(
        find("/auth").header("Authorization"),
        Some("Bearer s3cret-value")
    );
    assert_eq!(find("/query").path, "/query?token=s3cret-value");
    for request in &requests {
        assert!(
            !request.body.contains("s3cret-value"),
            "a secret reached a body: {request:?} ({in_body})"
        );
        assert!(
            !request
                .headers
                .iter()
                .any(|(_, value)| value.contains("other-value"))
        );
    }
    // Neither the model nor the run log's actions see a secret's value.
    let all = world.llm.requests();
    for request in &all {
        let text = request_text(request);
        assert!(!text.contains("s3cret-value"), "{text}");
        assert!(!text.contains("other-value"), "{text}");
    }
    let calls = step.actions_of("http.request");
    let calls = if calls.is_empty() {
        step.actions
            .iter()
            .filter(|action| {
                action["action"]
                    .as_str()
                    .unwrap_or_default()
                    .starts_with("http")
            })
            .collect()
    } else {
        calls
    };
    assert!(calls.len() >= 6, "{:?}", step.actions);
    let missing_call = calls
        .iter()
        .find(|action| action["path"] == "/missing")
        .unwrap_or_else(|| panic!("{calls:?}"));
    assert_eq!(missing_call["status"], 404);
    assert_eq!(missing_call["method"], "GET");
    assert_eq!(missing_call["host"], "127.0.0.1");
    for action in &calls {
        let text = action.to_string();
        assert!(!text.contains("fine"), "a body reached the actions: {text}");
        assert!(
            !text.contains("token="),
            "a query reached the actions: {text}"
        );
    }
    assert_eq!(step.code(11), "timeout", "an http call past its timeout");
}

fn skills_directory() -> TempDir {
    let directory = TempDir::new().unwrap();
    fs::write(
        directory.path().join("deploy.md"),
        "---\nname: deploy\ndescription: How to deploy the app\n---\nRun make deploy, then watch the logs.\n",
    )
    .unwrap();
    fs::write(directory.path().join("notes.txt"), "not markdown").unwrap();
    directory
}

#[test]
fn skills_are_listed_in_the_preamble_and_read() {
    let skills = skills_directory();
    let mut world = World::start(
        scripted(some(&[r#"
show(skills.skill_list())
show(skills.read("deploy"))
show(skills.read("missing"))
show(skills.write("greet", "Greeting people", "Say hi."))
show(skills.remove("deploy"))
"#])),
        &format!("[skills]\ndirectory = {:?}\n", skills.path()),
        "",
    );
    let step = &world.steps("skills", 1)[0];
    let system = system_of(&world.llm.requests()[0]);
    assert!(system.contains("deploy"), "{system}");
    assert!(system.contains("How to deploy the app"), "{system}");
    assert!(
        !system.contains("Run make deploy"),
        "a skill's body is read, not shown: {system}"
    );
    assert_eq!(
        step.value(0),
        &json!({"skill_list": [{"name": "deploy", "description": "How to deploy the app"}], "more": false})
    );
    assert!(
        step.text.contains("Run make deploy, then watch the logs."),
        "{}",
        step.text
    );
    let read = step
        .printed
        .iter()
        .find(|line| line.is_array() && line[0].is_null() && !line[1].is_null())
        .unwrap_or_else(|| panic!("reading a missing skill did not fail: {}", step.text));
    assert_eq!(read[1]["code"], "not_found");
    let n = step.printed.len();
    assert!(
        !step.printed[n - 2][1].is_null(),
        "writing while not writable: {}",
        step.text
    );
    assert!(
        !step.printed[n - 1][1].is_null(),
        "removing while not writable: {}",
        step.text
    );
    assert!(!skills.path().join("greet.md").exists());
    assert!(skills.path().join("deploy.md").exists());
}

#[test]
fn writable_skills_are_written_and_removed() {
    let skills = skills_directory();
    let mut world = World::start(
        scripted(some(&[
            r#"
show(skills.write("greet", "Greeting people", "Say hi, then ask how they are."))
show(skills.skill_list())
show(skills.write("../escape", "no", "no"))
"#,
            r#"
show(skills.read("greet"))
show(skills.remove("greet"))
show(skills.remove("greet"))
show(skills.skill_list())
"#,
        ])),
        &format!(
            "[skills]\ndirectory = {:?}\nwritable = true\n",
            skills.path()
        ),
        "",
    );
    let steps = world.steps("teach", 2);
    let step = &steps[0];
    assert_eq!(step.value(0)["name"], "greet");
    let names: Vec<String> = step.value(1)["skill_list"]
        .as_array()
        .unwrap()
        .iter()
        .map(|skill| skill["name"].as_str().unwrap().to_string())
        .collect();
    assert!(names.contains(&"greet".to_string()) && names.contains(&"deploy".to_string()));
    assert!(!step.printed[2][1].is_null(), "{}", step.text);
    assert!(!skills.path().parent().unwrap().join("escape.md").exists());
    assert!(!step.actions.is_empty(), "a skill change is an action");
    let step = &steps[1];
    assert!(
        step.text.contains("Say hi, then ask how they are."),
        "{}",
        step.text
    );
    let n = step.printed.len();
    assert_eq!(step.value(n - 3)["name"], "greet");
    assert_eq!(step.code(n - 2), "not_found");
    assert_eq!(step.value(n - 1)["skill_list"].as_array().unwrap().len(), 1);
    let mut left = Vec::new();
    for entry in fs::read_dir(skills.path()).unwrap() {
        left.push(entry.unwrap().file_name().to_string_lossy().to_string());
    }
    left.sort();
    assert_eq!(left, ["deploy.md", "notes.txt"]);
}

#[test]
fn a_failing_script_reports_and_the_wake_goes_on() {
    let mut world = World::start(
        scripted(some(&[
            "show(keryx.send())\nfail(\"boom-marker\")\nprint(\"never\")",
            "this is not starlark (",
            "show(nothing.here())",
            "def spin():\n    for i in range(100000000000):\n        pass\nprint(\"started\")\nspin()",
            "print(\"x\" * 200000)",
        ])),
        "[limits]\nscript_time = \"2s\"\nscript_output = \"1KiB\"\n",
        "",
    );
    let steps = world.steps("break", 5);
    assert_eq!(steps[0].code(0), "bad_argument");
    assert!(steps[0].text.contains("boom-marker"), "{}", steps[0].text);
    assert!(!steps[0].text.contains("never"), "{}", steps[0].text);
    assert!(steps[1].printed.is_empty() || !steps[1].text.is_empty());
    assert!(
        steps[1].text.len() > "output:\nactions:\n".len(),
        "{}",
        steps[1].text
    );
    assert!(steps[2].text.contains("nothing"), "{}", steps[2].text);
    assert!(steps[3].text.contains("started"), "{}", steps[3].text);
    assert!(
        steps[4].text.len() < 4096,
        "output is cut at its limit: {}",
        steps[4].text.len()
    );
    world.agent.wait_log("\"message\":\"a wake ended\"", 1);
}

#[test]
fn an_attached_image_goes_into_the_next_input() {
    let mut world = World::start(
        scripted(some(&[
            r#"
show(file.write("tmp/dot.png", "not really a png"))
show(llm.attach("tmp/dot.png"))
show(llm.attach("tmp/missing.png"))
show(llm.attach("../agent.db"))
"#,
            "print(\"after\")",
        ])),
        "",
        "",
    );
    let steps = world.steps("look", 2);
    let step = &steps[0];
    assert_eq!(step.value(1)["path"], "tmp/dot.png");
    assert_eq!(step.code(2), "not_found");
    assert_eq!(step.code(3), "path_not_allowed");
    assert!(
        !step.actions_of("llm.attach").is_empty(),
        "{:?}",
        step.actions
    );
    let request = &world.llm.requests()[1];
    let images: Vec<&Value> = request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["content"].is_array())
        .flat_map(|message| message["content"].as_array().unwrap())
        .filter(|part| part["type"] == "image_url")
        .collect();
    assert_eq!(images.len(), 1, "{request}");
    let url = images[0]["image_url"]["url"].as_str().unwrap();
    assert!(url.starts_with("data:image/png;base64,"), "{url}");
    // Only the next input carries it.
    let later = request_text(&world.llm.requests()[2]);
    assert_eq!(later.matches("data:image/png;base64,").count(), 1);
}

/// Messages the owner said before the one that woke the agent, with a rule waking only on "go".
fn search_world(plain: bool, scripts: &[&str]) -> (World, TempDir) {
    let rules = TempDir::new().unwrap();
    let mut world = World::start(
        scripted(some(scripts)),
        &format!("[search]\nplain = {plain}\n"),
        &wake_on(rules.path(), "go", ""),
    );
    for said in [
        "the blue whale sings at night",
        "lunch is at noon",
        "كتاب جديد",
        "Errors were found in the build",
    ] {
        world.owner.send(&format!("say {said}"));
        world.owner.expect("sent");
    }
    if plain {
        // The agent commits the message index in batches. Wait for the four fixtures to be visible.
        let path = world.agent.directory.join("index");
        let started = std::time::Instant::now();
        loop {
            let index = tantivy::Index::open_in_dir(&path).unwrap();
            let mut indexed = 0;
            for segment in index.load_metas().unwrap().segments {
                indexed += segment.num_docs();
            }
            if indexed >= 4 {
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "seed messages were not indexed"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    (world, rules)
}

fn texts(page: &Value) -> Vec<String> {
    let mut list = Vec::new();
    for message in page["message_list"].as_array().unwrap() {
        list.push(message["text"].as_str().unwrap_or_default().to_string());
    }
    list
}

#[test]
fn search_without_the_plain_index_scans_the_archive() {
    let (mut world, _rules) = search_world(
        false,
        &[r#"
show(search.message_list("WHALE"))
show(search.message_list("کتاب"))
show(search.message_list("noon", chat="$CHAT", sender="owner", limit=1))
show(search.message_list("noon", sender="helper_agent"))
show(search.message_list("nothing like this"))
show(search.message_list("whale", until="2000-01-01T00:00:00Z"))
"#],
    );
    let (mut sara, _) = world.sara();
    for n in 0..21 {
        sara.send(&format!("say noon from sara {n}"));
        sara.expect("sent");
    }
    world.agent.wait_log("\"kind\":\"message\"", 25);
    let step = &world.steps("go", 1)[0];
    assert_eq!(texts(step.value(0)), ["the blue whale sings at night"]);
    assert_eq!(
        texts(step.value(1)),
        ["كتاب جديد"],
        "Arabic forms match Persian ones"
    );
    assert_eq!(texts(step.value(2)), ["lunch is at noon"]);
    assert!(texts(step.value(3)).is_empty());
    assert!(texts(step.value(4)).is_empty());
    assert!(texts(step.value(5)).is_empty());
    let hit = &step.value(0)["message_list"][0];
    assert_eq!(hit["sender"]["username"], "owner");
    assert!(hit["seq"].as_u64().is_some());
}

#[test]
fn search_with_the_plain_index_ranks_and_hints() {
    let (mut world, _rules) = search_world(
        true,
        &[r#"
show(search.message_list("whale"))
show(search.message_list("error"))
show(search.message_list("(whale dolphin) sings"))
show(search.message_list("whale zzqqxx"))
show(search.message_list("کتاب"))
show(search.message_list("lunch -noon"))
show(search.message_list(""))
show(search.message_list("whale", chat="01JC0000000000000000000000"))
"#],
    );
    let step = &world.steps("go", 1)[0];
    let whale = step.value(0);
    assert_eq!(texts(whale), ["the blue whale sings at night"]);
    assert!(
        whale["message_list"][0]["snippet"]
            .as_str()
            .unwrap_or_default()
            .contains("whale"),
        "{whale}"
    );
    assert_eq!(
        texts(step.value(1)),
        ["Errors were found in the build"],
        "errors is found by error"
    );
    assert_eq!(texts(step.value(2)), ["the blue whale sings at night"]);
    let unknown = step.value(3);
    assert!(
        unknown["unknown_list"].to_string().contains("zzqqxx"),
        "{unknown}"
    );
    assert_eq!(texts(step.value(4)), ["كتاب جديد"]);
    assert!(texts(step.value(5)).is_empty());
    assert_eq!(step.code(6), "bad_argument");
    assert!(texts(step.value(7)).is_empty());
}

#[test]
fn groups_are_made_and_run_from_scripts() {
    let mut world = World::start_with(
        scripted(some(&[
            r#"
made = keryx.create("group", "Team", person_list=["owner"], bio="Our team")
show(made)
g = made[0]["chat"]
show(keryx.send(g, "hello team"))
show(keryx.member_list(g))
show(keryx.role(g, "owner", "moderator"))
show(keryx.topics(g, True))
t = keryx.topic_create(g, "Plans")
show(t)
show(keryx.send(g, "the plan", topic=t[0]["topic"]))
show(keryx.chat_options(g, reactions=False, slow_mode="10s"))
show(keryx.role(g, "owner", "king"))
show(keryx.create("room", "Nope"))
"#,
            r#"
g = [c for c in keryx.chat_list(type="group")[0]["chat_list"]][0]["id"]
show(keryx.chat(g))
show(keryx.member_list(g, role="moderator"))
show(keryx.history_list(g))
show(keryx.message_list(g, topic="general"))
"#,
        ])),
        "",
        "",
        "group.create",
    );
    let steps = world.steps("make a group", 2);
    let step = &steps[0];
    let group = step.value(0)["chat"].as_str().unwrap().to_string();
    assert!(step.value(1)["seq"].as_u64().is_some());
    let members = step.value(2);
    let mut roles = Vec::new();
    for member in members["member_list"].as_array().unwrap() {
        roles.push(format!(
            "{} {} {}",
            member["username"].as_str().unwrap(),
            member["role"].as_str().unwrap(),
            member["agent"]
        ));
    }
    roles.sort();
    assert_eq!(roles, ["helper_agent owner true", "owner member false"]);
    for n in [3, 4, 6, 7] {
        step.value(n);
    }
    let topic = step.value(5)["topic"].as_str().unwrap().to_string();
    assert_eq!(step.code(8), "bad_argument");
    assert_eq!(step.code(9), "bad_argument");
    assert!(
        !step.actions_of("keryx.create").is_empty(),
        "{:?}",
        step.actions
    );

    let step = &steps[1];
    let chat = step.value(0);
    assert_eq!(chat["id"], group);
    assert_eq!(chat["type"], "group");
    assert_eq!(chat["name"], "Team");
    assert_eq!(chat["bio"], "Our team");
    assert_eq!(chat["topics_on"], true);
    assert_eq!(chat["role"], "owner");
    assert_eq!(chat["options"]["reactions"], false);
    assert_eq!(chat["slow_mode"], "10s");
    assert!(
        chat["topic_list"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["id"] == topic && item["name"] == "Plans"),
        "{chat}"
    );
    let moderators = step.value(1);
    assert_eq!(moderators["member_list"].as_array().unwrap().len(), 1);
    assert_eq!(moderators["member_list"][0]["username"], "owner");
    assert!(
        !step.value(2)["history_list"].as_array().unwrap().is_empty(),
        "{}",
        step.text
    );
    let general = texts(step.value(3));
    assert!(general.contains(&"hello team".to_string()), "{general:?}");
    assert!(!general.contains(&"the plan".to_string()), "{general:?}");

    // The owner was put straight in and reads the agent's messages there.
    world.owner.pump_until(&format!("ready {group}"));
    world.owner.pump_until(&format!("msg {group} "));
}

#[test]
fn questions_and_suggestions_reach_the_owner() {
    let mut world = World::start(
        scripted(some(&[r#"
c = "$CHAT"
nodes = [
    {"id": "when", "text": "When?", "choice": {"multi": False, "options": [
        {"id": "am", "text": "Morning"}, {"id": "pm", "text": "Afternoon"}]}, "next": [{"to": "how"}]},
    {"id": "how", "text": "How many?", "number": {"min": 1, "max": 10}, "next": []},
]
show(keryx.question(c, "when", nodes, text="Planning lunch"))
show(keryx.question(c, "nowhere", nodes))
show(keryx.suggest(c, "Lunch", move=True, text="Keep lunch chats together"))
show(keryx.suggest(c, ""))
show(keryx.activity("Checking the menu", [c]))
show(keryx.activity("x" * 200, [c]))
"#])),
        "",
        "",
    );
    let chat = world.chat.clone();
    let step = &world.steps("ask me", 1)[0];
    assert!(step.value(0)["seq"].as_u64().is_some());
    assert_eq!(step.code(1), "bad_question", "core's own code");
    assert!(step.value(2)["seq"].as_u64().is_some());
    assert_eq!(step.code(3), "bad_suggestion", "core's own code");
    let activity = step.value(4);
    assert!(activity["seqs"][&chat].as_u64().is_some(), "{activity}");
    assert_eq!(step.code(5), "bad_activity", "core's own code");
    let mut kinds = Vec::new();
    for _ in 0..3 {
        kinds.push(world.owner.pump_until(&format!("msg {chat} ")));
    }
    assert!(kinds[0].contains("kind=Some(Question)"), "{kinds:?}");
    assert!(kinds[1].contains("kind=Some(Suggestion)"), "{kinds:?}");
    assert!(kinds[2].contains("kind=Some(Activity)"), "{kinds:?}");
}

#[test]
fn deferred_events_come_back_by_id() {
    let mut world = World::start(
        |number, request| {
            let events = common::events_of(request).unwrap_or(Value::Null);
            match number {
                0 => {
                    let id = events["group_list"][0]["event_list"][0]["id"]
                        .as_str()
                        .unwrap()
                        .to_string();
                    common::tool_call(
                        &format!(
                            "print(json.encode(inbox.defer([{id:?}], after=time.add(time.now()[0], \"2s\")[0])))\n\
                             print(json.encode(inbox.defer([\"01JC0000000000000000000000\"])))\n\
                             print(json.encode(inbox.defer([\"bad\"])))"
                        ),
                        json!([{"intention": "Later", "chat_list": []}]),
                    )
                }
                _ => common::text("done"),
            }
        },
        "",
        "",
    );
    world.owner.send("say remind me");
    world.owner.expect("sent");
    let requests = world.llm.wait_for(2);
    let step = step_of(&tool_output(&requests[1]));
    assert_eq!(step.value(0)["count"], 1);
    assert_eq!(step.value(1)["count"], 0);
    assert!(!step.printed[2][1].is_null(), "{}", step.text);
    assert!(
        !step.actions_of("inbox.defer").is_empty(),
        "{:?}",
        step.actions
    );
    let first = common::events_of(&requests[0]).unwrap();
    let id = first["group_list"][0]["event_list"][0]["id"].clone();
    let requests = world.llm.wait_for(3);
    let again = common::events_of(&requests[2]).unwrap();
    assert_eq!(again["group_list"][0]["event_list"][0]["id"], id, "{again}");
}

#[test]
fn a_skill_without_frontmatter_is_skipped() {
    let skills = skills_directory();
    fs::write(
        skills.path().join("broken.md"),
        "Nothing marks this as a skill.\n",
    )
    .unwrap();
    let mut world = World::start(
        scripted(some(&["show(skills.skill_list())"])),
        &format!("[skills]\ndirectory = {:?}\n", skills.path()),
        "",
    );
    let step = &world.steps("skills", 1)[0];
    assert_eq!(step.value(0)["skill_list"].as_array().unwrap().len(), 1);
    let system = system_of(&world.llm.requests()[0]);
    assert!(!system.contains("broken"), "{system}");
    assert!(
        world.agent.log().contains("broken.md"),
        "a skipped skill is logged"
    );
    let checked = std::process::Command::new(env!("CARGO_BIN_EXE_keryx-agent"))
        .arg("--config")
        .arg(world.home.path().join("agent.toml"))
        .arg("check")
        .output()
        .unwrap();
    assert_eq!(checked.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&checked.stderr).contains("broken.md"));
}
