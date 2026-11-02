//! Wrapping epoch keys to accounts, and unwrapping them with the words.
//!
//! ```text
//! secret    = MLS exporter("keryx v1 wrap secret", chat id, 32 bytes)
//! eph       = HKDF-SHA256(secret, info = "keryx v1 wrap ephemeral" | account id)
//! shared    = X25519(eph, account public)
//! key,nonce = HKDF-SHA256(shared, salt = eph public | account public,
//!                         info = "keryx v1 wrap key")          16 + 12 bytes
//! wrap      = version (1) | eph public (32) | AES-128-GCM(epoch key) (16 + 16 tag)
//! data      = "keryx v1 wrap" | chat id | epoch | account id
//! ```
//!
//! **Repeatable on purpose.** Running devices drop the words' keys and cannot open a wrap, so
//! the random part comes from the epoch's own secret: every member recomputes every wrap from
//! public keys and compares. **This layout is permanent:** every stored key depends on it, and
//! `tests/wrap.rs` pins the exact bytes.

use crate::ulid::Ulid;
use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes128Gcm, KeyInit, Nonce};
use hkdf::Hkdf;
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroizing;

/// The MLS exporter label for the secret wraps are made from.
pub const SECRET_LABEL: &str = "keryx v1 wrap secret";
/// The version byte, the ephemeral public key, the epoch key and the AES tag.
pub const WRAP_SIZE: usize = 65;

const VERSION: u8 = 1;
const EPHEMERAL_LABEL: &[u8] = b"keryx v1 wrap ephemeral";
const KEY_LABEL: &[u8] = b"keryx v1 wrap key";
const DATA_LABEL: &[u8] = b"keryx v1 wrap";

#[derive(Debug, thiserror::Error)]
pub enum WrapError {
    #[error("A wrapped key is {WRAP_SIZE} bytes, not {size}")]
    Size { size: usize },

    #[error("Wrapped key version {version} is not one this client knows")]
    Version { version: u8 },

    /// A low-order public key would make the shared secret a known value.
    #[error("This key cannot be used for wrapping: it gives a known shared secret")]
    WeakKey,

    #[error("Could not derive the wrapping key")]
    Derive,

    #[error("Could not encrypt the epoch key")]
    Encrypt,

    /// A corrupt wrap, or one made for a different chat, epoch or account.
    #[error("This wrapped key does not open with this account's key")]
    Open,
}

/// Wrap one epoch key to one account. The same inputs always give the same bytes.
pub fn wrap(
    secret: &[u8; 32],
    chat: &Ulid,
    epoch: u64,
    account: &Ulid,
    account_public: &[u8; 32],
    epoch_key: &[u8; 16],
) -> Result<[u8; WRAP_SIZE], WrapError> {
    let mut info = Vec::with_capacity(EPHEMERAL_LABEL.len() + 16);
    info.extend_from_slice(EPHEMERAL_LABEL);
    info.extend_from_slice(account.bytes());
    let mut ephemeral_bytes = Zeroizing::new([0u8; 32]);
    let hkdf = Hkdf::<Sha256>::new(None, secret);
    if hkdf.expand(&info, ephemeral_bytes.as_mut()).is_err() {
        return Err(WrapError::Derive);
    }
    let ephemeral = StaticSecret::from(*ephemeral_bytes);
    let ephemeral_public = PublicKey::from(&ephemeral).to_bytes();

    let shared = ephemeral.diffie_hellman(&PublicKey::from(*account_public));
    if !shared.was_contributory() {
        return Err(WrapError::WeakKey);
    }
    let (cipher, nonce) = cipher_and_nonce(shared.as_bytes(), &ephemeral_public, account_public)?;
    let payload = Payload {
        msg: epoch_key,
        aad: &associated_data(chat, epoch, account),
    };
    let sealed = match cipher.encrypt(Nonce::from_slice(nonce.as_ref()), payload) {
        Ok(sealed) => sealed,
        Err(_) => return Err(WrapError::Encrypt),
    };

    let mut wrapped = [0u8; WRAP_SIZE];
    wrapped[0] = VERSION;
    wrapped[1..33].copy_from_slice(&ephemeral_public);
    wrapped[33..].copy_from_slice(&sealed);
    Ok(wrapped)
}

/// Open a wrapped key with the account's private key, which only the words give.
pub fn unwrap(
    wrapped: &[u8],
    chat: &Ulid,
    epoch: u64,
    account: &Ulid,
    account_private: &[u8; 32],
) -> Result<Zeroizing<[u8; 16]>, WrapError> {
    if wrapped.len() != WRAP_SIZE {
        return Err(WrapError::Size {
            size: wrapped.len(),
        });
    }
    if wrapped[0] != VERSION {
        return Err(WrapError::Version {
            version: wrapped[0],
        });
    }
    let mut ephemeral_public = [0u8; 32];
    ephemeral_public.copy_from_slice(&wrapped[1..33]);

    let private = StaticSecret::from(*account_private);
    let account_public = PublicKey::from(&private).to_bytes();
    let shared = private.diffie_hellman(&PublicKey::from(ephemeral_public));
    if !shared.was_contributory() {
        return Err(WrapError::WeakKey);
    }
    let (cipher, nonce) = cipher_and_nonce(shared.as_bytes(), &ephemeral_public, &account_public)?;
    let payload = Payload {
        msg: &wrapped[33..],
        aad: &associated_data(chat, epoch, account),
    };
    let opened = match cipher.decrypt(Nonce::from_slice(nonce.as_ref()), payload) {
        Ok(opened) => Zeroizing::new(opened),
        Err(_) => return Err(WrapError::Open),
    };
    if opened.len() != 16 {
        return Err(WrapError::Open);
    }
    let mut epoch_key = Zeroizing::new([0u8; 16]);
    epoch_key.copy_from_slice(&opened);
    Ok(epoch_key)
}

fn cipher_and_nonce(
    shared: &[u8; 32],
    ephemeral_public: &[u8; 32],
    account_public: &[u8; 32],
) -> Result<(Aes128Gcm, Zeroizing<[u8; 12]>), WrapError> {
    let mut salt = [0u8; 64];
    salt[..32].copy_from_slice(ephemeral_public);
    salt[32..].copy_from_slice(account_public);
    let mut output = Zeroizing::new([0u8; 28]);
    let hkdf = Hkdf::<Sha256>::new(Some(&salt), shared);
    if hkdf.expand(KEY_LABEL, output.as_mut()).is_err() {
        return Err(WrapError::Derive);
    }
    let cipher = match Aes128Gcm::new_from_slice(&output[..16]) {
        Ok(cipher) => cipher,
        Err(_) => return Err(WrapError::Derive),
    };
    let mut nonce = Zeroizing::new([0u8; 12]);
    nonce.copy_from_slice(&output[16..]);
    Ok((cipher, nonce))
}

fn associated_data(chat: &Ulid, epoch: u64, account: &Ulid) -> Vec<u8> {
    let mut data = Vec::with_capacity(DATA_LABEL.len() + 40);
    data.extend_from_slice(DATA_LABEL);
    data.extend_from_slice(chat.bytes());
    data.extend_from_slice(&epoch.to_be_bytes());
    data.extend_from_slice(account.bytes());
    data
}
