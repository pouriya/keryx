use keryx_core::seed::{SeedError, derive, generate};

const WORDS: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

fn hex(bytes: &[u8]) -> String {
    let mut text = String::new();
    for byte in bytes {
        text.push_str(&format!("{byte:02x}"));
    }
    text
}

/// **If this fails, nobody's words give their keys any more.** These values were computed from the
/// spec by a separate implementation, on Node and OpenSSL. Never update them to make the test pass.
#[test]
fn derivation_is_pinned() {
    let keys = derive(WORDS).unwrap();
    assert_eq!(
        hex(keys.master_private()),
        "f9c5f923074f04a305586459159a65216b558f1728cc9a71dd2dc4e7f783e14f"
    );
    assert_eq!(
        hex(keys.master_public()),
        "cdb6aeec27098b4be3891592cfaf563192c0cde869935c445c45ca1a67227e9f"
    );
    assert_eq!(
        hex(keys.account_private()),
        "c9043f03ec087997dff0186e2e41b379acf3aa028fd3b0d65b5852e8d82305be"
    );
    assert_eq!(
        hex(keys.account_public()),
        "a8b4afab143861e81ee0eb9a14f1ed610f06a9f947685928a5a992553f9c7a0e"
    );
}

#[test]
fn same_words_same_keys() {
    let first = derive(WORDS).unwrap();
    let second = derive(WORDS).unwrap();
    assert_eq!(first.master_private(), second.master_private());
    assert_eq!(first.account_private(), second.account_private());
    assert_ne!(first.master_private(), first.account_private());
}

#[test]
fn account_public_key_matches_its_private_key() {
    let keys = derive(WORDS).unwrap();
    let private = x25519_dalek::StaticSecret::from(*keys.account_private());
    let public = x25519_dalek::PublicKey::from(&private);
    assert_eq!(public.as_bytes(), keys.account_public());
}

#[test]
fn generated_phrases_have_the_chosen_length_and_differ() {
    let twelve = generate(12).unwrap();
    assert_eq!(twelve.split_whitespace().count(), 12);
    assert_eq!(generate(24).unwrap().split_whitespace().count(), 24);
    assert_ne!(*twelve, *generate(12).unwrap());
    derive(&twelve).unwrap();
}

#[test]
fn only_twelve_or_twenty_four_words_are_generated() {
    assert!(matches!(
        generate(18),
        Err(SeedError::WordCount { words: 18 })
    ));
}

#[test]
fn restoring_accepts_any_valid_length() {
    let fifteen = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon \
                   abandon abandon abandon abandon address";
    derive(fifteen).unwrap();
}

#[test]
fn bad_phrases_are_refused_without_naming_a_word() {
    let wrong_checksum = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon";
    for phrase in [wrong_checksum, "secretword abandon", ""] {
        let Err(error) = derive(phrase) else {
            panic!("a bad phrase was accepted");
        };
        assert!(matches!(error, SeedError::Phrase { .. }));
        assert!(!format!("{error:?} {error}").contains("secretword"));
    }
}
