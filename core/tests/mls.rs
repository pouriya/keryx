use keryx_core::mls::{
    AccountKey, AccountWrap, Commit, Device, MlsError, ROTATION_MS, StateError, stamp_account_key,
    verify_message,
};
use keryx_core::protocol::{ContentKind, Header, Kind};
use keryx_core::seed::{AccountKeys, derive};
use keryx_core::ulid::Ulid;

const SARA_WORDS: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const BABAK_WORDS: &str =
    "legal winner thank year wave sausage worth useful legal winner thank yellow";
const NOW: i64 = 1_800_000_000_000;

struct Person {
    id: Ulid,
    keys: AccountKeys,
    key: AccountKey,
}

fn person(words: &str, id: u8) -> Person {
    let keys = derive(words).unwrap();
    let id = Ulid::from_bytes([id; 16]);
    let key = AccountKey {
        account: id,
        account_public: *keys.account_public(),
        master_public: *keys.master_public(),
        stamp: stamp_account_key(&keys, &id),
    };
    Person { id, keys, key }
}

fn device(person: &Person) -> Device {
    Device::create(&person.keys, person.id, Ulid::new().unwrap()).unwrap()
}

fn chat() -> Ulid {
    Ulid::from_bytes([50; 16])
}

fn header(sender: &Device, epoch: u64) -> Header {
    Header {
        chat: chat(),
        device: sender.record().device,
        kind: Kind::Content,
        content_kind: Some(ContentKind::Text),
        encrypted: true,
        epoch: Some(epoch),
        pointer_seq: None,
        call_seq: None,
        client_sent_at: NOW,
    }
}

/// Sara and Babak, one device each, in one chat.
fn direct_chat() -> (Person, Person, Device, Device, Commit) {
    let sara = person(SARA_WORDS, 1);
    let babak = person(BABAK_WORDS, 2);
    let mut sara_device = device(&sara);
    let mut babak_device = device(&babak);
    sara_device
        .create_chat(chat(), NOW, std::slice::from_ref(&sara.key))
        .unwrap();
    let package = babak_device.key_package().unwrap();
    let accounts = [sara.key.clone(), babak.key.clone()];
    let commit = sara_device
        .add_devices(chat(), &[package], &accounts)
        .unwrap();
    sara_device.commit_accepted(chat(), NOW).unwrap();
    babak_device
        .join_welcome(chat(), commit.welcome.as_ref().unwrap(), NOW)
        .unwrap();
    (sara, babak, sara_device, babak_device, commit)
}

#[test]
fn two_people_exchange_a_message_in_any_order_and_more_than_once() {
    let (_, _, sara_device, babak_device, commit) = direct_chat();
    assert_eq!(sara_device.epoch(chat()).unwrap(), commit.epoch);
    assert_eq!(babak_device.epoch(chat()).unwrap(), commit.epoch);

    let first = header(&sara_device, commit.epoch);
    let second = header(&sara_device, commit.epoch);
    let (first_body, first_signature) = sara_device.seal(&first, "سلام".as_bytes()).unwrap();
    let (second_body, second_signature) = sara_device.seal(&second, b"second").unwrap();
    let public = sara_device.record().device_public;

    let opened = babak_device
        .open(&second, &second_body, &second_signature, &public)
        .unwrap();
    assert_eq!(opened.as_slice(), b"second");
    for _ in 0..2 {
        let opened = babak_device
            .open(&first, &first_body, &first_signature, &public)
            .unwrap();
        assert_eq!(opened.as_slice(), "سلام".as_bytes());
    }
    assert_eq!(first_body.len(), 29 + "سلام".len());
    assert_eq!(first_body[0], 1);
}

#[test]
fn a_new_device_reads_everything_with_only_the_words() {
    let (sara, babak, sara_device, mut babak_device, first_commit) = direct_chat();
    let first = header(&sara_device, first_commit.epoch);
    let (body, signature) = sara_device.seal(&first, b"before").unwrap();

    // Babak lost his phone. Nobody else is online, and nobody approves anything.
    let mut new_device = device(&babak);
    let group_info = sara_device.group_info(chat()).unwrap();
    let accounts = [sara.key.clone(), babak.key.clone()];
    let join = new_device
        .join_external(chat(), &group_info, NOW, &accounts)
        .unwrap();
    new_device.commit_accepted(chat(), NOW).unwrap();
    assert_eq!(join.epoch, first_commit.epoch + 1);

    // It cannot read the old epoch until it opens the wrapped key with the words.
    let public = sara_device.record().device_public;
    assert!(matches!(
        new_device.open(&first, &body, &signature, &public),
        Err(MlsError::NoEpochKey { .. })
    ));
    let wrapped = wrap_for(&first_commit.wrap_list, babak.id);
    new_device
        .unwrap_history(&babak.keys, chat(), first_commit.epoch, &wrapped)
        .unwrap();
    let opened = new_device.open(&first, &body, &signature, &public).unwrap();
    assert_eq!(opened.as_slice(), b"before");

    // The other members apply the join and agree on the new key.
    let mut sara_device = sara_device;
    let processed = sara_device
        .process_commit(chat(), &join.commit, NOW)
        .unwrap();
    assert_eq!(processed.epoch, join.epoch);
    assert!(!processed.removed_self);
    assert!(!processed.group_info.is_empty());
    babak_device
        .process_commit(chat(), &join.commit, NOW)
        .unwrap();

    let message = header(&new_device, join.epoch);
    let (body, signature) = new_device.seal(&message, b"from the new phone").unwrap();
    let opened = sara_device
        .open(
            &message,
            &body,
            &signature,
            &new_device.record().device_public,
        )
        .unwrap();
    assert_eq!(opened.as_slice(), b"from the new phone");
    assert_eq!(sara_device.member_list(chat()).unwrap().len(), 3);

    // Each commit carries the group info for its epoch, even from a device that joined by itself,
    // so the next device joins from it without anyone uploading anything else.
    let rotation = new_device.rotate(chat(), &accounts).unwrap();
    new_device.commit_accepted(chat(), NOW).unwrap();
    sara_device
        .process_commit(chat(), &rotation.commit, NOW)
        .unwrap();
    let mut third_device = device(&babak);
    let join = third_device
        .join_external(chat(), &rotation.group_info, NOW, &accounts)
        .unwrap();
    assert_eq!(join.epoch, rotation.epoch + 1);
    third_device.commit_accepted(chat(), NOW).unwrap();
    let processed = sara_device
        .process_commit(chat(), &join.commit, NOW)
        .unwrap();
    assert_eq!(processed.epoch, join.epoch);
}

fn wrap_for(wrap_list: &[AccountWrap], account: Ulid) -> Vec<u8> {
    for wrap in wrap_list {
        if wrap.account == account {
            return wrap.wrapped.to_vec();
        }
    }
    panic!("no wrap for this account");
}

#[test]
fn signature_is_checked_with_public_keys_alone() {
    let (_, _, sara_device, _, commit) = direct_chat();
    let message = header(&sara_device, commit.epoch);
    let (body, signature) = sara_device.seal(&message, b"text").unwrap();
    let public = sara_device.record().device_public;
    verify_message(&message, &body, &signature, &public).unwrap();

    let mut tampered = body.clone();
    tampered[20] ^= 1;
    assert!(matches!(
        verify_message(&message, &tampered, &signature, &public),
        Err(MlsError::Signature)
    ));
    let mut moved = message.clone();
    moved.client_sent_at += 1;
    assert!(matches!(
        verify_message(&moved, &body, &signature, &public),
        Err(MlsError::Signature)
    ));
    assert!(matches!(
        verify_message(&message, &body, &signature, &[0; 32]),
        Err(MlsError::Signature)
    ));
}

#[test]
fn unencrypted_messages_are_signed_the_same_way() {
    let (_, _, sara_device, _, _) = direct_chat();
    let mut message = header(&sara_device, 0);
    message.encrypted = false;
    message.epoch = None;
    let signature = sara_device.sign(&message, b"announcement").unwrap();
    let public = sara_device.record().device_public;
    verify_message(&message, b"announcement", &signature, &public).unwrap();
    assert!(matches!(
        sara_device.seal(&message, b"x"),
        Err(MlsError::NotEncrypted)
    ));
    assert!(matches!(
        sara_device.open(&message, b"announcement", &signature, &public),
        Err(MlsError::NotEncrypted)
    ));
}

#[test]
fn a_body_that_is_signed_but_damaged_is_refused_clearly() {
    let (_, _, sara_device, babak_device, commit) = direct_chat();
    let public = sara_device.record().device_public;
    let message = header(&sara_device, commit.epoch);

    let empty = sara_device.sign(&message, b"").unwrap();
    assert!(matches!(
        babak_device.open(&message, b"", &empty, &public),
        Err(MlsError::BodyShort)
    ));
    let short = sara_device.sign(&message, &[1; 28]).unwrap();
    assert!(matches!(
        babak_device.open(&message, &[1; 28], &short, &public),
        Err(MlsError::BodyShort)
    ));
    let newer = sara_device.sign(&message, &[2; 40]).unwrap();
    assert!(matches!(
        babak_device.open(&message, &[2; 40], &newer, &public),
        Err(MlsError::BodyVersion { version: 2 })
    ));
    let garbage = sara_device.sign(&message, &[1; 40]).unwrap();
    assert!(matches!(
        babak_device.open(&message, &[1; 40], &garbage, &public),
        Err(MlsError::Decrypt)
    ));
    let mut other_epoch = message.clone();
    other_epoch.epoch = Some(99);
    let signature = sara_device.sign(&other_epoch, &[1; 40]).unwrap();
    assert!(matches!(
        babak_device.open(&other_epoch, &[1; 40], &signature, &public),
        Err(MlsError::NoEpochKey { epoch: 99, .. })
    ));
}

#[test]
fn a_device_seals_only_as_itself() {
    let (_, _, sara_device, babak_device, commit) = direct_chat();
    let message = header(&babak_device, commit.epoch);
    assert!(matches!(
        sara_device.seal(&message, b"x"),
        Err(MlsError::WrongDevice { .. })
    ));
}

#[test]
fn a_key_over_seven_days_old_must_be_rotated_before_sending() {
    let (sara, babak, mut sara_device, mut babak_device, commit) = direct_chat();
    let later = NOW + ROTATION_MS;
    assert!(!sara_device.rotation_due(chat(), later - 1).unwrap());
    assert!(sara_device.rotation_due(chat(), later).unwrap());

    let mut message = header(&sara_device, commit.epoch);
    message.client_sent_at = later;
    assert!(matches!(
        sara_device.seal(&message, b"x"),
        Err(MlsError::RotationDue { .. })
    ));

    let accounts = [sara.key.clone(), babak.key.clone()];
    let rotated = sara_device.rotate(chat(), &accounts).unwrap();
    assert_eq!(rotated.epoch, commit.epoch + 1);
    sara_device.commit_accepted(chat(), later).unwrap();
    babak_device
        .process_commit(chat(), &rotated.commit, later)
        .unwrap();
    assert!(!sara_device.rotation_due(chat(), later).unwrap());

    let mut message = header(&sara_device, rotated.epoch);
    message.client_sent_at = later;
    let (body, signature) = sara_device.seal(&message, b"x").unwrap();
    let public = sara_device.record().device_public;
    babak_device
        .open(&message, &body, &signature, &public)
        .unwrap();
}

#[test]
fn every_member_can_recompute_the_wraps_and_repair_a_bad_one() {
    let (sara, babak, _, mut babak_device, commit) = direct_chat();
    let accounts = [sara.key.clone(), babak.key.clone()];

    // The commit's wraps were honest, so Babak's device agrees with them all.
    assert!(
        babak_device
            .check_wraps(chat(), &commit.wrap_list, &accounts)
            .unwrap()
            .is_empty()
    );

    // One is bad and one is missing: the right bytes come back for each.
    let mut bad = commit.wrap_list[0].clone();
    bad.wrapped[64] ^= 1;
    let repair_list = babak_device.check_wraps(chat(), &[bad], &accounts).unwrap();
    assert_eq!(repair_list, commit.wrap_list);

    // A repaired wrap opens with the words.
    let wrapped = wrap_for(&repair_list, babak.id);
    let mut new_device = device(&babak);
    new_device
        .unwrap_history(&babak.keys, chat(), commit.epoch, &wrapped)
        .unwrap();
}

#[test]
fn a_swapped_account_key_is_refused_before_anything_is_wrapped_to_it() {
    let (sara, babak, mut sara_device, _, _) = direct_chat();
    let attacker =
        derive("letter advice cage absurd amount doctor acoustic avoid letter advice cage above")
            .unwrap();
    let mut swapped = babak.key.clone();
    swapped.account_public = *attacker.account_public();
    let accounts = [sara.key.clone(), swapped];
    assert!(matches!(
        sara_device.rotate(chat(), &accounts),
        Err(MlsError::BadStamp { .. })
    ));
}

#[test]
fn a_changed_master_key_warns_until_the_person_accepts_it() {
    let (sara, babak, mut sara_device, _, _) = direct_chat();
    let attacker = person(
        "letter advice cage absurd amount doctor acoustic avoid letter advice cage above",
        2,
    );
    assert_eq!(attacker.id, babak.id);
    let accounts = [sara.key.clone(), attacker.key.clone()];
    assert!(matches!(
        sara_device.rotate(chat(), &accounts),
        Err(MlsError::MasterKeyChanged { .. })
    ));

    let attacker_device = device(&attacker);
    assert!(matches!(
        sara_device.check_device(attacker_device.record()),
        Err(MlsError::MasterKeyChanged { .. })
    ));
    sara_device.repin(attacker.id, *attacker.keys.master_public());
    sara_device.check_device(attacker_device.record()).unwrap();
}

#[test]
fn a_device_with_a_forged_stamp_is_refused() {
    let (_, babak, mut sara_device, _, _) = direct_chat();
    let mut forged = device(&babak).record().clone();
    forged.stamp[0] ^= 1;
    assert!(matches!(
        sara_device.check_device(&forged),
        Err(MlsError::BadStamp { .. })
    ));
}

#[test]
fn removing_a_device_shuts_it_out_of_the_next_key() {
    let (sara, babak, mut sara_device, mut babak_device, _) = direct_chat();
    let babak_id = babak_device.record().device;
    let accounts = [sara.key.clone()];
    let commit = sara_device
        .remove_devices(chat(), &[babak_id], &accounts)
        .unwrap();
    assert_eq!(commit.wrap_list.len(), 1);
    assert_eq!(commit.wrap_list[0].account, sara.id);
    sara_device.commit_accepted(chat(), NOW).unwrap();

    let processed = babak_device
        .process_commit(chat(), &commit.commit, NOW)
        .unwrap();
    assert!(processed.removed_self);
    assert!(processed.group_info.is_empty());
    assert!(matches!(
        babak_device.epoch(chat()),
        Err(MlsError::UnknownChat { .. })
    ));

    let message = header(&sara_device, commit.epoch);
    let (body, signature) = sara_device.seal(&message, b"not for you").unwrap();
    assert!(matches!(
        babak_device.open(
            &message,
            &body,
            &signature,
            &sara_device.record().device_public
        ),
        Err(MlsError::NoEpochKey { .. })
    ));
    assert_eq!(sara_device.member_list(chat()).unwrap().len(), 1);
    let _ = babak;
}

#[test]
fn the_first_commit_wins_and_the_loser_catches_up() {
    let (sara, babak, mut sara_device, mut babak_device, commit) = direct_chat();
    let accounts = [sara.key.clone(), babak.key.clone()];
    let winner = sara_device.rotate(chat(), &accounts).unwrap();
    let loser = babak_device.rotate(chat(), &accounts).unwrap();
    assert_eq!(winner.epoch, loser.epoch);

    // The server takes Sara's and refuses Babak's.
    sara_device.commit_accepted(chat(), NOW).unwrap();
    babak_device.commit_rejected(chat()).unwrap();
    assert_eq!(babak_device.epoch(chat()).unwrap(), commit.epoch);
    babak_device
        .process_commit(chat(), &winner.commit, NOW)
        .unwrap();
    assert_eq!(babak_device.epoch(chat()).unwrap(), winner.epoch);

    // And retries from there.
    let retry = babak_device.rotate(chat(), &accounts).unwrap();
    assert_eq!(retry.epoch, winner.epoch + 1);
    babak_device.commit_accepted(chat(), NOW).unwrap();
    sara_device
        .process_commit(chat(), &retry.commit, NOW)
        .unwrap();
    assert_eq!(sara_device.epoch(chat()).unwrap(), retry.epoch);
}

#[test]
fn a_join_that_loses_its_race_leaves_nothing_behind() {
    let (sara, babak, sara_device, _, _) = direct_chat();
    let info = sara_device.group_info(chat()).unwrap();
    let accounts = [sara.key.clone(), babak.key.clone()];
    let mut joining = device(&babak);
    joining
        .join_external(chat(), &info, NOW, &accounts)
        .unwrap();
    assert!(matches!(
        joining.join_external(chat(), &info, NOW, &accounts),
        Err(MlsError::CommitPending { .. })
    ));
    joining.commit_rejected(chat()).unwrap();
    assert!(matches!(
        joining.epoch(chat()),
        Err(MlsError::UnknownChat { .. })
    ));
    joining
        .join_external(chat(), &info, NOW, &accounts)
        .unwrap();
}

#[test]
fn only_one_commit_at_a_time() {
    let (sara, babak, mut sara_device, _, _) = direct_chat();
    let accounts = [sara.key.clone(), babak.key.clone()];
    sara_device.rotate(chat(), &accounts).unwrap();
    assert!(matches!(
        sara_device.rotate(chat(), &accounts),
        Err(MlsError::CommitPending { .. })
    ));
    sara_device.commit_rejected(chat()).unwrap();
    assert!(matches!(
        sara_device.commit_rejected(chat()),
        Err(MlsError::NoCommitPending { .. })
    ));
    assert!(matches!(
        sara_device.commit_accepted(chat(), NOW),
        Err(MlsError::NoCommitPending { .. })
    ));
}

#[test]
fn someone_from_outside_cannot_join_by_themselves() {
    let (sara, babak, mut sara_device, _, _) = direct_chat();
    let info = sara_device.group_info(chat()).unwrap();
    let stranger = person(
        "letter advice cage absurd amount doctor acoustic avoid letter advice cage above",
        3,
    );
    let mut stranger_device = device(&stranger);
    let accounts = [sara.key.clone(), babak.key.clone(), stranger.key.clone()];
    let join = stranger_device
        .join_external(chat(), &info, NOW, &accounts)
        .unwrap();
    assert!(matches!(
        sara_device.process_commit(chat(), &join.commit, NOW),
        Err(MlsError::JoinRefused { .. })
    ));
}

#[test]
fn the_wrong_bytes_are_refused_clearly() {
    let (sara, _, mut sara_device, mut babak_device, commit) = direct_chat();
    let welcome = commit.welcome.as_ref().unwrap();
    let other = Ulid::from_bytes([60; 16]);
    let accounts = [sara.key.clone()];

    assert!(matches!(
        sara_device.process_commit(chat(), b"garbage", NOW),
        Err(MlsError::WrongMessage { expected: "commit" })
    ));
    assert!(matches!(
        sara_device.process_commit(chat(), welcome, NOW),
        Err(MlsError::WrongMessage { expected: "commit" })
    ));
    assert!(matches!(
        sara_device.process_commit(other, &commit.commit, NOW),
        Err(MlsError::UnknownChat { .. })
    ));
    assert!(matches!(
        babak_device.join_welcome(other, &commit.commit, NOW),
        Err(MlsError::WrongMessage {
            expected: "welcome"
        })
    ));
    assert!(matches!(
        babak_device.join_external(other, b"garbage", NOW, &accounts),
        Err(MlsError::WrongMessage { .. })
    ));
    assert!(matches!(
        sara_device.add_devices(chat(), &[b"garbage".to_vec()], &accounts),
        Err(MlsError::WrongMessage {
            expected: "key package"
        })
    ));
    assert!(matches!(
        sara_device.rotation_due(other, NOW),
        Err(MlsError::UnknownChat { .. })
    ));
}

#[test]
fn a_welcome_for_another_chat_is_refused() {
    let sara = person(SARA_WORDS, 1);
    let babak = person(BABAK_WORDS, 2);
    let mut sara_device = device(&sara);
    let mut babak_device = device(&babak);
    sara_device
        .create_chat(chat(), NOW, std::slice::from_ref(&sara.key))
        .unwrap();
    let package = babak_device.key_package().unwrap();
    let accounts = [sara.key.clone(), babak.key.clone()];
    let commit = sara_device
        .add_devices(chat(), &[package], &accounts)
        .unwrap();
    assert!(matches!(
        babak_device.join_welcome(
            Ulid::from_bytes([60; 16]),
            commit.welcome.as_ref().unwrap(),
            NOW
        ),
        Err(MlsError::WrongGroup { .. })
    ));
}

#[test]
fn state_survives_a_save_and_load_and_the_device_carries_on() {
    let (sara, babak, mut sara_device, mut babak_device, commit) = direct_chat();
    let message = header(&sara_device, commit.epoch);
    let (body, signature) = sara_device.seal(&message, b"kept").unwrap();
    let public = sara_device.record().device_public;
    let record = babak_device.record().clone();

    let snapshot = babak_device.export().unwrap();
    let mut loaded = Device::import(Some(&snapshot.bytes), None).unwrap();
    assert!(!loaded.fell_back);
    assert_eq!(loaded.device.record(), &record);

    let opened = loaded
        .device
        .open(&message, &body, &signature, &public)
        .unwrap();
    assert_eq!(opened.as_slice(), b"kept");

    let accounts = [sara.key.clone(), babak.key.clone()];
    let rotated = sara_device.rotate(chat(), &accounts).unwrap();
    sara_device.commit_accepted(chat(), NOW).unwrap();
    loaded
        .device
        .process_commit(chat(), &rotated.commit, NOW)
        .unwrap();
    assert_eq!(loaded.device.epoch(chat()).unwrap(), rotated.epoch);
    assert!(
        loaded
            .device
            .rotation_due(chat(), NOW + ROTATION_MS)
            .unwrap()
    );
}

#[test]
fn a_pending_commit_survives_a_save_and_a_crash() {
    let (sara, babak, mut sara_device, mut babak_device, _) = direct_chat();
    let accounts = [sara.key.clone(), babak.key.clone()];
    let rotated = sara_device.rotate(chat(), &accounts).unwrap();
    let snapshot = sara_device.export().unwrap();
    let mut loaded = Device::import(Some(&snapshot.bytes), None).unwrap().device;
    loaded.commit_accepted(chat(), NOW).unwrap();
    babak_device
        .process_commit(chat(), &rotated.commit, NOW)
        .unwrap();
    assert_eq!(loaded.epoch(chat()).unwrap(), rotated.epoch);
}

#[test]
fn saves_alternate_slots_and_the_newest_good_one_loads() {
    let (sara, babak, mut sara_device, _, commit) = direct_chat();
    let older = sara_device.export().unwrap();
    let accounts = [sara.key.clone(), babak.key.clone()];
    sara_device.rotate(chat(), &accounts).unwrap();
    sara_device.commit_accepted(chat(), NOW).unwrap();
    let newer = sara_device.export().unwrap();
    assert_ne!(older.slot, newer.slot);

    for (first, second) in [(&older, &newer), (&newer, &older)] {
        let loaded = Device::import(Some(&first.bytes), Some(&second.bytes)).unwrap();
        assert!(!loaded.fell_back);
        assert_eq!(loaded.device.epoch(chat()).unwrap(), commit.epoch + 1);
    }

    // A torn write of the newer one: the older one loads, and the client is told.
    let torn = &newer.bytes[..newer.bytes.len() / 2];
    let loaded = Device::import(Some(&older.bytes), Some(torn)).unwrap();
    assert!(loaded.fell_back);
    assert_eq!(loaded.device.epoch(chat()).unwrap(), commit.epoch);
    let loaded = Device::import(Some(torn), Some(&older.bytes)).unwrap();
    assert!(loaded.fell_back);
}

#[test]
fn every_kind_of_bad_save_has_its_own_error() {
    let (_, _, mut sara_device, _, _) = direct_chat();
    let snapshot = sara_device.export().unwrap();
    let good = snapshot.bytes.as_slice();

    let error = |bytes: &[u8]| match Device::import(Some(bytes), None) {
        Ok(_) => panic!("bad state was accepted"),
        Err(error) => error,
    };
    assert!(matches!(error(b""), StateError::Truncated));
    assert!(matches!(error(&[1, 2, 3]), StateError::Truncated));
    assert!(matches!(
        error(&good[..good.len() - 1]),
        StateError::Checksum
    ));
    let mut flipped = good.to_vec();
    flipped[40] ^= 1;
    assert!(matches!(error(&flipped), StateError::Checksum));
    let mut newer = good.to_vec();
    newer[0] = 2;
    assert!(matches!(
        error(&newer),
        StateError::UnknownVersion { version: 2 }
    ));

    assert!(matches!(Device::import(None, None), Err(StateError::Empty)));
    assert!(matches!(
        Device::import(Some(b""), Some(&flipped)),
        Err(StateError::NothingUsable { .. })
    ));
}

/// **If this fails, no stored statement verifies any more.** The layouts are rebuilt here by hand,
/// so that changing one in the code cannot also change what it is tested against.
#[test]
fn statements_verify_only_for_what_they_say() {
    use ed25519_dalek::{Signer, SigningKey};
    use keryx_core::mls::{
        sign_password_reset, sign_revocation, verify_account_key, verify_device_login,
        verify_device_stamp, verify_logout, verify_password_reset, verify_revocation,
    };

    let sara = person(SARA_WORDS, 1);
    let master = *sara.keys.master_public();
    let sara_device = device(&sara);
    let record = sara_device.record().clone();
    let (account, id, public) = (record.account, record.device, record.device_public);
    let other = Ulid::from_bytes([77; 16]);
    let challenge = [9u8; 32];

    let mut layout = b"keryx v1 revoke device".to_vec();
    layout.extend_from_slice(account.bytes());
    layout.extend_from_slice(id.bytes());
    let signed = SigningKey::from_bytes(sara.keys.master_private()).sign(&layout);
    assert_eq!(
        sign_revocation(&sara.keys, &account, &id),
        signed.to_bytes()
    );

    let revocation = sign_revocation(&sara.keys, &account, &id);
    verify_revocation(&master, &account, &id, &revocation).unwrap();
    verify_revocation(&master, &account, &other, &revocation).unwrap_err();
    verify_revocation(&public, &account, &id, &revocation).unwrap_err();

    let logout = sara_device.sign_logout().unwrap();
    verify_logout(&public, &account, &id, &logout).unwrap();
    verify_logout(&public, &account, &other, &logout).unwrap_err();
    // One label's signature can never be used as another's.
    verify_revocation(&public, &account, &id, &logout).unwrap_err();

    let login = sara_device.sign_device_login(&challenge).unwrap();
    verify_device_login(&public, &account, &id, &challenge, &login).unwrap();
    verify_device_login(&public, &account, &id, &[8; 32], &login).unwrap_err();
    verify_logout(&public, &account, &id, &login).unwrap_err();

    let reset = sign_password_reset(&sara.keys, &account, &challenge, "a new password");
    verify_password_reset(&master, &account, &challenge, "a new password", &reset).unwrap();
    verify_password_reset(&master, &account, &challenge, "another one", &reset).unwrap_err();
    verify_password_reset(&master, &account, &[8; 32], "a new password", &reset).unwrap_err();

    verify_device_stamp(&master, &account, &id, &public, &record.stamp).unwrap();
    verify_device_stamp(&master, &other, &id, &public, &record.stamp).unwrap_err();
    verify_account_key(
        &master,
        &account,
        sara.keys.account_public(),
        &sara.key.stamp,
    )
    .unwrap();
    verify_account_key(&master, &other, sara.keys.account_public(), &sara.key.stamp).unwrap_err();
}
