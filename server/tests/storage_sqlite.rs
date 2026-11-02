//! The schema's rules, and the storage trait's own promises.

use keryx::storage::sqlite::Sqlite;
use keryx::storage::{BuiltInRole, NewAccount, SetupLink, Storage};
use keryx_core::ulid::Ulid;
use rusqlite::{Connection, params};
use tempfile::TempDir;

const SCHEMA: &str = include_str!("../src/storage/sqlite.sql");

fn database() -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .unwrap();
    connection.execute_batch(SCHEMA).unwrap();
    connection
}

fn id(byte: u8) -> [u8; 16] {
    [byte; 16]
}

fn account(connection: &Connection, byte: u8, username: &str) {
    connection
        .execute(
            "INSERT INTO account (id, username, created_at) VALUES (?1, ?2, 1)",
            params![id(byte), username],
        )
        .unwrap();
}

fn device(connection: &Connection, byte: u8, owner: u8) {
    connection
        .execute(
            "INSERT INTO device (id, account_id, signing_public_key, master_stamp, platform, os,
             os_version, first_ip, last_ip, created_at, last_active_at)
             VALUES (?1, ?2, x'00', x'00', 0, 'linux', '7', 'ip', 'ip', 1, 1)",
            params![id(byte), id(owner)],
        )
        .unwrap();
}

fn direct(connection: &Connection, byte: u8, low: u8, high: u8) -> rusqlite::Result<usize> {
    connection.execute(
        "INSERT INTO conversation (id, type, direct_low, direct_high, created_at)
         VALUES (?1, ?2, ?3, ?4, 1)",
        params![id(byte), if low == high { 0 } else { 1 }, id(low), id(high)],
    )
}

fn message(
    connection: &Connection,
    chat: u8,
    seq: i64,
    device_byte: u8,
    kind: i64,
    content_kind: Option<i64>,
    epoch: Option<i64>,
) -> rusqlite::Result<usize> {
    connection.execute(
        "INSERT INTO message (conversation_id, seq, device_id, kind, content_kind, encrypted, epoch,
         client_sent_at, server_saved_at, signature, body)
         VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, 1, 1, x'00', x'00')",
        params![id(chat), seq, id(device_byte), kind, content_kind, epoch],
    )
}

/// A world with two accounts, a device each, and a chat.
fn world() -> Connection {
    let connection = database();
    account(&connection, 1, "sara_x");
    account(&connection, 2, "babak_x");
    device(&connection, 11, 1);
    device(&connection, 12, 2);
    direct(&connection, 30, 1, 2).unwrap();
    connection
}

#[test]
fn every_field_names_the_feature_that_needs_it() {
    let mut inside_table = false;
    let mut commented = false;
    let mut checked = 0;
    for line in SCHEMA.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("CREATE TABLE") {
            inside_table = true;
            commented = false;
            continue;
        }
        if trimmed.starts_with(')') && inside_table && !line.starts_with(' ') {
            inside_table = false;
            continue;
        }
        if !inside_table {
            continue;
        }
        if trimmed.starts_with("-- [") {
            commented = true;
            continue;
        }
        if trimmed.starts_with("--") {
            continue;
        }
        let mut words = trimmed.split_whitespace();
        let first = words.next().unwrap_or("");
        let second = words.next().unwrap_or("");
        let is_column = first.chars().all(|c| c.is_ascii_lowercase() || c == '_')
            && !first.is_empty()
            && ["BLOB", "TEXT", "INTEGER"].contains(&second);
        if is_column {
            assert!(commented, "{first} names no feature");
            commented = false;
            checked += 1;
        }
    }
    assert!(checked > 90, "the check read only {checked} fields");
}

#[test]
fn every_table_is_strict() {
    let connection = database();
    let mut statement = connection
        .prepare("SELECT name, strict FROM pragma_table_list WHERE schema = 'main' AND type = 'table' AND name NOT LIKE 'sqlite_%'")
        .unwrap();
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .unwrap();
    let mut count = 0;
    for row in rows {
        let (name, strict) = row.unwrap();
        assert_eq!(strict, 1, "{name}");
        count += 1;
    }
    assert!(count > 20, "{count}");
}

#[test]
fn usernames_are_lowercase_handles_not_starting_with_a_digit() {
    let connection = database();
    for username in ["", "Sara", "1sara", "سارا", "sa ra", "sa-ra"] {
        assert!(
            connection
                .execute(
                    "INSERT INTO account (id, username, created_at) VALUES (?1, ?2, 1)",
                    params![id(1), username],
                )
                .is_err(),
            "{username}"
        );
    }
    account(&connection, 1, "s");
    assert!(
        connection
            .execute(
                "INSERT INTO account (id, username, created_at) VALUES (?1, 's', 1)",
                params![id(2)],
            )
            .is_err()
    );
}

#[test]
fn a_pair_gets_one_chat_and_their_order_is_fixed() {
    let connection = world();
    assert!(direct(&connection, 31, 1, 2).is_err());
    assert!(
        direct(&connection, 31, 2, 1).is_err(),
        "low must come first"
    );
    direct(&connection, 31, 1, 1).unwrap();
    assert!(direct(&connection, 32, 1, 1).is_err(), "one self chat");
    assert!(
        connection
            .execute(
                "INSERT INTO conversation (id, type, created_at) VALUES (?1, 1, 1)",
                params![id(33)],
            )
            .is_err(),
        "a direct chat names its pair"
    );
    assert!(
        connection
            .execute(
                "INSERT INTO conversation (id, type, direct_low, direct_high, created_at)
                 VALUES (?1, 2, ?2, ?3, 1)",
                params![id(33), id(1), id(2)],
            )
            .is_err(),
        "a group has no pair"
    );
    connection
        .execute(
            "INSERT INTO conversation (id, type, created_at) VALUES (?1, 2, 1)",
            params![id(33)],
        )
        .unwrap();
}

#[test]
fn a_message_is_whole_or_only_its_sequence_id() {
    let connection = world();
    message(&connection, 30, 1, 11, 0, Some(0), Some(1)).unwrap();

    // Partly wiped is refused, field by field.
    for column in [
        "device_id",
        "encrypted",
        "client_sent_at",
        "server_saved_at",
        "signature",
        "body",
    ] {
        assert!(
            connection
                .execute(
                    &format!("UPDATE message SET {column} = NULL WHERE seq = 1"),
                    [],
                )
                .is_err(),
            "{column}"
        );
    }
    connection
        .execute(
            "UPDATE message SET device_id = NULL, kind = NULL, content_kind = NULL, encrypted = NULL,
             epoch = NULL, client_sent_at = NULL, server_saved_at = NULL, signature = NULL, body = NULL
             WHERE seq = 1",
            [],
        )
        .unwrap();
}

#[test]
fn kinds_and_their_fields_go_together() {
    let connection = world();
    // 10 is media_bundle, which is not built, and 11 does not exist.
    assert!(message(&connection, 30, 1, 11, 0, Some(10), Some(1)).is_err());
    assert!(message(&connection, 30, 1, 11, 5, None, Some(1)).is_err());
    assert!(message(&connection, 30, 1, 11, 0, None, Some(1)).is_err());
    assert!(message(&connection, 30, 1, 11, 1, Some(0), Some(1)).is_err());
    // An edit or delete needs its target.
    assert!(message(&connection, 30, 1, 11, 1, None, Some(1)).is_err());
    assert!(message(&connection, 30, 1, 11, 2, None, Some(1)).is_err());
    // An encrypted message and a commit need their epoch.
    assert!(message(&connection, 30, 1, 11, 0, Some(0), None).is_err());
    assert!(message(&connection, 30, 1, 11, 3, None, None).is_err());
    // A notice is unencrypted.
    assert!(message(&connection, 30, 1, 11, 4, None, Some(1)).is_err());
}

#[test]
fn the_first_commit_for_an_epoch_wins() {
    let connection = world();
    message(&connection, 30, 1, 11, 3, None, Some(1)).unwrap();
    assert!(message(&connection, 30, 2, 12, 3, None, Some(1)).is_err());
    message(&connection, 30, 2, 12, 3, None, Some(2)).unwrap();
    // A message under the same epoch is not a commit, so it is fine.
    message(&connection, 30, 3, 12, 0, Some(0), Some(2)).unwrap();
    // One sequence id, once.
    assert!(message(&connection, 30, 3, 12, 0, Some(0), Some(2)).is_err());
}

#[test]
fn a_device_has_both_its_end_and_its_statement_or_neither() {
    let connection = world();
    for set in ["terminated_at = 5", "revocation = x'00'"] {
        assert!(
            connection
                .execute(
                    &format!("UPDATE device SET {set} WHERE id = ?1"),
                    params![id(11)]
                )
                .is_err(),
            "{set}"
        );
    }
    connection
        .execute(
            "UPDATE device SET terminated_at = 5, revocation = x'00' WHERE id = ?1",
            params![id(11)],
        )
        .unwrap();
}

#[test]
fn foreign_keys_hold_and_a_chat_takes_its_rows_with_it() {
    let connection = world();
    assert!(
        connection
            .execute(
                "INSERT INTO member (conversation_id, account_id, joined_seq) VALUES (?1, ?2, 0)",
                params![id(30), id(99)],
            )
            .is_err()
    );
    connection
        .execute(
            "INSERT INTO member (conversation_id, account_id, joined_seq) VALUES (?1, ?2, 0)",
            params![id(30), id(1)],
        )
        .unwrap();
    message(&connection, 30, 1, 11, 0, Some(0), Some(1)).unwrap();
    connection
        .execute("DELETE FROM conversation WHERE id = ?1", params![id(30)])
        .unwrap();
    let left: i64 = connection
        .query_row("SELECT COUNT(*) FROM message", [], |row| row.get(0))
        .unwrap();
    assert_eq!(left, 0);
    let left: i64 = connection
        .query_row("SELECT COUNT(*) FROM member", [], |row| row.get(0))
        .unwrap();
    assert_eq!(left, 0);
    // Devices are never deleted while their account lives.
    assert!(
        connection
            .execute("DELETE FROM account WHERE id = ?1", params![id(1)])
            .is_err()
    );
}

#[test]
fn a_challenge_has_only_the_three_purposes() {
    let connection = world();
    for purpose in [0, 1, 2] {
        connection
            .execute(
                "INSERT INTO challenge (value, account_id, purpose, expires_at) VALUES (?1, ?2, ?3, 1)",
                params![vec![purpose as u8], id(1), purpose],
            )
            .unwrap();
    }
    assert!(
        connection
            .execute(
                "INSERT INTO challenge (value, account_id, purpose, expires_at) VALUES (x'09', ?1, 3, 1)",
                params![id(1)],
            )
            .is_err()
    );
}

#[tokio::test]
async fn one_file_with_the_classic_journal_and_opening_twice_keeps_everything() {
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("keryx.db");
    let storage = Sqlite::open(&path).unwrap();
    let role_id = Ulid::new().unwrap();
    let role = || BuiltInRole {
        id: role_id,
        scope: 0,
        name: "member",
        permission_list: vec!["directory.search"],
    };
    storage.seed_roles(vec![role()]).await.unwrap();
    // A second seed leaves it as it is, and does not fail.
    storage.seed_roles(vec![role()]).await.unwrap();
    assert_eq!(
        storage.permission_names().await.unwrap(),
        vec!["directory.search".to_string()]
    );
    let id = Ulid::new().unwrap();
    let new_account = NewAccount {
        id,
        username: "sara_x".to_string(),
        display_name: None,
        password_hash: None,
        admin: false,
        setup: None,
        now: 1,
    };
    assert!(storage.create_account(new_account).await.unwrap());
    assert!(
        storage
            .server_permitted(id, "directory.search")
            .await
            .unwrap()
    );
    assert!(
        !storage
            .server_permitted(id, "account.create")
            .await
            .unwrap()
    );
    drop(storage);

    let mut name_list = Vec::new();
    for entry in std::fs::read_dir(directory.path()).unwrap() {
        name_list.push(entry.unwrap().file_name().to_string_lossy().to_string());
    }
    assert_eq!(name_list, vec!["keryx.db".to_string()]);
    let check = Connection::open(&path).unwrap();
    let mode: String = check
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .unwrap();
    assert_eq!(mode, "delete");
    drop(check);

    let again = Sqlite::open(&path).unwrap();
    let account = again.account_by_id(id).await.unwrap().unwrap();
    assert_eq!(account.username, "sara_x");
}

#[tokio::test]
async fn a_setup_link_says_whether_it_is_unknown_used_or_expired() {
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("keryx.db");
    let storage = Sqlite::open(&path).unwrap();
    let id = Ulid::new().unwrap();
    let hash = [7u8; 32];
    storage
        .create_account(NewAccount {
            id,
            username: "sara_x".to_string(),
            display_name: None,
            password_hash: None,
            admin: false,
            setup: Some((hash, 100)),
            now: 1,
        })
        .await
        .unwrap();
    assert!(matches!(
        storage.setup_link([9u8; 32], 50).await.unwrap(),
        SetupLink::Unknown
    ));
    let SetupLink::Ready(ready) = storage.setup_link(hash, 50).await.unwrap() else {
        panic!("the link is inside its 7 days");
    };
    assert_eq!(ready.username, "sara_x");
    assert!(
        storage
            .finish_setup(id, hash, "hash".to_string(), 50)
            .await
            .unwrap()
    );
    assert!(matches!(
        storage.setup_link(hash, 500).await.unwrap(),
        SetupLink::Used
    ));

    let expired = Ulid::new().unwrap();
    storage
        .create_account(NewAccount {
            id: expired,
            username: "old_x".to_string(),
            display_name: None,
            password_hash: None,
            admin: false,
            setup: Some(([8u8; 32], 100)),
            now: 1,
        })
        .await
        .unwrap();
    assert!(matches!(
        storage.setup_link([8u8; 32], 101).await.unwrap(),
        SetupLink::Expired
    ));
    assert!(
        !storage
            .finish_setup(expired, [8u8; 32], "hash".to_string(), 101)
            .await
            .unwrap()
    );
}

#[test]
fn opening_the_first_database_keeps_a_used_link() {
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("keryx.db");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE account_setup (
                 token_hash BLOB PRIMARY KEY,
                 account_id BLOB NOT NULL,
                 expires_at INTEGER NOT NULL
             ) WITHOUT ROWID, STRICT;
             PRAGMA user_version = 1;",
        )
        .unwrap();
    drop(connection);
    Sqlite::open(&path).unwrap();
    let check = Connection::open(&path).unwrap();
    let version: i64 = check
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 2);
    let names: Vec<String> = check
        .prepare("PRAGMA table_info(account_setup)")
        .unwrap()
        .query_map([], |row| row.get(1))
        .unwrap()
        .map(|name| name.unwrap())
        .collect();
    assert!(names.contains(&"used_at".to_string()));
}
