//! Groups, and the per-epoch keys exported for messages, files and calls.
//!
//! MLS manages membership and produces one key per epoch. Message content is AES-GCM under that
//! key and signed by the device, rather than an MLS application message, so a device can decrypt
//! in any order and more than once.
//!
//! **A commit goes through three steps.** Building one leaves it pending and hands back its bytes
//! and wrapped keys. The client saves a snapshot, sends it, and tells this module what the server
//! said: [`Device::commit_accepted`] or [`Device::commit_rejected`]. A failed save means the
//! commit is never sent.

use crate::protocol::Header;
use crate::seed::AccountKeys;
use crate::ulid::Ulid;
use crate::wrap::{self, WRAP_SIZE, WrapError};
use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes128Gcm, KeyInit, Nonce};
use ed25519_dalek::{Signature, Signer as _, SigningKey, VerifyingKey};
use openmls::prelude::tls_codec::{Deserialize as _, Serialize as _};
use openmls::prelude::*;
use openmls::treesync::LeafNodeParameters;
use openmls_basic_credential::SignatureKeyPair;
use openmls_rust_crypto::OpenMlsRustCrypto;
use openmls_traits::signatures::Signer as _;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::error::Error as StdError;
use zeroize::Zeroizing;

/// How old an epoch key may get before a device must commit a new one before sending. Fixed here
/// and not in `server.toml`, because a server setting could be used to turn rotation off.
pub const ROTATION_MS: i64 = 7 * 24 * 60 * 60 * 1000;

const CIPHERSUITE: Ciphersuite = Ciphersuite::MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519;
const MESSAGE_KEY_LABEL: &str = "keryx v1 message key";
const DEVICE_STAMP_LABEL: &[u8] = b"keryx v1 device stamp";
const ACCOUNT_STAMP_LABEL: &[u8] = b"keryx v1 account key";
const REVOKE_LABEL: &[u8] = b"keryx v1 revoke device";
const LOGOUT_LABEL: &[u8] = b"keryx v1 logout";
const PASSWORD_RESET_LABEL: &[u8] = b"keryx v1 password reset";
const DEVICE_LOGIN_LABEL: &[u8] = b"keryx v1 device login";
const BODY_VERSION: u8 = 1;
/// The account id, device id, master public key and stamp.
const CREDENTIAL_SIZE: usize = 128;
/// The version, the generation, then the payload, then the checksum.
const STATE_VERSION: u8 = 1;
const STATE_CHECKSUM_SIZE: usize = 32;

#[derive(Debug, thiserror::Error)]
pub enum MlsError {
    #[error("MLS could not {operation}")]
    Group {
        operation: &'static str,
        #[source]
        source: Box<dyn StdError + Send + Sync>,
    },

    #[error("This device is not in chat {chat}")]
    UnknownChat { chat: Ulid },

    #[error("Chat {chat} already has a commit waiting for the server's answer")]
    CommitPending { chat: Ulid },

    #[error("Chat {chat} has no commit waiting for the server's answer")]
    NoCommitPending { chat: Ulid },

    #[error("A device's credential is not the {CREDENTIAL_SIZE} bytes keryx uses")]
    Credential,

    #[error("A stamp on account {account} does not verify against its master key")]
    BadStamp { account: Ulid },

    /// Shown loudly: this is what a server swapping a master key looks like.
    #[error("The master key of account {account} has changed")]
    MasterKeyChanged { account: Ulid },

    #[error("A device of account {account} joined by itself, and that account is not in the chat")]
    JoinRefused { account: Ulid },

    #[error("This is not the group it was meant to be for chat {chat}")]
    WrongGroup { chat: Ulid },

    #[error("Expected a {expected} here, and these bytes are something else")]
    WrongMessage { expected: &'static str },

    #[error("Device {device} is not this device")]
    WrongDevice { device: Ulid },

    #[error("An encrypted message needs an epoch, and to be flagged encrypted")]
    NotEncrypted,

    #[error("The key of chat {chat} is over 7 days old, so a new one must be committed first")]
    RotationDue { chat: Ulid },

    #[error("This device has no key for epoch {epoch} of chat {chat}")]
    NoEpochKey { chat: Ulid, epoch: u64 },

    #[error("Could not read random bytes from the operating system")]
    Random {
        #[source]
        source: getrandom::Error,
    },

    #[error("A message body is too short to be one")]
    BodyShort,

    #[error("Message body version {version} is not one this client knows")]
    BodyVersion { version: u8 },

    #[error("Could not encrypt the message")]
    Encrypt,

    #[error("Could not sign")]
    Sign,

    #[error("A signature does not verify against the sender's key")]
    Signature,

    /// Signed correctly but does not open: the wrong key, or a sender's bug.
    #[error("A message does not open with its epoch's key")]
    Decrypt,

    #[error("Could not wrap or unwrap a key")]
    Wrap(#[from] WrapError),
}

/// Each failure on load has its own error, so a client can say what happened.
#[derive(Debug, thiserror::Error)]
pub enum StateError {
    #[error("Saved state is cut short")]
    Truncated,

    #[error("Saved state does not match its checksum")]
    Checksum,

    #[error("Saved state has version {version}, which this client does not know")]
    UnknownVersion { version: u8 },

    #[error("Saved state cannot be read, because a lock was poisoned by an earlier panic")]
    Lock,

    #[error("Saved state has no signing key for this device")]
    NoSigningKey,

    #[error("Saved state has no group for chat {chat}")]
    NoGroup { chat: Ulid },

    #[error("Could not read a group from saved state")]
    Group {
        #[source]
        source: Box<dyn StdError + Send + Sync>,
    },

    #[error("Neither saved state slot holds anything")]
    Empty,

    #[error("Neither saved state slot is usable. First: {first}. Second: {second}")]
    NothingUsable {
        first: Box<StateError>,
        second: Box<StateError>,
    },
}

/// What other devices need to know about a device, and verify before trusting it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceRecord {
    pub account: Ulid,
    pub device: Ulid,
    pub device_public: [u8; 32],
    pub master_public: [u8; 32],
    pub stamp: [u8; 64],
}

/// An account's public key, which epoch keys are wrapped to, and the master key's stamp on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountKey {
    pub account: Ulid,
    pub account_public: [u8; 32],
    pub master_public: [u8; 32],
    pub stamp: [u8; 64],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountWrap {
    pub account: Ulid,
    pub wrapped: [u8; WRAP_SIZE],
}

/// A new chat's first group info and wrapped keys.
#[derive(Debug)]
pub struct Created {
    pub group_info: Vec<u8>,
    pub wrap_list: Vec<AccountWrap>,
}

/// A commit waiting to be saved, then sent.
#[derive(Debug)]
pub struct Commit {
    /// The epoch this moves the chat to.
    pub epoch: u64,
    pub commit: Vec<u8>,
    pub welcome: Option<Vec<u8>>,
    pub wrap_list: Vec<AccountWrap>,
}

/// Someone else's commit, applied.
#[derive(Debug)]
pub struct Processed {
    pub epoch: u64,
    /// This commit removed this device, so it has left the chat. Keys it held stay, so that
    /// messages it could already read still open.
    pub removed_self: bool,
    /// The chat's new group info, for the server. Empty if this device was removed.
    pub group_info: Vec<u8>,
}

/// One slot's worth of saved state. Save `bytes` in `slot`, as one write, before sending anything.
pub struct Snapshot {
    pub slot: u8,
    pub bytes: Zeroizing<Vec<u8>>,
}

pub struct Imported {
    pub device: Device,
    /// The other slot held something unusable. This one may be behind, so the client rejoins any
    /// chat it fails on by external commit.
    pub fell_back: bool,
}

struct Chat {
    group: MlsGroup,
    epoch_started_at: i64,
    /// Joining by external commit, so a rejected commit leaves no group at all.
    joining: bool,
}

/// One device: its key, its groups and its keys for reading. Holds no word and no key from the
/// words, so a stolen device cannot unwrap history or add a device.
pub struct Device {
    provider: OpenMlsRustCrypto,
    signer: SignatureKeyPair,
    record: DeviceRecord,
    generation: u64,
    pins: HashMap<Ulid, [u8; 32]>,
    epoch_keys: HashMap<(Ulid, u64), Zeroizing<[u8; 16]>>,
    chats: HashMap<Ulid, Chat>,
}

/// The master key's stamp on this account's public key, published once when the words are set up.
pub fn stamp_account_key(keys: &AccountKeys, account: &Ulid) -> [u8; 64] {
    let bytes = account_stamp_bytes(account, keys.account_public());
    SigningKey::from_bytes(keys.master_private())
        .sign(&bytes)
        .to_bytes()
}

/// Check a device's stamp with public keys only: the server does this when a device registers.
pub fn verify_device_stamp(
    master_public: &[u8; 32],
    account: &Ulid,
    device: &Ulid,
    device_public: &[u8; 32],
    stamp: &[u8; 64],
) -> Result<(), MlsError> {
    verify(
        master_public,
        &device_stamp_bytes(account, device, device_public),
        stamp,
        MlsError::BadStamp { account: *account },
    )
}

/// Check an account key's stamp with public keys only: the server does this when words are set up.
pub fn verify_account_key(
    master_public: &[u8; 32],
    account: &Ulid,
    account_public: &[u8; 32],
    stamp: &[u8; 64],
) -> Result<(), MlsError> {
    verify(
        master_public,
        &account_stamp_bytes(account, account_public),
        stamp,
        MlsError::BadStamp { account: *account },
    )
}

/// Terminating a device, which only the words can do. One-way, so it holds no time or nonce.
pub fn sign_revocation(keys: &AccountKeys, account: &Ulid, device: &Ulid) -> [u8; 64] {
    SigningKey::from_bytes(keys.master_private())
        .sign(&device_statement_bytes(REVOKE_LABEL, account, device))
        .to_bytes()
}

pub fn verify_revocation(
    master_public: &[u8; 32],
    account: &Ulid,
    device: &Ulid,
    signature: &[u8; 64],
) -> Result<(), MlsError> {
    verify(
        master_public,
        &device_statement_bytes(REVOKE_LABEL, account, device),
        signature,
        MlsError::Signature,
    )
}

pub fn verify_logout(
    device_public: &[u8; 32],
    account: &Ulid,
    device: &Ulid,
    signature: &[u8; 64],
) -> Result<(), MlsError> {
    verify(
        device_public,
        &device_statement_bytes(LOGOUT_LABEL, account, device),
        signature,
        MlsError::Signature,
    )
}

/// Resetting a password. The new password's hash is inside, so a captured signature cannot set a
/// different one.
pub fn sign_password_reset(
    keys: &AccountKeys,
    account: &Ulid,
    challenge: &[u8; 32],
    new_password: &str,
) -> [u8; 64] {
    SigningKey::from_bytes(keys.master_private())
        .sign(&password_reset_bytes(account, challenge, new_password))
        .to_bytes()
}

pub fn verify_password_reset(
    master_public: &[u8; 32],
    account: &Ulid,
    challenge: &[u8; 32],
    new_password: &str,
    signature: &[u8; 64],
) -> Result<(), MlsError> {
    verify(
        master_public,
        &password_reset_bytes(account, challenge, new_password),
        signature,
        MlsError::Signature,
    )
}

pub fn verify_device_login(
    device_public: &[u8; 32],
    account: &Ulid,
    device: &Ulid,
    challenge: &[u8; 32],
    signature: &[u8; 64],
) -> Result<(), MlsError> {
    let mut bytes = device_statement_bytes(DEVICE_LOGIN_LABEL, account, device);
    bytes.extend_from_slice(challenge);
    verify(device_public, &bytes, signature, MlsError::Signature)
}

/// Check a signature on a message, with public keys only, before decrypting anything.
pub fn verify_message(
    header: &Header,
    body: &[u8],
    signature: &[u8; 64],
    device_public: &[u8; 32],
) -> Result<(), MlsError> {
    let mut signed = header.bytes();
    signed.extend_from_slice(body);
    verify(device_public, &signed, signature, MlsError::Signature)
}

fn verify(
    public: &[u8; 32],
    bytes: &[u8],
    signature: &[u8; 64],
    bad: MlsError,
) -> Result<(), MlsError> {
    let key = match VerifyingKey::from_bytes(public) {
        Ok(key) => key,
        Err(_) => return Err(bad),
    };
    match key.verify_strict(bytes, &Signature::from_bytes(signature)) {
        Ok(()) => Ok(()),
        Err(_) => Err(bad),
    }
}

/// **Permanent:** stored statements depend on it, and `tests/mls.rs` pins the exact bytes through
/// signatures from a known key.
fn device_statement_bytes(label: &[u8], account: &Ulid, device: &Ulid) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(label.len() + 64);
    bytes.extend_from_slice(label);
    bytes.extend_from_slice(account.bytes());
    bytes.extend_from_slice(device.bytes());
    bytes
}

fn password_reset_bytes(account: &Ulid, challenge: &[u8; 32], new_password: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(PASSWORD_RESET_LABEL.len() + 80);
    bytes.extend_from_slice(PASSWORD_RESET_LABEL);
    bytes.extend_from_slice(account.bytes());
    bytes.extend_from_slice(challenge);
    bytes.extend_from_slice(&Sha256::digest(new_password.as_bytes()));
    bytes
}

fn device_stamp_bytes(account: &Ulid, device: &Ulid, device_public: &[u8; 32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(DEVICE_STAMP_LABEL.len() + 64);
    bytes.extend_from_slice(DEVICE_STAMP_LABEL);
    bytes.extend_from_slice(account.bytes());
    bytes.extend_from_slice(device.bytes());
    bytes.extend_from_slice(device_public);
    bytes
}

fn account_stamp_bytes(account: &Ulid, account_public: &[u8; 32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(ACCOUNT_STAMP_LABEL.len() + 48);
    bytes.extend_from_slice(ACCOUNT_STAMP_LABEL);
    bytes.extend_from_slice(account.bytes());
    bytes.extend_from_slice(account_public);
    bytes
}

fn group_error<E: StdError + Send + Sync + 'static>(
    operation: &'static str,
    source: E,
) -> MlsError {
    MlsError::Group {
        operation,
        source: Box::new(source),
    }
}

/// Trust the first master key seen for an account, and refuse a different one after that.
fn pin(
    pins: &mut HashMap<Ulid, [u8; 32]>,
    account: Ulid,
    master: &[u8; 32],
) -> Result<(), MlsError> {
    if let Some(pinned) = pins.get(&account) {
        if pinned != master {
            return Err(MlsError::MasterKeyChanged { account });
        }
        return Ok(());
    }
    pins.insert(account, *master);
    Ok(())
}

fn check_record(pins: &mut HashMap<Ulid, [u8; 32]>, record: &DeviceRecord) -> Result<(), MlsError> {
    let bytes = device_stamp_bytes(&record.account, &record.device, &record.device_public);
    let bad = MlsError::BadStamp {
        account: record.account,
    };
    verify(&record.master_public, &bytes, &record.stamp, bad)?;
    pin(pins, record.account, &record.master_public)
}

/// A leaf's credential and key, as a record: nothing is checked yet.
fn read_leaf(credential: &Credential, signature_key: &[u8]) -> Result<DeviceRecord, MlsError> {
    let basic = match BasicCredential::try_from(credential.clone()) {
        Ok(basic) => basic,
        Err(_) => return Err(MlsError::Credential),
    };
    let identity = basic.identity();
    if identity.len() != CREDENTIAL_SIZE || signature_key.len() != 32 {
        return Err(MlsError::Credential);
    }
    let mut account = [0u8; 16];
    let mut device = [0u8; 16];
    let mut device_public = [0u8; 32];
    let mut master_public = [0u8; 32];
    let mut stamp = [0u8; 64];
    account.copy_from_slice(&identity[..16]);
    device.copy_from_slice(&identity[16..32]);
    master_public.copy_from_slice(&identity[32..64]);
    stamp.copy_from_slice(&identity[64..]);
    device_public.copy_from_slice(signature_key);
    Ok(DeviceRecord {
        account: Ulid::from_bytes(account),
        device: Ulid::from_bytes(device),
        device_public,
        master_public,
        stamp,
    })
}

fn wire_out(operation: &'static str, message: &MlsMessageOut) -> Result<Vec<u8>, MlsError> {
    match message.tls_serialize_detached() {
        Ok(bytes) => Ok(bytes),
        Err(source) => Err(group_error(operation, source)),
    }
}

fn wire_in(expected: &'static str, mut bytes: &[u8]) -> Result<MlsMessageIn, MlsError> {
    match MlsMessageIn::tls_deserialize(&mut bytes) {
        Ok(message) => Ok(message),
        Err(_) => Err(MlsError::WrongMessage { expected }),
    }
}

impl Device {
    /// A new device with a random key, stamped by the master key. This is the only moment a
    /// device touches the words' keys, apart from unwrapping history.
    pub fn create(keys: &AccountKeys, account: Ulid, device: Ulid) -> Result<Self, MlsError> {
        let provider = OpenMlsRustCrypto::default();
        let signer = match SignatureKeyPair::new(SignatureScheme::ED25519) {
            Ok(signer) => signer,
            Err(source) => return Err(group_error("create a device key", source)),
        };
        if let Err(source) = signer.store(provider.storage()) {
            return Err(group_error("store a device key", source));
        }
        let mut device_public = [0u8; 32];
        if signer.public().len() != 32 {
            return Err(MlsError::Credential);
        }
        device_public.copy_from_slice(signer.public());
        let bytes = device_stamp_bytes(&account, &device, &device_public);
        let stamp = SigningKey::from_bytes(keys.master_private())
            .sign(&bytes)
            .to_bytes();
        let record = DeviceRecord {
            account,
            device,
            device_public,
            master_public: *keys.master_public(),
            stamp,
        };
        let mut pins = HashMap::new();
        pins.insert(account, *keys.master_public());
        Ok(Self {
            provider,
            signer,
            record,
            generation: 0,
            pins,
            epoch_keys: HashMap::new(),
            chats: HashMap::new(),
        })
    }

    pub fn record(&self) -> &DeviceRecord {
        &self.record
    }

    /// Verify another device against its account's pinned master key, before trusting its
    /// signatures.
    pub fn check_device(&mut self, record: &DeviceRecord) -> Result<(), MlsError> {
        check_record(&mut self.pins, record)
    }

    /// The person has accepted that this account's master key changed.
    pub fn repin(&mut self, account: Ulid, master_public: [u8; 32]) {
        self.pins.insert(account, master_public);
    }

    /// A one-use package other people add this device to a chat with.
    pub fn key_package(&self) -> Result<Vec<u8>, MlsError> {
        let bundle = match KeyPackage::builder().build(
            CIPHERSUITE,
            &self.provider,
            &self.signer,
            self.credential(),
        ) {
            Ok(bundle) => bundle,
            Err(source) => return Err(group_error("build a key package", source)),
        };
        match bundle.key_package().tls_serialize_detached() {
            Ok(bytes) => Ok(bytes),
            Err(source) => Err(group_error("encode a key package", source)),
        }
    }

    /// Start a chat's group, with this device alone in it.
    pub fn create_chat(
        &mut self,
        chat: Ulid,
        now: i64,
        account_list: &[AccountKey],
    ) -> Result<Created, MlsError> {
        let config = MlsGroupCreateConfig::builder()
            .ciphersuite(CIPHERSUITE)
            .use_ratchet_tree_extension(true)
            .build();
        let group = match MlsGroup::new_with_group_id(
            &self.provider,
            &self.signer,
            &config,
            GroupId::from_slice(chat.bytes()),
            self.credential(),
        ) {
            Ok(group) => group,
            Err(source) => return Err(group_error("create a group", source)),
        };
        self.chats.insert(
            chat,
            Chat {
                group,
                epoch_started_at: now,
                joining: false,
            },
        );
        self.keep_current_key(chat)?;
        let state = self.chat(chat)?;
        let secret = match state.group.export_secret(
            self.provider.crypto(),
            wrap::SECRET_LABEL,
            chat.bytes(),
            32,
        ) {
            Ok(secret) => Zeroizing::new(secret),
            Err(source) => return Err(group_error("export the wrap secret", source)),
        };
        let epoch = state.group.epoch().as_u64();
        let key = self.current_key(chat, epoch)?;
        let wrap_list = self.wrap_list(&secret, &key, chat, epoch, account_list)?;
        let group_info = self.group_info(chat)?;
        Ok(Created {
            group_info,
            wrap_list,
        })
    }

    /// Add devices. `account_list` is every account in the chat afterwards.
    pub fn add_devices(
        &mut self,
        chat: Ulid,
        key_package_list: &[Vec<u8>],
        account_list: &[AccountKey],
    ) -> Result<Commit, MlsError> {
        let mut packages = Vec::with_capacity(key_package_list.len());
        for bytes in key_package_list {
            let mut reader = bytes.as_slice();
            let package = match KeyPackageIn::tls_deserialize(&mut reader) {
                Ok(package) => package,
                Err(_) => {
                    return Err(MlsError::WrongMessage {
                        expected: "key package",
                    });
                }
            };
            let package = match package.validate(self.provider.crypto(), ProtocolVersion::Mls10) {
                Ok(package) => package,
                Err(source) => return Err(group_error("validate a key package", source)),
            };
            let leaf = package.leaf_node();
            let record = read_leaf(leaf.credential(), leaf.signature_key().as_slice())?;
            check_record(&mut self.pins, &record)?;
            packages.push(package);
        }
        self.refuse_second_commit(chat)?;
        let provider = &self.provider;
        let signer = &self.signer;
        let Some(state) = self.chats.get_mut(&chat) else {
            return Err(MlsError::UnknownChat { chat });
        };
        let (commit, welcome, _) = match state.group.add_members(provider, signer, &packages) {
            Ok(built) => built,
            Err(source) => return Err(group_error("add a device", source)),
        };
        let commit = wire_out("encode a commit", &commit)?;
        let welcome = wire_out("encode a welcome", &welcome)?;
        self.pending_commit(chat, commit, Some(welcome), account_list)
    }

    /// Remove devices, such as after a signed revocation. `account_list` is every account in the
    /// chat afterwards.
    pub fn remove_devices(
        &mut self,
        chat: Ulid,
        device_list: &[Ulid],
        account_list: &[AccountKey],
    ) -> Result<Commit, MlsError> {
        self.refuse_second_commit(chat)?;
        let provider = &self.provider;
        let signer = &self.signer;
        let Some(state) = self.chats.get_mut(&chat) else {
            return Err(MlsError::UnknownChat { chat });
        };
        let mut leaves = Vec::with_capacity(device_list.len());
        for member in state.group.members() {
            let record = read_leaf(&member.credential, &member.signature_key)?;
            if device_list.contains(&record.device) {
                leaves.push(member.index);
            }
        }
        let (commit, _, _) = match state.group.remove_members(provider, signer, &leaves) {
            Ok(built) => built,
            Err(source) => return Err(group_error("remove a device", source)),
        };
        let commit = wire_out("encode a commit", &commit)?;
        self.pending_commit(chat, commit, None, account_list)
    }

    /// Whether this chat's key is too old to send under.
    pub fn rotation_due(&self, chat: Ulid, now: i64) -> Result<bool, MlsError> {
        let state = self.chat(chat)?;
        Ok(now - state.epoch_started_at >= ROTATION_MS)
    }

    /// Commit a new key with nobody joining or leaving.
    pub fn rotate(&mut self, chat: Ulid, account_list: &[AccountKey]) -> Result<Commit, MlsError> {
        self.refuse_second_commit(chat)?;
        let provider = &self.provider;
        let signer = &self.signer;
        let Some(state) = self.chats.get_mut(&chat) else {
            return Err(MlsError::UnknownChat { chat });
        };
        let bundle = match state
            .group
            .self_update(provider, signer, LeafNodeParameters::default())
        {
            Ok(bundle) => bundle,
            Err(source) => return Err(group_error("rotate the key", source)),
        };
        let commit = wire_out("encode a commit", &bundle.into_commit())?;
        self.pending_commit(chat, commit, None, account_list)
    }

    /// Join a chat without anyone's help, from its public group info. This is what lets someone
    /// who lost every device come back with only the words.
    pub fn join_external(
        &mut self,
        chat: Ulid,
        group_info: &[u8],
        now: i64,
        account_list: &[AccountKey],
    ) -> Result<Commit, MlsError> {
        if self.chats.contains_key(&chat) {
            return Err(MlsError::CommitPending { chat });
        }
        let verifiable = match wire_in("group info", group_info)?.extract() {
            MlsMessageBodyIn::GroupInfo(verifiable) => verifiable,
            _ => {
                return Err(MlsError::WrongMessage {
                    expected: "group info",
                });
            }
        };
        // Lifetimes are skipped because a chat idle for months must still be joinable. Whether a
        // device belongs is decided by its stamp.
        let stage = match MlsGroup::external_commit_builder()
            .skip_lifetime_validation()
            .build_group(&self.provider, verifiable, self.credential())
        {
            Ok(stage) => stage,
            Err(source) => return Err(group_error("read group info", source)),
        };
        let stage = match stage.load_psks(self.provider.storage()) {
            Ok(stage) => stage,
            Err(source) => return Err(group_error("load keys to join", source)),
        };
        let stage = match stage.build(
            self.provider.rand(),
            self.provider.crypto(),
            &self.signer,
            |_| true,
        ) {
            Ok(stage) => stage,
            Err(source) => return Err(group_error("build a join", source)),
        };
        let (group, bundle) = match stage.finalize(&self.provider) {
            Ok(built) => built,
            Err(source) => return Err(group_error("finish a join", source)),
        };
        if group.group_id().as_slice() != chat.bytes() {
            return Err(MlsError::WrongGroup { chat });
        }
        for member in group.members() {
            let record = read_leaf(&member.credential, &member.signature_key)?;
            check_record(&mut self.pins, &record)?;
        }
        let commit = wire_out("encode a commit", &bundle.into_commit())?;
        self.chats.insert(
            chat,
            Chat {
                group,
                epoch_started_at: now,
                joining: true,
            },
        );
        self.pending_commit(chat, commit, None, account_list)
    }

    /// The server accepted this device's commit, so apply it. Returns the chat's new group info
    /// for the server.
    pub fn commit_accepted(&mut self, chat: Ulid, now: i64) -> Result<Vec<u8>, MlsError> {
        let provider = &self.provider;
        let Some(state) = self.chats.get_mut(&chat) else {
            return Err(MlsError::UnknownChat { chat });
        };
        // openmls has already applied a join when it is built, so only a member's commit merges.
        if !state.joining {
            if state.group.pending_commit().is_none() {
                return Err(MlsError::NoCommitPending { chat });
            }
            if let Err(source) = state.group.merge_pending_commit(provider) {
                return Err(group_error("apply a commit", source));
            }
        }
        state.epoch_started_at = now;
        state.joining = false;
        self.keep_current_key(chat)?;
        self.group_info(chat)
    }

    /// Another device's commit won this epoch. A join leaves nothing behind, and anything else
    /// goes back to how it was, to catch up and retry.
    pub fn commit_rejected(&mut self, chat: Ulid) -> Result<(), MlsError> {
        let provider = &self.provider;
        let Some(state) = self.chats.get_mut(&chat) else {
            return Err(MlsError::UnknownChat { chat });
        };
        if state.joining {
            if let Err(source) = state.group.delete(provider.storage()) {
                return Err(group_error("discard a join", source));
            }
            self.chats.remove(&chat);
            return Ok(());
        }
        if state.group.pending_commit().is_none() {
            return Err(MlsError::NoCommitPending { chat });
        }
        if let Err(source) = state.group.clear_pending_commit(provider.storage()) {
            return Err(group_error("discard a commit", source));
        }
        Ok(())
    }

    /// Apply another device's commit, after checking every device it brings in.
    pub fn process_commit(
        &mut self,
        chat: Ulid,
        bytes: &[u8],
        now: i64,
    ) -> Result<Processed, MlsError> {
        let message = match wire_in("commit", bytes)?.try_into_protocol_message() {
            Ok(message) => message,
            Err(_) => return Err(MlsError::WrongMessage { expected: "commit" }),
        };
        let provider = &self.provider;
        let Some(state) = self.chats.get_mut(&chat) else {
            return Err(MlsError::UnknownChat { chat });
        };
        let processed = match state.group.process_message(provider, message) {
            Ok(processed) => processed,
            Err(source) => return Err(group_error("read a commit", source)),
        };
        let external = matches!(processed.sender(), Sender::NewMemberCommit);
        let staged = match processed.into_content() {
            ProcessedMessageContent::StagedCommitMessage(staged) => *staged,
            _ => return Err(MlsError::WrongMessage { expected: "commit" }),
        };

        for add in staged.add_proposals() {
            let leaf = add.add_proposal().key_package().leaf_node();
            let record = read_leaf(leaf.credential(), leaf.signature_key().as_slice())?;
            check_record(&mut self.pins, &record)?;
        }
        if let Some(leaf) = staged.update_path_leaf_node() {
            let record = read_leaf(leaf.credential(), leaf.signature_key().as_slice())?;
            check_record(&mut self.pins, &record)?;
            if external {
                let mut account_is_in_chat = false;
                for member in state.group.members() {
                    let existing = read_leaf(&member.credential, &member.signature_key)?;
                    if existing.account == record.account {
                        account_is_in_chat = true;
                    }
                }
                if !account_is_in_chat {
                    return Err(MlsError::JoinRefused {
                        account: record.account,
                    });
                }
            }
        }

        let removed_self = staged.self_removed();
        let epoch = staged.group_context().epoch().as_u64();
        if let Err(source) = state.group.merge_staged_commit(provider, staged) {
            return Err(group_error("apply a commit", source));
        }
        state.epoch_started_at = now;
        if removed_self {
            if let Err(source) = state.group.delete(provider.storage()) {
                return Err(group_error("discard a group", source));
            }
            self.chats.remove(&chat);
            return Ok(Processed {
                epoch,
                removed_self,
                group_info: Vec::new(),
            });
        }
        self.keep_current_key(chat)?;
        let group_info = self.group_info(chat)?;
        Ok(Processed {
            epoch,
            removed_self,
            group_info,
        })
    }

    /// Join a chat this device was added to, after checking everyone already in it.
    pub fn join_welcome(&mut self, chat: Ulid, bytes: &[u8], now: i64) -> Result<(), MlsError> {
        let welcome = match wire_in("welcome", bytes)?.extract() {
            MlsMessageBodyIn::Welcome(welcome) => welcome,
            _ => {
                return Err(MlsError::WrongMessage {
                    expected: "welcome",
                });
            }
        };
        let config = MlsGroupJoinConfig::builder()
            .use_ratchet_tree_extension(true)
            .build();
        let builder = match StagedWelcome::build_from_welcome(&self.provider, &config, welcome) {
            Ok(builder) => builder,
            Err(source) => return Err(group_error("read a welcome", source)),
        };
        // Lifetimes are skipped for the same reason as in join_external.
        let staged = match builder.skip_lifetime_validation().build() {
            Ok(staged) => staged,
            Err(source) => return Err(group_error("read a welcome", source)),
        };
        if staged.group_context().group_id().as_slice() != chat.bytes() {
            return Err(MlsError::WrongGroup { chat });
        }
        for member in staged.members() {
            let record = read_leaf(&member.credential, &member.signature_key)?;
            check_record(&mut self.pins, &record)?;
        }
        let group = match staged.into_group(&self.provider) {
            Ok(group) => group,
            Err(source) => return Err(group_error("join from a welcome", source)),
        };
        self.chats.insert(
            chat,
            Chat {
                group,
                epoch_started_at: now,
                joining: false,
            },
        );
        self.keep_current_key(chat)
    }

    pub fn epoch(&self, chat: Ulid) -> Result<u64, MlsError> {
        Ok(self.chat(chat)?.group.epoch().as_u64())
    }

    /// The accounts and devices the group says are in this chat.
    pub fn member_list(&self, chat: Ulid) -> Result<Vec<DeviceRecord>, MlsError> {
        let state = self.chat(chat)?;
        let mut list = Vec::new();
        for member in state.group.members() {
            list.push(read_leaf(&member.credential, &member.signature_key)?);
        }
        Ok(list)
    }

    /// Redo every wrap of the current epoch and compare, returning the right wrap for each account
    /// whose stored wrap is wrong or missing. Each one is for the client to upload.
    pub fn check_wraps(
        &mut self,
        chat: Ulid,
        stored_list: &[AccountWrap],
        account_list: &[AccountKey],
    ) -> Result<Vec<AccountWrap>, MlsError> {
        let state = self.chat(chat)?;
        let secret = match state.group.export_secret(
            self.provider.crypto(),
            wrap::SECRET_LABEL,
            chat.bytes(),
            32,
        ) {
            Ok(secret) => Zeroizing::new(secret),
            Err(source) => return Err(group_error("export the wrap secret", source)),
        };
        let epoch = state.group.epoch().as_u64();
        let key = self.current_key(chat, epoch)?;
        let right_list = self.wrap_list(&secret, &key, chat, epoch, account_list)?;
        let mut repair_list = Vec::new();
        for right in right_list {
            if !stored_list.contains(&right) {
                repair_list.push(right);
            }
        }
        Ok(repair_list)
    }

    /// Open a stored wrapped key with the words, so this device can read that epoch's messages.
    pub fn unwrap_history(
        &mut self,
        keys: &AccountKeys,
        chat: Ulid,
        epoch: u64,
        wrapped: &[u8],
    ) -> Result<(), MlsError> {
        let key = wrap::unwrap(
            wrapped,
            &chat,
            epoch,
            &self.record.account,
            keys.account_private(),
        )?;
        self.epoch_keys.insert((chat, epoch), key);
        Ok(())
    }

    /// Encrypt and sign a message under the chat's current key. Refuses once the key is over 7 days
    /// old, going by the client send time in the header.
    pub fn seal(&self, header: &Header, plaintext: &[u8]) -> Result<(Vec<u8>, [u8; 64]), MlsError> {
        if header.device != self.record.device {
            return Err(MlsError::WrongDevice {
                device: header.device,
            });
        }
        let Some(epoch) = header.epoch else {
            return Err(MlsError::NotEncrypted);
        };
        if !header.encrypted {
            return Err(MlsError::NotEncrypted);
        }
        if self.rotation_due(header.chat, header.client_sent_at)? {
            return Err(MlsError::RotationDue { chat: header.chat });
        }
        let cipher = self.message_cipher(header.chat, epoch)?;
        let mut nonce = [0u8; 12];
        if let Err(source) = getrandom::fill(&mut nonce) {
            return Err(MlsError::Random { source });
        }
        let data = header.bytes();
        let payload = Payload {
            msg: plaintext,
            aad: &data,
        };
        let sealed = match cipher.encrypt(Nonce::from_slice(&nonce), payload) {
            Ok(sealed) => sealed,
            Err(_) => return Err(MlsError::Encrypt),
        };
        let mut body = Vec::with_capacity(13 + sealed.len());
        body.push(BODY_VERSION);
        body.extend_from_slice(&nonce);
        body.extend_from_slice(&sealed);
        let signature = self.sign(header, &body)?;
        Ok((body, signature))
    }

    /// Sign an unencrypted message, whose body is its content.
    pub fn sign(&self, header: &Header, body: &[u8]) -> Result<[u8; 64], MlsError> {
        let mut signed = header.bytes();
        signed.extend_from_slice(body);
        self.sign_bytes(&signed)
    }

    /// This device logging itself out. The server terminates it on this.
    pub fn sign_logout(&self) -> Result<[u8; 64], MlsError> {
        self.sign_bytes(&device_statement_bytes(
            LOGOUT_LABEL,
            &self.record.account,
            &self.record.device,
        ))
    }

    /// This device proving which one it is, when its session has expired.
    pub fn sign_device_login(&self, challenge: &[u8; 32]) -> Result<[u8; 64], MlsError> {
        let mut bytes = device_statement_bytes(
            DEVICE_LOGIN_LABEL,
            &self.record.account,
            &self.record.device,
        );
        bytes.extend_from_slice(challenge);
        self.sign_bytes(&bytes)
    }

    fn sign_bytes(&self, signed: &[u8]) -> Result<[u8; 64], MlsError> {
        let signature = match self.signer.sign(signed) {
            Ok(signature) => signature,
            Err(_) => return Err(MlsError::Sign),
        };
        let mut bytes = [0u8; 64];
        if signature.len() != 64 {
            return Err(MlsError::Sign);
        }
        bytes.copy_from_slice(&signature);
        Ok(bytes)
    }

    /// Check a message's signature, then decrypt it, as many times as needed and in any order.
    pub fn open(
        &self,
        header: &Header,
        body: &[u8],
        signature: &[u8; 64],
        device_public: &[u8; 32],
    ) -> Result<Zeroizing<Vec<u8>>, MlsError> {
        verify_message(header, body, signature, device_public)?;
        let Some(epoch) = header.epoch else {
            return Err(MlsError::NotEncrypted);
        };
        if !header.encrypted {
            return Err(MlsError::NotEncrypted);
        }
        if body.is_empty() {
            return Err(MlsError::BodyShort);
        }
        if body[0] != BODY_VERSION {
            return Err(MlsError::BodyVersion { version: body[0] });
        }
        if body.len() < 29 {
            return Err(MlsError::BodyShort);
        }
        let cipher = self.message_cipher(header.chat, epoch)?;
        let data = header.bytes();
        let payload = Payload {
            msg: &body[13..],
            aad: &data,
        };
        match cipher.decrypt(Nonce::from_slice(&body[1..13]), payload) {
            Ok(plaintext) => Ok(Zeroizing::new(plaintext)),
            Err(_) => Err(MlsError::Decrypt),
        }
    }

    /// Everything this device holds, as one blob, for the client to save before sending anything.
    /// Saves alternate between two slots, so a torn or failed write cannot destroy the only copy.
    pub fn export(&mut self) -> Result<Snapshot, StateError> {
        self.generation += 1;
        let mut out = Zeroizing::new(Vec::new());
        out.push(STATE_VERSION);
        out.extend_from_slice(&self.generation.to_be_bytes());
        out.extend_from_slice(self.record.account.bytes());
        out.extend_from_slice(self.record.device.bytes());
        out.extend_from_slice(&self.record.device_public);
        out.extend_from_slice(&self.record.master_public);
        out.extend_from_slice(&self.record.stamp);

        out.extend_from_slice(&(self.pins.len() as u32).to_be_bytes());
        for (account, master) in &self.pins {
            out.extend_from_slice(account.bytes());
            out.extend_from_slice(master);
        }
        out.extend_from_slice(&(self.epoch_keys.len() as u32).to_be_bytes());
        for ((chat, epoch), key) in &self.epoch_keys {
            out.extend_from_slice(chat.bytes());
            out.extend_from_slice(&epoch.to_be_bytes());
            out.extend_from_slice(key.as_ref());
        }
        out.extend_from_slice(&(self.chats.len() as u32).to_be_bytes());
        for (chat, state) in &self.chats {
            out.extend_from_slice(chat.bytes());
            out.extend_from_slice(&state.epoch_started_at.to_be_bytes());
            out.push(u8::from(state.joining));
        }
        let values = match self.provider.storage().values.read() {
            Ok(values) => values,
            Err(_) => return Err(StateError::Lock),
        };
        out.extend_from_slice(&(values.len() as u32).to_be_bytes());
        for (key, value) in values.iter() {
            out.extend_from_slice(&(key.len() as u32).to_be_bytes());
            out.extend_from_slice(key);
            out.extend_from_slice(&(value.len() as u32).to_be_bytes());
            out.extend_from_slice(value);
        }
        drop(values);

        let checksum = Sha256::digest(out.as_slice());
        out.extend_from_slice(checksum.as_slice());
        Ok(Snapshot {
            slot: (self.generation % 2) as u8,
            bytes: out,
        })
    }

    /// Load from the two slots, taking the newest one that checks out.
    pub fn import(first: Option<&[u8]>, second: Option<&[u8]>) -> Result<Imported, StateError> {
        let mut first_result = None;
        if let Some(bytes) = first {
            first_result = Some(Self::import_slot(bytes));
        }
        let mut second_result = None;
        if let Some(bytes) = second {
            second_result = Some(Self::import_slot(bytes));
        }
        match (first_result, second_result) {
            (None, None) => Err(StateError::Empty),
            (Some(Ok(device)), None) | (None, Some(Ok(device))) => Ok(Imported {
                device,
                fell_back: false,
            }),
            (Some(Err(error)), None) | (None, Some(Err(error))) => Err(error),
            (Some(Ok(device)), Some(Err(_))) | (Some(Err(_)), Some(Ok(device))) => Ok(Imported {
                device,
                fell_back: true,
            }),
            (Some(Err(first)), Some(Err(second))) => Err(StateError::NothingUsable {
                first: Box::new(first),
                second: Box::new(second),
            }),
            (Some(Ok(first)), Some(Ok(second))) => {
                let device = if first.generation >= second.generation {
                    first
                } else {
                    second
                };
                Ok(Imported {
                    device,
                    fell_back: false,
                })
            }
        }
    }

    fn import_slot(bytes: &[u8]) -> Result<Self, StateError> {
        if bytes.is_empty() {
            return Err(StateError::Truncated);
        }
        // Checked before the checksum, because a future version may lay the checksum out
        // differently, and then the honest answer is "unknown", not "corrupt".
        if bytes[0] != STATE_VERSION {
            return Err(StateError::UnknownVersion { version: bytes[0] });
        }
        if bytes.len() < STATE_CHECKSUM_SIZE + 1 {
            return Err(StateError::Truncated);
        }
        let (content, checksum) = bytes.split_at(bytes.len() - STATE_CHECKSUM_SIZE);
        if Sha256::digest(content).as_slice() != checksum {
            return Err(StateError::Checksum);
        }

        let mut reader = &content[1..];
        let generation = u64::from_be_bytes(read_array(&mut reader)?);
        let account = Ulid::from_bytes(read_array(&mut reader)?);
        let device = Ulid::from_bytes(read_array(&mut reader)?);
        let device_public: [u8; 32] = read_array(&mut reader)?;
        let master_public: [u8; 32] = read_array(&mut reader)?;
        let stamp: [u8; 64] = read_array(&mut reader)?;

        let mut pins = HashMap::new();
        let count = u32::from_be_bytes(read_array(&mut reader)?);
        for _ in 0..count {
            let pinned = Ulid::from_bytes(read_array(&mut reader)?);
            pins.insert(pinned, read_array(&mut reader)?);
        }
        let mut epoch_keys = HashMap::new();
        let count = u32::from_be_bytes(read_array(&mut reader)?);
        for _ in 0..count {
            let chat = Ulid::from_bytes(read_array(&mut reader)?);
            let epoch = u64::from_be_bytes(read_array(&mut reader)?);
            let key: [u8; 16] = read_array(&mut reader)?;
            epoch_keys.insert((chat, epoch), Zeroizing::new(key));
        }
        let mut chat_list = Vec::new();
        let count = u32::from_be_bytes(read_array(&mut reader)?);
        for _ in 0..count {
            let chat = Ulid::from_bytes(read_array(&mut reader)?);
            let epoch_started_at = i64::from_be_bytes(read_array(&mut reader)?);
            let [joining]: [u8; 1] = read_array(&mut reader)?;
            chat_list.push((chat, epoch_started_at, joining == 1));
        }

        let provider = OpenMlsRustCrypto::default();
        let count = u32::from_be_bytes(read_array(&mut reader)?);
        {
            let mut values = match provider.storage().values.write() {
                Ok(values) => values,
                Err(_) => return Err(StateError::Lock),
            };
            for _ in 0..count {
                let key = read_slice(&mut reader)?.to_vec();
                let value = read_slice(&mut reader)?.to_vec();
                values.insert(key, value);
            }
        }

        let signer = match SignatureKeyPair::read(
            provider.storage(),
            &device_public,
            SignatureScheme::ED25519,
        ) {
            Some(signer) => signer,
            None => return Err(StateError::NoSigningKey),
        };
        let mut chats = HashMap::with_capacity(chat_list.len());
        for (chat, epoch_started_at, joining) in chat_list {
            let group = match MlsGroup::load(provider.storage(), &GroupId::from_slice(chat.bytes()))
            {
                Ok(Some(group)) => group,
                Ok(None) => return Err(StateError::NoGroup { chat }),
                Err(source) => {
                    return Err(StateError::Group {
                        source: Box::new(source),
                    });
                }
            };
            chats.insert(
                chat,
                Chat {
                    group,
                    epoch_started_at,
                    joining,
                },
            );
        }
        Ok(Self {
            provider,
            signer,
            record: DeviceRecord {
                account,
                device,
                device_public,
                master_public,
                stamp,
            },
            generation,
            pins,
            epoch_keys,
            chats,
        })
    }

    fn credential(&self) -> CredentialWithKey {
        let mut identity = Vec::with_capacity(CREDENTIAL_SIZE);
        identity.extend_from_slice(self.record.account.bytes());
        identity.extend_from_slice(self.record.device.bytes());
        identity.extend_from_slice(&self.record.master_public);
        identity.extend_from_slice(&self.record.stamp);
        CredentialWithKey {
            credential: BasicCredential::new(identity).into(),
            signature_key: self.signer.public().into(),
        }
    }

    fn chat(&self, chat: Ulid) -> Result<&Chat, MlsError> {
        match self.chats.get(&chat) {
            Some(state) => Ok(state),
            None => Err(MlsError::UnknownChat { chat }),
        }
    }

    fn refuse_second_commit(&self, chat: Ulid) -> Result<(), MlsError> {
        let state = self.chat(chat)?;
        if state.joining || state.group.pending_commit().is_some() {
            return Err(MlsError::CommitPending { chat });
        }
        Ok(())
    }

    /// The chat's current group info, which any member can give the server, so a stale copy
    /// is never stuck there if the committer crashed.
    pub fn group_info(&self, chat: Ulid) -> Result<Vec<u8>, MlsError> {
        let state = self.chat(chat)?;
        let info = match state
            .group
            .export_group_info(self.provider.crypto(), &self.signer, true)
        {
            Ok(info) => info,
            Err(source) => return Err(group_error("export group info", source)),
        };
        wire_out("encode group info", &info)
    }

    /// Export the key of the epoch the chat is now in, and keep it for reading.
    fn keep_current_key(&mut self, chat: Ulid) -> Result<(), MlsError> {
        let state = self.chat(chat)?;
        let key = match state.group.export_secret(
            self.provider.crypto(),
            MESSAGE_KEY_LABEL,
            chat.bytes(),
            16,
        ) {
            Ok(key) => Zeroizing::new(key),
            Err(source) => return Err(group_error("export the epoch key", source)),
        };
        let epoch = state.group.epoch().as_u64();
        let mut bytes = Zeroizing::new([0u8; 16]);
        bytes.copy_from_slice(&key);
        self.epoch_keys.insert((chat, epoch), bytes);
        Ok(())
    }

    fn current_key(&self, chat: Ulid, epoch: u64) -> Result<Zeroizing<[u8; 16]>, MlsError> {
        match self.epoch_keys.get(&(chat, epoch)) {
            Some(key) => Ok(key.clone()),
            None => Err(MlsError::NoEpochKey { chat, epoch }),
        }
    }

    fn message_cipher(&self, chat: Ulid, epoch: u64) -> Result<Aes128Gcm, MlsError> {
        let key = self.current_key(chat, epoch)?;
        match Aes128Gcm::new_from_slice(key.as_ref()) {
            Ok(cipher) => Ok(cipher),
            Err(_) => Err(MlsError::Encrypt),
        }
    }

    /// The new epoch's secrets are readable before it is applied, so the wrapped keys can go up
    /// together with the commit that needs them. A join is already applied when it is built.
    fn pending_commit(
        &mut self,
        chat: Ulid,
        commit: Vec<u8>,
        welcome: Option<Vec<u8>>,
        account_list: &[AccountKey],
    ) -> Result<Commit, MlsError> {
        let state = self.chat(chat)?;
        let crypto = self.provider.crypto();
        let context = chat.bytes();
        let (secret, key, epoch) = if state.joining {
            let group = &state.group;
            (
                group.export_secret(crypto, wrap::SECRET_LABEL, context, 32),
                group.export_secret(crypto, MESSAGE_KEY_LABEL, context, 16),
                group.epoch().as_u64(),
            )
        } else {
            let Some(staged) = state.group.pending_commit() else {
                return Err(MlsError::NoCommitPending { chat });
            };
            (
                staged.export_secret(crypto, wrap::SECRET_LABEL, context, 32),
                staged.export_secret(crypto, MESSAGE_KEY_LABEL, context, 16),
                staged.group_context().epoch().as_u64(),
            )
        };
        let secret = match secret {
            Ok(secret) => Zeroizing::new(secret),
            Err(source) => return Err(group_error("export the wrap secret", source)),
        };
        let key = match key {
            Ok(key) => Zeroizing::new(key),
            Err(source) => return Err(group_error("export the epoch key", source)),
        };
        let mut key_bytes = Zeroizing::new([0u8; 16]);
        key_bytes.copy_from_slice(&key);
        let wrap_list = self.wrap_list(&secret, &key_bytes, chat, epoch, account_list)?;
        Ok(Commit {
            epoch,
            commit,
            welcome,
            wrap_list,
        })
    }

    fn wrap_list(
        &mut self,
        secret: &[u8],
        key: &[u8; 16],
        chat: Ulid,
        epoch: u64,
        account_list: &[AccountKey],
    ) -> Result<Vec<AccountWrap>, MlsError> {
        let mut secret_bytes = Zeroizing::new([0u8; 32]);
        secret_bytes.copy_from_slice(secret);
        let mut list = Vec::with_capacity(account_list.len());
        for account in account_list {
            let bytes = account_stamp_bytes(&account.account, &account.account_public);
            let bad = MlsError::BadStamp {
                account: account.account,
            };
            verify(&account.master_public, &bytes, &account.stamp, bad)?;
            pin(&mut self.pins, account.account, &account.master_public)?;
            let wrapped = wrap::wrap(
                &secret_bytes,
                &chat,
                epoch,
                &account.account,
                &account.account_public,
                key,
            )?;
            list.push(AccountWrap {
                account: account.account,
                wrapped,
            });
        }
        Ok(list)
    }
}

fn read_array<const N: usize>(reader: &mut &[u8]) -> Result<[u8; N], StateError> {
    if reader.len() < N {
        return Err(StateError::Truncated);
    }
    let (head, rest) = reader.split_at(N);
    let mut array = [0u8; N];
    array.copy_from_slice(head);
    *reader = rest;
    Ok(array)
}

fn read_slice<'a>(reader: &mut &'a [u8]) -> Result<&'a [u8], StateError> {
    let size = u32::from_be_bytes(read_array(reader)?) as usize;
    if reader.len() < size {
        return Err(StateError::Truncated);
    }
    let (head, rest) = reader.split_at(size);
    *reader = rest;
    Ok(head)
}
