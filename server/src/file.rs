//! Files' bytes on disk under `[files] directory`: a folder per chat named by its id, and one file per
//! upload named by its id, holding its encrypted chunks back to back. Chunk writes and reads, and
//! removing uploads no sent message named. Stickers live there too, in a `stickers` folder, a file
//! per sticker named by its id, each a WebP checked by its header, and profile photos in
//! `accounts/<account id>`, a file per photo named by its id, each one sealed chunk. The rows are in
//! storage. The bundled `default` sticker pack is compiled in from `stickers/` and held in memory. See
//! the root AGENTS.md.

use std::io::SeekFrom;
use std::path::{Path, PathBuf};

use keryx_core::protocol::{StickerItem, StickerPack};
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

/// The bundled `default` pack's id. It and its stickers' ids are fixed forever, since a sticker message
/// names its sticker by id.
pub const DEFAULT_PACK: &str = "01M75K3ET0RVS2RWCK815GA1N8";
/// The bundled pack's name, which no operator's pack may take, on or off, chosen by the author.
pub const DEFAULT_PACK_NAME: &str = "Default";

/// The bundled pack's stickers, in order: id, emoji, file name and bytes. Bit, a pixel robot by Godspeed
/// Mission Control, from github.com/MichaelZelbel/hermes-telegram-stickers under CC0, converted once to
/// lossless WebP. Each emoji is the first of the two its source names.
const DEFAULT_STICKER_LIST: [(&str, &str, &str, &[u8]); 15] = [
    (
        "01M75K3ET0J4MTEQ0J3G56RJVF",
        "🎧",
        "bit-today.webp",
        include_bytes!("stickers/bit-today.webp"),
    ),
    (
        "01M75K3ET0HJTQ3AQGM8D8ESGT",
        "💻",
        "bit-onit.webp",
        include_bytes!("stickers/bit-onit.webp"),
    ),
    (
        "01M75K3ET0ACPQXC056RSE4K61",
        "📝",
        "bit-note.webp",
        include_bytes!("stickers/bit-note.webp"),
    ),
    (
        "01M75K3ET0GMDRB84WRQBB6CEE",
        "📡",
        "bit-copy.webp",
        include_bytes!("stickers/bit-copy.webp"),
    ),
    (
        "01M75K3ET0GXK4E2AK9GA7SXCW",
        "🔧",
        "bit-fix.webp",
        include_bytes!("stickers/bit-fix.webp"),
    ),
    (
        "01M75K3ET0Y2RZK5P8V1B0CAES",
        "🫡",
        "bit-done.webp",
        include_bytes!("stickers/bit-done.webp"),
    ),
    (
        "01M75K3ET0G1341CS6F2X40D3A",
        "😴",
        "bit-quiet.webp",
        include_bytes!("stickers/bit-quiet.webp"),
    ),
    (
        "01M75K3ET018RHXBR62ED3YV75",
        "👍",
        "bit-ok.webp",
        include_bytes!("stickers/bit-ok.webp"),
    ),
    (
        "01M75K3ET095VQG0BZAJHQ9CX6",
        "🤔",
        "bit-think.webp",
        include_bytes!("stickers/bit-think.webp"),
    ),
    (
        "01M75K3ET06SM5ZPV02CBRWT2C",
        "🔍",
        "bit-found.webp",
        include_bytes!("stickers/bit-found.webp"),
    ),
    (
        "01M75K3ET0YY75648XYK8ACPF6",
        "😅",
        "bit-oops.webp",
        include_bytes!("stickers/bit-oops.webp"),
    ),
    (
        "01M75K3ET0MY6Y4ZZDG50W8Z5X",
        "🎉",
        "bit-party.webp",
        include_bytes!("stickers/bit-party.webp"),
    ),
    (
        "01M75K3ET0H2E2PWSSSQ69ZBT2",
        "🙏",
        "bit-thanks.webp",
        include_bytes!("stickers/bit-thanks.webp"),
    ),
    (
        "01M75K3ET0NRA36BCKP8T027B7",
        "☕",
        "bit-morning.webp",
        include_bytes!("stickers/bit-morning.webp"),
    ),
    (
        "01M75K3ET0S637G16NRFP5SR6K",
        "🌙",
        "bit-night.webp",
        include_bytes!("stickers/bit-night.webp"),
    ),
];

/// The bundled pack, held in memory and never in the database.
pub struct DefaultPack {
    pub id: Ulid,
    pub sticker_list: Vec<StickerItem>,
    bytes: Vec<&'static [u8]>,
}

impl DefaultPack {
    /// The pack as a client lists it.
    pub fn listed(&self) -> StickerPack {
        StickerPack {
            id: self.id,
            name: DEFAULT_PACK_NAME.to_string(),
            sticker_list: self.sticker_list.clone(),
        }
    }

    /// A bundled sticker's bytes, or `None` for any other id.
    pub fn image(&self, id: Ulid) -> Option<&'static [u8]> {
        self.sticker_list
            .iter()
            .position(|sticker| sticker.id == id)
            .and_then(|index| self.bytes.get(index).copied())
    }
}

/// The bundled pack, each sticker put through the same check an upload gets: a WebP of at most 512x512
/// and `limit` bytes. An `Err` names the file that failed, so startup stops.
pub fn default_pack(limit: u64) -> Result<DefaultPack, String> {
    let id = Ulid::parse(DEFAULT_PACK).map_err(|_| "the pack's id".to_string())?;
    let mut sticker_list = Vec::with_capacity(DEFAULT_STICKER_LIST.len());
    let mut bytes = Vec::with_capacity(DEFAULT_STICKER_LIST.len());
    for (sticker, emoji, name, image) in DEFAULT_STICKER_LIST {
        let sticker = Ulid::parse(sticker).map_err(|_| format!("{name}'s id"))?;
        let Some((width, height)) = webp_size(image) else {
            return Err(format!("{name}, which is not a WebP"));
        };
        if width == 0 || height == 0 || width > STICKER_SIDE || height > STICKER_SIDE {
            return Err(format!("{name}, which is larger than 512x512"));
        }
        if image.len() as u64 > limit {
            return Err(format!("{name}, which is over [limits.upload] sticker"));
        }
        sticker_list.push(StickerItem {
            id: sticker,
            emoji: Some(emoji.to_string()),
            width,
            height,
            size: image.len() as u64,
        });
        bytes.push(image);
    }
    Ok(DefaultPack {
        id,
        sticker_list,
        bytes,
    })
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

fn photo_path(directory: &Path, account: Ulid, photo: Ulid) -> PathBuf {
    directory
        .join("accounts")
        .join(account.to_string())
        .join(photo.to_string())
}

/// Writes a profile photo's sealed bytes, and waits until they are on disk.
pub async fn write_photo(
    app: &App,
    account: Ulid,
    photo: Ulid,
    bytes: &[u8],
) -> Result<(), Refused> {
    let path = photo_path(&app.settings.files.directory, account, photo);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(failed)?;
    }
    let mut handle = tokio::fs::File::create(&path).await.map_err(failed)?;
    handle.write_all(bytes).await.map_err(failed)?;
    handle.sync_data().await.map_err(failed)
}

pub async fn read_photo(app: &App, account: Ulid, photo: Ulid) -> Result<Vec<u8>, Refused> {
    let path = photo_path(&app.settings.files.directory, account, photo);
    tokio::fs::read(&path).await.map_err(failed)
}

/// Deletes a profile photo's bytes. One already gone is fine.
pub async fn remove_photo(app: &App, account: Ulid, photo: Ulid) {
    let path = photo_path(&app.settings.files.directory, account, photo);
    match tokio::fs::remove_file(&path).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => tracing::error!(msg = "Could not delete a photo's bytes", error = %error),
    }
}
