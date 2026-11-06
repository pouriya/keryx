//! The hand-written JNI glue around `keryx-core`'s client and `keryx-moq`'s call, and nothing else:
//! `Core.create`, `Core.step`, `Core.stepBytes`, `Core.chunk`, `Core.blurhash` and `Core.drop`,
//! then `Core.callConnect`, `Core.callAudio`, `Core.callVideo`, `Core.callState` and
//! `Core.callNext` on the Kotlin side. The flow is in `core/src/client.rs`. Inputs and outputs cross as JSON text in
//! core's own serde shapes, and media and file chunks as raw bytes. The app does the socket, the
//! requests, the locked files, the timer and the call's capture and codecs, and feeds back what
//! happened. See android/AGENTS.md.
//!
//! **A call's key never crosses JNI.** `step()` keeps `JoinCall`'s path, token, key and indexes
//! here and hands Kotlin only `{chat, call_seq}`, and `CallKey` and `CallText` go straight to the
//! joined call. **Nor does a file's key:** the ffi keeps each shown file's key and puts it back
//! into `download` and `edit`.
//!
//! Nothing here logs or puts what crosses into an error, since it holds passwords, words, tokens,
//! keys, messages and media: a bad input throws with a fixed message.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use jni::objects::{JByteArray, JClass, JString};
use jni::sys::{jboolean, jint, jlong};
use jni::{Env, EnvUnowned, jni_str};
use keryx_core::call::{CallIndex, CallKey, State};
use keryx_core::client::{Auth, Client, Input, Output, Platform, Secret};
use keryx_core::file;
use keryx_core::ulid::Ulid;
use keryx_moq::{Call, Event};
use serde_json::Value;
use zeroize::Zeroizing;

/// One client this process made, and the call it joined.
struct Entry {
    client: Client,
    /// The call core asked to join, until `Core.callConnect` takes it. `chat` and `call_seq` stay
    /// in `wanted` while it connects, so a leave in the meantime is seen.
    join: Option<Join>,
    wanted: Option<(Ulid, u64)>,
    call: Option<Arc<Call>>,
    /// Chunk bytes held back from JSON until `Core.chunk` takes them, by the number in their
    /// output. A `write_chunk`'s are a file's opened bytes, so they are wiped when dropped.
    chunks: HashMap<u64, Zeroizing<Vec<u8>>>,
    next_chunk: u64,
    /// Each shown file's key by its id, so Kotlin never holds one.
    file_keys: HashMap<String, Zeroizing<String>>,
}

impl Entry {
    /// Holds bytes back for `Core.chunk`, and returns their number.
    fn park(&mut self, bytes: Vec<u8>) -> u64 {
        let chunk = self.next_chunk;
        self.next_chunk += 1;
        self.chunks.insert(chunk, Zeroizing::new(bytes));
        chunk
    }
}

struct Join {
    chat: Ulid,
    call_seq: u64,
    path: String,
    bearer: Option<Secret>,
    key: CallKey,
    index_list: Vec<CallIndex>,
}

/// Every client this process made, by the handle Kotlin holds: its index here.
static CLIENTS: Mutex<Vec<Option<Entry>>> = Mutex::new(Vec::new());

fn clients() -> MutexGuard<'static, Vec<Option<Entry>>> {
    match CLIENTS.lock() {
        Ok(clients) => clients,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn entry(clients: &mut [Option<Entry>], handle: jlong) -> Option<&mut Entry> {
    usize::try_from(handle)
        .ok()
        .and_then(|index| clients.get_mut(index))
        .and_then(Option::as_mut)
}

/// The joined call, held for one use without the lock.
fn call_of(handle: jlong) -> Option<Arc<Call>> {
    entry(&mut clients(), handle).and_then(|entry| entry.call.clone())
}

/// A client for this platform, which logs in with a Bearer token, as android/AGENTS.md chose.
/// Returns its handle, or throws if `platform` is not a platform's JSON.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_pouriya_keryx_Core_create<'caller>(
    mut unowned: EnvUnowned<'caller>,
    _class: JClass<'caller>,
    platform: JString<'caller>,
) -> jlong {
    unowned
        .with_env(|env: &mut Env| -> jni::errors::Result<jlong> {
            let Ok(platform) = serde_json::from_str::<Platform>(&platform.to_string()) else {
                env.throw_new(
                    jni_str!("java/lang/IllegalArgumentException"),
                    jni_str!("keryx: that is not a platform"),
                )?;
                return Ok(-1);
            };
            let mut clients = clients();
            clients.push(Some(Entry {
                client: Client::new(Auth::Token, platform),
                join: None,
                wanted: None,
                call: None,
                chunks: HashMap::new(),
                next_chunk: 0,
                file_keys: HashMap::new(),
            }));
            Ok(clients.len() as jlong - 1)
        })
        .resolve::<jni::errors::ThrowRuntimeExAndDefault>()
}

/// What happened, as an input's JSON, and the time in Unix milliseconds. Returns what to do, in
/// order, as a JSON array of outputs: `join_call` holds only `chat` and `call_seq`, and
/// `call_key` and `call_text` never appear, since they go to the call here. A shown file has no
/// `key`, and `put_chunk` and `write_chunk` hold `chunk` in place of their bytes, for
/// `Core.chunk`. Throws if the handle is gone or the input is not an input.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_pouriya_keryx_Core_step<'caller>(
    mut unowned: EnvUnowned<'caller>,
    _class: JClass<'caller>,
    handle: jlong,
    input: JString<'caller>,
    now: jlong,
) -> JString<'caller> {
    unowned
        .with_env(
            |env: &mut Env<'caller>| -> jni::errors::Result<JString<'caller>> {
                let input = input.to_string();
                run(env, handle, now, |entry| input_of(&input, entry))
            },
        )
        .resolve::<jni::errors::ThrowRuntimeExAndDefault>()
}

/// `step()` for the two inputs that carry bytes: `{"type": "chunk_read", "handle", "n"}` or
/// `{"type": "fetched", "id", "status"}`, with the bytes beside them, as android/AGENTS.md chose.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_pouriya_keryx_Core_stepBytes<'caller>(
    mut unowned: EnvUnowned<'caller>,
    _class: JClass<'caller>,
    handle: jlong,
    input: JString<'caller>,
    bytes: JByteArray<'caller>,
    now: jlong,
) -> JString<'caller> {
    unowned
        .with_env(
            |env: &mut Env<'caller>| -> jni::errors::Result<JString<'caller>> {
                let bytes = env.convert_byte_array(&bytes)?;
                let input = serde_json::from_str::<Value>(&input.to_string()).ok();
                run(env, handle, now, move |_entry| {
                    let input = input?;
                    let number = |name: &str| input.get(name).and_then(Value::as_u64);
                    match input.get("type").and_then(Value::as_str)? {
                        "chunk_read" => Some(Input::ChunkRead {
                            handle: number("handle")?,
                            n: number("n")?,
                            bytes,
                        }),
                        "fetched" => Some(Input::Fetched {
                            id: number("id")?,
                            status: u16::try_from(number("status")?).ok()?,
                            bytes,
                        }),
                        _ => None,
                    }
                })
            },
        )
        .resolve::<jni::errors::ThrowRuntimeExAndDefault>()
}

/// The bytes a `put_chunk` or `write_chunk` held back as `chunk`, once: null if already taken.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_pouriya_keryx_Core_chunk<'caller>(
    mut unowned: EnvUnowned<'caller>,
    _class: JClass<'caller>,
    handle: jlong,
    chunk: jlong,
) -> JByteArray<'caller> {
    unowned
        .with_env(
            |env: &mut Env<'caller>| -> jni::errors::Result<JByteArray<'caller>> {
                let bytes = u64::try_from(chunk).ok().and_then(|chunk| {
                    entry(&mut clients(), handle).and_then(|entry| entry.chunks.remove(&chunk))
                });
                match bytes {
                    Some(bytes) => env.byte_array_from_slice(&bytes),
                    None => Ok(JByteArray::default()),
                }
            },
        )
        .resolve::<jni::errors::ThrowRuntimeExAndDefault>()
}

/// A BlurHash drawn as `width` by `height` RGBA pixels, to show a file's row while it is not yet
/// opened: null if it is not a BlurHash.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_pouriya_keryx_Core_blurhash<'caller>(
    mut unowned: EnvUnowned<'caller>,
    _class: JClass<'caller>,
    hash: JString<'caller>,
    width: jint,
    height: jint,
) -> JByteArray<'caller> {
    unowned
        .with_env(
            |env: &mut Env<'caller>| -> jni::errors::Result<JByteArray<'caller>> {
                let (Ok(width), Ok(height)) = (u32::try_from(width), u32::try_from(height)) else {
                    return Ok(JByteArray::default());
                };
                match file::blurhash_pixels(&hash.to_string(), width, height) {
                    Ok(pixels) => env.byte_array_from_slice(&pixels),
                    Err(_) => Ok(JByteArray::default()),
                }
            },
        )
        .resolve::<jni::errors::ThrowRuntimeExAndDefault>()
}

/// An input's JSON, with the keys of the files a `download` or `edit` names put back. Only those
/// two go through a JSON value, so a snapshot's long list of numbers is parsed once.
fn input_of(text: &str, entry: &Entry) -> Option<Input> {
    if !text.contains("\"download\"") && !text.contains("\"edit\"") {
        return serde_json::from_str(text).ok();
    }
    let mut input = serde_json::from_str::<Value>(text).ok()?;
    if let Some(command) = input.get_mut("command") {
        let files = match command.get("type").and_then(Value::as_str) {
            Some("download") => command.get_mut("file").into_iter().collect(),
            Some("edit") => command
                .get_mut("files")
                .and_then(Value::as_array_mut)
                .map(|files| files.iter_mut().collect())
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        for file in files {
            let Some(file) = file.as_object_mut() else {
                continue;
            };
            let key = file
                .get("id")
                .and_then(Value::as_str)
                .and_then(|id| entry.file_keys.get(id));
            if let Some(key) = key {
                file.insert("key".to_string(), Value::String(key.to_string()));
            }
        }
    }
    serde_json::from_value(input).ok()
}

/// Takes each shown file's key out of an output, keeping it by the file's id.
fn keep_keys(value: &mut Value, file_keys: &mut HashMap<String, Zeroizing<String>>) {
    let Some(files) = value
        .pointer_mut("/event/message/files")
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    for file in files.iter_mut().filter_map(Value::as_object_mut) {
        let key = file.remove("key");
        if let (Some(Value::String(key)), Some(id)) = (key, file.get("id").and_then(Value::as_str))
        {
            file_keys.insert(id.to_string(), Zeroizing::new(key));
        }
    }
}

/// Steps the client with what `input` makes of its entry, and writes the outputs as JSON.
fn run<'caller>(
    env: &mut Env<'caller>,
    handle: jlong,
    now: jlong,
    input: impl FnOnce(&Entry) -> Option<Input>,
) -> jni::errors::Result<JString<'caller>> {
    let mut clients = clients();
    let Some(entry) = entry(&mut clients, handle) else {
        env.throw_new(
            jni_str!("java/lang/IllegalStateException"),
            jni_str!("keryx: no client has that handle"),
        )?;
        return Ok(JString::default());
    };
    let Some(input) = input(entry) else {
        env.throw_new(
            jni_str!("java/lang/IllegalArgumentException"),
            jni_str!("keryx: that is not an input"),
        )?;
        return Ok(JString::default());
    };
    let mut shown = Vec::new();
    let mut left = Vec::new();
    for output in entry.client.step(input, now) {
        let value = match output {
            Output::JoinCall {
                chat,
                call_seq,
                path,
                bearer,
                key,
                index_list,
            } => {
                entry.join = Some(Join {
                    chat,
                    call_seq,
                    path,
                    bearer,
                    key,
                    index_list,
                });
                entry.wanted = Some((chat, call_seq));
                Ok(serde_json::json!({
                    "type": "join_call",
                    "chat": chat,
                    "call_seq": call_seq,
                }))
            }
            Output::CallKey {
                key, index_list, ..
            } => {
                if let Some(call) = &entry.call {
                    // An ended call needs no key, and its ending comes from next().
                    let _ = call.rekey(key, index_list);
                } else if let Some(join) = &mut entry.join {
                    join.key = key;
                    join.index_list = index_list;
                }
                continue;
            }
            Output::CallText { frame, .. } => {
                if let Some(call) = &entry.call {
                    let _ = call.text(&frame);
                }
                continue;
            }
            Output::LeaveCall { chat } => {
                entry.join = None;
                entry.wanted = None;
                left.extend(entry.call.take());
                serde_json::to_value(Output::LeaveCall { chat })
            }
            Output::PutChunk {
                id,
                path,
                bearer,
                bytes,
            } => {
                let chunk = entry.park(bytes);
                Ok(serde_json::json!({
                    "type": "put_chunk",
                    "id": id,
                    "path": path,
                    "bearer": bearer,
                    "chunk": chunk,
                }))
            }
            Output::WriteChunk {
                handle,
                n,
                offset,
                bytes,
            } => {
                let chunk = entry.park(bytes);
                Ok(serde_json::json!({
                    "type": "write_chunk",
                    "handle": handle,
                    "n": n,
                    "offset": offset,
                    "chunk": chunk,
                }))
            }
            other => serde_json::to_value(other).map(|mut value| {
                keep_keys(&mut value, &mut entry.file_keys);
                value
            }),
        };
        let Ok(value) = value else {
            env.throw_new(
                jni_str!("java/lang/IllegalStateException"),
                jni_str!("keryx: the outputs could not be written"),
            )?;
            return Ok(JString::default());
        };
        shown.push(value);
    }
    drop(clients);
    // A clean close may wait for the server, so it never holds up the engine.
    for call in left {
        std::thread::spawn(move || drop(call));
    }
    let Ok(text) = serde_json::to_string(&shown) else {
        env.throw_new(
            jni_str!("java/lang/IllegalStateException"),
            jni_str!("keryx: the outputs could not be written"),
        )?;
        return Ok(JString::default());
    };
    JString::from_str(env, text)
}

/// Forgets a client, and leaves its call. Its handle is never handed out again.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_pouriya_keryx_Core_drop<'caller>(
    mut unowned: EnvUnowned<'caller>,
    _class: JClass<'caller>,
    handle: jlong,
) {
    unowned
        .with_env(|_env: &mut Env| -> jni::errors::Result<()> {
            let gone = usize::try_from(handle)
                .ok()
                .and_then(|index| clients().get_mut(index).and_then(Option::take));
            if let Some(call) = gone.and_then(|entry| entry.call) {
                std::thread::spawn(move || drop(call));
            }
            Ok(())
        })
        .resolve::<jni::errors::ThrowRuntimeExAndDefault>()
}

/// Connects the call the last `join_call` named, at `server` such as `http://127.0.0.1:7777`,
/// blocking for up to 10 seconds. Call it on the call's own thread. False when there is nothing
/// to join, the connection failed, or the call was left meanwhile; a refusal after the upgrade
/// comes from `callNext` as closed.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_pouriya_keryx_Core_callConnect<'caller>(
    mut unowned: EnvUnowned<'caller>,
    _class: JClass<'caller>,
    handle: jlong,
    server: JString<'caller>,
) -> jboolean {
    unowned
        .with_env(|_env: &mut Env| -> jni::errors::Result<jboolean> {
            let Some(join) = entry(&mut clients(), handle).and_then(|entry| entry.join.take())
            else {
                return Ok(false);
            };
            let server = server.to_string();
            let server = server.trim().trim_end_matches('/');
            let base = match server.split_once("://") {
                Some(("http", rest)) => format!("ws://{rest}"),
                Some(("https", rest)) => format!("wss://{rest}"),
                _ => server.to_string(),
            };
            let Ok(call) = Call::connect(
                &format!("{base}{}", join.path),
                join.bearer.as_ref().map(|bearer| bearer.0.as_str()),
                join.key,
                join.index_list,
            ) else {
                return Ok(false);
            };
            let mut clients = clients();
            match entry(&mut clients, handle) {
                Some(entry) if entry.wanted == Some((join.chat, join.call_seq)) => {
                    entry.call = Some(Arc::new(call));
                    Ok(true)
                }
                _ => {
                    drop(clients);
                    drop(call);
                    Ok(false)
                }
            }
        })
        .resolve::<jni::errors::ThrowRuntimeExAndDefault>()
}

/// One encoded Opus packet, with its capture time in microseconds. Never blocks. Does nothing
/// without a joined call.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_pouriya_keryx_Core_callAudio<'caller>(
    mut unowned: EnvUnowned<'caller>,
    _class: JClass<'caller>,
    handle: jlong,
    ts: jlong,
    bytes: JByteArray<'caller>,
) {
    unowned
        .with_env(|env: &mut Env| -> jni::errors::Result<()> {
            let Some(call) = call_of(handle) else {
                return Ok(());
            };
            let bytes = Zeroizing::new(env.convert_byte_array(&bytes)?);
            // An ended call's ending comes from next().
            let _ = call.audio(ts as u64, &bytes);
            Ok(())
        })
        .resolve::<jni::errors::ThrowRuntimeExAndDefault>()
}

/// One encoded H.264 access unit in Annex B, with its capture time in microseconds. A keyframe
/// carries its SPS and PPS in front, and starts a new group. Never blocks.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_pouriya_keryx_Core_callVideo<'caller>(
    mut unowned: EnvUnowned<'caller>,
    _class: JClass<'caller>,
    handle: jlong,
    ts: jlong,
    bytes: JByteArray<'caller>,
    keyframe: jboolean,
) {
    unowned
        .with_env(|env: &mut Env| -> jni::errors::Result<()> {
            let Some(call) = call_of(handle) else {
                return Ok(());
            };
            let bytes = Zeroizing::new(env.convert_byte_array(&bytes)?);
            let _ = call.video(ts as u64, &bytes, keyframe);
            Ok(())
        })
        .resolve::<jni::errors::ThrowRuntimeExAndDefault>()
}

/// What this device has on, as the `state` track's JSON. Throws if it is not that JSON.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_pouriya_keryx_Core_callState<'caller>(
    mut unowned: EnvUnowned<'caller>,
    _class: JClass<'caller>,
    handle: jlong,
    state: JString<'caller>,
) {
    unowned
        .with_env(|env: &mut Env| -> jni::errors::Result<()> {
            let Ok(state) = serde_json::from_str::<State>(&state.to_string()) else {
                env.throw_new(
                    jni_str!("java/lang/IllegalArgumentException"),
                    jni_str!("keryx: that is not a call state"),
                )?;
                return Ok(());
            };
            if let Some(call) = call_of(handle) {
                let _ = call.state(&state);
            }
            Ok(())
        })
        .resolve::<jni::errors::ThrowRuntimeExAndDefault>()
}

/// Waits up to `timeout` milliseconds for what arrived in the call, as one byte array, or null
/// when nothing did or there is no call. Byte 0 is the kind: 0 audio and 1 video, then the
/// sender's 16-byte device id, its 8-byte big-endian timestamp and the codec bytes; 2 state,
/// 3 call event and 4 closed, then JSON: `{device, state}`, the call event to feed back into
/// `step()`, and `{code, reason}`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_pouriya_keryx_Core_callNext<'caller>(
    mut unowned: EnvUnowned<'caller>,
    _class: JClass<'caller>,
    handle: jlong,
    timeout: jlong,
) -> JByteArray<'caller> {
    unowned
        .with_env(
            |env: &mut Env<'caller>| -> jni::errors::Result<JByteArray<'caller>> {
                let Some(call) = call_of(handle) else {
                    return Ok(JByteArray::default());
                };
                let wait = Duration::from_millis(u64::try_from(timeout).unwrap_or(0));
                let Some(event) = call.next(wait) else {
                    return Ok(JByteArray::default());
                };
                drop(call);
                let video = matches!(event, Event::Video { .. });
                let mut frame = Zeroizing::new(Vec::new());
                let json = match event {
                    Event::Audio { device, ts, bytes } | Event::Video { device, ts, bytes } => {
                        frame.reserve(25 + bytes.len());
                        frame.push(u8::from(video));
                        frame.extend_from_slice(device.bytes());
                        frame.extend_from_slice(&ts.to_be_bytes());
                        frame.extend_from_slice(&bytes);
                        None
                    }
                    Event::State { device, state } => {
                        frame.push(2);
                        Some(serde_json::to_vec(
                            &serde_json::json!({"device": device, "state": state}),
                        ))
                    }
                    Event::CallEvent(event) => {
                        frame.push(3);
                        Some(serde_json::to_vec(&event))
                    }
                    Event::Closed { code, reason } => {
                        frame.push(4);
                        Some(serde_json::to_vec(
                            &serde_json::json!({"code": code, "reason": reason}),
                        ))
                    }
                };
                if let Some(json) = json {
                    let Ok(json) = json else {
                        env.throw_new(
                            jni_str!("java/lang/IllegalStateException"),
                            jni_str!("keryx: the call's frame could not be written"),
                        )?;
                        return Ok(JByteArray::default());
                    };
                    frame.extend_from_slice(&json);
                }
                env.byte_array_from_slice(&frame)
            },
        )
        .resolve::<jni::errors::ThrowRuntimeExAndDefault>()
}
