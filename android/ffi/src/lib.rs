//! The hand-written JNI glue around `keryx-core`'s client, and nothing else: `Core.create`,
//! `Core.step` and `Core.drop` on the Kotlin side. The flow is in `core/src/client.rs`. Inputs and
//! outputs cross as JSON text in core's own serde shapes. The app does the socket, the requests,
//! the locked files and the timer, and feeds back what happened. See android/AGENTS.md.
//!
//! Nothing here logs or puts what crosses into an error, since it holds passwords, words, tokens
//! and messages: a bad input throws with a fixed message.

use std::sync::Mutex;

use jni::objects::{JClass, JString};
use jni::sys::jlong;
use jni::{Env, EnvUnowned, jni_str};
use keryx_core::client::{Auth, Client, Input, Platform};

/// Every client this process made, by the handle Kotlin holds: its index here.
static CLIENTS: Mutex<Vec<Option<Client>>> = Mutex::new(Vec::new());

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
            let mut clients = match CLIENTS.lock() {
                Ok(clients) => clients,
                Err(poisoned) => poisoned.into_inner(),
            };
            clients.push(Some(Client::new(Auth::Token, platform)));
            Ok(clients.len() as jlong - 1)
        })
        .resolve::<jni::errors::ThrowRuntimeExAndDefault>()
}

/// What happened, as an input's JSON, and the time in Unix milliseconds. Returns what to do, in
/// order, as a JSON array of outputs. Throws if the handle is gone or the input is not an input.
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
                let Ok(input) = serde_json::from_str::<Input>(&input.to_string()) else {
                    env.throw_new(
                        jni_str!("java/lang/IllegalArgumentException"),
                        jni_str!("keryx: that is not an input"),
                    )?;
                    return Ok(JString::default());
                };
                let mut clients = match CLIENTS.lock() {
                    Ok(clients) => clients,
                    Err(poisoned) => poisoned.into_inner(),
                };
                let Some(Some(client)) = usize::try_from(handle)
                    .ok()
                    .and_then(|index| clients.get_mut(index))
                else {
                    env.throw_new(
                        jni_str!("java/lang/IllegalStateException"),
                        jni_str!("keryx: no client has that handle"),
                    )?;
                    return Ok(JString::default());
                };
                let outputs = client.step(input, now);
                drop(clients);
                let Ok(text) = serde_json::to_string(&outputs) else {
                    env.throw_new(
                        jni_str!("java/lang/IllegalStateException"),
                        jni_str!("keryx: the outputs could not be written"),
                    )?;
                    return Ok(JString::default());
                };
                JString::from_str(env, text)
            },
        )
        .resolve::<jni::errors::ThrowRuntimeExAndDefault>()
}

/// Forgets a client. Its handle is never handed out again.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_pouriya_keryx_Core_drop<'caller>(
    mut unowned: EnvUnowned<'caller>,
    _class: JClass<'caller>,
    handle: jlong,
) {
    unowned
        .with_env(|_env: &mut Env| -> jni::errors::Result<()> {
            let mut clients = match CLIENTS.lock() {
                Ok(clients) => clients,
                Err(poisoned) => poisoned.into_inner(),
            };
            if let Some(slot) = usize::try_from(handle)
                .ok()
                .and_then(|index| clients.get_mut(index))
            {
                *slot = None;
            }
            Ok(())
        })
        .resolve::<jni::errors::ThrowRuntimeExAndDefault>()
}
