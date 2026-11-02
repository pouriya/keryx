use keryx_core::seed::derive;
use keryx_core::ulid::Ulid;
use keryx_core::wrap::{WRAP_SIZE, WrapError, unwrap, wrap};

const WORDS: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const SECRET: [u8; 32] = [7; 32];
const KEY: [u8; 16] = [9; 16];

fn ids() -> (Ulid, Ulid) {
    (Ulid::from_bytes([1; 16]), Ulid::from_bytes([2; 16]))
}

fn hex(bytes: &[u8]) -> String {
    let mut text = String::new();
    for byte in bytes {
        text.push_str(&format!("{byte:02x}"));
    }
    text
}

/// **If this fails, no stored key opens any more.** These bytes were computed from the spec by a
/// separate implementation, on Node and OpenSSL. Never
/// update them to make the test pass.
#[test]
fn wrap_bytes_are_pinned() {
    let keys = derive(WORDS).unwrap();
    let (chat, account) = ids();
    let wrapped = wrap(&SECRET, &chat, 3, &account, keys.account_public(), &KEY).unwrap();
    assert_eq!(wrapped.len(), WRAP_SIZE);
    assert_eq!(
        hex(&wrapped),
        "013f64624b1b2a3cfb9d916f1f3c63bad8d525132a441e959b681e7b31545a860bc440968d4cb784655afdf360396e27f9249160f325be494280654e8515182c07"
    );
}

#[test]
fn wrapping_is_repeatable_and_opens_with_the_words() {
    let keys = derive(WORDS).unwrap();
    let (chat, account) = ids();
    let first = wrap(&SECRET, &chat, 3, &account, keys.account_public(), &KEY).unwrap();
    let second = wrap(&SECRET, &chat, 3, &account, keys.account_public(), &KEY).unwrap();
    assert_eq!(first, second);
    let opened = unwrap(&first, &chat, 3, &account, keys.account_private()).unwrap();
    assert_eq!(*opened, KEY);
}

#[test]
fn each_account_gets_a_different_ephemeral_key() {
    let keys = derive(WORDS).unwrap();
    let (chat, account) = ids();
    let other = Ulid::from_bytes([3; 16]);
    let first = wrap(&SECRET, &chat, 3, &account, keys.account_public(), &KEY).unwrap();
    let second = wrap(&SECRET, &chat, 3, &other, keys.account_public(), &KEY).unwrap();
    assert_ne!(first[1..33], second[1..33]);
}

#[test]
fn a_wrap_cannot_be_moved_to_another_chat_epoch_or_account() {
    let keys = derive(WORDS).unwrap();
    let (chat, account) = ids();
    let other = Ulid::from_bytes([3; 16]);
    let wrapped = wrap(&SECRET, &chat, 3, &account, keys.account_public(), &KEY).unwrap();
    let private = keys.account_private();
    assert!(matches!(
        unwrap(&wrapped, &other, 3, &account, private),
        Err(WrapError::Open)
    ));
    assert!(matches!(
        unwrap(&wrapped, &chat, 4, &account, private),
        Err(WrapError::Open)
    ));
    assert!(matches!(
        unwrap(&wrapped, &chat, 3, &other, private),
        Err(WrapError::Open)
    ));
}

#[test]
fn another_persons_words_do_not_open_it() {
    let keys = derive(WORDS).unwrap();
    let other =
        derive("legal winner thank year wave sausage worth useful legal winner thank yellow")
            .unwrap();
    let (chat, account) = ids();
    let wrapped = wrap(&SECRET, &chat, 3, &account, keys.account_public(), &KEY).unwrap();
    assert!(matches!(
        unwrap(&wrapped, &chat, 3, &account, other.account_private()),
        Err(WrapError::Open)
    ));
}

#[test]
fn damaged_wraps_are_refused() {
    let keys = derive(WORDS).unwrap();
    let (chat, account) = ids();
    let private = keys.account_private();
    let wrapped = wrap(&SECRET, &chat, 3, &account, keys.account_public(), &KEY).unwrap();

    assert!(matches!(
        unwrap(&wrapped[..64], &chat, 3, &account, private),
        Err(WrapError::Size { size: 64 })
    ));
    let mut newer = wrapped;
    newer[0] = 2;
    assert!(matches!(
        unwrap(&newer, &chat, 3, &account, private),
        Err(WrapError::Version { version: 2 })
    ));
    let mut flipped = wrapped;
    flipped[64] ^= 1;
    assert!(matches!(
        unwrap(&flipped, &chat, 3, &account, private),
        Err(WrapError::Open)
    ));
}

#[test]
fn a_low_order_key_is_refused() {
    let keys = derive(WORDS).unwrap();
    let (chat, account) = ids();
    assert!(matches!(
        wrap(&SECRET, &chat, 3, &account, &[0; 32], &KEY),
        Err(WrapError::WeakKey)
    ));
    let mut wrapped = [0u8; WRAP_SIZE];
    wrapped[0] = 1;
    assert!(matches!(
        unwrap(&wrapped, &chat, 3, &account, keys.account_private()),
        Err(WrapError::WeakKey)
    ));
}
