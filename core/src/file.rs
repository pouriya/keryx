//! Files: sealing and opening chunks, a file's description inside a message, and BlurHash. See the
//! root AGENTS.md, "How a file is built".
//!
//! Each file has its own random AES-128 key, used for that file only. A chunk is 1 MiB of plaintext,
//! sealed with its number as the nonce and, as associated data, whether it is the last, so a server
//! that reorders, swaps or cuts chunks makes the file fail to open. A sealed chunk is its ciphertext
//! and 16-byte tag, with no nonce stored.

use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes128Gcm, KeyInit, Nonce};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::protocol::{Bytes, ContentKind};
use crate::ulid::Ulid;

/// A chunk's plaintext, fixed by the author.
pub const CHUNK: u64 = 1024 * 1024;
/// AES-GCM's tag, after every sealed chunk.
pub const TAG: u64 = 16;
/// The most files one bundle holds, fixed by the author.
pub const BUNDLE_FILES: usize = 20;
/// Music's one type: MP3, chosen by the author.
pub const MUSIC_TYPE: &str = "audio/mpeg";

/// The associated data's label, so a chunk's seal can never be taken for anything else's.
const CHUNK_LABEL: &[u8] = b"keryx v1 file chunk";
/// BlurHash's components across and down: the format's usual 4 by 3.
const BLURHASH_X: u32 = 4;
const BLURHASH_Y: u32 = 3;

#[derive(Debug, thiserror::Error)]
pub enum FileError {
    #[error("Could not get random bytes for a file's key")]
    Random(#[source] getrandom::Error),
    #[error("A file's key is not 16 bytes")]
    BadKey,
    #[error("Could not seal a chunk")]
    Seal,
    #[error("Chunk {n} does not open: it was changed, moved or cut")]
    Open { n: u64 },
    #[error("Could not make a BlurHash")]
    Blurhash(#[source] blurhash::Error),
    #[error("A thumbnail's pixels are not its width times its height in RGBA")]
    BadPixels,
}

/// What a message says about one of its files, inside its encryption. `size` is the real size, and
/// `width`, `height` and `blurhash` are for images, videos and a music file's cover. `description`
/// is an image's or video's own text in a bundle. `title`, `artist` and `duration`, in milliseconds,
/// are music's, from its tags.
#[cfg_attr(feature = "wasm", derive(tsify::Tsify), tsify(missing_as_null))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileInfo {
    pub id: Ulid,
    pub kind: ContentKind,
    pub name: String,
    #[serde(rename = "type")]
    pub mime: String,
    pub size: u64,
    pub key: Bytes,
    pub chunks: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blurhash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<u64>,
}

/// A new random key, for one file only.
pub fn new_key() -> Result<Zeroizing<[u8; 16]>, FileError> {
    let mut key = Zeroizing::new([0u8; 16]);
    if let Err(source) = getrandom::fill(key.as_mut()) {
        return Err(FileError::Random(source));
    }
    Ok(key)
}

/// How many chunks a file of `size` plaintext bytes takes. An empty file is one chunk, its tag.
pub fn chunks_for(size: u64) -> u64 {
    size.div_ceil(CHUNK).max(1)
}

/// The sealed size, what the server stores and counts against its limits.
pub fn sealed_size(size: u64) -> u64 {
    size + TAG * chunks_for(size)
}

/// Chunk `n`'s plaintext length, for a file of `size` bytes.
pub fn chunk_plain(size: u64, n: u64) -> u64 {
    let start = n * CHUNK;
    size.saturating_sub(start).min(CHUNK)
}

fn cipher(key: &[u8]) -> Result<Aes128Gcm, FileError> {
    match Aes128Gcm::new_from_slice(key) {
        Ok(cipher) => Ok(cipher),
        Err(_) => Err(FileError::BadKey),
    }
}

fn nonce(n: u64) -> [u8; 12] {
    let mut nonce = [0u8; 12];
    nonce[4..].copy_from_slice(&n.to_be_bytes());
    nonce
}

fn associated(last: bool) -> Vec<u8> {
    let mut data = CHUNK_LABEL.to_vec();
    data.push(u8::from(last));
    data
}

/// Seals chunk `n` of a file. `last` says whether it is the file's last chunk.
pub fn seal_chunk(key: &[u8], n: u64, last: bool, plain: &[u8]) -> Result<Vec<u8>, FileError> {
    let aad = associated(last);
    let nonce = nonce(n);
    match cipher(key)?.encrypt(
        Nonce::from_slice(&nonce),
        Payload {
            msg: plain,
            aad: &aad,
        },
    ) {
        Ok(sealed) => Ok(sealed),
        Err(_) => Err(FileError::Seal),
    }
}

/// Opens chunk `n`. Fails if it is not that chunk of that file, or the last when it says so.
pub fn open_chunk(
    key: &[u8],
    n: u64,
    last: bool,
    sealed: &[u8],
) -> Result<Zeroizing<Vec<u8>>, FileError> {
    let aad = associated(last);
    let nonce = nonce(n);
    match cipher(key)?.decrypt(
        Nonce::from_slice(&nonce),
        Payload {
            msg: sealed,
            aad: &aad,
        },
    ) {
        Ok(plain) => Ok(Zeroizing::new(plain)),
        Err(_) => Err(FileError::Open { n }),
    }
}

/// A picture's BlurHash, from its small RGBA pixels, `width` by `height`.
pub fn blurhash_of(width: u32, height: u32, rgba: &[u8]) -> Result<String, FileError> {
    // The crate indexes the pixels without checking their length, and would panic.
    let expected = u64::from(width) * u64::from(height) * 4;
    if expected == 0 || rgba.len() as u64 != expected {
        return Err(FileError::BadPixels);
    }
    match blurhash::encode(BLURHASH_X, BLURHASH_Y, width, height, rgba) {
        Ok(hash) => Ok(hash),
        Err(source) => Err(FileError::Blurhash(source)),
    }
}

/// A BlurHash drawn back as RGBA pixels, `width` by `height`, to show while the file downloads.
pub fn blurhash_pixels(hash: &str, width: u32, height: u32) -> Result<Vec<u8>, FileError> {
    match blurhash::decode(hash, width, height, 1.0) {
        Ok(pixels) => Ok(pixels),
        Err(source) => Err(FileError::Blurhash(source)),
    }
}
