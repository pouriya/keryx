mod common;

use common::Server;
use serde_json::{Value, json};

const PASSWORD: &str = "a long enough password";

fn with_captcha(options: &str) -> Server {
    Server::start(
        "",
        &format!("[registration]\nallow = true\n[captcha]\n{options}\n"),
    )
}

fn issue(server: &Server) -> Value {
    let (status, reply) = server.call("GET", "/api/v1/captcha", None, None);
    assert_eq!(status, 200, "{reply}");
    reply["data"].clone()
}

fn register(server: &Server, username: &str, captcha: Option<Value>) -> (u16, Value) {
    server.call(
        "POST",
        "/api/v1/register",
        None,
        Some(&json!({"username": username, "password": PASSWORD, "captcha": captcha})),
    )
}

#[test]
fn without_the_section_there_is_no_captcha() {
    let server = Server::start("", "[registration]\nallow = true\n");
    let (status, reply) = server.call("GET", "/api/v1/captcha", None, None);
    assert_eq!(status, 404);
    assert_eq!(reply["code"], "captcha_off");
    assert_eq!(register(&server, "sara_x", None).0, 200);
}

#[test]
fn every_option_draws_a_png() {
    let server = with_captcha("test_answer = \"12345\"");
    let image = issue(&server)["image"].as_str().unwrap().to_string();
    assert!(image.starts_with("data:image/png;base64,"));
    drop(server);
    for style in ["amelia", "lucy", "mila"] {
        for difficulty in ["easy", "medium", "hard"] {
            for alphabet in ["digits", "letters", "mixed"] {
                let server = with_captcha(&format!(
                    "style = \"{style}\"\ndifficulty = \"{difficulty}\"\nalphabet = \"{alphabet}\"\n\
                     length = 4"
                ));
                let image = issue(&server)["image"].as_str().unwrap().to_string();
                assert!(image.starts_with("data:image/png;base64,"));
            }
        }
    }
}

#[test]
fn one_check_uses_it_up_right_or_wrong() {
    let server = with_captcha("test_answer = \"12345\"");
    let (status, reply) = register(&server, "sara_x", None);
    assert_eq!(status, 400);
    assert_eq!(reply["code"], "captcha_wrong");

    let issued = issue(&server);
    let wrong = json!({"id": issued["id"], "answer": "54321"});
    assert_eq!(
        register(&server, "sara_x", Some(wrong)).1["code"],
        "captcha_wrong"
    );
    let right = json!({"id": issued["id"], "answer": "12345"});
    assert_eq!(
        register(&server, "sara_x", Some(right)).1["code"],
        "captcha_wrong",
        "the wrong try used it up"
    );

    let issued = issue(&server);
    let right = json!({"id": issued["id"], "answer": " 12345 "});
    let (status, reply) = register(&server, "sara_x", Some(right.clone()));
    assert_eq!(status, 200, "{reply}");
    assert_eq!(
        register(&server, "babak_x", Some(right)).1["code"],
        "captcha_wrong",
        "the right try used it up too"
    );
}

#[test]
fn it_guards_password_login_before_any_password_is_checked() {
    let server = with_captcha("test_answer = \"12345\"");
    let issued = issue(&server);
    let right = json!({"id": issued["id"], "answer": "12345"});
    assert_eq!(register(&server, "sara_x", Some(right)).0, 200);
    let login = json!({"username": "sara_x", "password": "not the right password"});
    let (status, reply) = server.call("POST", "/api/v1/login", None, Some(&login));
    assert_eq!(status, 400);
    assert_eq!(reply["code"], "captcha_wrong");
    let issued = issue(&server);
    let login = json!({
        "username": "sara_x",
        "password": PASSWORD,
        "captcha": {"id": issued["id"], "answer": "12345"},
    });
    let (status, reply) = server.call("POST", "/api/v1/login", None, Some(&login));
    assert_eq!(status, 200, "{reply}");
}

#[test]
fn persian_and_arabic_digits_read_as_latin_and_case_is_a_setting() {
    let server = with_captcha("test_answer = \"12345\"");
    for (username, answer) in [("sara_x", "۱۲۳۴۵"), ("babak_x", "١٢٣٤٥")] {
        let issued = issue(&server);
        let answer = json!({"id": issued["id"], "answer": answer});
        assert_eq!(register(&server, username, Some(answer)).0, 200);
    }
    drop(server);

    let server = with_captcha("test_answer = \"AbC\"");
    let issued = issue(&server);
    let answer = json!({"id": issued["id"], "answer": "abc"});
    assert_eq!(register(&server, "sara_x", Some(answer)).0, 200);
    drop(server);

    let server = with_captcha("test_answer = \"AbC\"\ncase_sensitive = true");
    let issued = issue(&server);
    let answer = json!({"id": issued["id"], "answer": "abc"});
    assert_eq!(register(&server, "sara_x", Some(answer)).0, 400);
}

#[test]
fn an_expired_one_fails_and_the_sweep_removes_old_ones() {
    let server = with_captcha(
        "test_answer = \"12345\"\nttl = \"1s\"\nsweep_every = \"1s\"\nkeep_for = \"1s\"",
    );
    let expired = issue(&server);
    let swept = issue(&server);
    std::thread::sleep(std::time::Duration::from_millis(1200));
    let answer = json!({"id": expired["id"], "answer": "12345"});
    assert_eq!(
        register(&server, "sara_x", Some(answer)).1["code"],
        "captcha_wrong"
    );
    std::thread::sleep(std::time::Duration::from_millis(2200));
    let answer = json!({"id": swept["id"], "answer": "12345"});
    assert_eq!(
        register(&server, "sara_x", Some(answer)).1["code"],
        "captcha_wrong"
    );
}

#[test]
fn a_pass_code_is_accepted_and_the_drawn_answer_still_is() {
    let server = with_captcha("test_answer = \"12345\"\npass_code = \"THE CODE\"");
    let issued = issue(&server);
    let drawn = json!({"id": issued["id"], "answer": "12345"});
    assert_eq!(register(&server, "sara_x", Some(drawn)).0, 200);
    let issued = issue(&server);
    let pass = json!({"id": issued["id"], "answer": "the code"});
    assert_eq!(register(&server, "babak_x", Some(pass.clone())).0, 200);
    assert_eq!(
        register(&server, "reza_x", Some(pass)).1["code"],
        "captcha_wrong",
        "the earlier try used it up"
    );

    let server = with_captcha("pass_code = \"THE CODE\"\ncase_sensitive = true");
    let issued = issue(&server);
    let pass = json!({"id": issued["id"], "answer": "the code"});
    assert_eq!(
        register(&server, "sara_x", Some(pass)).1["code"],
        "captcha_wrong"
    );
    let issued = issue(&server);
    let pass = json!({"id": issued["id"], "answer": "THE CODE"});
    assert_eq!(register(&server, "sara_x", Some(pass)).0, 200);
}

#[test]
fn a_pass_code_warns_at_startup() {
    let mut server = with_captcha("pass_code = \"THE CODE\"");
    server.stop();
    let log = std::fs::read_to_string(server.path("server.log")).unwrap();
    assert!(log.contains("A captcha pass code is set"));
    assert!(!log.contains("THE CODE"));
}

#[test]
fn test_answer_warns_at_startup() {
    let mut server = with_captcha("test_answer = \"12345\"");
    server.stop();
    let log = std::fs::read_to_string(server.path("server.log")).unwrap();
    assert!(log.contains("Every captcha has one fixed answer"));
}
