//! Words to keys.
//!
//! ```text
//! seed    = BIP39 PBKDF2(words, no passphrase)                64 bytes
//! master  = HKDF-SHA256(seed, "keryx v1 master key")          32 bytes, an Ed25519 key
//! account = HKDF-SHA256(seed, "keryx v1 account key")         32 bytes,
//!           through HPKE's DeriveKeyPair, an X25519 key
//! ```
//!
//! **This can never change once accounts exist:** a derivation that differs by one byte
//! locks a person out of their own history. `tests/seed.rs` pins the exact output.
//!
//! Handing key material back to the caller is the whole job of this module, and it does
//! nothing else with it. Nothing here is logged, and no error carries a word.

use bip39::{Language, Mnemonic};
use ed25519_dalek::SigningKey;
use hkdf::Hkdf;
use openmls::prelude::{HpkeAeadType, HpkeConfig, HpkeKdfType, HpkeKemType, OpenMlsCrypto};
use openmls_rust_crypto::RustCrypto;
use sha2::Sha256;
use zeroize::Zeroizing;

const MASTER_LABEL: &[u8] = b"keryx v1 master key";
const ACCOUNT_LABEL: &[u8] = b"keryx v1 account key";
const PROFILE_LABEL: &[u8] = b"keryx v1 profile key";

#[derive(Debug, thiserror::Error)]
pub enum SeedError {
    #[error("A new phrase has 12 or 24 words, not {words}")]
    WordCount { words: usize },

    #[error("Could not read random bytes from the operating system")]
    Random {
        #[source]
        source: getrandom::Error,
    },

    /// `bip39`'s errors name a position or a count, never the word itself.
    #[error("These words are not a valid English BIP39 phrase")]
    Phrase {
        #[source]
        source: bip39::Error,
    },

    #[error("Could not derive a key from the seed")]
    Derive,

    #[error("Could not derive the account key")]
    AccountKey {
        #[source]
        source: openmls::prelude::CryptoError,
    },
}

/// What the words give. A running device drops this after joining, so that a stolen device
/// cannot unwrap history or add another device.
pub struct AccountKeys {
    master_private: Zeroizing<[u8; 32]>,
    master_public: [u8; 32],
    account_private: Zeroizing<[u8; 32]>,
    account_public: [u8; 32],
    profile: Zeroizing<[u8; 16]>,
}

impl AccountKeys {
    pub fn master_private(&self) -> &[u8; 32] {
        &self.master_private
    }

    pub fn master_public(&self) -> &[u8; 32] {
        &self.master_public
    }

    pub fn account_private(&self) -> &[u8; 32] {
        &self.account_private
    }

    pub fn account_public(&self) -> &[u8; 32] {
        &self.account_public
    }

    /// The AES-128 key the account's photo, bio and links are sealed under. Unlike the others, a
    /// running device keeps it, since it is shared with everyone in the account's chats anyway.
    pub fn profile(&self) -> &[u8; 16] {
        &self.profile
    }
}

/// A new random phrase of 12 or 24 words. A person never picks them, because human choices
/// have far less randomness.
pub fn generate(words: usize) -> Result<Zeroizing<String>, SeedError> {
    let entropy_bytes = match words {
        12 => 16,
        24 => 32,
        _ => return Err(SeedError::WordCount { words }),
    };
    let mut entropy = Zeroizing::new([0u8; 32]);
    if let Err(source) = getrandom::fill(&mut entropy[..entropy_bytes]) {
        return Err(SeedError::Random { source });
    }
    match Mnemonic::from_entropy_in(Language::English, &entropy[..entropy_bytes]) {
        Ok(mnemonic) => Ok(Zeroizing::new(mnemonic.to_string())),
        Err(source) => Err(SeedError::Phrase { source }),
    }
}

/// The keys these words give. Any valid BIP39 length is accepted, for restoring.
pub fn derive(words: &str) -> Result<AccountKeys, SeedError> {
    let mnemonic = match Mnemonic::parse_in(Language::English, words) {
        Ok(mnemonic) => mnemonic,
        Err(source) => return Err(SeedError::Phrase { source }),
    };
    let seed = Zeroizing::new(mnemonic.to_seed(""));
    let hkdf = Hkdf::<Sha256>::new(None, seed.as_ref());

    let mut master_private = Zeroizing::new([0u8; 32]);
    if hkdf.expand(MASTER_LABEL, master_private.as_mut()).is_err() {
        return Err(SeedError::Derive);
    }
    let master_public = SigningKey::from_bytes(&master_private)
        .verifying_key()
        .to_bytes();

    let mut account_material = Zeroizing::new([0u8; 32]);
    if hkdf
        .expand(ACCOUNT_LABEL, account_material.as_mut())
        .is_err()
    {
        return Err(SeedError::Derive);
    }
    let config = HpkeConfig(
        HpkeKemType::DhKem25519,
        HpkeKdfType::HkdfSha256,
        HpkeAeadType::AesGcm128,
    );
    let pair = match RustCrypto::default().derive_hpke_keypair(config, account_material.as_ref()) {
        Ok(pair) => pair,
        Err(source) => return Err(SeedError::AccountKey { source }),
    };
    let mut account_private = Zeroizing::new([0u8; 32]);
    let mut account_public = [0u8; 32];
    if pair.private.as_ref().len() != 32 || pair.public.len() != 32 {
        return Err(SeedError::Derive);
    }
    account_private.copy_from_slice(pair.private.as_ref());
    account_public.copy_from_slice(&pair.public);

    let mut profile = Zeroizing::new([0u8; 16]);
    if hkdf.expand(PROFILE_LABEL, profile.as_mut()).is_err() {
        return Err(SeedError::Derive);
    }

    Ok(AccountKeys {
        master_private,
        master_public,
        account_private,
        account_public,
        profile,
    })
}
