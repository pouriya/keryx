//! The client fed by hand: replies, frames, closes and the timer, including what a real server never
//! sends. The server's tests drive the same client against a real `keryx serve`.

use std::collections::{BTreeMap, HashMap, VecDeque};

use keryx_core::client::{
    Auth, Client, Command, Draft, Event, FileSpec, Input, Output, Platform, Pointer, Problem,
    Secret, ShownCallEvent, Thumbnail,
};
use keryx_core::file::{self, FileInfo};
use keryx_core::mls::{AccountKey, Device, sign_revocation, stamp_account_key};
use keryx_core::protocol::{
    AccountKeysBody, Bytes, CallBody, CallEvent, CallEventKind, CaptchaAnswer, ChatItem, ChatMode,
    ChatType, ClientFrame, ContentKind, DraftItem, Kind, MarkItem, MessageFrame, SendFrame,
    ServerFrame, SyncPosition, WrapEntry,
};
use keryx_core::seed::derive;
use keryx_core::ulid::Ulid;
use serde_json::{Value, json};

const WORDS: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const NOW: i64 = 1_800_000_000_000;

fn platform() -> Platform {
    Platform {
        platform: 1,
        os: "android".to_string(),
        os_version: "15".to_string(),
        web_platform: Some("chrome".to_string()),
        web_platform_version: Some("140".to_string()),
    }
}

fn id() -> Ulid {
    Ulid::new().unwrap()
}

fn command(client: &mut Client, command: Command) -> Vec<Output> {
    client.step(Input::Command { command }, NOW)
}

fn events(outputs: &[Output]) -> Vec<Event> {
    let mut list = Vec::new();
    for output in outputs {
        if let Output::Event { event } = output {
            list.push(event.clone());
        }
    }
    list
}

/// The one request among these: its id, method, path and body.
fn request(outputs: &[Output]) -> (u64, String, String, Value) {
    let mut found = None;
    for output in outputs {
        if let Output::Request {
            id,
            method,
            path,
            body,
            ..
        } = output
        {
            assert!(found.is_none(), "More than one request: {outputs:?}");
            let body = match body {
                Some(body) => serde_json::from_str(&body.0).unwrap(),
                None => Value::Null,
            };
            found = Some((*id, method.clone(), path.clone(), body));
        }
    }
    found.expect("a request")
}

fn requests(outputs: &[Output]) -> Vec<(u64, String)> {
    let mut list = Vec::new();
    for output in outputs {
        if let Output::Request { id, path, .. } = output {
            list.push((*id, path.clone()));
        }
    }
    list
}

fn ok(client: &mut Client, id: u64, data: Value) -> Vec<Output> {
    client.step(
        Input::Reply {
            id,
            status: 200,
            body: json!({"ok": true, "data": data}).to_string(),
        },
        NOW,
    )
}

fn frames(outputs: &[Output]) -> Vec<ClientFrame> {
    let mut list = Vec::new();
    for output in outputs {
        if let Output::Send { text } = output {
            list.push(serde_json::from_str(&text.0).unwrap());
        }
    }
    list
}

fn frame(client: &mut Client, frame: &ServerFrame, now: i64) -> Vec<Output> {
    client.step(
        Input::Frame {
            text: serde_json::to_string(frame).unwrap(),
        },
        now,
    )
}

/// Logged in, with new words set and a device registered, and connected.
fn ready() -> (Client, Ulid, Ulid) {
    let mut client = Client::new(Auth::Token, platform());
    let account = id();
    let device = id();
    let out = command(
        &mut client,
        Command::LogIn {
            username: "sara_x".to_string(),
            password: Secret("a long enough password".to_string()),
            captcha: None,
        },
    );
    let (asked, method, path, body) = request(&out);
    assert_eq!(
        (method.as_str(), path.as_str()),
        ("POST", "/api/v1/login/token")
    );
    assert_eq!(body["username"], "sara_x");
    assert!(body.get("device").is_none());
    let out = ok(
        &mut client,
        asked,
        json!({"account": account, "device": null, "token": "t"}),
    );
    assert!(matches!(
        events(&out)[0],
        Event::SignedIn { device: None, .. }
    ));
    assert_eq!(
        events(&out)[1],
        Event::Token {
            token: Some(Secret("t".to_string()))
        }
    );
    let (asked, _, path, _) = request(&out);
    assert_eq!(path, format!("/api/v1/accounts/{account}"));
    let out = ok(
        &mut client,
        asked,
        json!({"id": account, "username": "sara_x"}),
    );
    assert!(events(&out).contains(&Event::NeedWords { restore: false }));

    let out = command(
        &mut client,
        Command::SetWords {
            words: Secret(WORDS.to_string()),
        },
    );
    let (asked, method, path, body) = request(&out);
    assert_eq!(
        (method.as_str(), path.as_str()),
        ("PUT", "/api/v1/account/keys")
    );
    assert!(body["account_stamp"].is_string());
    let out = ok(&mut client, asked, json!({"self_chat": id()}));
    let (asked, _, path, _) = request(&out);
    assert_eq!(path, "/api/v1/devices/id");
    let out = ok(&mut client, asked, json!({"device": device}));
    let (asked, _, path, body) = request(&out);
    assert_eq!(path, "/api/v1/devices");
    assert_eq!(body["platform"], 1);
    assert_eq!(body["web_platform"], "chrome");
    let out = ok(&mut client, asked, json!({"device": device}));
    let mut connected = false;
    let mut saved = false;
    for output in &out {
        match output {
            Output::Connect { bearer } => {
                assert_eq!(bearer.as_ref().unwrap().0, "t");
                connected = true;
            }
            Output::Save { .. } => saved = true,
            _ => {}
        }
    }
    assert!(connected && saved, "{out:?}");
    assert_eq!(client.device().unwrap().record().device, device);
    (client, account, device)
}

#[test]
fn signing_up_registers_then_logs_in_and_new_words_set_up_a_device() {
    let mut client = Client::new(Auth::Cookie, platform());
    let out = command(
        &mut client,
        Command::SignUp {
            username: "sara_x".to_string(),
            password: Secret("a long enough password".to_string()),
            display_name: Some("Sara".to_string()),
            captcha: None,
        },
    );
    let (asked, _, path, body) = request(&out);
    assert_eq!(path, "/api/v1/register");
    assert_eq!(body["display_name"], "Sara");
    let out = ok(&mut client, asked, json!({"account": id()}));
    let (_, _, path, _) = request(&out);
    assert_eq!(
        path, "/api/v1/login",
        "a browser's login sets only the cookie"
    );
    for output in &out {
        if let Output::Request { bearer, .. } = output {
            assert!(bearer.is_none());
        }
    }

    let mut setup = Client::new(Auth::Cookie, platform());
    let out = command(
        &mut setup,
        Command::Setup {
            token: Secret("from-the-link".to_string()),
            password: Secret("a long enough password".to_string()),
            captcha: None,
        },
    );
    let (asked, _, path, body) = request(&out);
    assert_eq!(path, "/api/v1/setup");
    assert_eq!(body["token"], "from-the-link");
    let out = ok(
        &mut setup,
        asked,
        json!({"account": id(), "username": "sara_x"}),
    );
    assert_eq!(request(&out).2, "/api/v1/login");

    ready();
}

#[test]
fn secrets_never_show_in_debug_output() {
    let shown = format!("{:?}", Secret("hunter2 hunter2".to_string()));
    assert!(!shown.contains("hunter2"));
    let shown = format!(
        "{:?}",
        Command::LogIn {
            username: "sara_x".to_string(),
            password: Secret("hunter2 hunter2".to_string()),
            captcha: None,
        }
    );
    assert!(!shown.contains("hunter2"));
}

#[test]
fn words_are_generated_for_twelve_or_twenty_four_and_refused_otherwise() {
    let mut client = Client::new(Auth::Token, platform());
    for count in [12, 24] {
        let out = command(&mut client, Command::GenerateWords { count });
        let Event::Words { words } = &events(&out)[0] else {
            panic!("{out:?}");
        };
        assert_eq!(words.0.split(' ').count(), count);
    }
    let out = command(&mut client, Command::GenerateWords { count: 13 });
    assert!(matches!(&events(&out)[0], Event::Error { code, .. } if code == "bad_words"));
    let out = command(
        &mut client,
        Command::SetWords {
            words: Secret(WORDS.to_string()),
        },
    );
    assert!(matches!(&events(&out)[0], Event::Error { code, .. } if code == "not_logged_in"));
}

#[test]
fn restoring_needs_this_accounts_own_words() {
    let mut client = Client::new(Auth::Token, platform());
    let account = id();
    let out = command(
        &mut client,
        Command::LogIn {
            username: "sara_x".to_string(),
            password: Secret("a long enough password".to_string()),
            captcha: None,
        },
    );
    let out = ok(
        &mut client,
        request(&out).0,
        json!({"account": account, "device": null, "token": "t"}),
    );
    let keys = keryx_core::seed::derive(WORDS).unwrap();
    let master = keryx_core::protocol::Bytes(keys.master_public().to_vec());
    let out = ok(
        &mut client,
        request(&out).0,
        json!({"id": account, "username": "sara_x", "master_public": master}),
    );
    assert!(events(&out).contains(&Event::NeedWords { restore: true }));
    let other = "legal winner thank year wave sausage worth useful legal winner thank yellow";
    let out = command(
        &mut client,
        Command::Restore {
            words: Secret(other.to_string()),
        },
    );
    assert!(matches!(&events(&out)[0], Event::Error { code, .. } if code == "wrong_words"));
    let out = command(
        &mut client,
        Command::Restore {
            words: Secret("not words".to_string()),
        },
    );
    assert!(matches!(&events(&out)[0], Event::Error { code, .. } if code == "bad_words"));
    let out = command(
        &mut client,
        Command::Restore {
            words: Secret(WORDS.to_string()),
        },
    );
    assert_eq!(request(&out).2, "/api/v1/devices/id");
}

#[test]
fn connecting_syncs_and_tops_key_packages_up_to_ten() {
    let (mut client, _, _) = ready();
    let out = client.step(Input::Connected, NOW);
    assert!(events(&out).contains(&Event::Online));
    assert!(matches!(frames(&out)[0], ClientFrame::Sync { .. }));
    let (asked, _, path, body) = request(&out);
    assert_eq!(path, "/api/v1/key_packages");
    assert_eq!(body["body_list"], json!([]));
    let out = ok(&mut client, asked, json!({"count": 7}));
    let (asked, _, _, body) = request(&out);
    assert_eq!(body["body_list"].as_array().unwrap().len(), 3);
    assert!(ok(&mut client, asked, json!({"count": 10})).is_empty());

    let out = client.step(Input::Connected, NOW);
    let mut asked = 0;
    for (id, path) in requests(&out) {
        if path == "/api/v1/key_packages" {
            asked = id;
        }
    }
    let out = ok(&mut client, asked, json!({"count": 10}));
    assert!(requests(&out).is_empty(), "nothing to top up");
}

#[test]
fn a_quiet_connection_is_pinged_then_given_up_and_retried_after_a_growing_wait() {
    let (mut client, _, _) = ready();
    client.step(Input::Connected, NOW);
    let features = ServerFrame::Features {
        ping_after: 5,
        ping_every: 15,
        ping_timeout: 5,
    };
    let out = frame(&mut client, &features, NOW);
    assert!(
        out.iter()
            .any(|output| matches!(output, Output::WakeAt { at } if *at == NOW + 15_000))
    );

    // Too early does nothing, then a ping goes, then silence gives the connection up.
    assert!(frames(&client.step(Input::Timer, NOW + 1_000)).is_empty());
    let out = client.step(Input::Timer, NOW + 15_000);
    assert!(matches!(frames(&out)[0], ClientFrame::Ping { .. }));
    assert!(
        out.iter()
            .any(|output| matches!(output, Output::WakeAt { at } if *at == NOW + 20_000))
    );
    assert!(
        client
            .step(Input::Timer, NOW + 16_000)
            .iter()
            .all(|output| !matches!(output, Output::Disconnect))
    );
    let out = client.step(Input::Timer, NOW + 20_000);
    assert!(matches!(out[0], Output::Disconnect));
    assert!(events(&out).contains(&Event::Offline));
    let Some(Output::WakeAt { at }) = out.last() else {
        panic!("{out:?}");
    };
    let first_wait = at - (NOW + 20_000);
    assert!((1_000..1_300).contains(&first_wait), "{first_wait}");

    // The retry connects, and failing again waits longer.
    let out = client.step(Input::Timer, *at);
    assert!(matches!(out[0], Output::Connect { .. }));
    let out = client.step(
        Input::Closed {
            code: 1006,
            reason: String::new(),
        },
        *at,
    );
    let Some(Output::WakeAt { at: later }) = out.last() else {
        panic!("{out:?}");
    };
    assert!(later - at >= 2_000);

    // "The network is back" retries at once, and a working connection resets the wait.
    let out = command(&mut client, Command::NetworkBack);
    assert!(matches!(out[0], Output::Connect { .. }));
    client.step(Input::Connected, NOW);
    let out = client.step(
        Input::Closed {
            code: 1006,
            reason: String::new(),
        },
        NOW,
    );
    let Some(Output::WakeAt { at }) = out.last() else {
        panic!("{out:?}");
    };
    assert!(at - NOW < 1_300);
}

#[test]
fn a_refused_socket_stops_retrying_and_an_ended_device_wipes_itself() {
    let (mut client, _, _) = ready();
    client.step(Input::Connected, NOW);
    let out = client.step(
        Input::Closed {
            code: 4403,
            reason: "not_logged_in".to_string(),
        },
        NOW,
    );
    assert!(events(&out).contains(&Event::NeedLogin));
    assert!(
        !out.iter()
            .any(|output| matches!(output, Output::WakeAt { .. }))
    );

    let (mut client, _, _) = ready();
    let out = client.step(
        Input::Closed {
            code: 4403,
            reason: "device_limit".to_string(),
        },
        NOW,
    );
    assert!(matches!(events(&out)[0], Event::DeviceLimit { .. }));

    let (mut client, _, _) = ready();
    client.step(Input::Connected, NOW);
    let out = client.step(
        Input::Closed {
            code: 4403,
            reason: "device_ended".to_string(),
        },
        NOW,
    );
    assert!(out.iter().any(|output| matches!(output, Output::Wipe)));
    assert!(events(&out).contains(&Event::LoggedOut));
    assert!(client.device().is_none());
}

#[test]
fn a_key_package_count_refused_for_no_device_returns_to_login_without_an_error() {
    let (mut client, _, _) = ready();
    let out = client.step(Input::Connected, NOW);
    let (asked, _, path, _) = request(&out);
    assert_eq!(path, "/api/v1/key_packages");
    let out = client.step(
        Input::Reply {
            id: asked,
            status: 403,
            body:
                json!({"ok": false, "code": "no_device", "message": "This session has no device"})
                    .to_string(),
        },
        NOW,
    );
    assert!(events(&out).contains(&Event::NeedLogin));
    assert!(
        events(&out)
            .iter()
            .all(|event| !matches!(event, Event::Error { .. }))
    );
}

#[test]
fn what_cannot_be_read_or_done_is_an_error_event() {
    let (mut client, _, _) = ready();
    client.step(Input::Connected, NOW);
    let out = client.step(
        Input::Frame {
            text: "not a frame".to_string(),
        },
        NOW,
    );
    assert!(matches!(&events(&out)[0], Event::Error { code, .. } if code == "bad_frame"));

    let out = command(&mut client, Command::Devices);
    let (asked, _, path, _) = request(&out);
    assert_eq!(path, "/api/v1/devices");
    let out = client.step(
        Input::Reply {
            id: asked,
            status: 403,
            body: json!({"ok": false, "code": "no_device", "message": "No"}).to_string(),
        },
        NOW,
    );
    assert!(matches!(&events(&out)[0], Event::Error { code, .. } if code == "no_device"));

    let out = command(&mut client, Command::Devices);
    let out = client.step(
        Input::Reply {
            id: request(&out).0,
            status: 502,
            body: "bad gateway".to_string(),
        },
        NOW,
    );
    assert!(matches!(&events(&out)[0], Event::Error { code, .. } if code == "http_502"));

    let out = command(&mut client, Command::Devices);
    let out = client.step(
        Input::Reply {
            id: request(&out).0,
            status: 200,
            body: "{}".to_string(),
        },
        NOW,
    );
    assert!(matches!(&events(&out)[0], Event::Error { code, .. } if code == "bad_reply"));

    // A reply nobody asked for is ignored.
    assert!(
        client
            .step(
                Input::Reply {
                    id: 999,
                    status: 200,
                    body: String::new()
                },
                NOW
            )
            .is_empty()
    );

    // Sending into a chat this device is not in fails, and so does what waits on it.
    let chat = id();
    let out = command(
        &mut client,
        Command::SendText {
            chat,
            text: Secret("hello".to_string()),
            reply: None,
        },
    );
    let mut local = None;
    let mut failed = Vec::new();
    for event in events(&out) {
        match event {
            Event::Pending { local: pending, .. } => local = Some(pending),
            Event::Failed { code, local, .. } => failed.push((local, code)),
            _ => {}
        }
    }
    assert_eq!(failed, vec![(local.unwrap(), "not_joined".to_string())]);
}

#[test]
fn the_directory_search_is_escaped_and_answered() {
    let (mut client, _, _) = ready();
    let out = command(
        &mut client,
        Command::Search {
            text: "سا ra&x".to_string(),
        },
    );
    let (asked, _, path, _) = request(&out);
    assert_eq!(path, "/api/v1/accounts?search=%D8%B3%D8%A7%20ra%26x");
    let found = id();
    let out = ok(
        &mut client,
        asked,
        json!({"account_list": [{"id": found, "username": "sara_x"}]}),
    );
    let Event::SearchResults { account_list } = &events(&out)[0] else {
        panic!("{out:?}");
    };
    assert_eq!(account_list[0].id, found);
}

#[test]
fn the_login_screen_reads_what_the_server_allows() {
    let mut client = Client::new(Auth::Cookie, platform());
    let out = client.step(
        Input::Loaded {
            first: None,
            second: None,
            resume: true,
            token: None,
        },
        NOW,
    );
    assert_eq!(events(&out), vec![Event::NeedLogin]);
    let (asked, method, path, body) = request(&out);
    assert_eq!(method, "GET");
    assert_eq!(path, "/api/v1/public/features");
    assert!(body.is_null());
    let out = ok(
        &mut client,
        asked,
        json!({"registration": false, "captcha": true, "name": "North"}),
    );
    assert_eq!(
        events(&out),
        vec![Event::Features {
            registration: false,
            captcha: true,
            name: Some("North".to_string()),
        }]
    );
}

#[test]
fn a_captcha_is_fetched_and_signup_then_waits_for_another() {
    let mut client = Client::new(Auth::Cookie, platform());
    let out = command(&mut client, Command::Captcha);
    let (asked, method, path, _) = request(&out);
    assert_eq!(method, "GET");
    assert_eq!(path, "/api/v1/captcha");
    let captcha_id = id();
    let out = ok(
        &mut client,
        asked,
        json!({"id": captcha_id, "image": "data:image/png;base64,YQ=="}),
    );
    assert_eq!(
        events(&out),
        vec![Event::Captcha {
            id: captcha_id,
            image: "data:image/png;base64,YQ==".to_string(),
        }]
    );

    let out = command(
        &mut client,
        Command::SignUp {
            username: "sara_x".to_string(),
            password: Secret("a long enough password".to_string()),
            display_name: None,
            captcha: Some(CaptchaAnswer {
                id: captcha_id,
                answer: "THE CODE".to_string(),
            }),
        },
    );
    let (asked, _, path, body) = request(&out);
    assert_eq!(path, "/api/v1/register");
    assert_eq!(body["captcha"]["answer"], "THE CODE");
    let out = ok(&mut client, asked, json!({"account": id()}));
    assert_eq!(
        events(&out),
        vec![Event::Registered {
            username: "sara_x".to_string(),
        }]
    );
    assert!(requests(&out).is_empty());
}

#[test]
fn loading_saved_state_comes_back_or_asks_to_log_in() {
    let mut client = Client::new(Auth::Cookie, platform());
    let out = client.step(
        Input::Loaded {
            first: None,
            second: None,
            resume: true,
            token: None,
        },
        NOW,
    );
    assert_eq!(events(&out), vec![Event::NeedLogin]);
    let out = client.step(
        Input::Loaded {
            first: Some(vec![1, 2, 3]),
            second: None,
            resume: true,
            token: None,
        },
        NOW,
    );
    assert!(matches!(&events(&out)[0], Event::Error { code, .. } if code == "load_failed"));

    // What a device saved loads back, and a browser connects with its cookie at once.
    let (saver, _, _) = ready();
    let saved = saver
        .into_device()
        .unwrap()
        .export()
        .unwrap()
        .bytes
        .to_vec();
    let mut browser = Client::new(Auth::Cookie, platform());
    let out = browser.step(
        Input::Loaded {
            first: Some(saved.clone()),
            second: None,
            resume: true,
            token: None,
        },
        NOW,
    );
    assert!(
        out.iter()
            .any(|output| matches!(output, Output::Connect { bearer: None }))
    );
    let mut app = Client::new(Auth::Token, platform());
    let out = app.step(
        Input::Loaded {
            first: Some(saved),
            second: None,
            resume: true,
            token: None,
        },
        NOW,
    );
    assert_eq!(
        events(&out),
        vec![Event::NeedLogin],
        "with no token handed back, it logs in again"
    );

    // Logging in again proves the device with a signed challenge.
    let out = command(
        &mut app,
        Command::LogIn {
            username: "sara_x".to_string(),
            password: Secret("a long enough password".to_string()),
            captcha: None,
        },
    );
    let (asked, _, path, body) = request(&out);
    assert_eq!(path, "/api/v1/challenge");
    assert_eq!(body["purpose"], "device_login");
    let challenge = keryx_core::protocol::Bytes(vec![7; 32]);
    let out = ok(&mut app, asked, json!({"challenge": challenge}));
    let (_, _, path, body) = request(&out);
    assert_eq!(path, "/api/v1/login/token");
    assert_eq!(
        body["device"]["id"],
        json!(app.device().unwrap().record().device)
    );
    assert_eq!(body["device"]["signature"].as_str().unwrap().len(), 88);
}

#[test]
fn a_token_client_hands_out_its_token_takes_it_back_and_drops_it_when_the_session_goes() {
    let (client, _, _) = ready();
    let saved = client
        .into_device()
        .unwrap()
        .export()
        .unwrap()
        .bytes
        .to_vec();
    let mut app = Client::new(Auth::Token, platform());
    let out = app.step(
        Input::Loaded {
            first: Some(saved.clone()),
            second: None,
            resume: true,
            token: Some(Secret("kept".to_string())),
        },
        NOW,
    );
    assert!(
        out.iter().any(
            |output| matches!(output, Output::Connect { bearer: Some(bearer) } if bearer.0 == "kept")
        ),
        "{out:?}"
    );
    let out = app.step(
        Input::Closed {
            code: 4403,
            reason: "no_session".to_string(),
        },
        NOW,
    );
    assert_eq!(events(&out)[0], Event::Token { token: None });
    assert!(events(&out).contains(&Event::NeedLogin));

    let mut browser = Client::new(Auth::Cookie, platform());
    let out = browser.step(
        Input::Loaded {
            first: Some(saved),
            second: None,
            resume: true,
            token: Some(Secret("ignored".to_string())),
        },
        NOW,
    );
    assert!(
        out.iter()
            .any(|output| matches!(output, Output::Connect { bearer: None }))
    );
}

#[test]
fn loading_one_username_does_not_connect_until_that_login() {
    let (saver, _, _) = ready();
    let saved = saver
        .into_device()
        .unwrap()
        .export()
        .unwrap()
        .bytes
        .to_vec();
    let mut browser = Client::new(Auth::Cookie, platform());
    let out = browser.step(
        Input::Loaded {
            first: Some(saved),
            second: None,
            resume: false,
            token: None,
        },
        NOW,
    );
    assert!(
        out.iter()
            .all(|output| !matches!(output, Output::Connect { .. }))
    );
    let out = command(
        &mut browser,
        Command::LogIn {
            username: "sara_x".to_string(),
            password: Secret("a long enough password".to_string()),
            captcha: None,
        },
    );
    assert_eq!(request(&out).2, "/api/v1/challenge");
}

#[test]
fn a_logout_waits_for_the_server_and_a_terminate_signs_with_the_words() {
    let (mut client, account, device) = ready();
    let out = command(&mut client, Command::LogOut);
    let (asked, _, path, body) = request(&out);
    assert_eq!(path, format!("/api/v1/devices/{device}/logout"));
    let signature: keryx_core::protocol::Bytes =
        serde_json::from_value(body["signature"].clone()).unwrap();
    let public = client.device().unwrap().record().device_public;
    keryx_core::mls::verify_logout(&public, &account, &device, &signature.0.try_into().unwrap())
        .unwrap();
    let out = client.step(
        Input::Reply {
            id: asked,
            status: 503,
            body: json!({"ok": false, "code": "shutting_down", "message": "Later"}).to_string(),
        },
        NOW,
    );
    assert!(
        !out.iter().any(|output| matches!(output, Output::Wipe)),
        "kept until confirmed"
    );
    assert!(client.device().is_some());
    let out = command(&mut client, Command::LogOut);
    let out = ok(&mut client, request(&out).0, json!({}));
    assert!(out.iter().any(|output| matches!(output, Output::Wipe)));
    assert!(events(&out).contains(&Event::Token { token: None }));
    assert!(events(&out).contains(&Event::LoggedOut));
    let (_, method, path, _) = request(&out);
    assert_eq!(
        (method.as_str(), path.as_str()),
        ("GET", "/api/v1/public/features")
    );
    let out = command(&mut client, Command::LogOut);
    assert!(matches!(&events(&out)[0], Event::Error { code, .. } if code == "no_device"));

    let other = id();
    let out = command(
        &mut client,
        Command::Terminate {
            device: other,
            words: Secret(WORDS.to_string()),
        },
    );
    let (asked, _, path, _) = request(&out);
    assert_eq!(path, format!("/api/v1/devices/{other}/terminate"));
    let out = ok(&mut client, asked, json!({}));
    assert_eq!(events(&out), vec![Event::Terminated { device: other }]);
    let out = command(
        &mut client,
        Command::Terminate {
            device: other,
            words: Secret("wrong".to_string()),
        },
    );
    assert!(matches!(&events(&out)[0], Event::Error { code, .. } if code == "bad_words"));
}

#[test]
fn seen_typing_and_pointers_go_out_as_frames() {
    let (mut client, _, _) = ready();
    client.step(Input::Connected, NOW);
    let chat = id();
    let out = command(&mut client, Command::Seen { chat, up_to_seq: 4 });
    assert!(matches!(
        frames(&out)[0],
        ClientFrame::Seen {
            up_to_seq: 4,
            client_seen_at: NOW,
            ..
        }
    ));
    let out = command(&mut client, Command::Typing { chat, on: true });
    assert_eq!(frames(&out), vec![ClientFrame::Typing { chat, on: true }]);
    let out = command(
        &mut client,
        Command::Delete {
            chat,
            target: Pointer::Seq(3),
            remove: Vec::new(),
        },
    );
    assert!(matches!(
        &events(&out)[0],
        Event::Pending {
            target: Some(Pointer::Seq(3)),
            ..
        }
    ));
}

// -------------------------------------------------------------------------------------------------
// The scripted server: a small in-memory stand-in for `keryx serve`, with phones running the real
// client against it, and anything dropped, lost, refused or failed on cue.
// -------------------------------------------------------------------------------------------------

const SECOND: i64 = 1_000;
const DAY: i64 = 86_400_000;
const OMID_WORDS: &str =
    "legal winner thank year wave sausage worth useful legal winner thank yellow";
const MINA_WORDS: &str =
    "letter advice cage absurd amount doctor acoustic avoid letter advice cage above";

struct HubAccount {
    id: Ulid,
    username: String,
    keys: Option<AccountKeysBody>,
}

struct HubDevice {
    id: Ulid,
    account: Ulid,
    public: Vec<u8>,
    stamp: Vec<u8>,
    statement: Option<Vec<u8>>,
}

struct HubChat {
    id: Ulid,
    chat_type: ChatType,
    member_list: Vec<Ulid>,
    message_list: Vec<MessageFrame>,
    epoch: Option<u64>,
    group_info: Option<(u64, Vec<u8>)>,
    wrap_list: Vec<WrapEntry>,
    position_list: Vec<MarkItem>,
    draft_list: Vec<(Ulid, DraftItem)>,
}

struct HubFile {
    id: Ulid,
    chat: Ulid,
    account: Ulid,
    chunks: u64,
    held: Vec<Vec<u8>>,
    used: bool,
    removed: bool,
}

/// What happens to the next frame of a kind a phone sends.
#[derive(Clone, Copy)]
enum Fault {
    /// It never reaches the server.
    Drop,
    /// The server handles it, and its answer is lost.
    Lose,
    /// The server refuses it with this code, without handling it.
    Refuse(&'static str),
}

/// What happens to the next request for a method and path.
enum RestFault {
    /// This status and code, without handling it. Status 0 is nothing answering.
    Answer(u16, &'static str),
    /// The server handles it, and nothing answers.
    Lose,
    /// A 200 whose body cannot be read, without handling it.
    Unreadable,
}

struct Phone {
    client: Client,
    /// The session, as the server knows it.
    account: Option<Ulid>,
    device: Option<Ulid>,
    up: bool,
    connecting: bool,
    network: bool,
    /// Replaced by a reload, and left alone.
    gone: bool,
    requests: VecDeque<(u64, String, String, Option<String>)>,
    sent: VecDeque<ClientFrame>,
    inbox: VecDeque<ServerFrame>,
    saves: VecDeque<(u64, u8, Vec<u8>)>,
    slots: [Option<Vec<u8>>; 2],
    events: Vec<Event>,
    wake: Option<i64>,
    rest_faults: Vec<(String, RestFault)>,
    frame_faults: Vec<(&'static str, Fault)>,
    /// The next frame of each of these kinds pushed to this phone is lost on the way.
    lost_list: Vec<&'static str>,
    failing_saves: usize,
    /// Saves are left unanswered, as if the app stopped mid-write.
    stall_saves: bool,
    /// Every request, as "METHOD path", in order.
    asked_list: Vec<String>,
    /// Every frame that reached the server, by kind, in order.
    frame_list: Vec<&'static str>,
    connects: usize,
    wiped: bool,
    /// The files the platform holds, by handle: what it uploads and what it downloads into.
    held: HashMap<u64, Vec<u8>>,
    reads: VecDeque<(u64, u64, u64, u64)>,
    puts: VecDeque<(u64, String, Vec<u8>)>,
    gets: VecDeque<(u64, String)>,
    /// The next reads come back one byte short.
    short_reads: usize,
    /// What the client handed its call code: joins, keys, leaves and text.
    call_outputs: Vec<Output>,
    /// The archive the platform keeps on disk, which a reload keeps.
    archive: BTreeMap<(Ulid, u64), MessageFrame>,
    archive_calls: BTreeMap<(Ulid, u64, u64), CallEvent>,
    /// `SaveRows` and `ReadRows` waiting for the platform.
    archive_work: VecDeque<Output>,
    /// The next archive writes fail.
    failing_rows: usize,
}

struct World {
    now: i64,
    phones: Vec<Phone>,
    accounts: Vec<HubAccount>,
    devices: Vec<HubDevice>,
    issued: Vec<(Ulid, Ulid)>,
    packages: Vec<(Ulid, Vec<u8>)>,
    chats: Vec<HubChat>,
    welcomes: Vec<(Ulid, Ulid, Vec<u8>)>,
    max_devices: Option<usize>,
    /// Every Welcome is stored as garbage.
    garble_welcomes: bool,
    /// Every wrap a commit carries for this account is stored as garbage.
    garble_wraps_for: Option<Ulid>,
    /// The chat list leaves out each chat's epoch, as the field is optional.
    hide_epoch: bool,
    files: Vec<HubFile>,
    /// Every chunk handed out has its first byte changed.
    garble_chunks: bool,
    /// What happened inside each call, by chat and the call's sequence id.
    call_events: HashMap<(Ulid, u64), Vec<CallEvent>>,
}

fn reply_ok(data: Value) -> (u16, String) {
    (200, json!({"ok": true, "data": data}).to_string())
}

fn reply_no(status: u16, code: &str) -> (u16, String) {
    (
        status,
        json!({"ok": false, "code": code, "message": "Refused"}).to_string(),
    )
}

fn refused(reference: Ulid, code: &str) -> ServerFrame {
    ServerFrame::Refused {
        reference,
        code: code.to_string(),
        message: "Refused".to_string(),
        call_seq: None,
    }
}

fn parse(text: &str) -> Ulid {
    Ulid::parse(text).unwrap()
}

fn kind_of(frame: &ClientFrame) -> &'static str {
    match frame {
        ClientFrame::Send(send) if send.kind == Kind::Commit => "commit",
        ClientFrame::Send(_) => "send",
        ClientFrame::Seen { .. } => "seen",
        ClientFrame::Received { .. } => "received",
        ClientFrame::Typing { .. } => "typing",
        ClientFrame::Sync { .. } => "sync",
        ClientFrame::Times { .. } => "times",
        ClientFrame::Draft { .. } => "draft",
        ClientFrame::GroupInfo { .. } => "group_info",
        ClientFrame::Ping { .. } => "ping",
        ClientFrame::Wraps { .. } => "wraps",
        ClientFrame::CallEvent { .. } => "call_event",
        ClientFrame::ProfileKeys { .. } => "profile_keys",
    }
}

fn reference_of(frame: &ClientFrame) -> Ulid {
    match frame {
        ClientFrame::Send(send) => send.reference,
        ClientFrame::Seen { reference, .. }
        | ClientFrame::Received { reference, .. }
        | ClientFrame::Sync { reference, .. }
        | ClientFrame::Times { reference, .. }
        | ClientFrame::Draft { reference, .. }
        | ClientFrame::GroupInfo { reference, .. }
        | ClientFrame::Ping { reference }
        | ClientFrame::Wraps { reference, .. }
        | ClientFrame::CallEvent { reference, .. }
        | ClientFrame::ProfileKeys { reference, .. } => *reference,
        ClientFrame::Typing { .. } => Ulid::from_bytes([0; 16]),
    }
}

fn wiped(chat: Ulid, seq: u64, at: i64) -> MessageFrame {
    MessageFrame {
        chat,
        seq,
        device: None,
        kind: None,
        content_kind: None,
        encrypted: None,
        epoch: None,
        pointer_seq: None,
        client_sent_at: None,
        server_saved_at: Some(at),
        body: None,
        signature: None,
    }
}

/// How many rows the fake server puts in one `sync` page, small so tests reach a second page.
const SYNC_PAGE: usize = 20;

fn message_position(message: &MessageFrame) -> SyncPosition {
    SyncPosition {
        at: message.server_saved_at.unwrap_or(0),
        chat: message.chat,
        seq: message.seq,
        n: 0,
    }
}

fn call_position(event: &CallEvent) -> SyncPosition {
    SyncPosition {
        at: event.server_at,
        chat: event.chat,
        seq: event.call_seq,
        n: event.n,
    }
}

impl Phone {
    /// What the platform does with an archive output, and its answer.
    fn archive_do(&mut self, work: Output) -> Input {
        match work {
            Output::SaveRows {
                id,
                message_list,
                call_list,
            } => {
                if self.failing_rows > 0 {
                    self.failing_rows -= 1;
                    return Input::RowsSaveFailed { id };
                }
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
                Input::RowsSaved { id }
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
                        *c == chat && message_list.iter().any(|message| message.seq == *call_seq)
                    })
                    .map(|(_, event)| event.clone())
                    .collect();
                Input::RowsRead {
                    id,
                    message_list,
                    call_list,
                }
            }
            _ => unreachable!("not archive work"),
        }
    }
}

impl World {
    fn new() -> Self {
        Self {
            now: NOW,
            phones: Vec::new(),
            accounts: Vec::new(),
            devices: Vec::new(),
            issued: Vec::new(),
            packages: Vec::new(),
            chats: Vec::new(),
            welcomes: Vec::new(),
            max_devices: None,
            garble_welcomes: false,
            garble_wraps_for: None,
            hide_epoch: false,
            files: Vec::new(),
            garble_chunks: false,
            call_events: HashMap::new(),
        }
    }

    fn phone(&mut self, auth: Auth) -> usize {
        self.phones.push(Phone {
            client: Client::new(auth, platform()),
            account: None,
            device: None,
            up: false,
            connecting: false,
            network: true,
            gone: false,
            requests: VecDeque::new(),
            sent: VecDeque::new(),
            inbox: VecDeque::new(),
            saves: VecDeque::new(),
            slots: [None, None],
            events: Vec::new(),
            wake: None,
            rest_faults: Vec::new(),
            frame_faults: Vec::new(),
            lost_list: Vec::new(),
            failing_saves: 0,
            stall_saves: false,
            asked_list: Vec::new(),
            frame_list: Vec::new(),
            connects: 0,
            wiped: false,
            held: HashMap::new(),
            reads: VecDeque::new(),
            puts: VecDeque::new(),
            gets: VecDeque::new(),
            short_reads: 0,
            call_outputs: Vec::new(),
            archive: BTreeMap::new(),
            archive_calls: BTreeMap::new(),
            archive_work: VecDeque::new(),
            failing_rows: 0,
        });
        self.phones.len() - 1
    }

    fn input(&mut self, p: usize, input: Input) {
        let out = self.phones[p].client.step(input, self.now);
        let phone = &mut self.phones[p];
        for output in out {
            match output {
                Output::Send { text } => {
                    assert!(phone.up, "a frame while the socket is down");
                    phone.sent.push_back(serde_json::from_str(&text.0).unwrap());
                }
                Output::Request {
                    id,
                    method,
                    path,
                    body,
                    ..
                } => {
                    let body = body.map(|body| body.0.clone());
                    phone.requests.push_back((id, method, path, body));
                }
                Output::Connect { .. } => {
                    phone.connecting = true;
                    phone.connects += 1;
                }
                Output::Disconnect => {
                    phone.up = false;
                    phone.connecting = false;
                    phone.sent.clear();
                    phone.inbox.clear();
                }
                Output::Save { id, slot, bytes } => phone.saves.push_back((id, slot, bytes)),
                Output::Wipe => {
                    phone.slots = [None, None];
                    phone.wiped = true;
                }
                Output::WakeAt { at } => phone.wake = Some(at),
                Output::Event { event } => phone.events.push(event),
                Output::ReadChunk {
                    handle,
                    n,
                    offset,
                    length,
                } => phone.reads.push_back((handle, n, offset, length)),
                Output::PutChunk {
                    id, path, bytes, ..
                } => phone.puts.push_back((id, path, bytes)),
                Output::GetChunk { id, path, .. } => phone.gets.push_back((id, path)),
                Output::JoinCall { .. }
                | Output::CallKey { .. }
                | Output::LeaveCall { .. }
                | Output::CallText { .. } => phone.call_outputs.push(output),
                Output::SaveRows { .. } | Output::ReadRows { .. } => {
                    phone.archive_work.push_back(output)
                }
                Output::WriteChunk {
                    handle,
                    offset,
                    bytes,
                    ..
                } => {
                    let held = phone.held.entry(handle).or_default();
                    let start = usize::try_from(offset).unwrap();
                    if held.len() < start + bytes.len() {
                        held.resize(start + bytes.len(), 0);
                    }
                    held[start..start + bytes.len()].copy_from_slice(&bytes);
                }
            }
        }
    }

    fn command(&mut self, p: usize, command: Command) {
        self.input(p, Input::Command { command });
    }

    /// Runs every phone until nothing is left to do.
    fn settle(&mut self) {
        let mut rounds = 0;
        loop {
            let mut moved = false;
            for p in 0..self.phones.len() {
                if self.step_one(p) {
                    moved = true;
                }
            }
            if !moved {
                return;
            }
            rounds += 1;
            assert!(rounds < 20_000, "the phones never settle");
        }
    }

    /// One thing for this phone: a connection, a save, a request, a frame to the server or one
    /// from it.
    fn step_one(&mut self, p: usize) -> bool {
        if self.phones[p].gone {
            return false;
        }
        if self.phones[p].connecting {
            self.phones[p].connecting = false;
            if !self.phones[p].network {
                self.input(
                    p,
                    Input::Closed {
                        code: 1006,
                        reason: String::new(),
                    },
                );
                return true;
            }
            if let Some(reason) = self.refusal(p) {
                self.input(
                    p,
                    Input::Closed {
                        code: 4403,
                        reason: reason.to_string(),
                    },
                );
                return true;
            }
            self.phones[p].up = true;
            self.phones[p].inbox.push_back(ServerFrame::Features {
                ping_after: 5,
                ping_every: 3_000_000,
                ping_timeout: 5,
            });
            self.input(p, Input::Connected);
            self.hand_welcomes(p);
            return true;
        }
        if !self.phones[p].stall_saves
            && let Some((id, slot, bytes)) = self.phones[p].saves.pop_front()
        {
            let phone = &mut self.phones[p];
            if phone.failing_saves > 0 {
                phone.failing_saves -= 1;
                self.input(p, Input::SaveFailed { id });
            } else {
                phone.slots[usize::from(slot)] = Some(bytes);
                self.input(p, Input::Saved { id });
            }
            return true;
        }
        if let Some(work) = self.phones[p].archive_work.pop_front() {
            let input = self.phones[p].archive_do(work);
            self.input(p, input);
            return true;
        }
        if let Some((id, method, path, body)) = self.phones[p].requests.pop_front() {
            let (status, body) = self.answer(p, &method, &path, body.as_deref());
            self.input(p, Input::Reply { id, status, body });
            return true;
        }
        if let Some((handle, n, offset, length)) = self.phones[p].reads.pop_front() {
            let phone = &mut self.phones[p];
            let start = usize::try_from(offset).unwrap();
            let mut end = start + usize::try_from(length).unwrap();
            if phone.short_reads > 0 {
                phone.short_reads -= 1;
                end -= 1;
            }
            let bytes = phone.held[&handle][start..end].to_vec();
            self.input(p, Input::ChunkRead { handle, n, bytes });
            return true;
        }
        if let Some((id, path, bytes)) = self.phones[p].puts.pop_front() {
            let (status, body) = self.put_chunk(p, &path, bytes);
            self.input(p, Input::Reply { id, status, body });
            return true;
        }
        if let Some((id, path)) = self.phones[p].gets.pop_front() {
            let (status, bytes) = self.get_chunk(p, &path);
            self.input(p, Input::Fetched { id, status, bytes });
            return true;
        }
        if let Some(frame) = self.phones[p].sent.pop_front() {
            self.receive(p, frame);
            return true;
        }
        if let Some(frame) = self.phones[p].inbox.pop_front() {
            let text = serde_json::to_string(&frame).unwrap();
            self.input(p, Input::Frame { text });
            return true;
        }
        false
    }

    /// Moves the clock on, firing each phone's timer as it falls due.
    fn advance(&mut self, by: i64) {
        let end = self.now + by;
        loop {
            let mut next: Option<(i64, usize)> = None;
            for (p, phone) in self.phones.iter().enumerate() {
                if phone.gone {
                    continue;
                }
                if let Some(at) = phone.wake
                    && at <= end
                    && !matches!(next, Some((earliest, _)) if earliest <= at)
                {
                    next = Some((at, p));
                }
            }
            let Some((at, p)) = next else {
                break;
            };
            self.now = self.now.max(at);
            self.phones[p].wake = None;
            self.input(p, Input::Timer);
            self.settle();
        }
        self.now = end;
    }

    /// The connection drops, and whatever was on its way is lost.
    fn cut(&mut self, p: usize) {
        let phone = &mut self.phones[p];
        phone.up = false;
        phone.connecting = false;
        phone.sent.clear();
        phone.inbox.clear();
        self.input(
            p,
            Input::Closed {
                code: 1006,
                reason: String::new(),
            },
        );
    }

    fn offline(&mut self, p: usize) {
        self.phones[p].network = false;
        self.cut(p);
        self.settle();
    }

    fn online(&mut self, p: usize) {
        self.phones[p].network = true;
        self.command(p, Command::NetworkBack);
        self.settle();
    }

    /// The app restarts from what it saved, with its cookie still valid.
    fn reload(&mut self, p: usize) -> usize {
        self.reload_with(p, false)
    }

    /// The same, from a state that kept no list, names or devices, as one from before it did.
    fn reload_cold(&mut self, p: usize) -> usize {
        self.reload_with(p, true)
    }

    fn reload_with(&mut self, p: usize, cold: bool) -> usize {
        let (account, device) = (self.phones[p].account, self.phones[p].device);
        let [mut first, mut second] = self.phones[p].slots.clone();
        if cold {
            let mut device = keryx_core::mls::Device::import(first.as_deref(), second.as_deref())
                .unwrap()
                .device;
            device.set_shelf(Vec::new());
            let snapshot = device.export().unwrap();
            let bytes = Some(snapshot.bytes.to_vec());
            (first, second) = if snapshot.slot == 0 {
                (bytes, None)
            } else {
                (None, bytes)
            };
        }
        let phone = &mut self.phones[p];
        phone.gone = true;
        phone.up = false;
        let archive = std::mem::take(&mut phone.archive);
        let archive_calls = std::mem::take(&mut phone.archive_calls);
        let q = self.phone(Auth::Cookie);
        self.phones[q].account = account;
        self.phones[q].device = device;
        self.phones[q].archive = archive;
        self.phones[q].archive_calls = archive_calls;
        self.phones[q].slots = [first.clone(), second.clone()];
        self.input(
            q,
            Input::Loaded {
                first,
                second,
                resume: true,
                token: None,
            },
        );
        q
    }

    fn fail_rest(&mut self, p: usize, request: &str, fault: RestFault) {
        self.phones[p]
            .rest_faults
            .push((request.to_string(), fault));
    }

    // ---------------------------------------------------------------------------------------------
    // People
    // ---------------------------------------------------------------------------------------------

    /// Someone signs up and logs in, with no words yet.
    fn sign_up(&mut self, username: &str) -> usize {
        let p = self.phone(Auth::Token);
        self.command(
            p,
            Command::SignUp {
                username: username.to_string(),
                password: Secret("a long enough password".to_string()),
                display_name: None,
                captcha: None,
            },
        );
        self.settle();
        assert!(
            self.phones[p]
                .events
                .contains(&Event::NeedWords { restore: false })
        );
        p
    }

    /// Someone with new words and a device, whose self chat has started.
    fn person(&mut self, username: &str, words: &str) -> usize {
        let p = self.sign_up(username);
        self.command(
            p,
            Command::SetWords {
                words: Secret(words.to_string()),
            },
        );
        self.settle();
        let own = self.self_chat(self.account(p));
        assert!(self.ready(p, own), "{:?}", self.phones[p].events);
        p
    }

    /// A new device of an existing account, logged in, before its words.
    fn log_in(&mut self, username: &str) -> usize {
        let p = self.phone(Auth::Token);
        self.command(
            p,
            Command::LogIn {
                username: username.to_string(),
                password: Secret("a long enough password".to_string()),
                captcha: None,
            },
        );
        self.settle();
        assert!(
            self.phones[p]
                .events
                .contains(&Event::NeedWords { restore: true })
        );
        p
    }

    fn restore(&mut self, p: usize, words: &str) {
        self.command(
            p,
            Command::Restore {
                words: Secret(words.to_string()),
            },
        );
        self.settle();
    }

    fn account(&self, p: usize) -> Ulid {
        self.phones[p].client.account().unwrap()
    }

    fn device_of(&self, p: usize) -> Ulid {
        self.phones[p].client.device().unwrap().record().device
    }

    fn ready(&self, p: usize, chat: Ulid) -> bool {
        self.phones[p].events.contains(&Event::ChatReady { chat })
    }

    /// `p` starts a direct chat with `q`'s account.
    fn start(&mut self, p: usize, q: usize) -> Ulid {
        let account = self.account(q);
        self.command(p, Command::StartChat { account });
        self.settle();
        let chat = self.direct_chat(self.account(p), account).unwrap();
        assert!(
            self.phones[p]
                .events
                .contains(&Event::Started { chat, account })
        );
        assert!(self.ready(p, chat), "{:?}", self.phones[p].events);
        assert!(self.ready(q, chat), "{:?}", self.phones[q].events);
        chat
    }

    /// Sends a text, and returns its local id.
    fn say(&mut self, p: usize, chat: Ulid, text: &str) -> u64 {
        self.command(
            p,
            Command::SendText {
                chat,
                text: Secret(text.to_string()),
                reply: None,
            },
        );
        let mut local = None;
        for event in &self.phones[p].events {
            if let Event::Pending { local: pending, .. } = event {
                local = Some(*pending);
            }
        }
        local.unwrap()
    }

    /// The texts `p` was shown in a chat, by sequence id, the latest event for each winning.
    fn texts(&self, p: usize, chat: Ulid) -> Vec<String> {
        let mut by_seq = BTreeMap::new();
        for event in &self.phones[p].events {
            if let Event::Message { message } = event
                && message.chat == chat
                && let Some(text) = &message.text
            {
                by_seq.insert(message.seq, text.0.clone());
            }
        }
        let mut list = Vec::new();
        for text in by_seq.into_values() {
            list.push(text);
        }
        list
    }

    fn errors(&self, p: usize) -> Vec<String> {
        let mut list = Vec::new();
        for event in &self.phones[p].events {
            if let Event::Error { code, .. } = event {
                list.push(code.clone());
            }
        }
        list
    }

    fn sent_seq(&self, p: usize, local: u64) -> Option<u64> {
        let mut found = None;
        for event in &self.phones[p].events {
            if let Event::Sent {
                local: sent, seq, ..
            } = event
                && *sent == local
            {
                found = Some(*seq);
            }
        }
        found
    }

    fn failed(&self, p: usize, local: u64) -> Option<String> {
        let mut found = None;
        for event in &self.phones[p].events {
            if let Event::Failed {
                local: failed,
                code,
                ..
            } = event
                && *failed == local
            {
                found = Some(code.clone());
            }
        }
        found
    }

    fn asked(&self, p: usize, request: &str) -> usize {
        let mut count = 0;
        for asked in &self.phones[p].asked_list {
            if asked == request {
                count += 1;
            }
        }
        count
    }

    /// Marks everything stored in this chat as seen by this account's other device, so this one
    /// never handled it.
    fn seen_elsewhere(&mut self, p: usize, chat: Ulid) {
        let me = self.account(p);
        let up_to = self.hub_chat(chat).message_list.len() as u64;
        self.position(me, Ulid::new().unwrap(), chat, up_to, false);
        self.settle();
    }

    /// Marks everything stored in this chat as seen, as a person reading it would.
    fn see_all(&mut self, p: usize, chat: Ulid) {
        let up_to_seq = self.hub_chat(chat).message_list.len() as u64;
        self.command(p, Command::Seen { chat, up_to_seq });
        self.settle();
    }

    fn sent_frames(&self, p: usize, kind: &str) -> usize {
        let mut count = 0;
        for sent in &self.phones[p].frame_list {
            if *sent == kind {
                count += 1;
            }
        }
        count
    }

    // ---------------------------------------------------------------------------------------------
    // The server's side
    // ---------------------------------------------------------------------------------------------

    fn account_index(&self, account: Ulid) -> usize {
        let mut found = None;
        for (index, item) in self.accounts.iter().enumerate() {
            if item.id == account {
                found = Some(index);
            }
        }
        found.unwrap()
    }

    fn chat_index(&self, chat: Ulid) -> Option<usize> {
        let mut found = None;
        for (index, item) in self.chats.iter().enumerate() {
            if item.id == chat {
                found = Some(index);
            }
        }
        found
    }

    fn device_index(&self, device: Ulid) -> Option<usize> {
        let mut found = None;
        for (index, item) in self.devices.iter().enumerate() {
            if item.id == device {
                found = Some(index);
            }
        }
        found
    }

    fn self_chat(&self, account: Ulid) -> Ulid {
        let mut found = None;
        for chat in &self.chats {
            if chat.chat_type == ChatType::SelfChat && chat.member_list == [account] {
                found = Some(chat.id);
            }
        }
        found.unwrap()
    }

    fn direct_chat(&self, one: Ulid, other: Ulid) -> Option<Ulid> {
        let mut found = None;
        for chat in &self.chats {
            if chat.chat_type == ChatType::Direct
                && chat.member_list.contains(&one)
                && chat.member_list.contains(&other)
            {
                found = Some(chat.id);
            }
        }
        found
    }

    fn hub_chat(&self, chat: Ulid) -> &HubChat {
        &self.chats[self.chat_index(chat).unwrap()]
    }

    fn live_devices(&self, account: Ulid) -> usize {
        let mut count = 0;
        for device in &self.devices {
            if device.account == account && device.statement.is_none() {
                count += 1;
            }
        }
        count
    }

    fn refusal(&self, p: usize) -> Option<&'static str> {
        let phone = &self.phones[p];
        let (Some(account), Some(device)) = (phone.account, phone.device) else {
            return Some("not_logged_in");
        };
        if let Some(index) = self.device_index(device)
            && self.devices[index].statement.is_some()
        {
            return Some("device_ended");
        }
        if let Some(max) = self.max_devices
            && self.live_devices(account) > max
        {
            return Some("device_limit");
        }
        None
    }

    fn new_chat(&mut self, chat_type: ChatType, member_list: Vec<Ulid>) -> Ulid {
        let chat = id();
        self.chats.push(HubChat {
            id: chat,
            chat_type,
            member_list,
            message_list: Vec::new(),
            epoch: None,
            group_info: None,
            wrap_list: Vec::new(),
            position_list: Vec::new(),
            draft_list: Vec::new(),
        });
        chat
    }

    /// Hands a frame to every online device of these accounts but one.
    /// Saves something inside a call under its next number, as the server's chat task does.
    fn call_event(
        &mut self,
        chat: Ulid,
        call_seq: u64,
        kind: CallEventKind,
        device: Ulid,
        text: Option<keryx_core::protocol::CallText>,
    ) -> CallEvent {
        let now = self.now;
        let list = self.call_events.entry((chat, call_seq)).or_default();
        let event = CallEvent {
            chat,
            call_seq,
            n: list.len() as u64 + 1,
            kind,
            device,
            server_at: now,
            client_at: text.as_ref().map(|text| text.client_at),
            epoch: text.as_ref().map(|text| text.epoch),
            signature: text.as_ref().map(|text| text.signature.clone()),
            body: text.map(|text| text.body),
            outcome: None,
        };
        list.push(event.clone());
        event
    }

    fn deliver(&mut self, account_list: &[Ulid], except: Option<Ulid>, frame: &ServerFrame) {
        for p in 0..self.phones.len() {
            let phone = &self.phones[p];
            let (Some(account), Some(device)) = (phone.account, phone.device) else {
                continue;
            };
            if phone.gone || !phone.up || !account_list.contains(&account) || except == Some(device)
            {
                continue;
            }
            let ended = match self.device_index(device) {
                Some(index) => self.devices[index].statement.is_some(),
                None => false,
            };
            if ended && !matches!(frame, ServerFrame::Revoked { .. }) {
                continue;
            }
            self.push(p, frame.clone());
        }
    }

    fn push(&mut self, p: usize, frame: ServerFrame) {
        let kind = serde_json::to_value(&frame).unwrap()["type"]
            .as_str()
            .unwrap()
            .to_string();
        let phone = &mut self.phones[p];
        let mut index = 0;
        while index < phone.lost_list.len() {
            if phone.lost_list[index] == kind {
                phone.lost_list.remove(index);
                return;
            }
            index += 1;
        }
        phone.inbox.push_back(frame);
    }

    /// Stores a message under the next sequence id and pushes it to the chat's other devices.
    fn append(&mut self, mut message: MessageFrame) -> u64 {
        let index = self.chat_index(message.chat).unwrap();
        let chat = &mut self.chats[index];
        let seq = chat.message_list.len() as u64 + 1;
        message.seq = seq;
        chat.message_list.push(message.clone());
        let member_list = chat.member_list.clone();
        self.deliver(
            &member_list,
            message.device,
            &ServerFrame::Message(Box::new(message)),
        );
        seq
    }

    fn hand_welcomes(&mut self, p: usize) {
        let Some(device) = self.phones[p].device else {
            return;
        };
        if !self.phones[p].up {
            return;
        }
        let mut index = 0;
        while index < self.welcomes.len() {
            if self.welcomes[index].0 == device {
                let (_, chat, body) = self.welcomes.remove(index);
                self.push(
                    p,
                    ServerFrame::Welcome {
                        chat,
                        body: Bytes(body),
                    },
                );
            } else {
                index += 1;
            }
        }
    }

    /// Ends a device with its statement, and tells everyone sharing a chat with it.
    fn end_device(&mut self, device: Ulid, statement: Vec<u8>) {
        let index = self.device_index(device).unwrap();
        self.devices[index].statement = Some(statement.clone());
        let account = self.devices[index].account;
        let mut account_list = vec![account];
        for chat in &self.chats {
            if chat.member_list.contains(&account) {
                for member in &chat.member_list {
                    if !account_list.contains(member) {
                        account_list.push(*member);
                    }
                }
            }
        }
        self.packages.retain(|(owner, _)| *owner != device);
        self.deliver(
            &account_list,
            None,
            &ServerFrame::Revoked {
                account,
                device,
                statement: Bytes(statement),
            },
        );
    }

    /// Records a request, and takes the fault waiting for it. A fault's request ending in `*`
    /// matches every request it begins. With the network down, nothing answers.
    fn take_fault(&mut self, p: usize, method: &str, path: &str) -> Option<RestFault> {
        let request = format!("{method} {path}");
        self.phones[p].asked_list.push(request.clone());
        if !self.phones[p].network {
            return Some(RestFault::Answer(0, ""));
        }
        let phone = &mut self.phones[p];
        for index in 0..phone.rest_faults.len() {
            let key = &phone.rest_faults[index].0;
            let matches = match key.strip_suffix('*') {
                Some(prefix) => request.starts_with(prefix),
                None => *key == request,
            };
            if matches {
                return Some(phone.rest_faults.remove(index).1);
            }
        }
        None
    }

    fn answer(&mut self, p: usize, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
        match self.take_fault(p, method, path) {
            Some(RestFault::Answer(0, _)) => (0, String::new()),
            Some(RestFault::Answer(status, code)) => reply_no(status, code),
            Some(RestFault::Lose) => {
                self.route(p, method, path, body);
                (0, String::new())
            }
            Some(RestFault::Unreadable) => (200, "{}".to_string()),
            None => self.route(p, method, path, body),
        }
    }

    fn file_at(&self, path: &str) -> (usize, usize) {
        let route = path.strip_prefix("/api/v1/files/").unwrap();
        let (file, n) = route.split_once("/chunks/").unwrap();
        let file = parse(file);
        let index = self.files.iter().position(|item| item.id == file).unwrap();
        (index, n.parse().unwrap())
    }

    /// A chunk goes up in order: the next one, or the last one again.
    fn store_chunk(&mut self, p: usize, path: &str, bytes: Vec<u8>) -> (u16, String) {
        let (index, n) = self.file_at(path);
        let me = self.phones[p].account;
        let item = &mut self.files[index];
        if Some(item.account) != me {
            return reply_no(403, "not_yours");
        }
        if n == item.held.len() && (n as u64) < item.chunks {
            item.held.push(bytes);
        } else if n + 1 == item.held.len() {
            item.held[n] = bytes;
        } else {
            return reply_no(409, "chunk_out_of_order");
        }
        reply_ok(json!({ "received": item.held.len() }))
    }

    fn put_chunk(&mut self, p: usize, path: &str, bytes: Vec<u8>) -> (u16, String) {
        match self.take_fault(p, "PUT", path) {
            Some(RestFault::Answer(0, _)) => (0, String::new()),
            Some(RestFault::Answer(status, code)) => reply_no(status, code),
            Some(RestFault::Lose) => {
                self.store_chunk(p, path, bytes);
                (0, String::new())
            }
            Some(RestFault::Unreadable) => (200, "{}".to_string()),
            None => self.store_chunk(p, path, bytes),
        }
    }

    /// A chunk comes down only once a sent message kept its file, except to its uploader.
    fn get_chunk(&mut self, p: usize, path: &str) -> (u16, Vec<u8>) {
        match self.take_fault(p, "GET", path) {
            Some(RestFault::Answer(0, _) | RestFault::Lose) => return (0, Vec::new()),
            Some(RestFault::Answer(status, code)) => {
                return (status, reply_no(status, code).1.into_bytes());
            }
            Some(RestFault::Unreadable) => return (200, b"{}".to_vec()),
            None => {}
        }
        let (index, n) = self.file_at(path);
        let item = &self.files[index];
        if item.removed || !(item.used || Some(item.account) == self.phones[p].account) {
            return (404, reply_no(404, "no_such_file").1.into_bytes());
        }
        let mut bytes = item.held[n].clone();
        if self.garble_chunks {
            bytes[0] ^= 1;
        }
        (200, bytes)
    }

    fn route(&mut self, p: usize, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
        let body: Value = match body {
            Some(text) => serde_json::from_str(text).unwrap(),
            None => Value::Null,
        };
        let me = self.phones[p].account;
        let route = path.strip_prefix("/api/v1/").unwrap();
        if let Some(search) = route.strip_prefix("accounts?search=") {
            let mut account_list = Vec::new();
            for account in &self.accounts {
                if account.username.contains(search) {
                    account_list.push(json!({"id": account.id, "username": account.username}));
                }
            }
            return reply_ok(json!({ "account_list": account_list }));
        }
        let parts: Vec<&str> = route.split('/').collect();
        match (method, parts.as_slice()) {
            ("POST", ["chats", chat, "files"]) => {
                let file = id();
                self.files.push(HubFile {
                    id: file,
                    chat: parse(chat),
                    account: me.unwrap(),
                    chunks: body["chunks"].as_u64().unwrap(),
                    held: Vec::new(),
                    used: false,
                    removed: false,
                });
                reply_ok(json!({ "file": file }))
            }
            ("POST", ["register"]) => {
                let account = id();
                self.accounts.push(HubAccount {
                    id: account,
                    username: body["username"].as_str().unwrap().to_string(),
                    keys: None,
                });
                reply_ok(json!({ "account": account }))
            }
            ("POST", ["login"] | ["login", "token"]) => {
                let username = body["username"].as_str().unwrap();
                let mut account = None;
                for item in &self.accounts {
                    if item.username == username {
                        account = Some(item.id);
                    }
                }
                let Some(account) = account else {
                    return reply_no(401, "bad_login");
                };
                let mut device = None;
                if let Some(proof) = body["device"]["id"].as_str() {
                    let proof = parse(proof);
                    match self.device_index(proof) {
                        Some(index) if self.devices[index].statement.is_none() => {
                            device = Some(proof)
                        }
                        _ => return reply_no(403, "device_ended"),
                    }
                }
                self.phones[p].account = Some(account);
                self.phones[p].device = device;
                reply_ok(json!({"account": account, "device": device, "token": "t"}))
            }
            ("POST", ["challenge"]) => reply_ok(json!({ "challenge": Bytes(vec![7; 32]) })),
            ("GET", ["accounts", account]) => {
                let index = self.account_index(parse(account));
                let account = &self.accounts[index];
                let mut info = json!({"id": account.id, "username": account.username});
                if let Some(keys) = &account.keys {
                    info["master_public"] = json!(keys.master_public);
                    info["account_public"] = json!(keys.account_public);
                    info["account_stamp"] = json!(keys.account_stamp);
                }
                reply_ok(info)
            }
            // No profile is ever saved here: profiles are driven against the real server.
            ("GET", ["accounts", account, "profile"]) => {
                let index = self.account_index(parse(account));
                let account = &self.accounts[index];
                reply_ok(json!({"account": account.id, "username": account.username, "version": 0}))
            }
            ("PUT", ["account", "keys"]) => {
                let me = me.unwrap();
                let index = self.account_index(me);
                if self.accounts[index].keys.is_some() {
                    return reply_no(409, "keys_set");
                }
                self.accounts[index].keys = Some(serde_json::from_value(body).unwrap());
                let chat = self.new_chat(ChatType::SelfChat, vec![me]);
                reply_ok(json!({ "self_chat": chat }))
            }
            ("POST", ["devices", "id"]) => {
                let device = id();
                self.issued.push((me.unwrap(), device));
                reply_ok(json!({ "device": device }))
            }
            ("POST", ["devices"]) => self.register(p, &body),
            ("GET", ["devices"]) => {
                let mut device_list = Vec::new();
                for device in &self.devices {
                    if Some(device.account) == me {
                        device_list.push(json!({
                            "id": device.id, "platform": 1, "os": "android", "os_version": "15",
                            "first_ip": "127.0.0.1", "last_ip": "127.0.0.1", "created_at": NOW,
                            "last_active_at": NOW, "log_list": [],
                        }));
                    }
                }
                reply_ok(json!({ "device_list": device_list }))
            }
            ("POST", ["devices", device, "terminate" | "logout"]) => {
                let signature: Bytes = serde_json::from_value(body["signature"].clone()).unwrap();
                self.end_device(parse(device), signature.0);
                reply_ok(json!({}))
            }
            ("POST", ["key_packages"]) => {
                let Some(device) = self.phones[p].device else {
                    return reply_no(403, "no_device");
                };
                let body_list: Vec<Bytes> =
                    serde_json::from_value(body["body_list"].clone()).unwrap();
                for package in body_list {
                    self.packages.push((device, package.0));
                }
                let mut count = 0;
                for (owner, _) in &self.packages {
                    if *owner == device {
                        count += 1;
                    }
                }
                reply_ok(json!({ "count": count }))
            }
            ("POST", ["accounts", account, "key_packages", "claim"]) => {
                let account = parse(account);
                let mut device_list = Vec::new();
                for device in &self.devices {
                    if device.account == account && device.statement.is_none() {
                        device_list.push(device.id);
                    }
                }
                let mut key_package_list = Vec::new();
                for device in device_list {
                    let mut found = None;
                    for (index, (owner, _)) in self.packages.iter().enumerate() {
                        if *owner == device && found.is_none() {
                            found = Some(index);
                        }
                    }
                    if let Some(index) = found {
                        let (_, package) = self.packages.remove(index);
                        key_package_list.push(json!({"device": device, "body": Bytes(package)}));
                    }
                }
                if key_package_list.is_empty() {
                    return reply_no(404, "no_key_packages");
                }
                reply_ok(json!({ "key_package_list": key_package_list }))
            }
            ("POST", ["chats"]) => {
                let me = me.unwrap();
                let other = parse(body["account"].as_str().unwrap());
                if other == me {
                    return reply_ok(json!({"chat": self.self_chat(me), "created": false}));
                }
                match self.direct_chat(me, other) {
                    Some(chat) => reply_ok(json!({"chat": chat, "created": false})),
                    None => {
                        let chat = self.new_chat(ChatType::Direct, vec![me, other]);
                        reply_ok(json!({"chat": chat, "created": true}))
                    }
                }
            }
            ("GET", ["chats", chat, what]) => {
                let Some(index) = self.chat_index(parse(chat)) else {
                    return reply_no(404, "no_such_chat");
                };
                let chat = &self.chats[index];
                if !chat.member_list.contains(&me.unwrap()) {
                    return reply_no(404, "not_member");
                }
                match *what {
                    "group_info" => match &chat.group_info {
                        Some((epoch, group_info)) => reply_ok(
                            json!({"epoch": epoch, "group_info": Bytes(group_info.clone())}),
                        ),
                        None => reply_no(404, "no_group_info"),
                    },
                    "devices" => {
                        let mut device_list = Vec::new();
                        for device in &self.devices {
                            if !chat.member_list.contains(&device.account) {
                                continue;
                            }
                            let keys = self.accounts[self.account_index(device.account)]
                                .keys
                                .as_ref()
                                .unwrap();
                            let mut item = json!({
                                "account": device.account, "device": device.id,
                                "public_key": Bytes(device.public.clone()),
                                "stamp": Bytes(device.stamp.clone()),
                                "master_public": keys.master_public,
                            });
                            if let Some(statement) = &device.statement {
                                item["statement"] = json!(Bytes(statement.clone()));
                            }
                            device_list.push(item);
                        }
                        reply_ok(json!({ "device_list": device_list }))
                    }
                    "wraps" => reply_ok(json!({ "wrap_list": chat.wrap_list })),
                    _ => panic!("No such route: {method} {path}"),
                }
            }
            ("GET", ["chats", chat, "calls", call_seq, "events"]) => {
                let key = (parse(chat), call_seq.parse::<u64>().unwrap());
                let event_list = self.call_events.get(&key).cloned().unwrap_or_default();
                reply_ok(json!({ "event_list": event_list }))
            }
            ("GET", ["public", "features"]) => {
                reply_ok(json!({"registration": true, "captcha": false}))
            }
            _ => panic!("No such route: {method} {path}"),
        }
    }

    fn register(&mut self, p: usize, body: &Value) -> (u16, String) {
        let me = self.phones[p].account.unwrap();
        if self.phones[p].device.is_some() {
            return reply_no(409, "has_device");
        }
        let device = parse(body["device"].as_str().unwrap());
        let mut issued = None;
        for (index, (account, id)) in self.issued.iter().enumerate() {
            if *account == me && *id == device {
                issued = Some(index);
            }
        }
        let Some(index) = issued else {
            return reply_no(409, "unknown_device_id");
        };
        if let Some(max) = self.max_devices
            && self.live_devices(me) >= max
        {
            return reply_no(403, "device_limit");
        }
        self.issued.remove(index);
        let public: Bytes = serde_json::from_value(body["public_key"].clone()).unwrap();
        let stamp: Bytes = serde_json::from_value(body["stamp"].clone()).unwrap();
        self.devices.push(HubDevice {
            id: device,
            account: me,
            public: public.0,
            stamp: stamp.0.clone(),
            statement: None,
        });
        self.phones[p].device = Some(device);
        let chat = self.self_chat(me);
        self.append(MessageFrame {
            chat,
            seq: 0,
            device: Some(device),
            kind: Some(Kind::DeviceNotice),
            content_kind: None,
            encrypted: Some(false),
            epoch: None,
            pointer_seq: None,
            client_sent_at: Some(self.now),
            server_saved_at: Some(self.now),
            body: Some(Bytes(Vec::new())),
            signature: Some(stamp),
        });
        reply_ok(json!({ "device": device }))
    }

    fn receive(&mut self, p: usize, frame: ClientFrame) {
        let kind = kind_of(&frame);
        let mut fault = None;
        let phone = &mut self.phones[p];
        for index in 0..phone.frame_faults.len() {
            if phone.frame_faults[index].0 == kind {
                fault = Some(phone.frame_faults.remove(index).1);
                break;
            }
        }
        let answer = match fault {
            Some(Fault::Drop) => return,
            Some(Fault::Refuse(code)) => Some(refused(reference_of(&frame), code)),
            Some(Fault::Lose) => {
                self.phones[p].frame_list.push(kind);
                self.handle(p, frame);
                None
            }
            None => {
                self.phones[p].frame_list.push(kind);
                self.handle(p, frame)
            }
        };
        if let Some(answer) = answer {
            self.push(p, answer);
        }
    }

    fn handle(&mut self, p: usize, frame: ClientFrame) -> Option<ServerFrame> {
        let me = self.phones[p].account.unwrap();
        let device = self.phones[p].device.unwrap();
        match frame {
            ClientFrame::Send(send) => Some(self.send(me, device, *send)),
            ClientFrame::Seen {
                reference,
                chat,
                up_to_seq,
                ..
            } => Some(self.position(me, reference, chat, up_to_seq, false)),
            ClientFrame::Received {
                reference,
                chat,
                up_to_seq,
                ..
            } => Some(self.position(me, reference, chat, up_to_seq, true)),
            ClientFrame::Times {
                reference,
                chat,
                seq,
            } => Some(ServerFrame::Times {
                reference,
                chat,
                seq,
                member_list: Vec::new(),
            }),
            ClientFrame::Draft {
                reference,
                chat,
                draft,
            } => {
                let index = self.chat_index(chat).unwrap();
                let hub = &mut self.chats[index];
                hub.draft_list.retain(|(account, _)| *account != me);
                let kept = draft.map(|draft| DraftItem {
                    epoch: draft.epoch,
                    body: draft.body,
                    saved_at: 1,
                });
                if let Some(kept) = &kept {
                    hub.draft_list.push((me, kept.clone()));
                }
                self.deliver(
                    &[me],
                    Some(device),
                    &ServerFrame::Draft { chat, draft: kept },
                );
                Some(ServerFrame::Done { reference })
            }
            // Profiles are driven against the real server, in the server's tests.
            ClientFrame::ProfileKeys { reference, .. } => Some(ServerFrame::Done { reference }),
            ClientFrame::Typing { chat, on } => {
                let member_list = self.hub_chat(chat).member_list.clone();
                self.deliver(
                    &member_list,
                    Some(device),
                    &ServerFrame::Typing {
                        chat,
                        account: me,
                        on,
                    },
                );
                None
            }
            ClientFrame::CallEvent {
                reference,
                chat,
                call_seq,
                kind,
            } => {
                let event = self.call_event(chat, call_seq, kind, device, None);
                let member_list = self.hub_chat(chat).member_list.clone();
                self.deliver(
                    &member_list,
                    Some(device),
                    &ServerFrame::CallEvent(Box::new(event)),
                );
                Some(ServerFrame::Done { reference })
            }
            ClientFrame::Sync {
                reference,
                since,
                after,
                chat,
            } => {
                let from = since.map_or(0, |since| since - 1000);
                let wanted = |position: SyncPosition| match after {
                    Some(after) => position > after,
                    None => position.at >= from,
                };
                let mut rows: Vec<(SyncPosition, Result<MessageFrame, CallEvent>)> = Vec::new();
                for hub_chat in &self.chats {
                    if !hub_chat.member_list.contains(&me)
                        || chat.is_some_and(|only| only != hub_chat.id)
                    {
                        continue;
                    }
                    for message in &hub_chat.message_list {
                        let position = message_position(message);
                        if wanted(position) {
                            rows.push((position, Ok(message.clone())));
                        }
                    }
                    for ((call_chat, _), event_list) in &self.call_events {
                        if *call_chat != hub_chat.id {
                            continue;
                        }
                        for event in event_list {
                            let position = call_position(event);
                            if wanted(position) {
                                rows.push((position, Err(event.clone())));
                            }
                        }
                    }
                }
                rows.sort_by_key(|(position, _)| *position);
                let more = rows.len() > SYNC_PAGE;
                rows.truncate(SYNC_PAGE);
                let mut message_list = Vec::new();
                let mut call_list = Vec::new();
                for (_, row) in rows {
                    match row {
                        Ok(message) => message_list.push(message),
                        Err(event) => call_list.push(event),
                    }
                }
                Some(ServerFrame::Chats {
                    reference,
                    chat_list: match after {
                        Some(_) => Vec::new(),
                        None => self.chat_list(me),
                    },
                    message_list,
                    call_list,
                    more,
                })
            }
            ClientFrame::GroupInfo {
                reference,
                chat,
                epoch,
                body,
            } => {
                let index = self.chat_index(chat).unwrap();
                let chat = &mut self.chats[index];
                Some(match chat.epoch {
                    None if epoch == 0 => {
                        chat.epoch = Some(0);
                        chat.group_info = Some((0, body.0));
                        ServerFrame::Done { reference }
                    }
                    // The same upload again, after its answer was lost.
                    Some(0)
                        if epoch == 0
                            && matches!(&chat.group_info, Some((_, info)) if *info == body.0) =>
                    {
                        ServerFrame::Done { reference }
                    }
                    Some(_) if epoch == 0 => refused(reference, "chat_started"),
                    Some(current) if current == epoch => {
                        chat.group_info = Some((epoch, body.0));
                        ServerFrame::Done { reference }
                    }
                    _ => refused(reference, "stale_epoch"),
                })
            }
            ClientFrame::Ping { reference } => Some(ServerFrame::Done { reference }),
            ClientFrame::Wraps {
                reference,
                chat,
                epoch,
                wrap_list,
            } => {
                let index = self.chat_index(chat).unwrap();
                for wrap in wrap_list {
                    self.put_wrap(index, epoch, wrap.account, wrap.wrapped.0);
                }
                Some(ServerFrame::Done { reference })
            }
        }
    }

    fn put_wrap(&mut self, index: usize, epoch: u64, account: Ulid, wrapped: Vec<u8>) {
        let chat = &mut self.chats[index];
        chat.wrap_list
            .retain(|wrap| !(wrap.epoch == epoch && wrap.account == account));
        chat.wrap_list.push(WrapEntry {
            epoch,
            account,
            wrapped: Bytes(wrapped),
        });
    }

    fn send(&mut self, me: Ulid, device: Ulid, send: SendFrame) -> ServerFrame {
        let reference = send.reference;
        let Some(index) = self.chat_index(send.chat) else {
            return refused(reference, "not_member");
        };
        if !self.chats[index].member_list.contains(&me) {
            return refused(reference, "not_member");
        }
        if send.kind == Kind::Commit {
            match (self.chats[index].epoch, send.epoch) {
                (Some(current), Some(epoch)) if epoch == current + 1 => {}
                _ => return refused(reference, "commit_lost"),
            }
            for welcome in &send.welcome_list {
                let live = match self.device_index(welcome.device) {
                    Some(at) => self.devices[at].statement.is_none(),
                    None => false,
                };
                if !live {
                    return refused(reference, "no_such_device");
                }
            }
            let epoch = send.epoch.unwrap();
            self.chats[index].epoch = Some(epoch);
            self.chats[index].group_info = Some((epoch, send.group_info.clone().unwrap().0));
            for wrap in &send.wrap_list {
                let mut wrapped = wrap.wrapped.0.clone();
                if self.garble_wraps_for == Some(wrap.account) {
                    wrapped = vec![9; wrapped.len()];
                }
                self.put_wrap(index, epoch, wrap.account, wrapped);
            }
        }
        if send.kind == Kind::Delete
            && let Some(target) = send.pointer_seq
        {
            let now = self.now;
            let chat = &mut self.chats[index];
            for message in &mut chat.message_list {
                if message.seq == target
                    || (message.kind == Some(Kind::Edit) && message.pointer_seq == Some(target))
                {
                    *message = wiped(message.chat, message.seq, now);
                }
            }
        }
        for item in &mut self.files {
            if item.chat == send.chat && send.persist.contains(&item.id) {
                item.used = true;
            }
            if item.chat == send.chat && send.remove.contains(&item.id) {
                item.removed = true;
            }
        }
        let seq = self.append(MessageFrame {
            chat: send.chat,
            seq: 0,
            device: Some(device),
            kind: Some(send.kind),
            content_kind: send.content_kind,
            encrypted: Some(send.encrypted),
            epoch: send.epoch,
            pointer_seq: send.pointer_seq,
            client_sent_at: Some(send.client_sent_at),
            server_saved_at: Some(self.now),
            body: Some(send.body),
            signature: Some(send.signature),
        });
        let mut handed = Vec::new();
        for welcome in send.welcome_list {
            let mut body = welcome.body.0;
            if self.garble_welcomes {
                body = vec![9; body.len()];
            }
            self.welcomes.push((welcome.device, send.chat, body));
            handed.push(welcome.device);
        }
        for q in 0..self.phones.len() {
            if let Some(device) = self.phones[q].device
                && handed.contains(&device)
            {
                self.hand_welcomes(q);
            }
        }
        ServerFrame::Sent {
            reference,
            seq,
            server_saved_at: self.now,
        }
    }

    fn position(
        &mut self,
        me: Ulid,
        reference: Ulid,
        chat: Ulid,
        up_to: u64,
        received: bool,
    ) -> ServerFrame {
        let index = self.chat_index(chat).unwrap();
        let hub_chat = &mut self.chats[index];
        if up_to > hub_chat.message_list.len() as u64 {
            return refused(
                reference,
                if received { "bad_received" } else { "bad_seen" },
            );
        }
        let mut found = None;
        for (at, mark) in hub_chat.position_list.iter().enumerate() {
            if mark.account == me {
                found = Some(at);
            }
        }
        let at = match found {
            Some(at) => at,
            None => {
                hub_chat.position_list.push(MarkItem {
                    account: me,
                    received_seq: 0,
                    seen_seq: Some(0),
                });
                hub_chat.position_list.len() - 1
            }
        };
        let mark = &mut hub_chat.position_list[at];
        let rose = if received {
            let rose = up_to > mark.received_seq;
            mark.received_seq = mark.received_seq.max(up_to);
            rose
        } else {
            let rose = up_to > mark.seen_seq.unwrap_or(0);
            mark.seen_seq = Some(mark.seen_seq.unwrap_or(0).max(up_to));
            rose
        };
        let mark = mark.clone();
        if rose {
            let mut others = hub_chat.member_list.clone();
            others.retain(|account| *account != me);
            self.deliver(&others, None, &ServerFrame::Marks { chat, mark });
        }
        ServerFrame::Done { reference }
    }

    fn chat_list(&self, me: Ulid) -> Vec<ChatItem> {
        let mut chat_list = Vec::new();
        for chat in &self.chats {
            if !chat.member_list.contains(&me) {
                continue;
            }
            let mut last_content = None;
            for message in &chat.message_list {
                if matches!(message.kind, Some(Kind::Content | Kind::DeviceNotice)) {
                    last_content = Some(Box::new(message.clone()));
                }
            }
            let mut mark_list = Vec::new();
            for member in &chat.member_list {
                if *member == me {
                    continue;
                }
                let mut mark = MarkItem {
                    account: *member,
                    received_seq: 0,
                    seen_seq: Some(0),
                };
                for item in &chat.position_list {
                    if item.account == *member {
                        mark = item.clone();
                    }
                }
                mark_list.push(mark);
            }
            // As the real server does: this member's own seen position, 0 before any `seen`.
            let mut seen_seq = 0;
            for item in &chat.position_list {
                if item.account == me {
                    seen_seq = item.seen_seq.unwrap_or(0);
                }
            }
            chat_list.push(ChatItem {
                chat: chat.id,
                chat_type: chat.chat_type,
                mode: ChatMode::Private,
                last_seq: Some(chat.message_list.len() as u64),
                last_kind: None,
                last_at: None,
                last_content_seq: None,
                epoch: if self.hide_epoch { None } else { chat.epoch },
                last_content,
                mark_list,
                seen_seq: Some(seen_seq),
                draft: chat
                    .draft_list
                    .iter()
                    .find(|(account, _)| *account == me)
                    .map(|(_, draft)| draft.clone()),
                profile_key_list: Vec::new(),
            });
        }
        chat_list
    }
}

fn pair() -> (World, usize, usize, Ulid) {
    let mut world = World::new();
    let omid = world.person("omid", OMID_WORDS);
    let mina = world.person("mina", MINA_WORDS);
    let chat = world.start(omid, mina);
    // A reload pages the self chat's new-device notice until it is seen. Seeing it here keeps
    // the tests about the chat they are on.
    for phone in [omid, mina] {
        world.see_all(phone, world.self_chat(world.account(phone)));
    }
    (world, omid, mina, chat)
}

fn tell(world: &mut World, phone: usize, chat: Ulid, text: &str) -> u64 {
    let local = world.say(phone, chat, text);
    world.settle();
    local
}

fn error_count(world: &World, phone: usize, code: &str) -> usize {
    let mut count = 0;
    for error in world.errors(phone) {
        if error == code {
            count += 1;
        }
    }
    count
}

fn shown_times(world: &World, phone: usize, chat: Ulid, seq: u64) -> usize {
    let mut count = 0;
    for event in &world.phones[phone].events {
        if let Event::Message { message } = event
            && message.chat == chat
            && message.seq == seq
        {
            count += 1;
        }
    }
    count
}

fn has_notice(world: &World, phone: usize, problem: Option<Problem>) -> bool {
    for event in &world.phones[phone].events {
        if let Event::Notice { problem: found, .. } = event
            && *found == problem
        {
            return true;
        }
    }
    false
}

fn has_limit(world: &World, phone: usize) -> bool {
    for event in &world.phones[phone].events {
        if matches!(event, Event::DeviceLimit { .. }) {
            return true;
        }
    }
    false
}

fn has_login(world: &World, phone: usize) -> bool {
    world.phones[phone].events.contains(&Event::NeedLogin)
}

#[test]
fn messages_missed_while_away_are_fetched_on_reconnect_and_a_reload_syncs_from_its_last_kept_row() {
    let (mut world, omid, mina, chat) = pair();
    world.offline(mina);
    tell(&mut world, omid, chat, "one");
    tell(&mut world, omid, chat, "two");
    assert!(world.texts(mina, chat).is_empty());
    world.online(mina);
    assert_eq!(
        world.texts(mina, chat),
        vec!["one".to_string(), "two".to_string()]
    );
    let mut received = None;
    for mark in &world.hub_chat(chat).position_list {
        if mark.account == world.account(mina) {
            received = Some(mark.received_seq);
        }
    }
    // After the commit that added Mina, the two texts are 2 and 3.
    assert_eq!(received, Some(3));

    world.offline(mina);
    world.advance(2 * SECOND);
    tell(&mut world, omid, chat, "three");
    // A reload syncs from the last row it kept, a second back: "two" and what shares its moment come
    // again, and the archive drops them as copies.
    let restarted = world.reload(mina);
    world.settle();
    assert_eq!(world.texts(restarted, chat), vec!["one", "two", "three"]);

    world.offline(restarted);
    world.advance(2 * SECOND);
    tell(&mut world, omid, chat, "four");
    let again = world.reload(restarted);
    world.settle();
    // Only the second before "three" comes again. Nothing older is shown until the chat is opened,
    // from the archive.
    assert_eq!(world.texts(again, chat), vec!["three", "four"]);
    world.command(again, Command::OpenChat { chat });
    world.settle();
    assert_eq!(
        world.texts(again, chat),
        vec!["one", "two", "three", "four"]
    );
}

#[test]
fn an_unanswered_send_is_matched_by_its_signature_and_resent_only_when_it_is_absent() {
    let (mut world, omid, mina, chat) = pair();
    world.phones[omid].frame_faults.push(("send", Fault::Lose));
    let kept = world.say(omid, chat, "kept");
    world.settle();
    assert!(world.sent_seq(omid, kept).is_none());
    world.cut(omid);
    world.advance(2 * SECOND);
    assert!(world.sent_seq(omid, kept).is_some());
    assert_eq!(world.texts(mina, chat), vec!["kept".to_string()]);

    world.phones[omid].frame_faults.push(("send", Fault::Drop));
    let dropped = world.say(omid, chat, "dropped");
    world.settle();
    assert!(world.sent_seq(omid, dropped).is_none());
    world.cut(omid);
    world.advance(2 * SECOND);
    assert!(world.sent_seq(omid, dropped).is_some());
    assert_eq!(
        world.texts(mina, chat),
        vec!["kept".to_string(), "dropped".to_string()]
    );
}

#[test]
fn background_requests_retry_and_one_the_person_starts_fails_at_once() {
    let (mut world, omid, _, _) = pair();
    world.fail_rest(
        omid,
        "POST /api/v1/key_packages",
        RestFault::Answer(503, "unavailable"),
    );
    world.cut(omid);
    world.advance(2 * SECOND);
    let tried = world.asked(omid, "POST /api/v1/key_packages");
    assert!(tried >= 1);
    assert_eq!(error_count(&world, omid, "offline"), 0);
    world.advance(2 * SECOND);
    assert!(world.asked(omid, "POST /api/v1/key_packages") > tried);

    world.offline(omid);
    world.command(
        omid,
        Command::Search {
            text: "mina".to_string(),
        },
    );
    world.settle();
    assert!(error_count(&world, omid, "offline") >= 1);
}

#[test]
fn the_words_keys_stay_until_the_chat_has_been_restored() {
    let (mut world, omid, _mina, chat) = pair();
    tell(&mut world, omid, chat, "history");
    let second = world.log_in("omid");
    world.fail_rest(
        second,
        &format!("GET /api/v1/chats/{chat}/wraps"),
        RestFault::Answer(503, "unavailable"),
    );
    world.restore(second, OMID_WORDS);
    assert!(world.texts(second, chat).is_empty());
    world.advance(2 * SECOND);
    assert_eq!(world.texts(second, chat), vec!["history".to_string()]);
}

#[test]
fn what_needs_the_socket_is_kept_and_typing_is_not() {
    let (mut world, omid, mina, chat) = pair();
    tell(&mut world, omid, chat, "there");
    world.offline(mina);
    let (seen, syncs) = (
        world.sent_frames(mina, "seen"),
        world.sent_frames(mina, "sync"),
    );
    world.command(mina, Command::Seen { chat, up_to_seq: 1 });
    world.command(mina, Command::Typing { chat, on: true });
    world.settle();
    assert_eq!(world.sent_frames(mina, "seen"), seen);
    assert_eq!(world.sent_frames(mina, "typing"), 0);
    // Opening a chat reads the archive, so it needs no socket.
    world.phones[mina].events.clear();
    world.command(mina, Command::OpenChat { chat });
    world.settle();
    assert_eq!(world.texts(mina, chat), vec!["there"]);
    assert_eq!(world.sent_frames(mina, "sync"), syncs);
    world.online(mina);
    assert_eq!(world.sent_frames(mina, "seen"), seen + 1);
    // One `sync` for every chat.
    assert_eq!(world.sent_frames(mina, "sync"), syncs + 1);
    assert_eq!(world.sent_frames(mina, "typing"), 0);

    // A key change carries its group info, so nothing else goes up for it.
    world.advance(8 * DAY);
    let uploads = world.sent_frames(omid, "group_info");
    tell(&mut world, omid, chat, "rotated");
    assert_eq!(world.sent_frames(omid, "group_info"), uploads);
    let hub = world.hub_chat(chat);
    assert_eq!(hub.group_info.as_ref().map(|(epoch, _)| *epoch), hub.epoch);
}

#[test]
fn a_draft_is_sealed_sent_and_comes_back_in_the_list_but_is_not_kept_offline() {
    let (mut world, omid, mina, chat) = pair();
    tell(&mut world, omid, chat, "there");
    let draft = |text: &str| Command::Draft {
        chat,
        draft: Some(Draft {
            text: Secret(text.to_string()),
            reply: Some(1),
            edit: None,
        }),
    };
    world.command(mina, draft("half"));
    world.settle();
    assert_eq!(world.sent_frames(mina, "draft"), 1);
    let held = &world.hub_chat(chat).draft_list;
    assert_eq!(held.len(), 1);
    assert!(!held[0].1.body.0.windows(4).any(|part| part == b"half"));
    // The list brings it back, opened.
    world.phones[mina].events.clear();
    world.command(mina, Command::ChatList);
    world.settle();
    let shown = world.phones[mina]
        .events
        .iter()
        .find_map(|event| match event {
            Event::Draft { draft, .. } => draft
                .as_ref()
                .map(|draft| (draft.text.0.clone(), draft.reply)),
            _ => None,
        });
    assert_eq!(shown, Some(("half".to_string(), Some(1))));
    // Clearing sends none.
    world.command(mina, Command::Draft { chat, draft: None });
    world.settle();
    assert!(world.hub_chat(chat).draft_list.is_empty());
    // Offline, nothing waits to be sent.
    world.offline(mina);
    world.command(mina, draft("later"));
    world.settle();
    world.online(mina);
    assert_eq!(world.sent_frames(mina, "draft"), 2);
}

#[test]
fn a_start_whose_group_info_answer_was_lost_sends_it_again_and_carries_on() {
    let mut world = World::new();
    let omid = world.person("omid", OMID_WORDS);
    let mina = world.person("mina", MINA_WORDS);
    world.phones[omid]
        .frame_faults
        .push(("group_info", Fault::Lose));
    let account = world.account(mina);
    world.command(omid, Command::StartChat { account });
    world.settle();
    let chat = world.direct_chat(world.account(omid), account).unwrap();
    assert!(!world.ready(mina, chat));
    let uploads = world.sent_frames(omid, "group_info");
    world.cut(omid);
    world.advance(2 * SECOND);
    assert_eq!(world.sent_frames(omid, "group_info"), uploads + 1);
    assert!(world.ready(omid, chat), "{:?}", world.errors(omid));
    assert!(world.ready(mina, chat), "{:?}", world.errors(mina));
    assert!(world.errors(omid).is_empty(), "{:?}", world.errors(omid));
    tell(&mut world, omid, chat, "started");
    assert_eq!(world.texts(mina, chat), vec!["started"]);
}

#[test]
fn a_message_that_cannot_be_shown_still_appears_with_its_problem() {
    let (mut world, omid, mina, chat) = pair();
    world.offline(mina);
    let signed = tell(&mut world, omid, chat, "signed");
    let broken = tell(&mut world, omid, chat, "broken");
    let signed_seq = world.sent_seq(omid, signed).unwrap();
    let broken_seq = world.sent_seq(omid, broken).unwrap();
    let index = world.chat_index(chat).unwrap();
    for message in &mut world.chats[index].message_list {
        if message.seq == signed_seq
            && let Some(signature) = &mut message.signature
        {
            signature.0[0] ^= 1;
        }
        if message.seq == broken_seq {
            message.signature = None;
        }
    }
    world.append(MessageFrame {
        chat,
        seq: 0,
        device: Some(Ulid::from_bytes([9; 16])),
        kind: Some(Kind::Content),
        content_kind: Some(ContentKind::Text),
        encrypted: Some(true),
        epoch: Some(0),
        pointer_seq: None,
        client_sent_at: Some(world.now),
        server_saved_at: Some(world.now),
        body: Some(Bytes(vec![1, 2, 3])),
        signature: Some(Bytes(vec![4; 64])),
    });
    world.online(mina);
    let mut found = Vec::new();
    for event in &world.phones[mina].events {
        if let Event::Message { message } = event
            && message.chat == chat
            && let Some(problem) = message.problem
        {
            found.push(problem);
        }
    }
    assert!(found.contains(&Problem::BadSignature));
    assert!(found.contains(&Problem::Malformed));
    assert!(found.contains(&Problem::UnknownDevice));
}

#[test]
fn a_failed_commit_or_welcome_rejoins_and_one_refused_for_what_it_is_does_not() {
    let mut world = World::new();
    let omid = world.person("omid", OMID_WORDS);
    let mina = world.person("mina", MINA_WORDS);
    world.garble_welcomes = true;
    world.command(
        omid,
        Command::StartChat {
            account: world.account(mina),
        },
    );
    world.settle();
    let chat = world
        .direct_chat(world.account(omid), world.account(mina))
        .unwrap();
    assert!(world.ready(mina, chat), "{:?}", world.errors(mina));
    assert_eq!(error_count(&world, mina, "bad_welcome"), 0);
    assert!(world.sent_frames(mina, "commit") >= 1);
    world.garble_welcomes = false;

    world.offline(mina);
    world.advance(8 * DAY);
    tell(&mut world, omid, chat, "rotated");
    let index = world.chat_index(chat).unwrap();
    let mut newest = 0;
    for message in &world.chats[index].message_list {
        if message.kind == Some(Kind::Commit) && message.seq > newest {
            newest = message.seq;
        }
    }
    for message in &mut world.chats[index].message_list {
        if message.seq == newest {
            message.body = Some(Bytes(vec![1, 2, 3]));
        }
    }
    let before = world.sent_frames(mina, "commit");
    world.online(mina);
    assert!(
        world.sent_frames(mina, "commit") > before,
        "{:?}",
        world.errors(mina)
    );
    let after = tell(&mut world, omid, chat, "after");
    assert!(world.sent_seq(omid, after).is_some());
    assert_eq!(
        world.texts(mina, chat).last().map(String::as_str),
        Some("after")
    );

    let (_, group_info) = world.hub_chat(chat).group_info.clone().unwrap();
    let stranger_id = Ulid::new().unwrap();
    let stranger_keys = derive(WORDS).unwrap();
    let stranger_account = Ulid::new().unwrap();
    let mut stranger = Device::create(&stranger_keys, stranger_account, stranger_id).unwrap();
    let mut account_list = Vec::new();
    for (phone, words) in [(omid, OMID_WORDS), (mina, MINA_WORDS)] {
        let keys = derive(words).unwrap();
        account_list.push(AccountKey {
            account: world.account(phone),
            account_public: *keys.account_public(),
            master_public: *keys.master_public(),
            stamp: stamp_account_key(&keys, &world.account(phone)),
        });
    }
    account_list.push(AccountKey {
        account: stranger_account,
        account_public: *stranger_keys.account_public(),
        master_public: *stranger_keys.master_public(),
        stamp: stamp_account_key(&stranger_keys, &stranger_account),
    });
    let join = stranger
        .join_external(chat, &group_info, world.now, &account_list)
        .unwrap();
    let commits_before = world.sent_frames(omid, "commit");
    world.append(MessageFrame {
        chat,
        seq: 0,
        device: Some(stranger_id),
        kind: Some(Kind::Commit),
        content_kind: None,
        encrypted: Some(true),
        epoch: Some(join.epoch),
        pointer_seq: None,
        client_sent_at: Some(world.now),
        server_saved_at: Some(world.now),
        body: Some(Bytes(join.commit)),
        signature: None,
    });
    world.settle();
    assert!(error_count(&world, omid, "bad_commit") >= 1);
    assert_eq!(world.sent_frames(omid, "commit"), commits_before);
    let kept = tell(&mut world, mina, chat, "still here");
    assert!(world.sent_seq(mina, kept).is_some());
}

#[test]
fn a_send_waits_for_a_key_change_and_then_goes_out() {
    let (mut world, omid, mina, chat) = pair();
    world.advance(8 * DAY);
    let local = world.say(omid, chat, "after");
    assert!(world.sent_seq(omid, local).is_none());
    world.settle();
    assert!(world.sent_seq(omid, local).is_some());
    assert!(world.failed(omid, local).is_none());
    assert_eq!(
        world.texts(mina, chat).last().map(String::as_str),
        Some("after")
    );
}

#[test]
fn a_new_device_notice_is_checked() {
    let mut world = World::new();
    let first = world.person("omid", OMID_WORDS);
    let second = world.log_in("omid");
    world.restore(second, OMID_WORDS);
    let chat = world.self_chat(world.account(first));
    assert!(
        has_notice(&world, first, None),
        "{:?}",
        world.phones[first].events
    );
    world.append(MessageFrame {
        chat,
        seq: 0,
        device: Some(world.device_of(second)),
        kind: Some(Kind::DeviceNotice),
        content_kind: None,
        encrypted: Some(false),
        epoch: None,
        pointer_seq: None,
        client_sent_at: Some(world.now),
        server_saved_at: Some(world.now),
        body: Some(Bytes(Vec::new())),
        signature: Some(Bytes(vec![1; 64])),
    });
    world.settle();
    assert!(has_notice(&world, first, Some(Problem::BadSignature)));
}

#[test]
fn the_same_message_can_be_shown_more_than_once() {
    let (mut world, omid, mina, chat) = pair();
    let local = tell(&mut world, omid, chat, "again");
    let seq = world.sent_seq(omid, local).unwrap();
    assert_eq!(shown_times(&world, mina, chat, seq), 1);
    world.command(mina, Command::OpenChat { chat });
    world.settle();
    assert!(shown_times(&world, mina, chat, seq) >= 2);
}

#[test]
fn a_failed_state_save_is_retried_and_reported_once() {
    let mut world = World::new();
    let omid = world.person("omid", OMID_WORDS);
    let mina = world.person("mina", MINA_WORDS);
    world.phones[mina].failing_saves = 1;
    world.command(
        omid,
        Command::StartChat {
            account: world.account(mina),
        },
    );
    world.settle();
    assert_eq!(error_count(&world, mina, "save_failed"), 1);
    world.advance(2 * SECOND);
    assert_eq!(error_count(&world, mina, "save_failed"), 1);
    let chat = world
        .direct_chat(world.account(omid), world.account(mina))
        .unwrap();
    assert!(world.ready(mina, chat));
}

#[test]
fn a_gone_session_an_ended_device_and_the_device_limit_are_acted_on() {
    let (mut world, omid, _, _) = pair();
    world.fail_rest(
        omid,
        "POST /api/v1/key_packages",
        RestFault::Answer(401, "unauthorized"),
    );
    world.cut(omid);
    world.advance(2 * SECOND);
    assert!(has_login(&world, omid));
    let connects = world.phones[omid].connects;
    world.advance(8 * SECOND);
    assert_eq!(world.phones[omid].connects, connects);

    let mut world = World::new();
    let omid = world.person("omid", OMID_WORDS);
    world.end_device(world.device_of(omid), vec![9; 64]);
    world.settle();
    assert!(!world.phones[omid].wiped);
    world.cut(omid);
    world.advance(2 * SECOND);
    assert!(world.phones[omid].wiped);

    let mut world = World::new();
    world.max_devices = Some(1);
    let first = world.person("omid", OMID_WORDS);
    let first_device = world.device_of(first);
    let second = world.log_in("omid");
    world.restore(second, OMID_WORDS);
    assert!(has_limit(&world, second), "{:?}", world.errors(second));
    world.command(
        second,
        Command::Terminate {
            device: first_device,
            words: Secret(OMID_WORDS.to_string()),
        },
    );
    world.settle();
    assert!(world.phones[second].device.is_some());
    assert!(world.phones[second].up);
    let own = world.self_chat(world.account(second));
    assert!(world.ready(second, own), "{:?}", world.errors(second));
}

#[test]
fn set_words_continues_when_the_server_already_holds_these_keys() {
    let mut world = World::new();
    let omid = world.sign_up("omid");
    world.fail_rest(omid, "PUT /api/v1/account/keys", RestFault::Lose);
    world.command(
        omid,
        Command::SetWords {
            words: Secret(OMID_WORDS.to_string()),
        },
    );
    world.settle();
    assert!(world.phones[omid].device.is_none());
    world.advance(2 * SECOND);
    let own = world.self_chat(world.account(omid));
    assert!(world.ready(omid, own), "{:?}", world.errors(omid));
}

fn commit_count(world: &World, chat: Ulid) -> usize {
    let mut count = 0;
    for message in &world.hub_chat(chat).message_list {
        if message.kind == Some(Kind::Commit) {
            count += 1;
        }
    }
    count
}

#[test]
fn two_devices_changing_the_key_at_once_end_on_one_commit_and_both_messages_go() {
    let (mut world, omid, mina, chat) = pair();
    let commits = commit_count(&world, chat);
    let sent = world.sent_frames(omid, "commit") + world.sent_frames(mina, "commit");
    world.advance(8 * DAY);
    let from_omid = world.say(omid, chat, "from omid");
    let from_mina = world.say(mina, chat, "from mina");
    world.settle();
    // Both committed, the server kept the first, and the loser caught up instead of rotating again.
    assert_eq!(
        world.sent_frames(omid, "commit") + world.sent_frames(mina, "commit"),
        sent + 2
    );
    assert_eq!(commit_count(&world, chat), commits + 1);
    assert!(world.sent_seq(omid, from_omid).is_some());
    assert!(world.sent_seq(mina, from_mina).is_some());
    assert!(world.errors(omid).is_empty(), "{:?}", world.errors(omid));
    assert!(world.errors(mina).is_empty(), "{:?}", world.errors(mina));
    assert_eq!(world.texts(omid, chat), vec!["from mina"]);
    assert_eq!(world.texts(mina, chat), vec!["from omid"]);
}

#[test]
fn two_members_removing_an_ended_device_at_once_commit_it_once() {
    let (mut world, omid, mina, chat) = pair();
    let second = world.log_in("omid");
    world.restore(second, OMID_WORDS);
    assert!(world.ready(second, chat), "{:?}", world.errors(second));
    let commits = commit_count(&world, chat);
    let ended = world.device_of(second);
    world.command(
        omid,
        Command::Terminate {
            device: ended,
            words: Secret(OMID_WORDS.to_string()),
        },
    );
    world.settle();
    assert!(
        world.phones[omid]
            .events
            .contains(&Event::Terminated { device: ended })
    );
    assert!(world.phones[second].wiped);
    assert!(world.sent_frames(mina, "commit") >= 1);
    assert_eq!(commit_count(&world, chat), commits + 1);
    assert!(world.errors(omid).is_empty(), "{:?}", world.errors(omid));
    assert!(world.errors(mina).is_empty(), "{:?}", world.errors(mina));
    tell(&mut world, omid, chat, "after");
    tell(&mut world, mina, chat, "back");
    assert_eq!(world.texts(mina, chat), vec!["after"]);
    assert_eq!(world.texts(omid, chat), vec!["back"]);
}

#[test]
fn a_join_that_lost_its_race_is_tried_again_after_a_wait() {
    let (mut world, omid, _, chat) = pair();
    tell(&mut world, omid, chat, "before");
    let second = world.log_in("omid");
    world.phones[second]
        .frame_faults
        .push(("commit", Fault::Refuse("commit_lost")));
    world.restore(second, OMID_WORDS);
    let own = world.self_chat(world.account(omid));
    assert!(!(world.ready(second, own) && world.ready(second, chat)));
    let commits = world.sent_frames(second, "commit");
    world.advance(2 * SECOND);
    assert_eq!(world.sent_frames(second, "commit"), commits + 1);
    assert!(world.ready(second, own), "{:?}", world.errors(second));
    assert!(world.ready(second, chat), "{:?}", world.errors(second));
    assert!(
        world.errors(second).is_empty(),
        "{:?}",
        world.errors(second)
    );
    world.command(second, Command::OpenChat { chat });
    world.settle();
    assert_eq!(world.texts(second, chat), vec!["before"]);
}

#[test]
fn two_people_starting_the_same_chat_at_once_end_in_one_group() {
    let mut world = World::new();
    let omid = world.person("omid", OMID_WORDS);
    let mina = world.person("mina", MINA_WORDS);
    let (omid_account, mina_account) = (world.account(omid), world.account(mina));
    world.command(
        omid,
        Command::StartChat {
            account: mina_account,
        },
    );
    world.command(
        mina,
        Command::StartChat {
            account: omid_account,
        },
    );
    world.settle();
    let chat = world.direct_chat(omid_account, mina_account).unwrap();
    assert!(world.ready(omid, chat), "{:?}", world.errors(omid));
    // Mina's own join was on its way when Omid's Welcome came, so she joins again after a wait.
    world.advance(2 * SECOND);
    assert!(world.ready(mina, chat), "{:?}", world.errors(mina));
    // Both self chats put up their wraps, and of the two starts only the one whose group info
    // was kept went on to its wraps.
    assert_eq!(
        world.sent_frames(omid, "wraps") + world.sent_frames(mina, "wraps"),
        3
    );
    assert!(world.errors(omid).is_empty(), "{:?}", world.errors(omid));
    assert!(world.errors(mina).is_empty(), "{:?}", world.errors(mina));
    tell(&mut world, omid, chat, "hello");
    tell(&mut world, mina, chat, "hi");
    assert_eq!(world.texts(omid, chat), vec!["hi"]);
    assert_eq!(world.texts(mina, chat), vec!["hello"]);
}

#[test]
fn many_missed_messages_are_fetched_page_by_page_in_order() {
    let (mut world, omid, mina, chat) = pair();
    // A first reconnect pages the self chat's notice, so the pages below are this chat's alone.
    world.cut(mina);
    world.advance(2 * SECOND);
    world.offline(mina);
    let mut expected = Vec::new();
    for n in 0..120 {
        let text = format!("m{n}");
        tell(&mut world, omid, chat, &text);
        expected.push(text);
    }
    let pages = world.sent_frames(mina, "sync");
    world.online(mina);
    assert_eq!(world.texts(mina, chat), expected);
    // 120 rows, and the chat's first rows from the second before the last one kept, at the fake
    // server's 20 a page, each page asked for once the one before was kept.
    assert_eq!(world.sent_frames(mina, "sync"), pages + 7);
    assert!(world.errors(mina).is_empty(), "{:?}", world.errors(mina));
}

#[test]
fn a_live_message_that_skips_ahead_fetches_the_gap_and_keeps_what_comes_meanwhile() {
    let (mut world, omid, mina, chat) = pair();
    world.phones[mina].lost_list.push("message");
    tell(&mut world, omid, chat, "one");
    assert!(world.texts(mina, chat).is_empty());
    let pages = world.sent_frames(mina, "sync");
    world.say(omid, chat, "two");
    world.say(omid, chat, "three");
    world.settle();
    assert_eq!(world.texts(mina, chat), vec!["one", "two", "three"]);
    // One `sync` of this chat alone, from its last row.
    assert_eq!(world.sent_frames(mina, "sync"), pages + 1);
    assert!(world.errors(mina).is_empty(), "{:?}", world.errors(mina));
}

#[test]
fn a_refused_sync_is_reported_and_runs_again_on_the_next_connection() {
    let (mut world, omid, mina, chat) = pair();
    // A first reconnect pages the self chat's notice, so the pages below are this chat's alone.
    world.cut(mina);
    world.advance(2 * SECOND);
    world.offline(mina);
    tell(&mut world, omid, chat, "one");
    tell(&mut world, omid, chat, "two");
    world.phones[mina]
        .frame_faults
        .push(("sync", Fault::Refuse("internal")));
    world.online(mina);
    assert_eq!(error_count(&world, mina, "internal"), 1);
    // Nothing came, not even the chat list.
    assert!(world.texts(mina, chat).is_empty());
    world.cut(mina);
    world.advance(2 * SECOND);
    assert_eq!(world.texts(mina, chat), vec!["one", "two"]);
    assert_eq!(error_count(&world, mina, "internal"), 1);
}

#[test]
fn a_reloaded_device_behind_on_keys_catches_up_through_its_sync() {
    let (mut world, omid, mina, chat) = pair();
    world.offline(mina);
    world.advance(8 * DAY);
    let mut expected = Vec::new();
    for n in 0..60 {
        let text = format!("k{n}");
        tell(&mut world, omid, chat, &text);
        expected.push(text);
    }
    // Read on her other device, so this one starts at the newest and has to look back.
    world.seen_elsewhere(mina, chat);
    let mina = world.reload(mina);
    world.settle();
    // The commit and the messages after it span more than one page.
    assert!(world.sent_frames(mina, "sync") >= 2);
    assert!(world.errors(mina).is_empty(), "{:?}", world.errors(mina));
    // The `sync` opened all of them on its way.
    assert_eq!(world.texts(mina, chat), expected);
    tell(&mut world, mina, chat, "caught up");
    assert_eq!(world.texts(omid, chat), vec!["caught up"]);
}

#[test]
fn a_refused_sync_after_a_reload_is_reported_and_runs_again_on_the_next_connection() {
    let (mut world, omid, mina, chat) = pair();
    world.offline(mina);
    world.advance(8 * DAY);
    tell(&mut world, omid, chat, "rotated");
    world.seen_elsewhere(mina, chat);
    let mina = world.reload(mina);
    world.phones[mina]
        .frame_faults
        .push(("sync", Fault::Refuse("internal")));
    world.settle();
    assert_eq!(error_count(&world, mina, "internal"), 1);
    // Nothing came, so the key change and the text wait.
    assert!(world.texts(mina, chat).is_empty());
    world.cut(mina);
    world.advance(2 * SECOND);
    world.command(mina, Command::OpenChat { chat });
    world.settle();
    assert_eq!(world.texts(mina, chat), vec!["rotated"]);
    assert_eq!(error_count(&world, mina, "internal"), 1);
}

fn unknown_message(world: &World, chat: Ulid) -> MessageFrame {
    MessageFrame {
        chat,
        seq: 0,
        device: Some(Ulid::from_bytes([9; 16])),
        kind: Some(Kind::Content),
        content_kind: Some(ContentKind::Text),
        encrypted: Some(true),
        epoch: Some(0),
        pointer_seq: None,
        client_sent_at: Some(world.now),
        server_saved_at: Some(world.now),
        body: Some(Bytes(vec![1, 2, 3])),
        signature: Some(Bytes(vec![4; 64])),
    }
}

fn problems(world: &World, phone: usize, chat: Ulid) -> Vec<Problem> {
    let mut list = Vec::new();
    for event in &world.phones[phone].events {
        if let Event::Message { message } = event
            && message.chat == chat
            && let Some(problem) = message.problem
        {
            list.push(problem);
        }
    }
    list
}

#[test]
fn a_registration_whose_answer_was_lost_is_picked_up_when_it_goes_again() {
    let mut world = World::new();
    let omid = world.sign_up("omid");
    world.fail_rest(omid, "POST /api/v1/devices", RestFault::Lose);
    world.command(
        omid,
        Command::SetWords {
            words: Secret(OMID_WORDS.to_string()),
        },
    );
    world.settle();
    assert!(world.phones[omid].client.device().is_none());
    world.advance(2 * SECOND);
    let own = world.self_chat(world.account(omid));
    assert!(world.ready(omid, own), "{:?}", world.errors(omid));
    assert_eq!(world.asked(omid, "POST /api/v1/devices"), 2);
    assert_eq!(world.live_devices(world.account(omid)), 1);
    assert!(world.errors(omid).is_empty(), "{:?}", world.errors(omid));
}

#[test]
fn a_device_id_that_expired_while_nothing_answered_is_replaced() {
    let mut world = World::new();
    let omid = world.sign_up("omid");
    world.fail_rest(omid, "POST /api/v1/devices", RestFault::Answer(0, ""));
    world.command(
        omid,
        Command::SetWords {
            words: Secret(OMID_WORDS.to_string()),
        },
    );
    world.settle();
    world.issued.clear();
    world.advance(2 * SECOND);
    let own = world.self_chat(world.account(omid));
    assert!(world.ready(omid, own), "{:?}", world.errors(omid));
    assert_eq!(world.asked(omid, "POST /api/v1/devices/id"), 2);
    assert!(world.errors(omid).is_empty(), "{:?}", world.errors(omid));
}

#[test]
fn other_words_for_an_account_that_has_its_keys_are_refused() {
    let mut world = World::new();
    world.person("omid", OMID_WORDS);
    let second = world.log_in("omid");
    world.command(
        second,
        Command::SetWords {
            words: Secret(MINA_WORDS.to_string()),
        },
    );
    world.settle();
    assert_eq!(world.errors(second), vec!["keys_set"]);
    assert!(world.phones[second].client.device().is_none());
    assert_eq!(world.asked(second, "POST /api/v1/devices/id"), 0);
}

#[test]
fn a_chat_with_no_group_info_is_not_joined_and_what_waited_fails() {
    let (mut world, omid, mina, _) = pair();
    let account_list = vec![world.account(omid), world.account(mina)];
    let empty = world.new_chat(ChatType::Direct, account_list);
    world.cut(mina);
    let local = world.say(mina, empty, "lost");
    world.advance(2 * SECOND);
    assert_eq!(world.failed(mina, local).as_deref(), Some("not_joined"));
    assert!(!world.ready(mina, empty));
    assert_eq!(
        world.asked(mina, &format!("GET /api/v1/chats/{empty}/group_info")),
        1
    );
}

#[test]
fn a_self_chat_with_no_group_info_is_started_again_by_a_new_device() {
    let mut world = World::new();
    let omid = world.person("omid", OMID_WORDS);
    let own = world.self_chat(world.account(omid));
    let index = world.chat_index(own).unwrap();
    world.chats[index].group_info = None;
    world.chats[index].epoch = None;
    let second = world.log_in("omid");
    world.restore(second, OMID_WORDS);
    assert!(world.ready(second, own), "{:?}", world.errors(second));
    assert!(world.hub_chat(own).group_info.is_some());
}

#[test]
fn a_start_with_no_key_package_for_the_other_person_fails_and_can_be_tried_again() {
    let mut world = World::new();
    let omid = world.person("omid", OMID_WORDS);
    let mina = world.person("mina", MINA_WORDS);
    let mina_device = world.device_of(mina);
    world.packages.retain(|(owner, _)| *owner != mina_device);
    let account = world.account(mina);
    world.command(omid, Command::StartChat { account });
    world.settle();
    assert_eq!(world.errors(omid), vec!["no_key_packages"]);
    let chat = world.direct_chat(world.account(omid), account).unwrap();
    assert!(!world.ready(omid, chat));
    // Mina's next connection tops her packages up, and starting again works.
    world.cut(mina);
    world.advance(2 * SECOND);
    world.command(omid, Command::StartChat { account });
    world.settle();
    assert!(world.ready(omid, chat), "{:?}", world.errors(omid));
    assert!(world.ready(mina, chat), "{:?}", world.errors(mina));
}

#[test]
fn a_start_goes_ahead_when_this_accounts_other_devices_have_no_key_package_left() {
    let mut world = World::new();
    let omid = world.person("omid", OMID_WORDS);
    let second = world.log_in("omid");
    world.restore(second, OMID_WORDS);
    let mina = world.person("mina", MINA_WORDS);
    let (first_device, second_device) = (world.device_of(omid), world.device_of(second));
    world
        .packages
        .retain(|(owner, _)| *owner != first_device && *owner != second_device);
    let chat = world.start(omid, mina);
    assert!(!world.ready(second, chat));
    // The other device joins by itself on its next connection.
    world.cut(second);
    world.advance(2 * SECOND);
    assert!(world.ready(second, chat), "{:?}", world.errors(second));
    tell(&mut world, mina, chat, "to both");
    assert_eq!(world.texts(omid, chat), vec!["to both"]);
    assert_eq!(world.texts(second, chat), vec!["to both"]);
}

#[test]
fn a_refused_device_list_shows_what_waited_for_it_as_from_an_unknown_device() {
    let (mut world, _, mina, chat) = pair();
    world.fail_rest(
        mina,
        &format!("GET /api/v1/chats/{chat}/devices"),
        RestFault::Answer(403, "not_member"),
    );
    let message = unknown_message(&world, chat);
    world.append(message);
    world.settle();
    assert_eq!(problems(&world, mina, chat), vec![Problem::UnknownDevice]);
    assert_eq!(world.errors(mina), vec!["not_member"]);
}

#[test]
fn logging_in_again_on_a_device_ended_meanwhile_wipes_it_and_asks_for_the_words() {
    let mut world = World::new();
    let omid = world.person("omid", OMID_WORDS);
    world.fail_rest(
        omid,
        "POST /api/v1/key_packages",
        RestFault::Answer(401, "unauthorized"),
    );
    world.cut(omid);
    world.advance(2 * SECOND);
    assert!(has_login(&world, omid));
    world.end_device(world.device_of(omid), vec![9; 64]);
    world.command(
        omid,
        Command::LogIn {
            username: "omid".to_string(),
            password: Secret("a long enough password".to_string()),
            captcha: None,
        },
    );
    world.settle();
    assert!(world.phones[omid].wiped);
    assert!(
        world.phones[omid]
            .events
            .contains(&Event::NeedWords { restore: true })
    );
    assert!(world.phones[omid].client.device().is_none());
}

#[test]
fn a_key_change_refused_for_good_fails_what_waited_and_the_next_send_tries_again() {
    let (mut world, omid, mina, chat) = pair();
    world.advance(8 * DAY);
    world.phones[omid]
        .frame_faults
        .push(("commit", Fault::Refuse("forbidden")));
    let first = tell(&mut world, omid, chat, "refused");
    assert_eq!(world.failed(omid, first).as_deref(), Some("forbidden"));
    assert_eq!(world.errors(omid), vec!["forbidden"]);
    let second = tell(&mut world, omid, chat, "goes");
    assert!(world.sent_seq(omid, second).is_some());
    assert_eq!(world.texts(mina, chat), vec!["goes"]);
}

#[test]
fn a_key_change_whose_state_cannot_be_saved_is_never_sent() {
    let (mut world, omid, mina, chat) = pair();
    world.advance(8 * DAY);
    let commits = world.sent_frames(omid, "commit");
    world.phones[omid].failing_saves = 1;
    let first = tell(&mut world, omid, chat, "unsaved");
    assert_eq!(world.failed(omid, first).as_deref(), Some("save_failed"));
    assert_eq!(world.sent_frames(omid, "commit"), commits);
    assert_eq!(world.errors(omid), vec!["save_failed"]);
    let second = tell(&mut world, omid, chat, "saved");
    assert!(world.sent_seq(omid, second).is_some());
    assert_eq!(world.texts(mina, chat), vec!["saved"]);
}

#[test]
fn a_start_whose_group_info_is_refused_fails_and_starting_again_works() {
    let mut world = World::new();
    let omid = world.person("omid", OMID_WORDS);
    let mina = world.person("mina", MINA_WORDS);
    world.phones[omid]
        .frame_faults
        .push(("group_info", Fault::Refuse("internal")));
    let account = world.account(mina);
    world.command(omid, Command::StartChat { account });
    world.settle();
    assert_eq!(world.errors(omid), vec!["internal"]);
    let chat = world.direct_chat(world.account(omid), account).unwrap();
    assert!(!world.ready(omid, chat));
    assert!(!world.ready(mina, chat));
    world.command(omid, Command::StartChat { account });
    world.settle();
    assert!(world.ready(omid, chat), "{:?}", world.errors(omid));
    assert!(world.ready(mina, chat), "{:?}", world.errors(mina));
    tell(&mut world, omid, chat, "started");
    assert_eq!(world.texts(mina, chat), vec!["started"]);
}

#[test]
fn a_start_whose_first_wraps_are_refused_goes_on_to_add_the_other_person() {
    let mut world = World::new();
    let omid = world.person("omid", OMID_WORDS);
    let mina = world.person("mina", MINA_WORDS);
    world.phones[omid]
        .frame_faults
        .push(("wraps", Fault::Refuse("internal")));
    let account = world.account(mina);
    world.command(omid, Command::StartChat { account });
    world.settle();
    assert_eq!(world.errors(omid), vec!["internal"]);
    let chat = world.direct_chat(world.account(omid), account).unwrap();
    assert!(world.ready(omid, chat), "{:?}", world.errors(omid));
    assert!(world.ready(mina, chat), "{:?}", world.errors(mina));
    tell(&mut world, omid, chat, "started");
    assert_eq!(world.texts(mina, chat), vec!["started"]);
    // A device of Mina's restoring later still opens it, from the wraps the adding commit carried.
    let second = world.log_in("mina");
    world.restore(second, MINA_WORDS);
    world.command(second, Command::OpenChat { chat });
    world.settle();
    assert_eq!(world.texts(second, chat), vec!["started"]);
}

#[test]
fn a_refused_seen_is_reported_once_and_not_sent_again() {
    let (mut world, _, mina, chat) = pair();
    world.command(
        mina,
        Command::Seen {
            chat,
            up_to_seq: 99,
        },
    );
    world.settle();
    assert_eq!(world.errors(mina), vec!["bad_seen"]);
    let seen = world.sent_frames(mina, "seen");
    world.cut(mina);
    world.advance(2 * SECOND);
    assert_eq!(world.sent_frames(mina, "seen"), seen);
    assert_eq!(world.errors(mina), vec!["bad_seen"]);
}

#[test]
fn older_pages_load_from_the_archive_until_the_first_message_even_offline() {
    let (mut world, omid, mina, chat) = pair();
    let mut expected = Vec::new();
    for n in 0..70 {
        let text = format!("o{n}");
        tell(&mut world, omid, chat, &text);
        expected.push(text);
    }
    world.advance(2 * SECOND);
    tell(&mut world, omid, chat, "later");
    expected.push("later".to_string());
    world.see_all(mina, chat);
    let mina = world.reload(mina);
    world.settle();
    // The reload's `sync` brought only the second before its last kept row.
    assert_eq!(world.texts(mina, chat), vec!["later"]);
    world.command(mina, Command::OpenChat { chat });
    world.settle();
    assert_eq!(world.texts(mina, chat), expected[21..].to_vec());
    world.offline(mina);
    world.command(mina, Command::LoadOlder { chat });
    world.settle();
    assert_eq!(world.texts(mina, chat), expected);
    world.online(mina);
    world.command(mina, Command::LoadOlder { chat });
    world.settle();
    let mut loaded = Vec::new();
    for event in &world.phones[mina].events {
        if let Event::PageLoaded { has_older, .. } = event {
            loaded.push(*has_older);
        }
    }
    assert_eq!(loaded.last(), Some(&false));
    assert!(world.errors(mina).is_empty(), "{:?}", world.errors(mina));
}

#[test]
fn edits_and_deletes_reach_the_other_side_and_a_deleted_message_stays_removed() {
    let (mut world, omid, mina, chat) = pair();
    // The edit points at a message still waiting for its sequence id.
    world.phones[omid].frame_faults.push(("send", Fault::Lose));
    let first = world.say(omid, chat, "one");
    world.command(
        omid,
        Command::Edit {
            chat,
            target: Pointer::Local(first),
            text: Secret("uno".to_string()),
            files: Vec::new(),
            remove: Vec::new(),
        },
    );
    world.settle();
    world.cut(omid);
    world.advance(2 * SECOND);
    let seq = world.sent_seq(omid, first).unwrap();
    let mut edited = Vec::new();
    for event in &world.phones[mina].events {
        if let Event::Edited {
            message,
            target_seq,
        } = event
        {
            edited.push((*target_seq, message.text.as_ref().unwrap().0.clone()));
        }
    }
    assert_eq!(edited, vec![(seq, "uno".to_string())]);
    world.command(
        omid,
        Command::Delete {
            chat,
            target: Pointer::Seq(seq),
            remove: Vec::new(),
        },
    );
    world.settle();
    let mut deleted = Vec::new();
    for event in &world.phones[mina].events {
        if let Event::Deleted { target_seq, .. } = event {
            deleted.push(*target_seq);
        }
    }
    assert_eq!(deleted, vec![seq]);
    world.see_all(mina, chat);
    let mina = world.reload(mina);
    world.settle();
    world.command(mina, Command::OpenChat { chat });
    world.settle();
    let mut removed = Vec::new();
    for event in &world.phones[mina].events {
        if let Event::Removed { seq, .. } = event {
            removed.push(*seq);
        }
    }
    // Shown by the reload's `sync`, then again when the chat opened from the archive.
    removed.sort_unstable();
    removed.dedup();
    assert_eq!(removed, vec![seq, seq + 1]);
    assert!(world.texts(mina, chat).is_empty());
}

#[test]
fn a_revocation_from_an_unknown_account_is_checked_once_its_keys_arrive() {
    let (mut world, _, mina, _) = pair();
    let sara = world.person("sara", WORDS);
    let (account, device) = (world.account(sara), world.device_of(sara));
    let statement = sign_revocation(&derive(WORDS).unwrap(), &account, &device);
    world.push(
        mina,
        ServerFrame::Revoked {
            account,
            device,
            statement: Bytes(statement.to_vec()),
        },
    );
    world.settle();
    assert_eq!(
        world.asked(mina, &format!("GET /api/v1/accounts/{account}")),
        1
    );
    assert!(world.errors(mina).is_empty(), "{:?}", world.errors(mina));
    for statement in [vec![1; 64], vec![1; 3]] {
        world.push(
            mina,
            ServerFrame::Revoked {
                account,
                device,
                statement: Bytes(statement),
            },
        );
    }
    world.settle();
    assert_eq!(world.errors(mina), vec!["bad_signature", "bad_signature"]);
}

#[test]
fn rate_limited_background_requests_wait_and_go_again() {
    let (mut world, omid, _, _) = pair();
    world.fail_rest(
        omid,
        "POST /api/v1/key_packages",
        RestFault::Answer(429, "rate_limited"),
    );
    world.cut(omid);
    world.advance(2 * SECOND);
    let tried = world.asked(omid, "POST /api/v1/key_packages");
    world.advance(5 * SECOND);
    assert!(world.asked(omid, "POST /api/v1/key_packages") > tried);
    assert!(world.errors(omid).is_empty(), "{:?}", world.errors(omid));
}

#[test]
fn a_restore_whose_wraps_are_refused_or_unreadable_shows_what_waited_and_lets_the_words_go() {
    for fault in [RestFault::Answer(403, "not_member"), RestFault::Unreadable] {
        let (mut world, omid, _, chat) = pair();
        tell(&mut world, omid, chat, "history");
        let second = world.log_in("omid");
        world.fail_rest(second, &format!("GET /api/v1/chats/{chat}/wraps"), fault);
        world.restore(second, OMID_WORDS);
        assert!(world.ready(second, chat), "{:?}", world.errors(second));
        assert_eq!(world.errors(second).len(), 1, "{:?}", world.errors(second));
        // The key from before this device joined never came, so the preview shows with no text,
        // and so does the same message again, fetched from the account's seen position.
        let mut shown = Vec::new();
        for event in &world.phones[second].events {
            if let Event::Message { message } = event
                && message.chat == chat
            {
                shown.push(message.text.is_some());
            }
        }
        assert_eq!(shown, vec![false, false]);
        // Nothing else is waiting on the words: a later chat is not restored with them.
        assert_eq!(
            world.asked(second, &format!("GET /api/v1/chats/{chat}/wraps")),
            1
        );
    }
}

#[test]
fn an_unreadable_group_info_fails_the_join_and_releases_what_waited() {
    let (mut world, omid, _, chat) = pair();
    tell(&mut world, omid, chat, "before");
    let second = world.log_in("omid");
    world.fail_rest(
        second,
        &format!("GET /api/v1/chats/{chat}/group_info"),
        RestFault::Unreadable,
    );
    world.restore(second, OMID_WORDS);
    assert!(!world.ready(second, chat));
    assert_eq!(world.errors(second), vec!["bad_reply"]);
    // The device joins the next time it connects.
    world.cut(second);
    world.advance(2 * SECOND);
    assert!(world.ready(second, chat), "{:?}", world.errors(second));
}

#[test]
fn a_key_change_whose_answer_was_lost_is_found_and_one_that_was_dropped_goes_again() {
    for fault in [Fault::Lose, Fault::Drop] {
        let (mut world, omid, mina, chat) = pair();
        world.advance(8 * DAY);
        let commits = commit_count(&world, chat);
        world.phones[omid].frame_faults.push(("commit", fault));
        let local = tell(&mut world, omid, chat, "after the change");
        assert!(world.sent_seq(omid, local).is_none());
        world.cut(omid);
        world.advance(2 * SECOND);
        assert!(world.sent_seq(omid, local).is_some());
        assert_eq!(commit_count(&world, chat), commits + 1);
        assert_eq!(world.texts(mina, chat), vec!["after the change"]);
        assert!(world.errors(omid).is_empty(), "{:?}", world.errors(omid));
    }
}

#[test]
fn a_reload_after_a_key_change_whose_answer_never_came_rejoins_and_carries_on() {
    let (mut world, omid, mina, chat) = pair();
    world.advance(8 * DAY);
    world.phones[omid]
        .frame_faults
        .push(("commit", Fault::Lose));
    let lost = tell(&mut world, omid, chat, "lost with the queue");
    assert!(world.sent_seq(omid, lost).is_none());
    let omid = world.reload(omid);
    world.settle();
    // Its own commit came from a state it no longer has, so it joined again.
    assert!(world.ready(omid, chat), "{:?}", world.errors(omid));
    let local = tell(&mut world, omid, chat, "after the reload");
    assert!(world.sent_seq(omid, local).is_some());
    assert_eq!(world.texts(mina, chat), vec!["after the reload"]);
    tell(&mut world, mina, chat, "read");
    assert_eq!(world.texts(omid, chat), vec!["read"]);
}

#[test]
fn live_messages_arriving_during_a_sync_are_kept_and_shown_after_it() {
    let (mut world, omid, mina, chat) = pair();
    // A first reconnect syncs the self chat's notice, so the sync below brings this chat's alone.
    world.cut(mina);
    world.advance(2 * SECOND);
    world.offline(mina);
    tell(&mut world, omid, chat, "one");
    world.phones[mina].network = true;
    world.command(mina, Command::NetworkBack);
    let mut filling = false;
    while !filling {
        assert!(world.step_one(mina));
        for frame in &world.phones[mina].sent {
            if let ClientFrame::Sync { since, .. } = frame
                && since.is_some()
            {
                filling = true;
            }
        }
    }
    world.say(omid, chat, "two");
    while world.phones[mina].inbox.is_empty() {
        assert!(world.step_one(omid));
    }
    let pages = world.sent_frames(mina, "sync");
    world.settle();
    assert_eq!(world.texts(mina, chat), vec!["one", "two"]);
    // The live one followed what the sync brought, so it needed no sync of its own.
    assert_eq!(world.sent_frames(mina, "sync"), pages + 1);
}

#[test]
fn a_sync_lost_with_the_connection_is_asked_again() {
    let (mut world, omid, mina, chat) = pair();
    world.offline(mina);
    for n in 0..60 {
        tell(&mut world, omid, chat, &format!("p{n}"));
    }
    world.phones[mina].frame_faults.push(("sync", Fault::Drop));
    world.online(mina);
    assert!(world.texts(mina, chat).is_empty());
    world.cut(mina);
    world.advance(2 * SECOND);
    assert_eq!(world.texts(mina, chat).len(), 60);
    assert!(world.errors(mina).is_empty(), "{:?}", world.errors(mina));
}

#[test]
fn a_device_whose_stamp_does_not_verify_is_refused_and_its_messages_are_not_trusted() {
    let (mut world, omid, mina, chat) = pair();
    let index = world.device_index(world.device_of(omid)).unwrap();
    world.devices[index].stamp = vec![7; 64];
    let mina = world.reload_cold(mina);
    world.settle();
    tell(&mut world, omid, chat, "unproven");
    world.command(mina, Command::OpenChat { chat });
    world.settle();
    assert!(world.errors(mina).contains(&"bad_device".to_string()));
    assert!(world.texts(mina, chat).is_empty());
    assert!(problems(&world, mina, chat).contains(&Problem::UnknownDevice));
}

#[test]
fn logging_in_as_another_account_on_a_device_is_refused() {
    let (mut world, omid, _, _) = pair();
    world.fail_rest(
        omid,
        "POST /api/v1/key_packages",
        RestFault::Answer(401, "unauthorized"),
    );
    world.cut(omid);
    world.advance(2 * SECOND);
    world.command(
        omid,
        Command::LogIn {
            username: "mina".to_string(),
            password: Secret("a long enough password".to_string()),
            captcha: None,
        },
    );
    world.settle();
    assert!(world.errors(omid).contains(&"wrong_account".to_string()));
    assert!(!world.phones[omid].wiped);
    assert!(!world.phones[omid].up);
}

#[test]
fn a_lowered_device_limit_stops_a_device_until_another_is_terminated() {
    let mut world = World::new();
    let first = world.person("omid", OMID_WORDS);
    let second = world.log_in("omid");
    world.restore(second, OMID_WORDS);
    world.max_devices = Some(1);
    world.cut(second);
    world.advance(2 * SECOND);
    assert!(has_limit(&world, second));
    let connects = world.phones[second].connects;
    world.advance(60 * SECOND);
    assert_eq!(world.phones[second].connects, connects);
    world.command(
        second,
        Command::Terminate {
            device: world.device_of(first),
            words: Secret(OMID_WORDS.to_string()),
        },
    );
    world.settle();
    assert!(world.phones[second].up);
    assert!(world.phones[first].wiped);

    // A request refused for the limit stops the device the same way.
    let (mut world, omid, _, _) = pair();
    world.fail_rest(
        omid,
        "POST /api/v1/key_packages",
        RestFault::Answer(403, "device_limit"),
    );
    world.cut(omid);
    world.advance(2 * SECOND);
    assert!(has_limit(&world, omid));
}

#[test]
fn small_refusals_say_what_failed_and_leave_the_rest_working() {
    let (mut world, omid, mina, chat) = pair();
    // A refused message fails with the server's code.
    world.phones[omid]
        .frame_faults
        .push(("send", Fault::Refuse("too_large")));
    let refused = tell(&mut world, omid, chat, "too much");
    assert_eq!(world.failed(omid, refused).as_deref(), Some("too_large"));
    // A refused chat start can be asked again.
    world.fail_rest(
        omid,
        "POST /api/v1/chats",
        RestFault::Answer(403, "forbidden"),
    );
    world.command(
        omid,
        Command::StartChat {
            account: world.account(mina),
        },
    );
    world.settle();
    world.command(
        omid,
        Command::StartChat {
            account: world.account(mina),
        },
    );
    world.settle();
    assert_eq!(world.asked(omid, "POST /api/v1/chats"), 3);
    // Starting the self chat again finds it ready.
    let own = world.self_chat(world.account(omid));
    world.command(
        omid,
        Command::StartChat {
            account: world.account(omid),
        },
    );
    world.settle();
    let mut ready = 0;
    for event in &world.phones[omid].events {
        if *event == (Event::ChatReady { chat: own }) {
            ready += 1;
        }
    }
    assert_eq!(ready, 2);
    // A wrong password, the chat list, typing and the device list.
    let stranger = world.phone(Auth::Token);
    world.command(
        stranger,
        Command::LogIn {
            username: "nobody".to_string(),
            password: Secret("a long enough password".to_string()),
            captcha: None,
        },
    );
    world.command(mina, Command::ChatList);
    world.command(mina, Command::Typing { chat, on: true });
    world.command(omid, Command::Devices);
    world.settle();
    assert_eq!(world.errors(stranger), vec!["bad_login"]);
    assert!(world.phones[omid].events.contains(&Event::Typing {
        chat,
        account: world.account(mina),
        on: true,
    }));
    assert!(
        world.phones[omid]
            .events
            .iter()
            .any(|event| matches!(event, Event::DeviceList { .. }))
    );
    assert_eq!(world.errors(omid), vec!["forbidden"]);
}

#[test]
fn a_welcome_from_someone_whose_master_key_changed_is_refused() {
    let (mut world, omid, mina, chat) = pair();
    // Someone claiming Omid's account under other words starts a chat with one of Mina's packages.
    let omid_account = world.account(omid);
    let mina_account = world.account(mina);
    let forged = derive(WORDS).unwrap();
    let mina_keys = derive(MINA_WORDS).unwrap();
    let account_list = vec![
        AccountKey {
            account: omid_account,
            account_public: *forged.account_public(),
            master_public: *forged.master_public(),
            stamp: stamp_account_key(&forged, &omid_account),
        },
        AccountKey {
            account: mina_account,
            account_public: *mina_keys.account_public(),
            master_public: *mina_keys.master_public(),
            stamp: stamp_account_key(&mina_keys, &mina_account),
        },
    ];
    let mut forger = Device::create(&forged, omid_account, Ulid::new().unwrap()).unwrap();
    let other = Ulid::new().unwrap();
    forger.create_chat(other, world.now, &account_list).unwrap();
    let mina_device = world.device_of(mina);
    let mut package = None;
    for (owner, body) in &world.packages {
        if *owner == mina_device {
            package = Some(body.clone());
        }
    }
    let commit = forger
        .add_devices(other, &[package.unwrap()], &account_list)
        .unwrap();
    world.push(
        mina,
        ServerFrame::Welcome {
            chat: other,
            body: Bytes(commit.welcome.unwrap()),
        },
    );
    world.settle();
    assert_eq!(world.errors(mina), vec!["bad_welcome"]);
    assert!(!world.ready(mina, other));
    tell(&mut world, mina, chat, "still here");
    assert_eq!(world.texts(omid, chat), vec!["still here"]);
}

#[test]
fn a_device_whose_saved_state_fell_behind_catches_up_when_a_newer_key_change_arrives() {
    let (mut world, omid, mina, chat) = pair();
    tell(&mut world, omid, chat, "before");
    let old = world.phones[mina].slots.clone();
    // Two new devices of Omid's join: two key changes with no message after them.
    for _ in 0..2 {
        let more = world.log_in("omid");
        world.restore(more, OMID_WORDS);
        assert!(world.ready(more, chat), "{:?}", world.errors(more));
    }
    world.see_all(mina, chat);
    // Both slots hold the state from before those joins, and the list does not say how far the
    // chat has moved. The old state's `since` is older too, so its `sync` brings the key changes.
    world.phones[mina].slots = old;
    world.hide_epoch = true;
    let mina = world.reload(mina);
    world.settle();
    assert_eq!(world.texts(mina, chat), vec!["before"]);
    world.advance(8 * DAY);
    tell(&mut world, omid, chat, "after");
    assert!(world.errors(mina).is_empty(), "{:?}", world.errors(mina));
    assert_eq!(world.texts(mina, chat), vec!["before", "after"]);
    tell(&mut world, mina, chat, "caught up");
    assert_eq!(world.texts(omid, chat), vec!["caught up"]);
}

#[test]
fn an_unencrypted_message_shows_as_such_and_one_with_no_device_is_malformed() {
    let (mut world, omid, mina, chat) = pair();
    let [first, second] = world.phones[omid].slots.clone();
    let device = Device::import(first.as_deref(), second.as_deref())
        .unwrap()
        .device;
    let mut message = MessageFrame {
        chat,
        seq: 0,
        device: Some(world.device_of(omid)),
        kind: Some(Kind::Content),
        content_kind: Some(ContentKind::Text),
        encrypted: Some(false),
        epoch: world.hub_chat(chat).epoch,
        pointer_seq: None,
        client_sent_at: Some(world.now),
        server_saved_at: Some(world.now),
        body: Some(Bytes(br#"{"text":"in the clear"}"#.to_vec())),
        signature: None,
    };
    let signature = device
        .sign(
            &message.header().unwrap(),
            &message.body.as_ref().unwrap().0,
        )
        .unwrap();
    message.signature = Some(Bytes(signature.to_vec()));
    let clear = world.append(message.clone());
    message.device = None;
    let malformed = world.append(message);
    world.settle();
    let mut shown = Vec::new();
    for event in &world.phones[mina].events {
        if let Event::Message { message } = event
            && message.chat == chat
        {
            shown.push((
                message.seq,
                message.encrypted,
                message.text.as_ref().map(|text| text.0.clone()),
                message.problem,
            ));
        }
    }
    assert_eq!(
        shown,
        vec![
            (clear, false, Some("in the clear".to_string()), None),
            (malformed, false, None, Some(Problem::Malformed)),
        ]
    );
}

#[test]
fn a_device_removed_by_a_commit_keeps_what_it_could_read_stops_and_rejoins_on_reconnect() {
    let (mut world, omid, mina, chat) = pair();
    tell(&mut world, omid, chat, "before");
    // Another device of Omid's removes Mina's device alone, which keryx itself never does.
    let more = world.log_in("omid");
    world.restore(more, OMID_WORDS);
    assert!(world.ready(more, chat), "{:?}", world.errors(more));
    let [first, second] = world.phones[more].slots.clone();
    let mut device = Device::import(first.as_deref(), second.as_deref())
        .unwrap()
        .device;
    let mut account_list = Vec::new();
    for (phone, words) in [(omid, OMID_WORDS), (mina, MINA_WORDS)] {
        let keys = derive(words).unwrap();
        account_list.push(AccountKey {
            account: world.account(phone),
            account_public: *keys.account_public(),
            master_public: *keys.master_public(),
            stamp: stamp_account_key(&keys, &world.account(phone)),
        });
    }
    let removal = device
        .remove_devices(chat, &[world.device_of(mina)], &account_list)
        .unwrap();
    let index = world.chat_index(chat).unwrap();
    world.chats[index].epoch = Some(removal.epoch);
    world.chats[index].group_info = Some((removal.epoch, removal.group_info));
    let commits = world.sent_frames(mina, "commit");
    let join_info = format!("GET /api/v1/chats/{chat}/group_info");
    let asked = world.asked(mina, &join_info);
    world.append(MessageFrame {
        chat,
        seq: 0,
        device: Some(world.device_of(more)),
        kind: Some(Kind::Commit),
        content_kind: None,
        encrypted: Some(true),
        epoch: Some(removal.epoch),
        pointer_seq: None,
        client_sent_at: Some(world.now),
        server_saved_at: Some(world.now),
        body: Some(Bytes(removal.commit)),
        signature: None,
    });
    world.settle();
    assert!(
        world.phones[mina]
            .events
            .contains(&Event::ChatLeft { chat })
    );
    // It stops: no join until the next connection.
    assert_eq!(world.sent_frames(mina, "commit"), commits);
    assert_eq!(world.asked(mina, &join_info), asked);
    assert!(world.errors(mina).is_empty(), "{:?}", world.errors(mina));
    // The keys it had stay, so what it could read still opens.
    let mina = world.reload(mina);
    world.settle();
    world.command(mina, Command::OpenChat { chat });
    world.settle();
    assert!(world.texts(mina, chat).contains(&"before".to_string()));
    // Its account is still in the chat, so it joins again.
    assert!(world.ready(mina, chat), "{:?}", world.errors(mina));
    assert_eq!(world.asked(mina, &join_info), 1);
}

#[test]
fn messages_from_this_account_are_marked_as_its_own_after_a_reload() {
    let (mut world, omid, mina, chat) = pair();
    tell(&mut world, omid, chat, "from omid");
    tell(&mut world, mina, chat, "from mina");
    // Omid never sent a seen, so after the list's preview a reload fetches both, his own included.
    let omid = world.reload(omid);
    world.settle();
    let mut shown = Vec::new();
    for event in &world.phones[omid].events {
        if let Event::Message { message } = event
            && message.chat == chat
        {
            shown.push((message.text.as_ref().unwrap().0.clone(), message.mine));
        }
    }
    assert_eq!(
        shown,
        vec![
            ("from mina".to_string(), false),
            ("from omid".to_string(), true),
            ("from mina".to_string(), false)
        ]
    );
}

fn spec(handle: u64, kind: ContentKind, size: u64) -> FileSpec {
    FileSpec {
        handle,
        kind,
        name: format!("file {handle}"),
        mime: "application/octet-stream".to_string(),
        size,
        width: None,
        height: None,
        thumbnail: None,
        description: None,
        title: None,
        artist: None,
        duration: None,
    }
}

/// The files `p` was shown in a chat, by sequence id, the latest event for each winning.
fn shown_files(
    world: &World,
    p: usize,
    chat: Ulid,
) -> BTreeMap<u64, (Option<String>, Vec<FileInfo>)> {
    let mut by_seq = BTreeMap::new();
    for event in &world.phones[p].events {
        // An edit replaces what its target shows.
        let (message, seq) = match event {
            Event::Message { message } => (message, message.seq),
            Event::Edited {
                message,
                target_seq,
            } => (message, *target_seq),
            _ => continue,
        };
        if message.chat == chat && !message.files.is_empty() {
            let text = message.text.as_ref().map(|text| text.0.clone());
            by_seq.insert(seq, (text, message.files.clone()));
        }
    }
    by_seq
}

fn progress(world: &World, p: usize, handle: u64) -> Vec<(u64, u64)> {
    let mut list = Vec::new();
    for event in &world.phones[p].events {
        if let Event::Progress {
            handle: of,
            done,
            total,
        } = event
            && *of == handle
        {
            list.push((*done, *total));
        }
    }
    list
}

fn transfer_failed(world: &World, p: usize, handle: u64) -> Option<String> {
    let mut found = None;
    for event in &world.phones[p].events {
        if let Event::TransferFailed { handle: of, code } = event
            && *of == handle
        {
            found = Some(code.clone());
        }
    }
    found
}

fn downloaded(world: &World, p: usize, handle: u64) -> bool {
    world.phones[p]
        .events
        .contains(&Event::Downloaded { handle })
}

#[test]
fn a_file_goes_up_in_chunks_after_text_sent_meanwhile_and_comes_down_whole() {
    let (mut world, omid, mina, chat) = pair();
    let bytes: Vec<u8> = (0..file::CHUNK + 10).map(|at| (at % 251) as u8).collect();
    world.phones[omid].held.insert(1, bytes.clone());
    // Nothing answers the first put, and the second's answer is lost after it was stored, so
    // the same chunk goes again and is taken as the last one sent again.
    world.fail_rest(omid, "PUT /api/v1/files/*", RestFault::Answer(0, ""));
    world.fail_rest(omid, "PUT /api/v1/files/*", RestFault::Lose);
    let mut picture = spec(1, ContentKind::Image, bytes.len() as u64);
    picture.width = Some(640);
    picture.height = Some(480);
    picture.thumbnail = Some(Thumbnail {
        width: 4,
        height: 4,
        rgba: vec![120; 4 * 4 * 4],
    });
    world.command(
        omid,
        Command::SendFiles {
            chat,
            files: vec![picture],
            text: Some(Secret("look".to_string())),
        },
    );
    tell(&mut world, omid, chat, "while it goes up");
    world.advance(120_000);
    world.settle();
    assert_eq!(progress(&world, omid, 1), vec![(0, 2), (1, 2), (2, 2)]);
    assert!(world.errors(omid).is_empty(), "{:?}", world.errors(omid));
    // The text was not held behind the upload, and the caption comes after it.
    assert_eq!(
        world.texts(mina, chat),
        vec!["while it goes up".to_string(), "look".to_string()]
    );
    let files = shown_files(&world, mina, chat);
    let (seq, (caption, list)) = files.iter().next().unwrap();
    assert!(*seq > 1);
    assert_eq!(caption.as_deref(), Some("look"));
    let info = list[0].clone();
    assert_eq!(
        (info.kind, info.size, info.chunks, info.width, info.height),
        (
            ContentKind::Image,
            bytes.len() as u64,
            2,
            Some(640),
            Some(480)
        )
    );
    assert!(info.blurhash.is_some());
    // Mina's first fetch finds nothing answering, and is retried.
    world.fail_rest(mina, "GET /api/v1/files/*", RestFault::Answer(0, ""));
    world.command(
        mina,
        Command::Download {
            file: info,
            handle: 7,
        },
    );
    world.settle();
    world.advance(120_000);
    assert!(downloaded(&world, mina, 7));
    assert_eq!(progress(&world, mina, 7), vec![(0, 2), (1, 2), (2, 2)]);
    assert_eq!(world.phones[mina].held[&7], bytes);
}

#[test]
fn a_bundle_holds_images_and_videos_and_an_edit_and_a_delete_name_the_files_they_drop() {
    let (mut world, omid, mina, chat) = pair();
    for handle in [1, 2] {
        world.phones[omid].held.insert(handle, Vec::new());
    }
    let mut first = spec(1, ContentKind::Image, 0);
    first.description = Some("the first".to_string());
    // Pixels that do not match their size make no BlurHash, and the file still goes.
    first.thumbnail = Some(Thumbnail {
        width: 4,
        height: 4,
        rgba: vec![1, 2, 3],
    });
    let second = spec(2, ContentKind::Video, 0);
    world.command(
        omid,
        Command::SendFiles {
            chat,
            files: vec![first, second],
            text: None,
        },
    );
    world.settle();
    let files = shown_files(&world, mina, chat);
    let (&seq, (caption, list)) = files.iter().next().unwrap();
    assert_eq!(*caption, None);
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].description.as_deref(), Some("the first"));
    assert_eq!(list[0].blurhash, None);
    let mut shown_kind = None;
    for event in &world.phones[mina].events {
        if let Event::Message { message } = event
            && message.seq == seq
            && message.chat == chat
        {
            shown_kind = message.content_kind;
        }
    }
    assert_eq!(shown_kind, Some(ContentKind::MediaBundle));
    let (kept, dropped) = (list[0].clone(), list[1].id);
    // The edit keeps the first and drops the second, which the server then removes.
    world.command(
        omid,
        Command::Edit {
            chat,
            target: Pointer::Seq(seq),
            text: Secret("one left".to_string()),
            files: vec![kept.clone()],
            remove: vec![dropped],
        },
    );
    world.settle();
    let files = shown_files(&world, mina, chat);
    assert_eq!(files[&seq].1, vec![kept.clone()]);
    assert!(
        world
            .files
            .iter()
            .any(|item| item.id == dropped && item.removed)
    );
    world.command(
        mina,
        Command::Download {
            file: list[1].clone(),
            handle: 9,
        },
    );
    world.settle();
    assert_eq!(
        transfer_failed(&world, mina, 9).as_deref(),
        Some("no_such_file")
    );
    world.command(
        omid,
        Command::Delete {
            chat,
            target: Pointer::Seq(seq),
            remove: vec![kept.id],
        },
    );
    world.settle();
    assert!(world.files.iter().all(|item| item.removed));
    assert!(world.errors(omid).is_empty(), "{:?}", world.errors(omid));
}

#[test]
fn music_carries_its_tags_and_cover_and_other_files_do_not() {
    let (mut world, omid, mina, chat) = pair();
    for handle in [1, 2] {
        world.phones[omid].held.insert(handle, vec![7; 10]);
    }
    let tagged = |mut one: FileSpec| {
        one.title = Some("Song".to_string());
        one.artist = Some("Band".to_string());
        one.duration = Some(225_500);
        one
    };
    let mut song = tagged(spec(1, ContentKind::Music, 10));
    song.mime = file::MUSIC_TYPE.to_string();
    song.width = Some(300);
    song.height = Some(300);
    song.thumbnail = Some(Thumbnail {
        width: 4,
        height: 4,
        rgba: vec![90; 4 * 4 * 4],
    });
    let plain = tagged(spec(2, ContentKind::File, 10));
    for one in [song, plain] {
        world.command(
            omid,
            Command::SendFiles {
                chat,
                files: vec![one],
                text: None,
            },
        );
        world.settle();
    }
    assert!(world.errors(omid).is_empty(), "{:?}", world.errors(omid));
    let files = shown_files(&world, mina, chat);
    let mut shown = files.values().map(|(_, list)| list[0].clone());
    let music = shown.next().unwrap();
    assert_eq!(
        (
            music.kind,
            music.mime.as_str(),
            music.title.as_deref(),
            music.artist.as_deref(),
            music.duration,
            music.width
        ),
        (
            ContentKind::Music,
            "audio/mpeg",
            Some("Song"),
            Some("Band"),
            Some(225_500),
            Some(300)
        )
    );
    assert!(music.blurhash.is_some());
    let other = shown.next().unwrap();
    assert_eq!(
        (other.kind, other.title, other.artist, other.duration),
        (ContentKind::File, None, None, None)
    );
}

#[test]
fn a_message_holds_one_file_or_a_bundle_and_a_client_with_no_device_sends_none() {
    let mut client = Client::new(Auth::Cookie, platform());
    let out = command(
        &mut client,
        Command::SendFiles {
            chat: id(),
            files: vec![spec(1, ContentKind::File, 1)],
            text: None,
        },
    );
    assert!(matches!(&events(&out)[0], Event::Error { code, .. } if code == "no_device"));
    let (mut world, omid, _, chat) = pair();
    let image = |handle| spec(handle, ContentKind::Image, 1);
    let mut mp3 = spec(2, ContentKind::Music, 1);
    mp3.mime = file::MUSIC_TYPE.to_string();
    for files in [
        Vec::new(),
        vec![spec(1, ContentKind::Reaction, 1)],
        vec![image(1), spec(2, ContentKind::Doc, 1)],
        (0..21).map(image).collect(),
        // Music is an MP3 only, and never in a bundle.
        vec![spec(1, ContentKind::Music, 1)],
        vec![image(1), mp3],
    ] {
        let before = world.errors(omid).len();
        world.command(
            omid,
            Command::SendFiles {
                chat,
                files,
                text: None,
            },
        );
        assert_eq!(world.errors(omid)[before..], ["bad_files".to_string()]);
    }
    world.settle();
    assert_eq!(
        world.asked(omid, &format!("POST /api/v1/chats/{chat}/files")),
        0
    );
}

#[test]
fn an_upload_that_is_refused_or_read_short_stops_and_its_message_is_never_sent() {
    let (mut world, omid, mina, chat) = pair();
    for handle in 1..=4 {
        world.phones[omid].held.insert(handle, vec![5; 10]);
    }
    let send = |world: &mut World, files: Vec<FileSpec>| {
        world.command(
            omid,
            Command::SendFiles {
                chat,
                files,
                text: None,
            },
        );
        world.settle();
    };
    // Every file of a refused bundle is told.
    let create = format!("POST /api/v1/chats/{chat}/files");
    world.fail_rest(omid, &create, RestFault::Answer(403, "forbidden"));
    send(
        &mut world,
        vec![
            spec(1, ContentKind::Image, 10),
            spec(2, ContentKind::Image, 10),
        ],
    );
    assert_eq!(
        transfer_failed(&world, omid, 1).as_deref(),
        Some("forbidden")
    );
    assert_eq!(
        transfer_failed(&world, omid, 2).as_deref(),
        Some("forbidden")
    );
    world.fail_rest(
        omid,
        "PUT /api/v1/files/*",
        RestFault::Answer(409, "chunk_out_of_order"),
    );
    send(&mut world, vec![spec(3, ContentKind::Doc, 10)]);
    assert_eq!(
        transfer_failed(&world, omid, 3).as_deref(),
        Some("chunk_out_of_order")
    );
    world.phones[omid].short_reads = 1;
    send(&mut world, vec![spec(4, ContentKind::Voice, 10)]);
    assert_eq!(
        transfer_failed(&world, omid, 4).as_deref(),
        Some("read_failed")
    );
    assert!(shown_files(&world, mina, chat).is_empty());
    assert!(world.files.iter().all(|item| !item.used));
    // What answers nothing that was asked is ignored.
    world.input(
        omid,
        Input::ChunkRead {
            handle: 99,
            n: 0,
            bytes: Vec::new(),
        },
    );
    world.input(
        omid,
        Input::Fetched {
            id: 99_999,
            status: 200,
            bytes: Vec::new(),
        },
    );
    assert!(world.errors(omid).is_empty(), "{:?}", world.errors(omid));
}

#[test]
fn a_download_that_is_changed_cut_or_unreadable_stops_with_its_reason() {
    let (mut world, omid, mina, chat) = pair();
    world.phones[omid].held.insert(1, vec![5; 10]);
    world.command(
        omid,
        Command::SendFiles {
            chat,
            files: vec![spec(1, ContentKind::File, 10)],
            text: None,
        },
    );
    world.settle();
    let info = shown_files(&world, mina, chat)
        .into_values()
        .next()
        .unwrap()
        .1
        .remove(0);
    let download = |world: &mut World, handle: u64| {
        world.command(
            mina,
            Command::Download {
                file: info.clone(),
                handle,
            },
        );
        world.settle();
    };
    world.garble_chunks = true;
    download(&mut world, 1);
    assert_eq!(
        transfer_failed(&world, mina, 1).as_deref(),
        Some("bad_file")
    );
    world.garble_chunks = false;
    world.fail_rest(mina, "GET /api/v1/files/*", RestFault::Unreadable);
    download(&mut world, 2);
    assert_eq!(
        transfer_failed(&world, mina, 2).as_deref(),
        Some("bad_file")
    );
    world.fail_rest(
        mina,
        "GET /api/v1/files/*",
        RestFault::Answer(404, "no_such_file"),
    );
    download(&mut world, 3);
    assert_eq!(
        transfer_failed(&world, mina, 3).as_deref(),
        Some("no_such_file")
    );
    // A chunk's answer handed back as a plain reply cannot be its bytes.
    world.command(
        mina,
        Command::Download {
            file: info.clone(),
            handle: 4,
        },
    );
    let (asked, _) = world.phones[mina].gets.pop_front().unwrap();
    world.input(
        mina,
        Input::Reply {
            id: asked,
            status: 200,
            body: "{}".to_string(),
        },
    );
    assert_eq!(
        transfer_failed(&world, mina, 4).as_deref(),
        Some("bad_reply")
    );
    // The one that is whole still comes down.
    download(&mut world, 5);
    assert!(downloaded(&world, mina, 5));
    assert_eq!(world.phones[mina].held[&5], vec![5; 10]);
}

fn call_events(world: &World, phone: usize) -> Vec<ShownCallEvent> {
    let mut list = Vec::new();
    for event in &world.phones[phone].events {
        if let Event::CallEvent { event } = event {
            list.push(event.clone());
        }
    }
    list
}

fn last_call_output(world: &World, phone: usize) -> Output {
    world.phones[phone].call_outputs.last().unwrap().clone()
}

/// What the relay does with text from a device's `text` track: saves it in the call, and every
/// device in it reads it back from the `events` track.
fn relay_text(world: &mut World, from: usize, to: &[usize]) -> CallEvent {
    let Output::CallText {
        chat,
        call_seq,
        frame,
    } = last_call_output(world, from)
    else {
        panic!("no text went up");
    };
    let device = world.phones[from].device.unwrap();
    let event = world.call_event(chat, call_seq, CallEventKind::Text, device, Some(frame));
    for &p in to {
        world.input(
            p,
            Input::CallEvent {
                event: event.clone(),
            },
        );
    }
    event
}

fn start_call(world: &mut World, from: usize, to: usize, chat: Ulid) -> u64 {
    world.command(
        from,
        Command::StartCall {
            chat,
            video: true,
            voice: true,
        },
    );
    world.settle();
    let mut ring = None;
    for event in &world.phones[to].events {
        if let Event::Message { message } = event
            && message.content_kind == Some(ContentKind::Call)
        {
            ring = Some(message.clone());
        }
    }
    // The start is a message, and its delivery is the ring.
    let ring = ring.unwrap();
    assert_eq!(
        ring.call,
        Some(CallBody {
            video: true,
            voice: true
        })
    );
    assert_eq!(ring.call_outcome, None);
    ring.seq
}

#[test]
fn a_call_rings_is_declined_joined_written_in_and_replayed() {
    let (mut world, omid, mina, chat) = pair();
    let call_seq = start_call(&mut world, omid, mina, chat);

    // Mina declines, and Omid sees it inside the call.
    world.command(mina, Command::DeclineCall { chat, call_seq });
    world.settle();
    let declined = call_events(&world, omid);
    assert_eq!(declined.len(), 1);
    assert_eq!(declined[0].kind, CallEventKind::Decline);
    assert_eq!(declined[0].n, 1);
    assert!(!declined[0].mine);

    // Both join: the same secret, each its own index, the same path.
    world.command(omid, Command::JoinCall { chat, call_seq });
    world.command(mina, Command::JoinCall { chat, call_seq });
    let Output::JoinCall {
        path,
        key: omid_key,
        index_list,
        ..
    } = last_call_output(&world, omid)
    else {
        panic!("no join went out");
    };
    let Output::JoinCall { key: mina_key, .. } = last_call_output(&world, mina) else {
        panic!("no join went out");
    };
    assert_eq!(path, format!("/api/v1/calls/{chat}/{call_seq}/moq"));
    assert_eq!(omid_key.secret, mina_key.secret);
    assert_ne!(omid_key.index, mina_key.index);
    assert_eq!(index_list.len(), 2);

    // Text written in the call reaches both, once, even when it comes twice.
    world.command(
        omid,
        Command::CallText {
            chat,
            call_seq,
            text: Secret("سلام in the call".to_string()),
        },
    );
    let event = relay_text(&mut world, omid, &[omid, mina]);
    world.input(mina, Input::CallEvent { event });
    let mina_saw = call_events(&world, mina);
    // The decline went to her other devices, not back to the one that sent it.
    assert_eq!(mina_saw.len(), 1);
    assert_eq!(
        mina_saw[0].text,
        Some(Secret("سلام in the call".to_string()))
    );
    assert_eq!(mina_saw[0].kind, CallEventKind::Text);
    assert!(!mina_saw[0].mine);
    let omid_saw = call_events(&world, omid);
    assert!(omid_saw.last().unwrap().mine);
    assert_eq!(omid_saw.last().unwrap().text, mina_saw[0].text);
    // The chat itself holds no row for it.
    assert!(
        !world
            .texts(mina, chat)
            .iter()
            .any(|text| text.contains("in the call"))
    );

    // The app restarts and replays the call from its archive, which kept every row of it.
    let reader = world.reload(mina);
    world.settle();
    world.command(reader, Command::OpenChat { chat });
    world.settle();
    let replay = call_events(&world, reader);
    assert_eq!(replay.len(), 2);
    assert_eq!(replay[1].text, Some(Secret("سلام in the call".to_string())));

    // Leaving tells the call code, and a second leave does nothing.
    world.command(omid, Command::LeaveCall { chat });
    assert!(matches!(
        last_call_output(&world, omid),
        Output::LeaveCall { .. }
    ));
    let count = world.phones[omid].call_outputs.len();
    world.command(omid, Command::LeaveCall { chat });
    assert_eq!(world.phones[omid].call_outputs.len(), count);
}

#[test]
fn text_in_a_call_that_does_not_check_out_shows_its_problem() {
    let (mut world, omid, mina, chat) = pair();
    let call_seq = start_call(&mut world, omid, mina, chat);
    // Not joined yet: nothing to write in.
    world.command(
        omid,
        Command::CallText {
            chat,
            call_seq,
            text: Secret("too early".to_string()),
        },
    );
    assert_eq!(error_count(&world, omid, "not_in_call"), 1);
    world.command(omid, Command::JoinCall { chat, call_seq });
    world.command(
        omid,
        Command::CallText {
            chat,
            call_seq,
            text: Secret("checked".to_string()),
        },
    );
    let good = relay_text(&mut world, omid, &[]);

    let mut changed = good.clone();
    changed.n = 10;
    changed.body.as_mut().unwrap().0[5] ^= 1;
    let mut unsigned = good.clone();
    unsigned.n = 11;
    unsigned.signature = None;
    let mut stranger = good.clone();
    stranger.n = 12;
    stranger.device = Ulid::from_bytes([77; 16]);
    for event in [changed, unsigned, stranger] {
        world.input(mina, Input::CallEvent { event });
    }
    world.settle();
    let problems: Vec<Option<Problem>> = call_events(&world, mina)
        .iter()
        .map(|event| event.problem)
        .collect();
    assert_eq!(
        problems,
        vec![
            Some(Problem::BadSignature),
            Some(Problem::Malformed),
            Some(Problem::UnknownDevice)
        ]
    );
}

#[test]
fn a_joined_call_gets_the_new_key_when_the_chat_moves_on_and_there_is_one_call_at_a_time() {
    let (mut world, omid, mina, chat) = pair();
    let call_seq = start_call(&mut world, omid, mina, chat);
    world.command(omid, Command::JoinCall { chat, call_seq });
    let Output::JoinCall { key: first, .. } = last_call_output(&world, omid) else {
        panic!("no join went out");
    };
    // A week on, the next send changes the key first, and the call follows it.
    world.advance(8 * DAY);
    tell(&mut world, omid, chat, "after a week");
    let Output::CallKey { key: next, .. } = last_call_output(&world, omid) else {
        panic!("no new key went out");
    };
    assert_eq!(next.epoch, first.epoch + 1);
    assert_ne!(next.secret, first.secret);

    // Joining another call leaves this one first.
    world.command(
        omid,
        Command::JoinCall {
            chat,
            call_seq: call_seq + 100,
        },
    );
    let outputs = &world.phones[omid].call_outputs;
    assert!(matches!(
        outputs[outputs.len() - 2],
        Output::LeaveCall { .. }
    ));
    assert!(matches!(
        outputs[outputs.len() - 1],
        Output::JoinCall { .. }
    ));

    // A chat this device is not in has no call to join.
    world.command(
        omid,
        Command::JoinCall {
            chat: Ulid::from_bytes([88; 16]),
            call_seq: 1,
        },
    );
    assert_eq!(error_count(&world, omid, "not_joined"), 1);

    // Declining needs the socket.
    world.offline(mina);
    world.command(mina, Command::DeclineCall { chat, call_seq });
    assert_eq!(error_count(&world, mina, "offline"), 1);
}
