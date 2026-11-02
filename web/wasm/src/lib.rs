//! The wasm-bindgen glue around `keryx-core`'s client, and nothing else: one class, [`Client`], with
//! one `step`. The flow is in `core/src/client.rs`. The worker does the socket, the requests,
//! IndexedDB and the timer, and feeds back what happened. See web/AGENTS.md.

use keryx_core::client::{Auth, Input, Output, Platform};
use tsify::{Ts, Tsify};
use wasm_bindgen::prelude::*;

/// One device's client.
#[wasm_bindgen]
pub struct Client(keryx_core::client::Client);

#[wasm_bindgen]
impl Client {
    /// How it logs in, cookie or token, is set here for good.
    #[wasm_bindgen(constructor)]
    pub fn new(auth: Ts<Auth>, platform: Ts<Platform>) -> Result<Client, JsError> {
        Ok(Self(keryx_core::client::Client::new(
            auth.to_rust()?,
            platform.to_rust()?,
        )))
    }

    /// What happened, and the time in Unix milliseconds. Returns what to do, in order.
    pub fn step(&mut self, input: Ts<Input>, now: f64) -> Result<Vec<Ts<Output>>, JsError> {
        let mut list = Vec::new();
        for output in self.0.step(input.to_rust()?, now as i64) {
            list.push(output.into_ts()?);
        }
        Ok(list)
    }
}
