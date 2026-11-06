//! The embedded relay: MoQ over qmux on a WebSocket, admitting each device to its call. See the root
//! AGENTS.md, "Calls".
//!
//! Each device publishes into its own origin, scoped to its device id, and the relay copies its
//! `state`, `audio` and `video` frames into the call's shared origin, which every device in the call
//! reads, checking each frame's declared size against `[limits] call_frame` before reading its bytes.
//! Its `text` is not forwarded: it goes to the chat's task, which saves it and republishes it on the
//! `server` broadcast's `events` track. The relay never opens a frame, and writes every timestamp as
//! zero, so only track names are in the clear.

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, Utf8Bytes, WebSocket};
use futures::{Sink, Stream};
use keryx_core::protocol::CallText;
use keryx_core::ulid::Ulid;
use moq_net::{Timestamp, broadcast, origin, track};
use qmux::ws::tungstenite;
use tokio::sync::{Notify, mpsc, oneshot};

use crate::api::{App, Caller, Need, check, current_trace};
use crate::chat::{Job, Work};

/// The call socket's one subprotocol, chosen by the author: qmux's own form, its newest draft, and
/// moq-lite 05 only.
pub const PROTOCOL: &str = "qmux-02.moq-lite-05";
/// The MoQ version inside [`PROTOCOL`], the only one moq-net may speak.
const VERSION: &str = "moq-lite-05";
/// The longest a closing call socket waits for its close frame to go out.
const CLOSE_WAIT: Duration = Duration::from_secs(10);

/// A track's name, priority and latency window, fixed by the author: events and text first, then
/// audio, state and video, so voice survives a weak link.
#[derive(Clone, Copy)]
pub struct Kind {
    pub name: &'static str,
    pub priority: u8,
    pub latency: Duration,
}

impl Kind {
    pub fn info(self) -> track::Info {
        track::Info::default()
            .with_priority(self.priority)
            .with_latency_max(self.latency)
    }
}

pub const EVENTS: Kind = Kind {
    name: "events",
    priority: 3,
    latency: Duration::from_secs(2),
};
const TEXT: Kind = Kind {
    name: "text",
    priority: 3,
    latency: Duration::from_secs(2),
};
const AUDIO: Kind = Kind {
    name: "audio",
    priority: 2,
    latency: Duration::from_millis(500),
};
const STATE: Kind = Kind {
    name: "state",
    priority: 1,
    latency: Duration::from_secs(2),
};
const VIDEO: Kind = Kind {
    name: "video",
    priority: 0,
    latency: Duration::from_secs(2),
};

/// What the chat's task gives an admitted device: the call's shared origin, and its connection's
/// number, which its later events carry so a replaced connection's are ignored.
pub struct Joined {
    pub shared: origin::Producer,
    pub connection: u64,
}

/// How the chat's task closes a device's call socket: the close code and reason.
pub type Closer = oneshot::Sender<(u16, &'static str)>;

/// Maps axum's WebSocket messages to tungstenite's for qmux, records whether the device sent a close
/// frame, which makes its leave clean, and sends the relay's own close frame when asked. That frame
/// goes straight on the WebSocket, after whatever qmux already wrote, and qmux's own close is never
/// sent: a device's moq-net closes its session locally the moment it reads one, and that close would
/// hide the relay's code and reason.
struct Bridge {
    socket: WebSocket,
    shared: Arc<BridgeShared>,
    flushing: bool,
}

#[derive(Default)]
struct BridgeShared {
    peer_closed: AtomicBool,
    close_with: Mutex<Option<(u16, &'static str)>>,
    /// Woken to send a requested close: qmux's reader always polls the socket.
    reader: Mutex<Option<Waker>>,
    sent: Notify,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn axum_error(error: axum::Error) -> tungstenite::Error {
    tungstenite::Error::Io(std::io::Error::other(error))
}

impl Bridge {
    /// Sends the requested close frame and flushes it. Ready at once when none was asked for.
    fn poll_send_close(
        &mut self,
        context: &mut Context<'_>,
    ) -> Poll<Result<(), tungstenite::Error>> {
        if !self.flushing {
            let Some((code, reason)) = locked(&self.shared.close_with).take() else {
                return Poll::Ready(Ok(()));
            };
            match Pin::new(&mut self.socket).poll_ready(context) {
                Poll::Pending => {
                    *locked(&self.shared.close_with) = Some((code, reason));
                    return Poll::Pending;
                }
                Poll::Ready(Err(error)) => return Poll::Ready(Err(axum_error(error))),
                Poll::Ready(Ok(())) => {}
            }
            let frame = Message::Close(Some(CloseFrame {
                code,
                reason: Utf8Bytes::from(reason),
            }));
            if let Err(error) = Pin::new(&mut self.socket).start_send(frame) {
                return Poll::Ready(Err(axum_error(error)));
            }
            self.flushing = true;
        }
        match Pin::new(&mut self.socket).poll_flush(context) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(flushed) => {
                self.flushing = false;
                self.shared.sent.notify_one();
                match flushed {
                    Ok(()) => Poll::Ready(Ok(())),
                    Err(error) => Poll::Ready(Err(axum_error(error))),
                }
            }
        }
    }
}

impl Stream for Bridge {
    type Item = Result<tungstenite::Message, tungstenite::Error>;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        *locked(&this.shared.reader) = Some(context.waker().clone());
        if let Poll::Ready(Err(error)) = this.poll_send_close(context) {
            return Poll::Ready(Some(Err(error)));
        }
        let message = match Pin::new(&mut this.socket).poll_next(context) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(None) => return Poll::Ready(None),
            Poll::Ready(Some(Err(error))) => return Poll::Ready(Some(Err(axum_error(error)))),
            Poll::Ready(Some(Ok(message))) => message,
        };
        let message = match message {
            Message::Text(text) => tungstenite::Message::Text(text.as_str().into()),
            Message::Binary(bytes) => tungstenite::Message::Binary(bytes),
            Message::Ping(bytes) => tungstenite::Message::Ping(bytes),
            Message::Pong(bytes) => tungstenite::Message::Pong(bytes),
            Message::Close(frame) => {
                this.shared.peer_closed.store(true, Ordering::Relaxed);
                let frame = match frame {
                    Some(frame) => Some(tungstenite::protocol::CloseFrame {
                        code: frame.code.into(),
                        reason: frame.reason.as_str().into(),
                    }),
                    None => None,
                };
                tungstenite::Message::Close(frame)
            }
        };
        Poll::Ready(Some(Ok(message)))
    }
}

impl Sink<tungstenite::Message> for Bridge {
    type Error = tungstenite::Error;

    fn poll_ready(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        match Pin::new(&mut self.socket).poll_ready(context) {
            Poll::Ready(Err(error)) => Poll::Ready(Err(axum_error(error))),
            Poll::Ready(Ok(())) => Poll::Ready(Ok(())),
            Poll::Pending => Poll::Pending,
        }
    }

    fn start_send(mut self: Pin<&mut Self>, item: tungstenite::Message) -> Result<(), Self::Error> {
        let message = match item {
            tungstenite::Message::Text(text) => Message::Text(Utf8Bytes::from(text.as_str())),
            tungstenite::Message::Binary(bytes) => Message::Binary(bytes),
            tungstenite::Message::Ping(bytes) => Message::Ping(bytes),
            tungstenite::Message::Pong(bytes) => Message::Pong(bytes),
            tungstenite::Message::Close(frame) => Message::Close(match frame {
                Some(frame) => Some(CloseFrame {
                    code: frame.code.into(),
                    reason: Utf8Bytes::from(frame.reason.as_str()),
                }),
                None => None,
            }),
            tungstenite::Message::Frame(_) => return Ok(()),
        };
        match Pin::new(&mut self.socket).start_send(message) {
            Ok(()) => Ok(()),
            Err(error) => Err(axum_error(error)),
        }
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        match Pin::new(&mut self.socket).poll_flush(context) {
            Poll::Ready(Err(error)) => Poll::Ready(Err(axum_error(error))),
            Poll::Ready(Ok(())) => Poll::Ready(Ok(())),
            Poll::Pending => Poll::Pending,
        }
    }

    /// A close the relay asked for still goes out first, so the device reads why.
    fn poll_close(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        let this = self.get_mut();
        match this.poll_send_close(context) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
            Poll::Ready(Ok(())) => {}
        }
        let polled = match Pin::new(&mut this.socket).poll_close(context) {
            Poll::Ready(Err(error)) => Poll::Ready(Err(axum_error(error))),
            Poll::Ready(Ok(())) => Poll::Ready(Ok(())),
            Poll::Pending => Poll::Pending,
        };
        if polled.is_ready() {
            this.shared.sent.notify_one();
        }
        polled
    }
}

/// One device's call socket, from its upgrade to its leave. A refusal closes it with 4403 and the
/// refusal's code before any MoQ, as the chat's socket does.
pub async fn connection(
    app: Arc<App>,
    mut socket: WebSocket,
    caller: Caller,
    spoke_version: bool,
    chat: Ulid,
    call_seq: u64,
) {
    let (close, closing) = oneshot::channel();
    let admitted = if spoke_version {
        match check(&app, &caller, Need::Member(chat)).await {
            Ok(()) => match caller {
                Caller::Session(session) => match session.device() {
                    Ok(device) => {
                        let (reply, answer) = oneshot::channel();
                        let job = Job {
                            trace: current_trace(),
                            session,
                            work: Work::Join {
                                call_seq,
                                close,
                                reply,
                            },
                            reply: None,
                        };
                        app.hub.submit(&app, chat, job).await;
                        match answer.await {
                            Ok(Ok(joined)) => Ok((session, device, joined)),
                            Ok(Err(refusal)) => Err(refusal.code),
                            Err(_) => Err("internal"),
                        }
                    }
                    Err(refusal) => Err(refusal.code),
                },
                _ => Err("no_device"),
            },
            Err(refusal) => Err(refusal.code),
        }
    } else {
        Err("version_mismatch")
    };
    let (session, device, joined) = match admitted {
        Ok(admitted) => admitted,
        Err(code) => {
            tracing::warn!(msg = "Refused a call socket", code = code);
            let frame = Message::Close(Some(CloseFrame {
                code: 4403,
                reason: Utf8Bytes::from(code),
            }));
            if let Err(error) = socket.send(frame).await {
                tracing::debug!(msg = "Could not send a close", error = %error);
            }
            return;
        }
    };

    let limits = &app.settings.limits;
    let shared = Arc::new(BridgeShared::default());
    let keep_alive = qmux::ws::KeepAlive::new(
        Duration::from_secs(limits.ping_after),
        Duration::from_secs(limits.ping_after + limits.ping_timeout),
    );
    let transport = qmux::ws::Upgraded::new(Bridge {
        socket,
        shared: Arc::clone(&shared),
        flushing: false,
    })
    .with_alpn(PROTOCOL)
    .with_keep_alive(keep_alive)
    .accept();
    let path = device.to_string();
    let own = origin::Info::new(moq_net::Origin::random())
        .with_pool(app.call_pool.clone())
        .produce();
    let gone = |clean| Job {
        trace: current_trace(),
        session,
        work: Work::Gone {
            call_seq,
            connection: joined.connection,
            clean,
        },
        reply: None,
    };
    let (Some(scoped), Ok(version)) = (
        own.scope(&[moq_net::Path::new(&path)]),
        VERSION.parse::<moq_net::Version>(),
    ) else {
        tracing::error!(msg = "Could not set up a call socket's origin");
        app.hub.submit(&app, chat, gone(true)).await;
        return;
    };
    let server = moq_net::Server::new()
        .with_publisher(&joined.shared)
        .with_subscriber(scoped)
        .with_versions(version.into());
    let (moq, driver) = match server.accept(transport.clone()).await {
        Ok(accepted) => accepted,
        Err(error) => {
            tracing::debug!(msg = "A call socket's MoQ did not start", error = %error);
            app.hub.submit(&app, chat, gone(false)).await;
            return;
        }
    };
    let driving = tokio::spawn(driver);

    // Rule breaks from the copying tasks, with the refusal's code.
    let (breaking, mut broken) = mpsc::channel::<&'static str>(1);
    let mut announced = own.consume().announced();
    let mut copies = Vec::new();
    let mut copied = None;
    let mut stopping = app.stopping.subscribe();
    tokio::pin!(closing);
    let (close, leave) = loop {
        tokio::select! {
            closed = &mut closing => match closed {
                // The chat's task ended the call or a newer connection replaced this one, and has
                // already written what that means.
                Ok(close) => break (Some(close), None),
                Err(_) => break (None, None),
            },
            code = broken.recv() => {
                let code = code.unwrap_or("internal");
                tracing::warn!(msg = "Closed a call socket that broke a rule", code = code);
                break (Some((4403, code)), Some(true));
            }
            _ = stopping.changed() => break (Some((1001, "shutting_down")), None),
            announce = announced.next() => {
                let Some(announce) = announce else {
                    break (None, Some(false));
                };
                let Some(source) = announce.broadcast else {
                    continue;
                };
                if copied.is_some() {
                    continue;
                }
                let mut target = match joined.shared.create_broadcast(path.as_str(), broadcast::Route::announced()) {
                    Ok(target) => target,
                    Err(error) => {
                        tracing::error!(msg = "Could not publish a device in its call", error = %error);
                        break (Some((4403, "internal")), Some(true));
                    }
                };
                for kind in [STATE, AUDIO, VIDEO] {
                    let output = match target.create_track(kind.name, kind.info()) {
                        Ok(output) => output,
                        Err(error) => {
                            tracing::error!(msg = "Could not publish a device's track", error = %error);
                            continue;
                        }
                    };
                    let input = source.clone();
                    let breaking = breaking.clone();
                    let limit = limits.call_frame as u64;
                    copies.push(tokio::spawn(async move {
                        let Ok(track) = input.track(kind.name) else {
                            return;
                        };
                        let subscription = track::Subscription::default()
                            .with_priority(kind.priority)
                            .with_latency_max(kind.latency);
                        let Ok(mut subscriber) = track.subscribe(subscription).await else {
                            return;
                        };
                        let mut output = output;
                        while let Ok(Some(mut group)) = subscriber.recv_group().await {
                            let Ok(mut copy) = output.create_group(moq_net::group::Info {
                                sequence: group.sequence,
                            }) else {
                                continue;
                            };
                            while let Ok(Some(mut frame)) = group.next_frame().await {
                                if frame.size > limit {
                                    let _ = breaking.send("frame_too_large").await;
                                    return;
                                }
                                let Ok(bytes) = frame.read_all().await else {
                                    break;
                                };
                                if copy.write_frame(Timestamp::ZERO, bytes).is_err() {
                                    break;
                                }
                            }
                            let _ = copy.finish();
                        }
                    }));
                }
                let app = Arc::clone(&app);
                let breaking = breaking.clone();
                let limit = limits.body.text;
                copies.push(tokio::spawn(async move {
                    let Ok(track) = source.track(TEXT.name) else {
                        return;
                    };
                    let subscription = track::Subscription::default()
                        .with_priority(TEXT.priority)
                        .with_latency_max(TEXT.latency);
                    let Ok(mut subscriber) = track.subscribe(subscription).await else {
                        return;
                    };
                    while let Ok(Some(mut group)) = subscriber.recv_group().await {
                        while let Ok(Some(mut frame)) = group.next_frame().await {
                            if frame.size > limit {
                                let _ = breaking.send("frame_too_large").await;
                                return;
                            }
                            let Ok(bytes) = frame.read_all().await else {
                                break;
                            };
                            let Ok(text) = serde_json::from_slice::<CallText>(&bytes) else {
                                let _ = breaking.send("bad_frame").await;
                                return;
                            };
                            let job = Job {
                                trace: current_trace(),
                                session,
                                work: Work::Text {
                                    call_seq,
                                    connection: joined.connection,
                                    text,
                                },
                                reply: None,
                            };
                            app.hub.submit(&app, chat, job).await;
                        }
                    }
                }));
                copied = Some(target);
            }
            error = moq.closed() => {
                // A device that sent a close frame or qmux's own close left on purpose. Anything
                // else dropped, and gets the grace.
                let clean = shared.peer_closed.load(Ordering::Relaxed)
                    || matches!(
                        moq_net::web_transport_trait::Session::closed(&transport).await,
                        qmux::Error::ConnectionClosed { .. }
                    );
                tracing::debug!(msg = "A call socket ended", error = %error, clean = clean);
                break (None, Some(clean));
            }
        }
    };
    if let Some((code, reason)) = close {
        *locked(&shared.close_with) = Some((code, reason));
        if let Some(reader) = locked(&shared.reader).take() {
            reader.wake();
        }
        let sent = async {
            tokio::select! {
                () = shared.sent.notified() => {}
                _ = moq.closed() => {}
            }
        };
        if tokio::time::timeout(CLOSE_WAIT, sent).await.is_err() {
            tracing::debug!(msg = "A call socket's close frame did not go out");
        }
    }
    for copy in copies {
        copy.abort();
    }
    driving.abort();
    drop(copied);
    if let Some(clean) = leave {
        app.hub.submit(&app, chat, gone(clean)).await;
    }
}
