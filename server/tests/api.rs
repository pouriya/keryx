mod common;
#[path = "common/person.rs"]
mod person;

use common::{OPERATOR_TOKEN, Server};
use keryx_core::mls::{Device, sign_password_reset, sign_revocation};
use keryx_core::protocol::Bytes;
use keryx_core::ulid::Ulid;
use person::{BABAK_WORDS, PASSWORD, SARA_WORDS, add_device, log_in, sign_up, ulid_of};
use serde_json::{Value, json};

fn open_server() -> Server {
    Server::start("", "[registration]\nallow = true\n")
}

fn refused(reply: (u16, Value), status: u16, code: &str) {
    assert_eq!(reply.0, status, "{}", reply.1);
    assert_eq!(reply.1["ok"], false);
    assert_eq!(reply.1["code"], code);
    assert!(reply.1["message"].as_str().is_some());
}

fn raw(server: &Server, path: &str) -> ureq::http::Response<ureq::Body> {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .new_agent()
        .get(format!("{}{path}", server.origin))
        .call()
        .unwrap()
}

#[test]
fn every_reply_is_wrapped_and_carries_the_headers() {
    let server = open_server();
    let mut guide = raw(&server, "/api/v1");
    assert_eq!(guide.status(), 200);
    let headers = guide.headers().clone();
    assert!(
        headers["server"]
            .to_str()
            .unwrap()
            .starts_with("keryx/0.1.0 (https://github.com/pouriya/keryx)")
    );
    assert_eq!(headers["x-keryx-version"], "0.1.0");
    Ulid::parse(headers["x-trace-id"].to_str().unwrap()).unwrap();
    let text = guide.body_mut().read_to_string().unwrap();
    assert!(text.contains(&format!("{}/api/v1", server.origin)));

    refused(
        server.call("GET", "/api/v1/nothing/here", None, None),
        404,
        "not_found",
    );
    refused(
        server.call("DELETE", "/api/v1/login", None, None),
        405,
        "wrong_method",
    );
    refused(
        server.call("POST", "/api/v1/login", None, Some(&json!({"nope": 1}))),
        400,
        "bad_body",
    );
    refused(
        server.call("GET", "/api/v1/ws", None, None),
        400,
        "not_websocket",
    );
    refused(
        server.call("GET", "/api/v1/sessions", None, None),
        401,
        "not_logged_in",
    );
    refused(
        server.call("GET", "/api/v1/sessions", Some("not a token"), None),
        401,
        "not_logged_in",
    );
}

#[test]
fn the_web_client_is_served_with_index_for_any_path() {
    let server = open_server();
    let mut file = raw(&server, "/page.txt");
    assert_eq!(file.body_mut().read_to_string().unwrap(), "a file");
    let mut index = raw(&server, "/setup");
    assert_eq!(index.status(), 200);
    assert_eq!(index.body_mut().read_to_string().unwrap(), "the web client");
}

#[test]
fn a_new_server_is_closed_and_an_operator_makes_accounts() {
    let server = Server::start("", "");
    let body = json!({"username": "sara_x", "password": PASSWORD});
    refused(
        server.call("POST", "/api/v1/register", None, Some(&body)),
        403,
        "registration_closed",
    );

    // The operator's token counts as admin, and has no account of its own.
    refused(
        server.call("GET", "/api/v1/sessions", Some(OPERATOR_TOKEN), None),
        403,
        "needs_account",
    );
    let body = json!({"username": "sara_x", "admin": true, "display_name": "سارا"});
    refused(
        server.call("POST", "/api/v1/accounts", None, Some(&body)),
        401,
        "not_logged_in",
    );
    let (status, reply) = server.call(
        "POST",
        "/api/v1/accounts",
        Some(OPERATOR_TOKEN),
        Some(&body),
    );
    assert_eq!(status, 200, "{reply}");
    let link = reply["data"]["link"].as_str().unwrap();
    let (base, token) = link.split_once('#').unwrap();
    assert_eq!(base, format!("{}/setup", server.origin));
    refused(
        server.call(
            "POST",
            "/api/v1/accounts",
            Some(OPERATOR_TOKEN),
            Some(&body),
        ),
        409,
        "username_taken",
    );

    // No password yet, so no login.
    let login = json!({"username": "sara_x", "password": PASSWORD});
    refused(
        server.call("POST", "/api/v1/login/token", None, Some(&login)),
        401,
        "bad_login",
    );
    let setup = json!({"token": token, "password": "short"});
    refused(
        server.call("POST", "/api/v1/setup", None, Some(&setup)),
        400,
        "bad_password",
    );
    let setup = json!({"token": token, "password": PASSWORD});
    let (status, reply) = server.call("POST", "/api/v1/setup", None, Some(&setup));
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["data"]["username"], "sara_x");
    refused(
        server.call("POST", "/api/v1/setup", None, Some(&setup)),
        404,
        "bad_link",
    );

    // The first admin makes the next account, and a plain member cannot.
    let admin_token = log_in(&server, "sara_x");
    let body = json!({"username": "ab"});
    let (status, reply) = server.call("POST", "/api/v1/accounts", Some(&admin_token), Some(&body));
    assert_eq!(status, 200, "{reply}");
    let token = reply["data"]["link"]
        .as_str()
        .unwrap()
        .split_once('#')
        .unwrap()
        .1
        .to_string();
    let setup = json!({"token": token, "password": PASSWORD});
    server.call("POST", "/api/v1/setup", None, Some(&setup));
    let member_token = log_in(&server, "ab");
    refused(
        server.call(
            "POST",
            "/api/v1/accounts",
            Some(&member_token),
            Some(&json!({"username": "cc_c"})),
        ),
        403,
        "no_permission",
    );
}

#[test]
fn usernames_and_passwords_follow_the_rules() {
    let server = open_server();
    for username in ["ab", "Sara_x", "1sara", "سارا_x", &"a".repeat(33)] {
        refused(
            server.call(
                "POST",
                "/api/v1/register",
                None,
                Some(&json!({"username": username, "password": PASSWORD})),
            ),
            400,
            "bad_username",
        );
    }
    for password in ["short", &"x".repeat(1025)] {
        refused(
            server.call(
                "POST",
                "/api/v1/register",
                None,
                Some(&json!({"username": "sara_x", "password": password})),
            ),
            400,
            "bad_password",
        );
    }
    let body = json!({"username": "sara_x", "password": "۱۲۳۴۵۶۷۸۹۰۱۲"});
    assert_eq!(
        server.call("POST", "/api/v1/register", None, Some(&body)).0,
        200
    );
    refused(
        server.call("POST", "/api/v1/register", None, Some(&body)),
        409,
        "username_taken",
    );
    let wrong = json!({"username": "sara_x", "password": "not the right one"});
    refused(
        server.call("POST", "/api/v1/login", None, Some(&wrong)),
        401,
        "bad_login",
    );
    let unknown = json!({"username": "nobody_x", "password": PASSWORD});
    refused(
        server.call("POST", "/api/v1/login", None, Some(&unknown)),
        401,
        "bad_login",
    );
}

#[test]
fn the_two_login_routes_differ_and_sessions_can_be_seen_and_ended() {
    let server = open_server();
    let sara = sign_up(&server, "sara_x", SARA_WORDS);
    let body = json!({"username": "sara_x", "password": PASSWORD});

    let agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .new_agent();
    let mut response = agent
        .post(format!("{}/api/v1/login", server.origin))
        .send_json(&body)
        .unwrap();
    let cookie = response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .to_string();
    assert!(cookie.starts_with("keryx_session="));
    assert!(cookie.contains("HttpOnly"));
    assert!(cookie.contains("SameSite=Strict"));
    assert!(!cookie.contains("Secure"), "public_url is http here");
    let reply: Value = response.body_mut().read_json().unwrap();
    assert!(reply["data"].get("token").is_none());

    // The cookie works as a session.
    let pair = cookie.split(';').next().unwrap();
    let mut listed = agent
        .get(format!("{}/api/v1/sessions", server.origin))
        .header("Cookie", &format!("other=1; {pair}"))
        .call()
        .unwrap();
    assert_eq!(listed.status(), 200);
    let listed: Value = listed.body_mut().read_json().unwrap();
    let session_list = listed["data"]["session_list"].as_array().unwrap();
    assert_eq!(session_list.len(), 2);

    let mut token_login = agent
        .post(format!("{}/api/v1/login/token", server.origin))
        .send_json(&body)
        .unwrap();
    assert!(token_login.headers().get("set-cookie").is_none());
    let token_reply: Value = token_login.body_mut().read_json().unwrap();
    let token = token_reply["data"]["token"].as_str().unwrap().to_string();

    // Ending the other session leaves this one working.
    let (_, listed) = server.call("GET", "/api/v1/sessions", Some(&token), None);
    let mut ended = 0;
    for session in listed["data"]["session_list"].as_array().unwrap() {
        if session.get("device").is_none() {
            let id = session["id"].as_str().unwrap();
            let route = format!("/api/v1/sessions/{id}");
            if server.call("DELETE", &route, Some(&sara.token), None).0 == 200 {
                ended += 1;
            }
        }
    }
    assert_eq!(ended, 2);
    refused(
        server.call("GET", "/api/v1/sessions", Some(&token), None),
        401,
        "not_logged_in",
    );
    refused(
        server.call(
            "DELETE",
            &format!("/api/v1/sessions/{}", Ulid::new().unwrap()),
            Some(&sara.token),
            None,
        ),
        404,
        "no_such_session",
    );
    refused(
        server.call(
            "DELETE",
            "/api/v1/sessions/nonsense",
            Some(&sara.token),
            None,
        ),
        404,
        "not_found",
    );
}

#[test]
fn the_cookie_is_secure_when_the_public_url_is_https() {
    // `public_url` comes first in a TOML file, and a repeated key stops startup, so this rewrites it.
    let mut server = open_server();
    let config = std::fs::read_to_string(server.path("server.toml")).unwrap();
    server.stop();
    let port = server.origin.rsplit(':').next().unwrap().to_string();
    let config = config.replace(
        &format!("public_url = \"http://127.0.0.1:{port}\""),
        &format!("public_url = \"https://127.0.0.1:{port}/\""),
    );
    let config_path = server.path("https.toml");
    std::fs::write(&config_path, config).unwrap();
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_keryx"))
        .arg("--config")
        .arg(&config_path)
        .arg("--quiet")
        .arg("serve")
        .spawn()
        .unwrap();
    let agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .new_agent();
    let mut up = false;
    for _ in 0..500 {
        if agent
            .get(format!("{}/api/v1", server.origin))
            .call()
            .is_ok()
        {
            up = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(up);
    let body = json!({"username": "sara_x", "password": PASSWORD});
    agent
        .post(format!("{}/api/v1/register", server.origin))
        .send_json(&body)
        .unwrap();
    let response = agent
        .post(format!("{}/api/v1/login", server.origin))
        .send_json(&body)
        .unwrap();
    let cookie = response.headers()["set-cookie"].to_str().unwrap();
    assert!(cookie.contains("; Secure"));
    let terminated = std::process::Command::new("kill")
        .arg("-TERM")
        .arg(child.id().to_string())
        .status()
        .unwrap();
    assert!(terminated.success());
    assert!(child.wait().unwrap().success());
}

#[test]
fn keys_are_set_once_and_their_stamp_must_verify() {
    let server = open_server();
    let sara = sign_up(&server, "sara_x", SARA_WORDS);
    let babak = sign_up(&server, "babak_x", BABAK_WORDS);
    let (status, reply) = server.call(
        "GET",
        &format!("/api/v1/accounts/{}", sara.id),
        Some(&babak.token),
        None,
    );
    assert_eq!(status, 200);
    assert_eq!(reply["data"]["username"], sara.username);
    assert_eq!(
        reply["data"]["master_public"],
        json!(Bytes(sara.key.master_public.to_vec()))
    );
    assert_eq!(
        reply["data"]["account_stamp"],
        json!(Bytes(sara.key.stamp.to_vec()))
    );
    refused(
        server.call(
            "GET",
            &format!("/api/v1/accounts/{}", Ulid::new().unwrap()),
            Some(&babak.token),
            None,
        ),
        404,
        "no_such_account",
    );

    let body = json!({
        "master_public": Bytes(sara.key.master_public.to_vec()),
        "account_public": Bytes(sara.key.account_public.to_vec()),
        "account_stamp": Bytes(sara.key.stamp.to_vec()),
    });
    refused(
        server.call(
            "PUT",
            "/api/v1/account/keys",
            Some(&sara.token),
            Some(&body),
        ),
        409,
        "keys_set",
    );

    // Someone else's stamp is not for this account.
    let (_, third) = server.call(
        "POST",
        "/api/v1/register",
        None,
        Some(&json!({"username": "third_x", "password": PASSWORD})),
    );
    assert!(third["ok"].as_bool().unwrap());
    let third_token = log_in(&server, "third_x");
    refused(
        server.call(
            "PUT",
            "/api/v1/account/keys",
            Some(&third_token),
            Some(&body),
        ),
        403,
        "bad_signature",
    );
    let short = json!({"master_public": "AQ==", "account_public": "AQ==", "account_stamp": "AQ=="});
    refused(
        server.call(
            "PUT",
            "/api/v1/account/keys",
            Some(&third_token),
            Some(&short),
        ),
        400,
        "bad_body",
    );

    // No words yet means nobody can start a chat with this person, and no device can be added.
    let (_, third_id) = server.call(
        "GET",
        "/api/v1/accounts?search=third",
        Some(&third_token),
        None,
    );
    let third_id = ulid_of(&third_id["data"]["account_list"][0]["id"]);
    refused(
        server.call(
            "POST",
            "/api/v1/chats",
            Some(&sara.token),
            Some(&json!({"account": third_id})),
        ),
        409,
        "words_not_set",
    );
    let (_, id) = server.call("POST", "/api/v1/devices/id", Some(&third_token), None);
    refused(
        server.call(
            "POST",
            "/api/v1/devices",
            Some(&third_token),
            Some(&json!({
                "device": id["data"]["device"],
                "public_key": Bytes(vec![0; 32]),
                "stamp": Bytes(vec![0; 64]),
                "platform": 0,
                "os": "linux",
                "os_version": "7",
            })),
        ),
        409,
        "words_not_set",
    );
}

#[test]
fn the_directory_finds_partial_names_and_escapes_wildcards() {
    let server = open_server();
    let sara = sign_up(&server, "sara_x", SARA_WORDS);
    server.call(
        "POST",
        "/api/v1/register",
        None,
        Some(&json!({"username": "babak_x", "password": PASSWORD, "display_name": "بابک"})),
    );
    let (status, reply) = server.call(
        "GET",
        "/api/v1/accounts?search=bab",
        Some(&sara.token),
        None,
    );
    assert_eq!(status, 200);
    let account_list = reply["data"]["account_list"].as_array().unwrap();
    assert_eq!(account_list.len(), 1);
    assert_eq!(account_list[0]["display_name"], "بابک");

    let (_, everyone) = server.call("GET", "/api/v1/accounts", Some(&sara.token), None);
    assert_eq!(
        everyone["data"]["account_list"].as_array().unwrap().len(),
        2
    );
    let (_, literal) = server.call(
        "GET",
        "/api/v1/accounts?search=%25",
        Some(&sara.token),
        None,
    );
    assert_eq!(literal["data"]["account_list"].as_array().unwrap().len(), 0);
    refused(
        server.call("GET", "/api/v1/accounts?search=a", None, None),
        401,
        "not_logged_in",
    );
}

#[test]
fn words_reset_a_forgotten_password_and_the_challenge_works_once() {
    let server = open_server();
    let sara = sign_up(&server, "sara_x", SARA_WORDS);
    let new_password = "a different long password";
    let asked = json!({"username": "sara_x", "purpose": "password_reset"});
    let (status, reply) = server.call("POST", "/api/v1/challenge", None, Some(&asked));
    assert_eq!(status, 200);
    let challenge: Bytes = serde_json::from_value(reply["data"]["challenge"].clone()).unwrap();
    let fixed: [u8; 32] = challenge.0.clone().try_into().unwrap();
    let signature = sign_password_reset(&sara.keys, &sara.id, &fixed, new_password);

    // A signature for one password cannot set another. Trying uses the challenge up.
    let wrong = json!({
        "username": "sara_x",
        "challenge": challenge,
        "new_password": "yet another long password",
        "signature": Bytes(signature.to_vec()),
    });
    refused(
        server.call("POST", "/api/v1/password/reset", None, Some(&wrong)),
        403,
        "bad_signature",
    );
    let right = json!({
        "username": "sara_x",
        "challenge": challenge,
        "new_password": new_password,
        "signature": Bytes(signature.to_vec()),
    });
    refused(
        server.call("POST", "/api/v1/password/reset", None, Some(&right)),
        403,
        "bad_signature",
    );

    let (_, reply) = server.call("POST", "/api/v1/challenge", None, Some(&asked));
    let challenge: Bytes = serde_json::from_value(reply["data"]["challenge"].clone()).unwrap();
    let fixed: [u8; 32] = challenge.0.clone().try_into().unwrap();
    let signature = sign_password_reset(&sara.keys, &sara.id, &fixed, new_password);
    let right = json!({
        "username": "sara_x",
        "challenge": challenge,
        "new_password": new_password,
        "signature": Bytes(signature.to_vec()),
    });
    let (status, reply) = server.call("POST", "/api/v1/password/reset", None, Some(&right));
    assert_eq!(status, 200, "{reply}");

    // All sessions ended, and only the new password logs in.
    refused(
        server.call("GET", "/api/v1/sessions", Some(&sara.token), None),
        401,
        "not_logged_in",
    );
    let old = json!({"username": "sara_x", "password": PASSWORD});
    refused(
        server.call("POST", "/api/v1/login/token", None, Some(&old)),
        401,
        "bad_login",
    );
    let new = json!({"username": "sara_x", "password": new_password});
    assert_eq!(
        server
            .call("POST", "/api/v1/login/token", None, Some(&new))
            .0,
        200
    );

    refused(
        server.call(
            "POST",
            "/api/v1/challenge",
            None,
            Some(&json!({"username": "sara_x", "purpose": "anything"})),
        ),
        400,
        "bad_body",
    );
    refused(
        server.call(
            "POST",
            "/api/v1/challenge",
            None,
            Some(&json!({"username": "nobody_x", "purpose": "device_login"})),
        ),
        404,
        "no_such_account",
    );
}

#[test]
fn a_device_id_is_handed_out_and_used_once() {
    let server = open_server();
    let sara = sign_up(&server, "sara_x", SARA_WORDS);
    let token = log_in(&server, "sara_x");

    // An id the server did not make is refused, even with a good stamp.
    let chosen = Ulid::new().unwrap();
    let device = Device::create(&sara.keys, sara.id, chosen).unwrap();
    let record = device.record();
    let body = json!({
        "device": chosen,
        "public_key": Bytes(record.device_public.to_vec()),
        "stamp": Bytes(record.stamp.to_vec()),
        "platform": 1,
        "os": "android",
        "os_version": "16",
    });
    refused(
        server.call("POST", "/api/v1/devices", Some(&token), Some(&body)),
        409,
        "unknown_device_id",
    );

    // A stamp that does not verify, and a platform that does not exist.
    let (_, id) = server.call("POST", "/api/v1/devices/id", Some(&token), None);
    let id = ulid_of(&id["data"]["device"]);
    let mut wrong_stamp = body.clone();
    wrong_stamp["device"] = json!(id);
    refused(
        server.call("POST", "/api/v1/devices", Some(&token), Some(&wrong_stamp)),
        403,
        "bad_signature",
    );
    let mut wrong_platform = wrong_stamp.clone();
    wrong_platform["platform"] = json!(7);
    refused(
        server.call(
            "POST",
            "/api/v1/devices",
            Some(&token),
            Some(&wrong_platform),
        ),
        400,
        "bad_body",
    );
    let mut too_short = wrong_stamp.clone();
    too_short["stamp"] = json!(Bytes(vec![0; 3]));
    refused(
        server.call("POST", "/api/v1/devices", Some(&token), Some(&too_short)),
        400,
        "bad_body",
    );

    // A session has one device.
    let (_, another) = server.call("POST", "/api/v1/devices/id", Some(&sara.token), None);
    let mut has_device = body.clone();
    has_device["device"] = another["data"]["device"].clone();
    refused(
        server.call(
            "POST",
            "/api/v1/devices",
            Some(&sara.token),
            Some(&has_device),
        ),
        409,
        "has_device",
    );
}

#[test]
fn a_password_only_session_sees_a_reduced_list_and_cannot_message() {
    let server = open_server();
    let sara = sign_up(&server, "sara_x", SARA_WORDS);
    let babak = sign_up(&server, "babak_x", BABAK_WORDS);
    let token = log_in(&server, "sara_x");

    let (_, reduced) = server.call("GET", "/api/v1/devices", Some(&token), None);
    let device = &reduced["data"]["device_list"][0];
    assert_eq!(device["id"], json!(sara.device.record().device));
    assert_eq!(device["platform"], 0);
    assert!(device["last_active_day"].as_i64().unwrap() > 0);
    assert!(device.get("last_ip").is_none());
    assert!(device.get("log_list").is_none());

    let (_, full) = server.call("GET", "/api/v1/devices", Some(&sara.token), None);
    let device = &full["data"]["device_list"][0];
    assert_eq!(device["os"], "linux");
    assert_eq!(device["first_ip"], "127.0.0.1");

    for (method, route) in [
        ("POST", "/api/v1/key_packages"),
        ("POST", "/api/v1/chats"),
        (
            "POST",
            &format!("/api/v1/accounts/{}/key_packages/claim", babak.id),
        ),
        ("GET", &format!("/api/v1/chats/{}/wraps", sara.self_chat)),
        ("GET", &format!("/api/v1/chats/{}/devices", sara.self_chat)),
        (
            "GET",
            &format!("/api/v1/chats/{}/group_info", sara.self_chat),
        ),
    ] {
        let body = json!({"account": babak.id, "body_list": []});
        refused(
            server.call(method, route, Some(&token), Some(&body)),
            403,
            "no_device",
        );
    }
}

#[test]
fn the_device_limit_blocks_a_new_device_until_one_is_terminated() {
    let server = Server::start("max_devices = 1", "[registration]\nallow = true\n");
    let sara = sign_up(&server, "sara_x", SARA_WORDS);

    // A blocked device still gets a session.
    let token = log_in(&server, "sara_x");
    let (_, id) = server.call("POST", "/api/v1/devices/id", Some(&token), None);
    let id = ulid_of(&id["data"]["device"]);
    let second = Device::create(&sara.keys, sara.id, id).unwrap();
    let body = json!({
        "device": id,
        "public_key": Bytes(second.record().device_public.to_vec()),
        "stamp": Bytes(second.record().stamp.to_vec()),
        "platform": 0,
        "os": "linux",
        "os_version": "7",
        "web_platform": "firefox",
        "web_platform_version": "150",
    });
    let reply = server.call("POST", "/api/v1/devices", Some(&token), Some(&body));
    assert_eq!(reply.0, 403);
    assert_eq!(reply.1["code"], "device_limit");
    assert!(reply.1["message"].as_str().unwrap().contains('1'));

    // Terminating needs the words: a wrong statement changes nothing.
    let first = sara.device.record().device;
    let route = format!("/api/v1/devices/{first}/terminate");
    let wrong = json!({"signature": Bytes(vec![0; 64])});
    refused(
        server.call("POST", &route, Some(&token), Some(&wrong)),
        403,
        "bad_signature",
    );
    let signed = sign_revocation(&sara.keys, &sara.id, &first);
    let right = json!({"signature": Bytes(signed.to_vec())});
    let (status, reply) = server.call("POST", &route, Some(&token), Some(&right));
    assert_eq!(status, 200, "{reply}");
    // Repeating it changes nothing.
    assert_eq!(
        server.call("POST", &route, Some(&token), Some(&right)).0,
        200
    );

    // The ended device's session is gone, and there is now room.
    refused(
        server.call("GET", "/api/v1/sessions", Some(&sara.token), None),
        401,
        "not_logged_in",
    );
    let third = add_device(&server, sara.id, &sara.keys, &token);
    let (_, full) = server.call("GET", "/api/v1/devices", Some(&token), None);
    let device_list = full["data"]["device_list"].as_array().unwrap();
    assert_eq!(device_list.len(), 2);
    let mut live = 0;
    for device in device_list {
        if device.get("terminated_at").is_none() {
            assert_eq!(device["id"], json!(third.record().device));
            live += 1;
        }
    }
    assert_eq!(live, 1);
}

#[test]
fn lowering_the_limit_stops_existing_devices_at_their_next_request() {
    let mut server = Server::start("max_devices = 2", "[registration]\nallow = true\n");
    let sara = sign_up(&server, "sara_x", SARA_WORDS);
    let token = log_in(&server, "sara_x");
    add_device(&server, sara.id, &sara.keys, &token);
    let config = std::fs::read_to_string(server.path("server.toml")).unwrap();
    server.stop();
    let config_path = server.path("lower.toml");
    std::fs::write(
        &config_path,
        config.replace("max_devices = 2", "max_devices = 1"),
    )
    .unwrap();
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_keryx"))
        .arg("--config")
        .arg(&config_path)
        .arg("--quiet")
        .arg("serve")
        .spawn()
        .unwrap();
    let mut body = None;
    for _ in 0..500 {
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build()
            .new_agent();
        let response = agent
            .post(format!("{}/api/v1/key_packages", server.origin))
            .header("Authorization", &format!("Bearer {token}"))
            .send_json(json!({"body_list": []}));
        if let Ok(mut response) = response {
            let reply: Value = response.body_mut().read_json().unwrap();
            body = Some(reply);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(body.unwrap()["code"], "device_limit");
    let terminated = std::process::Command::new("kill")
        .arg("-TERM")
        .arg(child.id().to_string())
        .status()
        .unwrap();
    assert!(terminated.success());
    assert!(child.wait().unwrap().success());
}

#[test]
fn key_packages_have_a_limit_and_a_claim_uses_one_up() {
    let server = Server::start(
        "",
        "[registration]\nallow = true\n[limits]\nkey_packages = 10\n",
    );
    // Each client keeps 10, and starting its self chat used one of its own, so two more go past 10.
    let sara = sign_up(&server, "sara_x", SARA_WORDS);
    let babak = sign_up(&server, "babak_x", BABAK_WORDS);
    let too_many = json!({"body_list": [Bytes(vec![1]), Bytes(vec![2])]});
    refused(
        server.call(
            "POST",
            "/api/v1/key_packages",
            Some(&babak.token),
            Some(&too_many),
        ),
        409,
        "too_many_key_packages",
    );
    let route = format!("/api/v1/accounts/{}/key_packages/claim", babak.id);
    let mut seen = Vec::new();
    for _ in 0..9 {
        let (status, reply) = server.call("POST", &route, Some(&sara.token), None);
        assert_eq!(status, 200, "{reply}");
        let key_package_list = reply["data"]["key_package_list"].as_array().unwrap();
        assert_eq!(key_package_list.len(), 1);
        assert_eq!(
            key_package_list[0]["device"],
            json!(babak.device.record().device)
        );
        let body = key_package_list[0]["body"].clone();
        assert!(!seen.contains(&body));
        seen.push(body);
    }
    refused(
        server.call("POST", &route, Some(&sara.token), None),
        409,
        "no_key_packages",
    );
}

#[test]
fn a_pair_has_one_chat_and_a_person_one_self_chat() {
    let server = open_server();
    let sara = sign_up(&server, "sara_x", SARA_WORDS);
    let babak = sign_up(&server, "babak_x", BABAK_WORDS);
    let mine = json!({"account": sara.id});
    let (_, reply) = server.call("POST", "/api/v1/chats", Some(&sara.token), Some(&mine));
    assert_eq!(reply["data"]["chat"], json!(sara.self_chat));
    assert_eq!(reply["data"]["created"], false);

    let with_babak = json!({"account": babak.id});
    let (_, first) = server.call(
        "POST",
        "/api/v1/chats",
        Some(&sara.token),
        Some(&with_babak),
    );
    assert_eq!(first["data"]["created"], true);
    let with_sara = json!({"account": sara.id});
    let (_, second) = server.call(
        "POST",
        "/api/v1/chats",
        Some(&babak.token),
        Some(&with_sara),
    );
    assert_eq!(second["data"]["created"], false);
    assert_eq!(first["data"]["chat"], second["data"]["chat"]);

    let unknown = json!({"account": Ulid::new().unwrap()});
    refused(
        server.call("POST", "/api/v1/chats", Some(&sara.token), Some(&unknown)),
        404,
        "no_such_account",
    );
    // A stranger is not in that chat, and is told nothing more.
    let third = sign_up(&server, "third_x", BABAK_WORDS);
    let chat = first["data"]["chat"].as_str().unwrap();
    for suffix in ["wraps", "devices", "group_info"] {
        refused(
            server.call(
                "GET",
                &format!("/api/v1/chats/{chat}/{suffix}"),
                Some(&third.token),
                None,
            ),
            404,
            "not_member",
        );
    }
    refused(
        server.call(
            "GET",
            &format!("/api/v1/chats/{chat}/group_info"),
            Some(&sara.token),
            None,
        ),
        404,
        "no_group_info",
    );
}

#[test]
fn a_large_body_gets_the_wrapped_refusal() {
    let server = Server::start(
        "",
        "[registration]\nallow = true\n[limits]\nrest_body = \"1KiB\"\n",
    );
    let body = json!({"username": "sara_x", "password": "x".repeat(2048)});
    refused(
        server.call("POST", "/api/v1/register", None, Some(&body)),
        413,
        "body_too_large",
    );
}

#[test]
fn no_secret_reaches_the_log() {
    let mut server = open_server();
    let sara = sign_up(&server, "sara_x", SARA_WORDS);
    let (_, reply) = server.call(
        "POST",
        "/api/v1/accounts",
        Some(OPERATOR_TOKEN),
        Some(&json!({"username": "new_one"})),
    );
    let link = reply["data"]["link"].as_str().unwrap().to_string();
    let setup_token = link.split_once('#').unwrap().1;
    server.stop();
    let log = std::fs::read_to_string(server.path("server.log")).unwrap();
    assert!(log.contains("HTTP request"));
    assert!(log.contains("\"http_route\":\"/api/v1/register\""));
    assert!(!log.contains(PASSWORD));
    assert!(!log.contains(&sara.token));
    assert!(!log.contains(OPERATOR_TOKEN));
    assert!(!log.contains(setup_token));
    assert!(!log.contains(SARA_WORDS));
}

#[test]
fn a_client_that_stops_reading_a_reply_is_closed_after_send_timeout() {
    let server = Server::start(
        "",
        "[tcp]\nsend_timeout = \"1s\"\nsend_buffer = \"64KiB\"\n",
    );
    // Far more than the operating system buffers, and sparse, so it takes no disk.
    let size = 1024 * 1024 * 1024;
    let file = std::fs::File::create(server.path("www/big.bin")).unwrap();
    file.set_len(size).unwrap();

    let address = server.origin.strip_prefix("http://").unwrap();
    let mut stream = std::net::TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .unwrap();
    std::io::Write::write_all(
        &mut stream,
        format!("GET /big.bin HTTP/1.1\r\nHost: {address}\r\n\r\n").as_bytes(),
    )
    .unwrap();
    std::thread::sleep(std::time::Duration::from_secs(3));

    // What the operating system already held still arrives, and then the connection is over.
    let mut buffer = vec![0; 1024 * 1024];
    let mut total = 0;
    loop {
        match std::io::Read::read(&mut stream, &mut buffer) {
            Ok(0) => break,
            Ok(count) => total += count as u64,
            Err(error) => {
                assert_eq!(
                    error.kind(),
                    std::io::ErrorKind::ConnectionReset,
                    "the connection stayed open"
                );
                break;
            }
        }
    }
    assert!(total < size, "the whole file arrived, so nothing timed out");
}
