//! Files' bytes on disk under `[files] directory`: a folder per chat named by its id, and one file per
//! upload named by its id, holding its encrypted chunks back to back. Chunk writes and reads, and
//! removing uploads no sent message named. Stickers live there too, in a `stickers` folder, a file
//! per sticker named by its id, each a WebP checked by its header. The rows are in storage. See the
//! root AGENTS.md.

use std::io::SeekFrom;
use std::path::{Path, PathBuf};

use keryx_core::ulid::Ulid;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

use crate::api::{App, Refused, refused};
use crate::storage::Storage;

/// A chunk's plaintext, fixed by the author at 1 MiB.
pub const CHUNK: u64 = 1024 * 1024;
/// AES-GCM's tag, which every encrypted chunk carries.
pub const TAG: u64 = 16;
/// An upload no sent message named is removed once it is older than this, fixed by the author.
pub const UNUSED_AFTER_MS: i64 = 24 * 60 * 60 * 1000;

/// Whether `size` encrypted bytes in `chunks` chunks is a shape a sealed file can have: every chunk
/// but the last is full, and the last holds at least its tag.
pub fn fits(size: u64, chunks: u64) -> bool {
    if chunks == 0 {
        return false;
    }
    let full = CHUNK + TAG;
    let Some(before) = (chunks - 1).checked_mul(full) else {
        return false;
    };
    let Some(last) = size.checked_sub(before) else {
        return false;
    };
    last >= TAG && last <= full && (chunks == 1 || last > TAG)
}

/// Chunk `n`'s encrypted length, for a file that `fits`.
pub fn chunk_length(size: u64, chunks: u64, n: u64) -> u64 {
    let full = CHUNK + TAG;
    if n + 1 < chunks {
        full
    } else {
        size - (chunks - 1) * full
    }
}

fn path(directory: &Path, chat: Ulid, file: Ulid) -> PathBuf {
    directory.join(chat.to_string()).join(file.to_string())
}

fn failed(error: std::io::Error) -> Refused {
    tracing::error!(msg = "A file's bytes failed", error = %error);
    refused(500, "internal", "Something failed on the server")
}

/// Writes chunk `n` at its place, and waits until it is on disk.
pub async fn write_chunk(
    app: &App,
    chat: Ulid,
    file: Ulid,
    n: u64,
    bytes: &[u8],
) -> Result<(), Refused> {
    let path = path(&app.settings.files.directory, chat, file);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(failed)?;
    }
    let mut handle = tokio::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .await
        .map_err(failed)?;
    handle
        .seek(SeekFrom::Start(n * (CHUNK + TAG)))
        .await
        .map_err(failed)?;
    handle.write_all(bytes).await.map_err(failed)?;
    handle.sync_data().await.map_err(failed)
}

/// Reads chunk `n`, `length` bytes.
pub async fn read_chunk(
    app: &App,
    chat: Ulid,
    file: Ulid,
    n: u64,
    length: u64,
) -> Result<Vec<u8>, Refused> {
    let path = path(&app.settings.files.directory, chat, file);
    let mut handle = tokio::fs::File::open(&path).await.map_err(failed)?;
    handle
        .seek(SeekFrom::Start(n * (CHUNK + TAG)))
        .await
        .map_err(failed)?;
    let mut bytes = vec![0; usize::try_from(length).unwrap_or(0)];
    handle.read_exact(&mut bytes).await.map_err(failed)?;
    Ok(bytes)
}

/// Deletes a file's bytes. One already gone is fine, as a sweep may finish what a crash left.
pub async fn remove(app: &App, chat: Ulid, file: Ulid) {
    let path = path(&app.settings.files.directory, chat, file);
    match tokio::fs::remove_file(&path).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => tracing::error!(msg = "Could not delete a file's bytes", error = %error),
    }
}

/// Removes the uploads no sent message named within a day, in one chat or, with `None`, in all.
/// There is no job on a timer: a chat's task calls this as it loads, and startup for every chat.
/// Files a delete or edit removed whose bytes a crash left behind go too.
pub async fn sweep(app: &App, chat: Option<Ulid>) -> Result<(), Refused> {
    let now = Ulid::now();
    let gone = app
        .storage
        .take_unused_files(chat, now - UNUSED_AFTER_MS)
        .await?;
    for (chat, file) in gone {
        remove(app, chat, file).await;
    }
    Ok(())
}

/// The largest a sticker may be on either side, fixed by the author.
pub const STICKER_SIDE: u32 = 512;

/// A sticker's width and height, read from its WebP header without decoding a pixel: a RIFF file of
/// type WEBP whose first chunk is VP8 (lossy), VP8L (lossless) or VP8X (extended, which animation
/// needs). `None` for anything else.
pub fn webp_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 30 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WEBP" {
        return None;
    }
    let riff = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    if usize::try_from(riff).ok()? + 8 != bytes.len() {
        return None;
    }
    let data = &bytes[20..];
    match &bytes[12..16] {
        b"VP8 " => {
            // A frame tag of 3 bytes, the start code, then 14-bit width and height.
            if data[3..6] != [0x9d, 0x01, 0x2a] {
                return None;
            }
            let width = u32::from(u16::from_le_bytes([data[6], data[7]]) & 0x3fff);
            let height = u32::from(u16::from_le_bytes([data[8], data[9]]) & 0x3fff);
            Some((width, height))
        }
        b"VP8L" => {
            // The signature, then width - 1 and height - 1 in 14 bits each.
            if data[0] != 0x2f {
                return None;
            }
            let bits = u32::from_le_bytes([data[1], data[2], data[3], data[4]]);
            Some(((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1))
        }
        b"VP8X" => {
            // Flags and 3 reserved bytes, then the canvas's width - 1 and height - 1 in 24 bits.
            let width = u32::from_le_bytes([data[4], data[5], data[6], 0]) + 1;
            let height = u32::from_le_bytes([data[7], data[8], data[9], 0]) + 1;
            Some((width, height))
        }
        _ => None,
    }
}

fn sticker_path(directory: &Path, sticker: Ulid) -> PathBuf {
    directory.join("stickers").join(sticker.to_string())
}

/// Writes a sticker's bytes, and waits until they are on disk.
pub async fn write_sticker(app: &App, sticker: Ulid, bytes: &[u8]) -> Result<(), Refused> {
    let path = sticker_path(&app.settings.files.directory, sticker);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(failed)?;
    }
    let mut handle = tokio::fs::File::create(&path).await.map_err(failed)?;
    handle.write_all(bytes).await.map_err(failed)?;
    handle.sync_data().await.map_err(failed)
}

pub async fn read_sticker(app: &App, sticker: Ulid) -> Result<Vec<u8>, Refused> {
    let path = sticker_path(&app.settings.files.directory, sticker);
    tokio::fs::read(&path).await.map_err(failed)
}

/// Deletes a sticker's bytes. One already gone is fine.
pub async fn remove_sticker(app: &App, sticker: Ulid) {
    let path = sticker_path(&app.settings.files.directory, sticker);
    match tokio::fs::remove_file(&path).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => tracing::error!(msg = "Could not delete a sticker's bytes", error = %error),
    }
}
