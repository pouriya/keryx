use keryx_core::call::{CallError, CallKey, Opener, SUITE, Sealer, State};
use keryx_core::mls::{AccountKey, Device, MlsError, stamp_account_key};
use keryx_core::protocol::Bytes;
use keryx_core::seed::derive;
use keryx_core::ulid::Ulid;
use sframe::frame::{MediaFrame, MonotonicCounter};
use sframe::key::EncryptionKey;
use sframe::mls::{MlsKeyId, MlsKeyIdBitRange};

const SARA_WORDS: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const BABAK_WORDS: &str =
    "legal winner thank year wave sausage worth useful legal winner thank yellow";
const NOW: i64 = 1_800_000_000_000;

fn key(words: &str, id: u8) -> (keryx_core::seed::AccountKeys, AccountKey) {
    let keys = derive(words).unwrap();
    let id = Ulid::from_bytes([id; 16]);
    let key = AccountKey {
        account: id,
        account_public: *keys.account_public(),
        master_public: *keys.master_public(),
        stamp: stamp_account_key(&keys, &id),
    };
    (keys, key)
}

fn chat() -> Ulid {
    Ulid::from_bytes([50; 16])
}

/// Sara and Babak, one device each, in one chat.
fn direct_chat() -> (Device, Device, [AccountKey; 2]) {
    let (sara_keys, sara) = key(SARA_WORDS, 1);
    let (babak_keys, babak) = key(BABAK_WORDS, 2);
    let mut sara_device = Device::create(&sara_keys, sara.account, Ulid::new().unwrap()).unwrap();
    let mut babak_device =
        Device::create(&babak_keys, babak.account, Ulid::new().unwrap()).unwrap();
    sara_device
        .create_chat(chat(), NOW, std::slice::from_ref(&sara))
        .unwrap();
    let package = babak_device.key_package().unwrap();
    let accounts = [sara, babak];
    let commit = sara_device
        .add_devices(chat(), &[package], &accounts)
        .unwrap();
    sara_device.commit_accepted(chat(), NOW).unwrap();
    babak_device
        .join_welcome(chat(), commit.welcome.as_ref().unwrap(), NOW)
        .unwrap();
    (sara_device, babak_device, accounts)
}

#[test]
fn both_devices_get_the_same_secret_and_each_its_own_index() {
    let (sara, babak, _) = direct_chat();
    let (sara_key, sara_list) = sara.call_key(chat()).unwrap();
    let (babak_key, babak_list) = babak.call_key(chat()).unwrap();
    assert_eq!(sara_key.secret, babak_key.secret);
    assert_eq!(sara_key.epoch, babak_key.epoch);
    assert_ne!(sara_key.index, babak_key.index);
    assert_eq!(sara_list, babak_list);
    assert_eq!(sara_list.len(), 2);
    let sara_index = sara_list
        .iter()
        .find(|entry| entry.device == sara.record().device)
        .unwrap();
    assert_eq!(sara_index.index, sara_key.index);
    // The secret never shows in a debug print.
    assert!(!format!("{sara_key:?}").contains("secret"));
    assert!(matches!(
        sara.call_key(Ulid::from_bytes([9; 16])),
        Err(MlsError::UnknownChat { .. })
    ));
}

#[test]
fn a_frame_opens_for_the_other_side_only_as_its_sender_and_unchanged() {
    let (sara, babak, _) = direct_chat();
    let (sara_key, _) = sara.call_key(chat()).unwrap();
    let (babak_key, _) = babak.call_key(chat()).unwrap();
    let mut sealer = Sealer::new(&sara_key).unwrap();
    let mut opener = Opener::default();
    opener.add(&babak_key);

    let sealed = sealer.seal(40_000, b"a keyframe").unwrap();
    let (timestamp, codec) = opener.open(sara_key.index, &sealed).unwrap();
    assert_eq!(timestamp, 40_000);
    assert_eq!(codec.as_slice(), b"a keyframe");
    // A second frame uses the next counter, so the two differ even with the same bytes.
    let again = sealer.seal(40_000, b"a keyframe").unwrap();
    assert_ne!(again, sealed);
    assert_eq!(opener.open(sara_key.index, &again).unwrap().0, 40_000);

    // Carried on Babak's broadcast, it is not Babak's.
    assert!(matches!(
        opener.open(babak_key.index, &sealed),
        Err(CallError::WrongSender { .. })
    ));
    let mut changed = sealed.clone();
    let last = changed.len() - 1;
    changed[last] ^= 1;
    assert!(matches!(
        opener.open(sara_key.index, &changed),
        Err(CallError::Open)
    ));
    assert!(matches!(
        opener.open(sara_key.index, &[]),
        Err(CallError::Open)
    ));
    let state = sealer.seal_state(&State {
        video: true,
        voice: false,
        width: Some(640),
        height: Some(360),
        rotation: Some(90),
    });
    assert!(
        opener
            .open_state(sara_key.index, &state.unwrap())
            .unwrap()
            .video
    );
}

#[test]
fn state_says_what_is_on_and_junk_is_refused() {
    let (sara, babak, _) = direct_chat();
    let (sara_key, _) = sara.call_key(chat()).unwrap();
    let (babak_key, _) = babak.call_key(chat()).unwrap();
    let mut sealer = Sealer::new(&sara_key).unwrap();
    let mut opener = Opener::default();
    opener.add(&babak_key);
    let state = State {
        video: false,
        voice: true,
        width: None,
        height: None,
        rotation: None,
    };
    let sealed = sealer.seal_state(&state).unwrap();
    assert_eq!(opener.open_state(sara_key.index, &sealed).unwrap(), state);
    // A media frame is not a state, and too short a frame is not a media frame.
    let media = sealer.seal(1, b"x").unwrap();
    assert!(matches!(
        opener.open_state(sara_key.index, &media),
        Err(CallError::State(_))
    ));
    // A member sealing by hand, with too few bytes for a timestamp.
    let id = MlsKeyId::try_new(
        0u64,
        sara_key.epoch,
        u64::from(sara_key.index),
        MlsKeyIdBitRange::new(8, 16),
    )
    .unwrap();
    let key = EncryptionKey::derive_from(SUITE, id, &sara_key.secret.0).unwrap();
    let mut counter = MonotonicCounter::default();
    let tiny = MediaFrame::try_new(&mut counter, b"abc".as_slice())
        .unwrap()
        .encrypt(&key)
        .unwrap();
    assert!(matches!(
        opener.open(sara_key.index, tiny.sframe_cipher_text()),
        Err(CallError::Short)
    ));
}

#[test]
fn a_new_epoch_needs_its_own_key_and_old_frames_still_open() {
    let (mut sara, mut babak, accounts) = direct_chat();
    let (old_key, _) = sara.call_key(chat()).unwrap();
    let mut old_sealer = Sealer::new(&old_key).unwrap();
    let old_frame = old_sealer.seal(1, b"before").unwrap();

    let commit = sara.rotate(chat(), &accounts).unwrap();
    sara.commit_accepted(chat(), NOW).unwrap();
    babak.process_commit(chat(), &commit.commit, NOW).unwrap();
    let (new_key, _) = sara.call_key(chat()).unwrap();
    assert_eq!(new_key.epoch, old_key.epoch + 1);
    assert_ne!(new_key.secret, old_key.secret);
    let mut sealer = Sealer::new(&new_key).unwrap();
    let new_frame = sealer.seal(2, b"after").unwrap();

    let mut opener = Opener::default();
    opener.add(&old_key);
    // Not given the new epoch yet: its frames do not open.
    assert!(matches!(
        opener.open(new_key.index, &new_frame),
        Err(CallError::Open)
    ));
    let (babak_new, _) = babak.call_key(chat()).unwrap();
    opener.add(&babak_new);
    assert_eq!(opener.open(new_key.index, &new_frame).unwrap().0, 2);
    assert_eq!(opener.open(old_key.index, &old_frame).unwrap().0, 1);
}

#[test]
fn a_sealer_never_counts_from_the_same_place_twice() {
    let (sara, _, _) = direct_chat();
    let (key, _) = sara.call_key(chat()).unwrap();
    let first = Sealer::new(&key).unwrap().seal(1, b"same").unwrap();
    let second = Sealer::new(&key).unwrap().seal(1, b"same").unwrap();
    assert_ne!(first, second);
    // A key the suite cannot use is refused.
    let bad = CallKey {
        epoch: 0,
        index: 70_000,
        secret: Bytes(vec![1; 32]),
    };
    assert!(matches!(Sealer::new(&bad), Err(CallError::Key(_))));
}
