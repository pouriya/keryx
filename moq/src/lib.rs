//! keryx-moq: the native client for a keryx call's MoQ. It opens the call's WebSocket itself, with
//! the Bearer header and the `qmux-02.moq-lite-05` subprotocol, hands it to qmux, and speaks
//! moq-lite 05 through moq-net. It seals what it sends and opens what it reads with keryx-core's
//! SFrame, so its callers hand it plain codec frames and get plain frames back, and keys never
//! leave Rust. See the root AGENTS.md, "Calls", and moq/AGENTS.md.
//!
//! Each call runs one current-thread tokio runtime on its own thread. Its callers are synchronous:
//! sends never block, as they queue to that runtime, and what arrives comes from a blocking
//! [`Call::next`]. Nothing here logs: every ending comes back as [`Event::Closed`].

use std::collections::{HashMap, VecDeque};
use std::pin::Pin;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use futures::{Sink, Stream};
use keryx_core::call::{CallError, CallIndex, CallKey, Opener, Sealer, State};
use keryx_core::protocol::{CallEvent, CallText};
use keryx_core::ulid::Ulid;
use moq_net::{Timestamp, broadcast, track};
use tokio::sync::{Notify, mpsc};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::handshake::client::Request;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::http::header::{AUTHORIZATION, SEC_WEBSOCKET_PROTOCOL};
use tokio_tungstenite::tungstenite::{self, Message};
use zeroize::Zeroizing;

/// The call socket's one subprotocol, chosen by the author: qmux's own form, its newest draft,
/// and moq-lite 05 only, so a stock MoQ client can connect too.
pub const PROTOCOL: &str = "qmux-02.moq-lite-05";
/// The MoQ version inside [`PROTOCOL`], the only one moq-net may speak.
const VERSION: &str = "moq-lite-05";
/// The longest [`Call::connect`] waits for the WebSocket upgrade, fixed by the author: the ~10
/// seconds a dead socket takes to be noticed.
const CONNECT_WAIT: Duration = Duration::from_secs(10);
/// Events waiting for [`Call::next`], chosen by the author. Past it new media is dropped, and call
/// events and the close are always kept.
const QUEUE: usize = 256;
/// The close code when the socket ended without a close frame, as the WebSocket standard says.
const NO_CLOSE_FRAME: u16 = 1006;
/// The server's broadcast, whose `events` track everyone in the call reads.
const SERVER: &str = "server";

/// keryx's tracks, with their priority and latency window, fixed by the author: events and text
/// first, then audio, state and video, so voice survives a weak link.
const EVENTS: Kind = Kind {
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

#[derive(Clone, Copy, PartialEq, Eq)]
struct Kind {
    name: &'static str,
    priority: u8,
    latency: Duration,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("The call's address is not a WebSocket request")]
    Url(#[source] tungstenite::Error),
    #[error("The Bearer token cannot go in a header")]
    Bearer,
    #[error("This device's MLS index is not in the call's index list")]
    NotMember,
    #[error("Could not start the call's runtime")]
    Runtime(#[source] std::io::Error),
    #[error("Could not start the call's thread")]
    Thread(#[source] std::io::Error),
    #[error("Could not open the call's WebSocket")]
    Connect(#[source] tungstenite::Error),
    #[error("The server did not upgrade the call's WebSocket within 10 seconds")]
    Timeout,
    #[error("The server did not choose the call's subprotocol")]
    Protocol,
    #[error("Could not seal a frame")]
    Seal(#[source] CallError),
    #[error("Could not write call text")]
    Text(#[source] serde_json::Error),
    #[error("The call has ended")]
    Ended,
}

/// What arrives in a call. Media and state come from another device, named by its broadcast;
/// this device's own never come back.
pub enum Event {
    Audio {
        device: Ulid,
        ts: u64,
        bytes: Zeroizing<Vec<u8>>,
    },
    Video {
        device: Ulid,
        ts: u64,
        bytes: Zeroizing<Vec<u8>>,
    },
    State {
        device: Ulid,
        state: State,
    },
    CallEvent(CallEvent),
    /// The call socket ended: 4403 and the refusal's code when the server refused it, 1000 and
    /// `call_ended` or `replaced`, or 1006 when it ended without a close frame.
    Closed {
        code: u16,
        reason: String,
    },
}

/// Never the media, which is plaintext.
impl std::fmt::Debug for Event {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Audio { device, ts, bytes } => {
                write!(formatter, "Audio({device}, {ts}, {} bytes)", bytes.len())
            }
            Self::Video { device, ts, bytes } => {
                write!(formatter, "Video({device}, {ts}, {} bytes)", bytes.len())
            }
            Self::State { device, state } => write!(formatter, "State({device}, {state:?})"),
            Self::CallEvent(event) => write!(formatter, "{event:?}"),
            Self::Closed { code, reason } => write!(formatter, "Closed({code}, {reason})"),
        }
    }
}

/// What the call's thread is asked to do. Frames are sealed before they get here.
enum Command {
    Audio(Vec<u8>),
    Video(Vec<u8>, bool),
    State(Vec<u8>),
    Text(Vec<u8>),
    Rekey(CallKey, Vec<CallIndex>),
}

/// Opens everyone's frames, with each member device's MLS index. A rekey changes both.
struct Keys {
    opener: Opener,
    index: HashMap<Ulid, u32>,
}

/// Events waiting for [`Call::next`].
#[derive(Default)]
struct Queue {
    events: Mutex<VecDeque<Event>>,
    ready: Condvar,
}

impl Queue {
    fn lock(&self) -> MutexGuard<'_, VecDeque<Event>> {
        match self.events.lock() {
            Ok(events) => events,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// Media is dropped when the queue is full, since a late frame is useless. Call events and
    /// the close are always kept.
    fn push(&self, event: Event) {
        let mut events = self.lock();
        let media = matches!(
            event,
            Event::Audio { .. } | Event::Video { .. } | Event::State { .. }
        );
        if media && events.len() >= QUEUE {
            return;
        }
        events.push_back(event);
        self.ready.notify_one();
    }
}

/// One joined call. Every method takes `&self`, so a receive thread can block in [`Call::next`]
/// while capture threads send. Dropping it leaves the call with a clean close.
pub struct Call {
    sealer: Mutex<Sealer>,
    commands: mpsc::UnboundedSender<Command>,
    queue: Arc<Queue>,
}

impl Call {
    /// Opens the call's WebSocket at `url`, such as `ws://host/api/v1/calls/{chat}/{seq}/moq`, and
    /// returns once the server upgraded it. A refusal after that comes from [`Call::next`] as
    /// `Closed{4403, code}`, as every other ending does. `bearer` is the session's token; `key` is
    /// the chat's current call key, and `index_list` every member device's MLS index.
    pub fn connect(
        url: &str,
        bearer: Option<&str>,
        key: CallKey,
        index_list: Vec<CallIndex>,
    ) -> Result<Self, Error> {
        let mut device = None;
        let mut index = HashMap::new();
        for member in &index_list {
            if member.index == key.index {
                device = Some(member.device);
            }
            index.insert(member.device, member.index);
        }
        let Some(device) = device else {
            return Err(Error::NotMember);
        };
        let mut request = match url.into_client_request() {
            Ok(request) => request,
            Err(source) => return Err(Error::Url(source)),
        };
        request
            .headers_mut()
            .insert(SEC_WEBSOCKET_PROTOCOL, HeaderValue::from_static(PROTOCOL));
        if let Some(bearer) = bearer {
            let Ok(value) = HeaderValue::from_str(&format!("Bearer {bearer}")) else {
                return Err(Error::Bearer);
            };
            request.headers_mut().insert(AUTHORIZATION, value);
        }
        let sealer = match Sealer::new(&key) {
            Ok(sealer) => sealer,
            Err(source) => return Err(Error::Seal(source)),
        };
        let mut opener = Opener::default();
        opener.add(&key);
        let keys = Arc::new(Mutex::new(Keys { opener, index }));
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(source) => return Err(Error::Runtime(source)),
        };
        let (commands, receiver) = mpsc::unbounded_channel();
        let queue = Arc::new(Queue::default());
        let (ready, upgraded) = std::sync::mpsc::channel();
        let thread_queue = queue.clone();
        let spawned = std::thread::Builder::new()
            .name("keryx-call".to_string())
            .spawn(move || {
                runtime.block_on(run(request, device, keys, receiver, thread_queue, ready));
            });
        if let Err(source) = spawned {
            return Err(Error::Thread(source));
        }
        match upgraded.recv() {
            Ok(Ok(())) => Ok(Self {
                sealer: Mutex::new(sealer),
                commands,
                queue,
            }),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(Error::Ended),
        }
    }

    fn sealer(&self) -> MutexGuard<'_, Sealer> {
        match self.sealer.lock() {
            Ok(sealer) => sealer,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn send(&self, command: Command) -> Result<(), Error> {
        match self.commands.send(command) {
            Ok(()) => Ok(()),
            Err(_) => Err(Error::Ended),
        }
    }

    /// One audio frame, its own MoQ group, so a late one is just dropped.
    pub fn audio(&self, ts: u64, bytes: &[u8]) -> Result<(), Error> {
        let sealed = match self.sealer().seal(ts, bytes) {
            Ok(sealed) => sealed,
            Err(source) => return Err(Error::Seal(source)),
        };
        self.send(Command::Audio(sealed))
    }

    /// One video frame. A keyframe starts a new MoQ group.
    pub fn video(&self, ts: u64, bytes: &[u8], keyframe: bool) -> Result<(), Error> {
        let sealed = match self.sealer().seal(ts, bytes) {
            Ok(sealed) => sealed,
            Err(source) => return Err(Error::Seal(source)),
        };
        self.send(Command::Video(sealed, keyframe))
    }

    /// What this device has on, its own MoQ group, so a joiner reads the newest at once.
    pub fn state(&self, state: &State) -> Result<(), Error> {
        let sealed = match self.sealer().seal_state(state) {
            Ok(sealed) => sealed,
            Err(source) => return Err(Error::Seal(source)),
        };
        self.send(Command::State(sealed))
    }

    /// Text core already sealed and signed, its own MoQ group on this device's `text` track. The
    /// server saves it and sends it back to everyone on `events`.
    pub fn text(&self, text: &CallText) -> Result<(), Error> {
        let frame = match serde_json::to_vec(text) {
            Ok(frame) => frame,
            Err(source) => return Err(Error::Text(source)),
        };
        self.send(Command::Text(frame))
    }

    /// The chat moved to a new epoch: seal with this key from now on, and open with it too.
    pub fn rekey(&self, key: CallKey, index_list: Vec<CallIndex>) -> Result<(), Error> {
        let sealer = match Sealer::new(&key) {
            Ok(sealer) => sealer,
            Err(source) => return Err(Error::Seal(source)),
        };
        *self.sealer() = sealer;
        self.send(Command::Rekey(key, index_list))
    }

    /// The next event, or `None` when `timeout` passes first. After `Closed` nothing more comes.
    pub fn next(&self, timeout: Duration) -> Option<Event> {
        let until = Instant::now() + timeout;
        let mut events = self.queue.lock();
        loop {
            if let Some(event) = events.pop_front() {
                return Some(event);
            }
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            events = match self.queue.ready.wait_timeout(events, left) {
                Ok((events, _)) => events,
                Err(poisoned) => poisoned.into_inner().0,
            };
        }
    }
}

/// Records the close frame the server sent, which qmux turns into a bare `Closed`, and tells when
/// this end's close frame went out.
struct Recorder<S> {
    socket: S,
    shared: Arc<Shared>,
}

#[derive(Default)]
struct Shared {
    close: Mutex<Option<(u16, String)>>,
    sent: Notify,
}

impl<S> Stream for Recorder<S>
where
    S: Stream<Item = Result<Message, tungstenite::Error>> + Unpin,
{
    type Item = Result<Message, tungstenite::Error>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let polled = Pin::new(&mut self.socket).poll_next(context);
        if let Poll::Ready(Some(Ok(Message::Close(Some(frame))))) = &polled {
            let mut close = match self.shared.close.lock() {
                Ok(close) => close,
                Err(poisoned) => poisoned.into_inner(),
            };
            *close = Some((u16::from(frame.code), frame.reason.to_string()));
        }
        polled
    }
}

impl<S> Sink<Message> for Recorder<S>
where
    S: Sink<Message, Error = tungstenite::Error> + Unpin,
{
    type Error = tungstenite::Error;

    fn poll_ready(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.socket).poll_ready(context)
    }

    fn start_send(mut self: Pin<&mut Self>, item: Message) -> Result<(), Self::Error> {
        Pin::new(&mut self.socket).start_send(item)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.socket).poll_flush(context)
    }

    fn poll_close(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        let polled = Pin::new(&mut self.socket).poll_close(context);
        if polled.is_ready() {
            self.shared.sent.notify_one();
        }
        polled
    }
}

/// The call's thread: upgrade, say so, then publish and read until the socket ends or the
/// [`Call`] is dropped.
async fn run(
    request: Request,
    device: Ulid,
    keys: Arc<Mutex<Keys>>,
    mut commands: mpsc::UnboundedReceiver<Command>,
    queue: Arc<Queue>,
    ready: std::sync::mpsc::Sender<Result<(), Error>>,
) {
    let socket =
        match tokio::time::timeout(CONNECT_WAIT, tokio_tungstenite::connect_async(request)).await {
            Err(_) => {
                let _ = ready.send(Err(Error::Timeout));
                return;
            }
            Ok(Err(source)) => {
                let _ = ready.send(Err(Error::Connect(source)));
                return;
            }
            Ok(Ok((socket, response))) => {
                if response.headers().get(SEC_WEBSOCKET_PROTOCOL)
                    != Some(&HeaderValue::from_static(PROTOCOL))
                {
                    let _ = ready.send(Err(Error::Protocol));
                    return;
                }
                socket
            }
        };
    let _ = ready.send(Ok(()));
    let shared = Arc::new(Shared::default());
    let session = qmux::ws::Upgraded::new(Recorder {
        socket,
        shared: shared.clone(),
    })
    .with_alpn(PROTOCOL)
    .connect();
    let transport = session.clone();

    let publish = moq_net::Origin::random().produce();
    let subscribe = moq_net::Origin::random().produce();
    let mut broadcast =
        match publish.create_broadcast(device.to_string(), broadcast::Route::announced()) {
            Ok(broadcast) => broadcast,
            Err(error) => return closed(&queue, &shared, Some(error.to_string())),
        };
    let mut tracks = HashMap::new();
    for kind in [STATE, AUDIO, VIDEO, TEXT] {
        let info = track::Info::default()
            .with_priority(kind.priority)
            .with_latency_max(kind.latency);
        match broadcast.create_track(kind.name, info) {
            Ok(track) => tracks.insert(kind.name, track),
            Err(error) => return closed(&queue, &shared, Some(error.to_string())),
        };
    }
    let versions = match VERSION.parse::<moq_net::Version>() {
        Ok(version) => version,
        Err(reason) => return closed(&queue, &shared, Some(reason)),
    };
    let client = moq_net::Client::new()
        .with_publisher(&publish)
        .with_subscriber(subscribe.clone())
        .with_versions(versions.into());
    let (moq, driver) = match client.connect(session).await {
        Ok(connected) => connected,
        Err(error) => return closed(&queue, &shared, Some(error.to_string())),
    };
    tokio::spawn(driver);

    let mut announced = subscribe.consume().announced();
    let mut video: Option<moq_net::group::Producer> = None;
    loop {
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else {
                    // The Call was dropped: a clean close is a leave at once.
                    moq.abort(moq_net::Error::Cancel);
                    let _ = tokio::time::timeout(CONNECT_WAIT, shared.sent.notified()).await;
                    return;
                };
                // A write fails only once the track or session has ended, which the session's own
                // close reports below.
                let _ = match command {
                    Command::Audio(sealed) => match tracks.get_mut(AUDIO.name) {
                        Some(track) => track.write_frame(Timestamp::ZERO, sealed),
                        None => Ok(()),
                    },
                    Command::State(sealed) => match tracks.get_mut(STATE.name) {
                        Some(track) => track.write_frame(Timestamp::ZERO, sealed),
                        None => Ok(()),
                    },
                    Command::Text(frame) => match tracks.get_mut(TEXT.name) {
                        Some(track) => track.write_frame(Timestamp::ZERO, frame),
                        None => Ok(()),
                    },
                    Command::Video(sealed, keyframe) => {
                        if (keyframe || video.is_none())
                            && let Some(track) = tracks.get_mut(VIDEO.name)
                        {
                            if let Some(mut group) = video.take() {
                                let _ = group.finish();
                            }
                            match track.append_group() {
                                Ok(group) => video = Some(group),
                                Err(error) => return closed(&queue, &shared, Some(error.to_string())),
                            }
                        }
                        match video.as_mut() {
                            Some(group) => group.write_frame(Timestamp::ZERO, sealed),
                            None => Ok(()),
                        }
                    }
                    Command::Rekey(key, index_list) => {
                        let mut keys = match keys.lock() {
                            Ok(keys) => keys,
                            Err(poisoned) => poisoned.into_inner(),
                        };
                        keys.opener.add(&key);
                        keys.index.clear();
                        for member in &index_list {
                            keys.index.insert(member.device, member.index);
                        }
                        Ok(())
                    }
                };
            }
            announce = announced.next() => {
                let Some(announce) = announce else {
                    return ended(&queue, &shared, &transport).await;
                };
                let Some(consumer) = announce.broadcast else {
                    continue;
                };
                if announce.path.as_str() == SERVER {
                    tokio::spawn(read(consumer, EVENTS, device, keys.clone(), queue.clone()));
                    continue;
                }
                let Ok(sender) = Ulid::parse(announce.path.as_str()) else {
                    continue;
                };
                if sender == device {
                    continue;
                }
                for kind in [STATE, AUDIO, VIDEO] {
                    tokio::spawn(read(consumer.clone(), kind, sender, keys.clone(), queue.clone()));
                }
            }
            _ = moq.closed() => {
                return ended(&queue, &shared, &transport).await;
            }
        }
    }
}

/// The socket ended: says how, from the close frame the server sent if there was one, else from
/// qmux's application close, which the server sends with the same code and reason just before it.
fn closed(queue: &Queue, shared: &Shared, fallback: Option<String>) {
    let recorded = match shared.close.lock() {
        Ok(mut close) => close.take(),
        Err(poisoned) => poisoned.into_inner().take(),
    };
    let (code, reason) = match recorded {
        Some(close) => close,
        None => (NO_CLOSE_FRAME, fallback.unwrap_or_default()),
    };
    queue.push(Event::Closed { code, reason });
}

/// The session ended: records qmux's application close as the close, unless a close frame came.
async fn ended(queue: &Queue, shared: &Shared, transport: &qmux::Session) {
    let error = moq_net::web_transport_trait::Session::closed(transport).await;
    if let qmux::Error::ConnectionClosed { code, reason } = error
        && let Ok(code) = u16::try_from(code.into_inner())
    {
        let mut close = match shared.close.lock() {
            Ok(close) => close,
            Err(poisoned) => poisoned.into_inner(),
        };
        if close.is_none() {
            *close = Some((code, reason));
        }
    }
    closed(queue, shared, None);
}

/// Reads one track of one broadcast into the queue until it ends. Media and state read the newest
/// group, each group to its end, and skip older ones that arrive late. Events read every group in
/// the order it arrived, since none may be lost. A frame that won't open is dropped silently,
/// chosen by the author: a rekey in flight makes some for a moment.
async fn read(
    consumer: broadcast::Consumer,
    kind: Kind,
    device: Ulid,
    keys: Arc<Mutex<Keys>>,
    queue: Arc<Queue>,
) {
    let Ok(track) = consumer.track(kind.name) else {
        return;
    };
    let subscription = track::Subscription::default()
        .with_priority(kind.priority)
        .with_latency_max(kind.latency);
    let Ok(mut subscriber) = track.subscribe(subscription).await else {
        return;
    };
    let mut current: Option<moq_net::group::Consumer> = None;
    loop {
        let frame = match current.as_mut() {
            None => {
                let next = if kind == EVENTS {
                    subscriber.recv_group().await
                } else {
                    subscriber.next_group().await
                };
                match next {
                    Ok(Some(group)) => {
                        current = Some(group);
                        continue;
                    }
                    _ => return,
                }
            }
            Some(group) => match group.read_frame().await {
                Ok(Some(frame)) => frame,
                _ => {
                    current = None;
                    continue;
                }
            },
        };
        if kind == EVENTS {
            if let Ok(event) = serde_json::from_slice::<CallEvent>(&frame.payload) {
                queue.push(Event::CallEvent(event));
            }
            continue;
        }
        let mut keys = match keys.lock() {
            Ok(keys) => keys,
            Err(poisoned) => poisoned.into_inner(),
        };
        let Some(index) = keys.index.get(&device).copied() else {
            continue;
        };
        if kind == STATE {
            if let Ok(state) = keys.opener.open_state(index, &frame.payload) {
                queue.push(Event::State { device, state });
            }
            continue;
        }
        let Ok((ts, bytes)) = keys.opener.open(index, &frame.payload) else {
            continue;
        };
        drop(keys);
        if kind == AUDIO {
            queue.push(Event::Audio { device, ts, bytes });
        } else {
            queue.push(Event::Video { device, ts, bytes });
        }
    }
}
