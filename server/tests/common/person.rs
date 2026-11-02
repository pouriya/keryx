//! A person with an account, their words' keys, and one registered device: `keryx-core` as the client.
//! For the tests that talk as one. Apart from `mod.rs`, because a test file that has no use for it would
//! fail clippy on dead code, and `#[allow]` is not used here.

use keryx_core::mls::{AccountKey, Device, stamp_account_key};
use keryx_core::protocol::Bytes;
use keryx_core::seed::{AccountKeys, derive};
use keryx_core::ulid::Ulid;
use serde_json::{Value, json};

use crate::common::Server;

pub const PASSWORD: &str = "a long enough password";
pub const SARA_WORDS: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
pub const BABAK_WORDS: &str =
    "legal winner thank year wave sausage worth useful legal winner thank yellow";

pub struct Person {
    pub id: Ulid,
    pub username: String,
    pub keys: AccountKeys,
    pub key: AccountKey,
    pub self_chat: Ulid,
    pub token: String,
    pub device: Device,
}

pub fn ulid_of(value: &Value) -> Ulid {
    Ulid::parse(value.as_str().unwrap()).unwrap()
}

/// A password login, with no device.
pub fn log_in(server: &Server, username: &str) -> String {
    let (status, reply) = server.call(
        "POST",
        "/api/v1/login/token",
        None,
        Some(&json!({"username": username, "password": PASSWORD})),
    );
    assert_eq!(status, 200, "{reply}");
    reply["data"]["token"].as_str().unwrap().to_string()
}

/// Asks for an id, stamps it, and registers it on this session.
pub fn add_device(server: &Server, account: Ulid, keys: &AccountKeys, token: &str) -> Device {
    let (status, reply) = server.call("POST", "/api/v1/devices/id", Some(token), None);
    assert_eq!(status, 200, "{reply}");
    let id = ulid_of(&reply["data"]["device"]);
    let device = Device::create(keys, account, id).unwrap();
    let record = device.record();
    let (status, reply) = server.call(
        "POST",
        "/api/v1/devices",
        Some(token),
        Some(&json!({
            "device": id,
            "public_key": Bytes(record.device_public.to_vec()),
            "stamp": Bytes(record.stamp.to_vec()),
            "platform": 0,
            "os": "linux",
            "os_version": "7",
        })),
    );
    assert_eq!(status, 200, "{reply}");
    device
}

/// Registers, logs in, sets up the words, adds a device, and uploads three key packages.
pub fn sign_up(server: &Server, username: &str, words: &str) -> Person {
    let (status, reply) = server.call(
        "POST",
        "/api/v1/register",
        None,
        Some(&json!({"username": username, "password": PASSWORD})),
    );
    assert_eq!(status, 200, "{reply}");
    let id = ulid_of(&reply["data"]["account"]);
    let token = log_in(server, username);
    let keys = derive(words).unwrap();
    let key = AccountKey {
        account: id,
        account_public: *keys.account_public(),
        master_public: *keys.master_public(),
        stamp: stamp_account_key(&keys, &id),
    };
    let (status, reply) = server.call(
        "PUT",
        "/api/v1/account/keys",
        Some(&token),
        Some(&json!({
            "master_public": Bytes(key.master_public.to_vec()),
            "account_public": Bytes(key.account_public.to_vec()),
            "account_stamp": Bytes(key.stamp.to_vec()),
        })),
    );
    assert_eq!(status, 200, "{reply}");
    let self_chat = ulid_of(&reply["data"]["self_chat"]);
    let device = add_device(server, id, &keys, &token);
    let mut body_list = Vec::new();
    for _ in 0..3 {
        body_list.push(Bytes(device.key_package().unwrap()));
    }
    let (status, reply) = server.call(
        "POST",
        "/api/v1/key_packages",
        Some(&token),
        Some(&json!({"body_list": body_list})),
    );
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["data"]["count"], 3);
    Person {
        id,
        username: username.to_string(),
        keys,
        key,
        self_chat,
        token,
        device,
    }
}
