//! Calls, with no I/O: SFrame sealing and opening, a media frame's layout, the `state` track. See the
//! root AGENTS.md, "Calls".
//!
//! Every device in a chat's MLS group exports the same SFrame base secret per epoch. A frame's key id
//! names its sender's MLS index and the epoch's low bits, so each sender has its own key with no
//! coordination. Sealing never touches a socket: the platform hands frames in and sends what comes out.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use sframe::CipherSuite;
use sframe::frame::{EncryptedFrameView, MediaFrame, MonotonicCounter};
use sframe::header::Counter;
use sframe::key::{DecryptionKey, EncryptionKey};
use sframe::mls::{MlsKeyId, MlsKeyIdBitRange};
use zeroize::{Zeroize, Zeroizing};

use crate::protocol::Bytes;

/// RFC 9605's exporter label for the base secret, with an empty context.
pub const BASE_LABEL: &str = "SFrame 1.0 Base Key";
/// The MLS suite's strength: AES-128-GCM with SHA-256 and a 16-byte tag.
pub const SUITE: CipherSuite = CipherSuite::AesGcm128Sha256;
/// The bytes the exporter gives for the base secret, what the suite derives keys from.
pub const BASE_LEN: usize = 32;
/// Bits of a key id for the sender's MLS index and for the epoch's low bits, fixed by the author.
const INDEX_BITS: u8 = 16;
const EPOCH_BITS: u8 = 8;
/// A media frame's plaintext starts with its timestamp, 8 bytes big-endian microseconds.
const TIMESTAMP: usize = 8;

#[derive(Debug, thiserror::Error)]
pub enum CallError {
    #[error("Could not get random bytes for a frame counter")]
    Random(#[source] getrandom::Error),
    #[error("SFrame refused a key")]
    Key(#[source] sframe::error::SframeError),
    #[error("Could not seal a frame")]
    Seal(#[source] sframe::error::SframeError),
    #[error("A frame does not open: it was changed, or its key is unknown")]
    Open,
    #[error("A frame came from MLS index {index}, not the device whose broadcast carried it")]
    WrongSender { index: u64 },
    #[error("A frame is too short to hold its timestamp")]
    Short,
    #[error("A state frame is not the state's JSON")]
    State(#[source] serde_json::Error),
}

/// One epoch's base secret, and this device's own MLS index in it. It stays in the Rust that holds
/// the call, and never goes into a platform's own language.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Clone, Serialize, Deserialize)]
pub struct CallKey {
    pub epoch: u64,
    pub index: u32,
    pub secret: Bytes,
}

impl std::fmt::Debug for CallKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CallKey")
            .field("epoch", &self.epoch)
            .field("index", &self.index)
            .finish_non_exhaustive()
    }
}

impl Drop for CallKey {
    fn drop(&mut self) {
        self.secret.0.zeroize();
    }
}

/// A member device's MLS index, which its frames' key ids must name.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallIndex {
    pub device: crate::ulid::Ulid,
    pub index: u32,
}

/// What a device has on, on its `state` track.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    pub video: bool,
    pub voice: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation: Option<u16>,
}

fn range() -> MlsKeyIdBitRange {
    MlsKeyIdBitRange::new(EPOCH_BITS, INDEX_BITS)
}

fn key_id(epoch: u64, index: u64) -> Result<MlsKeyId, CallError> {
    match MlsKeyId::try_new(0u64, epoch, index, range()) {
        Ok(id) => Ok(id),
        Err(source) => Err(CallError::Key(source)),
    }
}

/// Seals this device's frames under one epoch's key. A new epoch takes a new sealer.
pub struct Sealer {
    key: EncryptionKey,
    counter: MonotonicCounter,
}

impl Sealer {
    /// The counter starts at a random 63-bit value: a key depends only on the epoch and the sender,
    /// so a device that rejoins or restarts in the same epoch must never count from 0 again.
    pub fn new(key: &CallKey) -> Result<Self, CallError> {
        let id = key_id(key.epoch, u64::from(key.index))?;
        let key = match EncryptionKey::derive_from(SUITE, id, &key.secret.0) {
            Ok(key) => key,
            Err(source) => return Err(CallError::Key(source)),
        };
        let mut start = [0u8; 8];
        if let Err(source) = getrandom::fill(&mut start) {
            return Err(CallError::Random(source));
        }
        let start: Counter = u64::from_be_bytes(start) >> 1;
        Ok(Self {
            key,
            counter: MonotonicCounter::with_start_value(start, Counter::MAX),
        })
    }

    fn seal_plain(&mut self, plain: &[u8]) -> Result<Vec<u8>, CallError> {
        let frame = match MediaFrame::try_new(&mut self.counter, plain) {
            Ok(frame) => frame,
            Err(source) => return Err(CallError::Seal(source)),
        };
        match frame.encrypt(&self.key) {
            Ok(sealed) => Ok(sealed.sframe_cipher_text().to_vec()),
            Err(source) => Err(CallError::Seal(source)),
        }
    }

    /// A media frame: its timestamp in microseconds, then the codec's bytes.
    pub fn seal(&mut self, timestamp: u64, codec: &[u8]) -> Result<Vec<u8>, CallError> {
        let mut plain = Zeroizing::new(Vec::with_capacity(TIMESTAMP + codec.len()));
        plain.extend_from_slice(&timestamp.to_be_bytes());
        plain.extend_from_slice(codec);
        self.seal_plain(&plain)
    }

    /// What this device has on.
    pub fn seal_state(&mut self, state: &State) -> Result<Vec<u8>, CallError> {
        let plain = match serde_json::to_vec(state) {
            Ok(plain) => plain,
            Err(source) => return Err(CallError::State(source)),
        };
        self.seal_plain(&plain)
    }
}

/// Opens everyone's frames, with every epoch's secret it was given.
#[derive(Default)]
pub struct Opener {
    secrets: HashMap<u64, Zeroizing<Vec<u8>>>,
    keys: HashMap<u64, DecryptionKey>,
}

impl Opener {
    /// Keeps an epoch's secret, under the low bits a frame's key id carries. A newer epoch with the
    /// same low bits replaces an older one.
    pub fn add(&mut self, key: &CallKey) {
        let low = key.epoch & ((1 << EPOCH_BITS) - 1);
        self.secrets
            .insert(low, Zeroizing::new(key.secret.0.clone()));
        self.keys
            .retain(|id, _| MlsKeyId::from_key_id(*id, range()).epoch_lsb() != low);
    }

    fn open_plain(&mut self, index: u32, sealed: &[u8]) -> Result<Zeroizing<Vec<u8>>, CallError> {
        let Ok(view) = EncryptedFrameView::try_new(&sealed) else {
            return Err(CallError::Open);
        };
        let id = view.header().key_id();
        let mls = MlsKeyId::from_key_id(id, range());
        if mls.member_index() != u64::from(index) {
            return Err(CallError::WrongSender {
                index: mls.member_index(),
            });
        }
        if !self.keys.contains_key(&id) {
            let Some(secret) = self.secrets.get(&mls.epoch_lsb()) else {
                return Err(CallError::Open);
            };
            let Ok(key) = DecryptionKey::derive_from(SUITE, id, secret.as_slice()) else {
                return Err(CallError::Open);
            };
            self.keys.insert(id, key);
        }
        let Some(key) = self.keys.get(&id) else {
            return Err(CallError::Open);
        };
        match view.decrypt(key) {
            Ok(frame) => Ok(Zeroizing::new(frame.payload().to_vec())),
            Err(_) => Err(CallError::Open),
        }
    }

    /// A media frame from the device at MLS `index`: its timestamp and the codec's bytes.
    pub fn open(
        &mut self,
        index: u32,
        sealed: &[u8],
    ) -> Result<(u64, Zeroizing<Vec<u8>>), CallError> {
        let plain = self.open_plain(index, sealed)?;
        if plain.len() < TIMESTAMP {
            return Err(CallError::Short);
        }
        let mut timestamp = [0u8; TIMESTAMP];
        timestamp.copy_from_slice(&plain[..TIMESTAMP]);
        Ok((
            u64::from_be_bytes(timestamp),
            Zeroizing::new(plain[TIMESTAMP..].to_vec()),
        ))
    }

    /// What the device at MLS `index` has on.
    pub fn open_state(&mut self, index: u32, sealed: &[u8]) -> Result<State, CallError> {
        let plain = self.open_plain(index, sealed)?;
        match serde_json::from_slice(&plain) {
            Ok(state) => Ok(state),
            Err(source) => Err(CallError::State(source)),
        }
    }
}
