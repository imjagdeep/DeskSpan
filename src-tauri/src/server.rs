//! The local server receivers talk to: the receiver page, pairing and
//! signalling over a WebSocket.
//!
//! Listens only while sharing is on, and by default only on home/office
//! network addresses (10/8, 172.16/12, 192.168/16) and this computer; never on
//! Tailscale's 100.64/10 or a public address unless "listen everywhere" is on.
//!
//! A new device needs the one-time link from the QR code or the six-digit
//! code, and then the user's OK on this computer. It then gets an id + key
//! (devices.rs) and reconnects with those.
//!
//! Messages are JSON text, `{"t": "...", ...}`; see receiver/protocol.ts.

use crate::devices::{self, Devices};
use crate::secret;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, State as AxState};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};
use tokio::sync::{mpsc, oneshot};

/// A QR link or code works this long, once.
const PAIRING_TTL: Duration = Duration::from_secs(10 * 60);
/// Wrong codes before a new code is made.
const MAX_WRONG: u32 = 5;
/// How long the user has to answer "Allow this device?".
const ASK_TIMEOUT: Duration = Duration::from_secs(120);
/// A receiver must say hello this soon after connecting.
const HELLO_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ScreenInfo {
    /// CSS pixels
    pub w: u32,
    pub h: u32,
    /// device pixels per CSS pixel
    pub dpr: f64,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Auth {
    Token { token: String },
    Code { code: String },
    Device { id: String, key: String },
}

/// From a receiver.
// Answer/Ice are read by the media engine (next step of milestone 1).
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(tag = "t", rename_all = "kebab-case")]
pub enum Inbound {
    Hello {
        name: String,
        screen: Option<ScreenInfo>,
        auth: Auth,
    },
    Answer {
        sdp: String,
    },
    Ice {
        c: serde_json::Value,
    },
    Screen {
        w: u32,
        h: u32,
        dpr: f64,
    },
}

/// To a receiver.
// Offer/Ice are sent by the media engine (next step of milestone 1).
#[allow(dead_code)]
#[derive(Debug, Serialize)]
#[serde(tag = "t", rename_all = "kebab-case")]
pub enum Outbound<'a> {
    /// The link or saved key didn't work: ask for the code.
    NeedCode,
    WrongCode {
        left: u32,
    },
    /// Waiting for the user's OK on the computer.
    Wait,
    Denied,
    Welcome {
        id: &'a str,
        /// Only for a newly allowed device: keep it to reconnect.
        #[serde(skip_serializing_if = "Option::is_none")]
        key: Option<&'a str>,
        host: &'a str,
        control: bool,
    },
    Offer {
        sdp: &'a str,
    },
    Ice {
        c: &'a serde_json::Value,
    },
    Bye {
        reason: &'a str,
    },
}

struct Pairing {
    token: String,
    code: String,
    made: Instant,
    wrong: u32,
}

impl Pairing {
    fn new() -> Result<Pairing, String> {
        Ok(Pairing {
            token: secret::random_hex(16)?,
            code: secret::six_digits()?,
            made: Instant::now(),
            wrong: 0,
        })
    }
    fn expired(&self) -> bool {
        self.made.elapsed() > PAIRING_TTL
    }
}

struct Request {
    id: String,
    name: String,
    address: String,
    answer: oneshot::Sender<Decision>,
}

#[derive(Debug, Clone, Copy)]
pub struct Decision {
    pub allow: bool,
    pub control: bool,
}

struct Session {
    device_id: String,
    name: String,
    address: String,
    since: u64,
    screen: Option<ScreenInfo>,
    tx: mpsc::UnboundedSender<String>,
}

struct Running {
    port: u16,
    addresses: Vec<IpAddr>,
    stop: Vec<oneshot::Sender<()>>,
}

struct Inner {
    devices: Devices,
    pairing: Option<Pairing>,
    requests: Vec<Request>,
    sessions: HashMap<u64, Session>,
    next_session: u64,
    running: Option<Running>,
    /// Where signalling from receivers goes (the media engine), if listening.
    signals: Option<mpsc::UnboundedSender<(u64, Inbound)>>,
}

pub struct Hub {
    app: AppHandle,
    inner: Mutex<Inner>,
}

#[derive(Debug, Serialize)]
pub struct SessionView {
    pub session: u64,
    pub device_id: String,
    pub name: String,
    pub address: String,
    pub since: u64,
    pub screen: Option<ScreenInfo>,
}

#[derive(Debug, Serialize)]
pub struct RequestView {
    pub id: String,
    pub name: String,
    pub address: String,
}

#[derive(Debug, Serialize)]
pub struct Status {
    pub running: bool,
    pub port: u16,
    /// One link per address the server listens on.
    pub urls: Vec<String>,
    /// The first link with the one-time token (what the QR code holds).
    pub pair_url: Option<String>,
    pub code: Option<String>,
    /// SVG of the QR code for `pair_url`.
    pub qr_svg: Option<String>,
    /// Seconds until the link and code stop working.
    pub expires_in: u64,
    pub sessions: Vec<SessionView>,
    pub requests: Vec<RequestView>,
}

impl Hub {
    pub fn new(app: AppHandle, data_dir: &Path) -> Arc<Hub> {
        Arc::new(Hub {
            app,
            inner: Mutex::new(Inner {
                devices: Devices::load(data_dir),
                pairing: None,
                requests: Vec::new(),
                sessions: HashMap::new(),
                next_session: 1,
                running: None,
                signals: None,
            }),
        })
    }

    /// A panic elsewhere must not lock everyone out: take the data anyway.
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Tell the window something changed; it asks for the status again.
    fn changed(&self) {
        if let Err(e) = self.app.emit("hub-changed", ()) {
            tracing::warn!("cannot update the window: {e}");
        }
    }

    pub fn status(&self) -> Status {
        let mut g = self.lock();
        let (running, port, addresses) = match &g.running {
            Some(r) => (true, r.port, r.addresses.clone()),
            None => (false, 0, Vec::new()),
        };
        // An old link and code are replaced as soon as anyone looks.
        if running && g.pairing.as_ref().is_none_or(Pairing::expired) {
            match Pairing::new() {
                Ok(p) => g.pairing = Some(p),
                Err(e) => tracing::warn!("{e}"),
            }
        }
        let urls: Vec<String> = addresses.iter().map(|a| url_for(*a, port)).collect();
        let pairing = g.pairing.as_ref().filter(|_| running);
        let pair_url = match (urls.first(), pairing) {
            (Some(u), Some(p)) => Some(format!("{u}#t={}", p.token)),
            _ => None,
        };
        let qr_svg = pair_url.as_deref().and_then(qr_svg);
        Status {
            running,
            port,
            urls,
            code: pairing.map(|p| p.code.clone()),
            expires_in: pairing
                .map(|p| PAIRING_TTL.saturating_sub(p.made.elapsed()).as_secs())
                .unwrap_or(0),
            pair_url,
            qr_svg,
            sessions: g
                .sessions
                .iter()
                .map(|(id, s)| SessionView {
                    session: *id,
                    device_id: s.device_id.clone(),
                    name: s.name.clone(),
                    address: s.address.clone(),
                    since: s.since,
                    screen: s.screen,
                })
                .collect(),
            requests: g
                .requests
                .iter()
                .map(|r| RequestView {
                    id: r.id.clone(),
                    name: r.name.clone(),
                    address: r.address.clone(),
                })
                .collect(),
        }
    }

    /// Start listening. Errors if no address could be opened.
    pub async fn start(self: &Arc<Self>, port: u16, everywhere: bool) -> Result<(), String> {
        if self.lock().running.is_some() {
            return Ok(());
        }
        let wanted = listen_addresses(everywhere)?;
        let mut stop = Vec::new();
        let mut opened = Vec::new();
        let mut last_err = String::new();
        for ip in wanted {
            let listener = match tokio::net::TcpListener::bind(SocketAddr::new(ip, port)).await {
                Ok(l) => l,
                Err(e) => {
                    tracing::warn!("cannot listen on {ip}:{port}: {e}");
                    last_err = format!("{ip}:{port}: {e}");
                    continue;
                }
            };
            let (tx, rx) = oneshot::channel::<()>();
            let router = Router::new()
                .route("/ws", get(ws_upgrade))
                .fallback(page)
                .with_state(self.clone());
            tauri::async_runtime::spawn(async move {
                let serve = axum::serve(
                    listener,
                    router.into_make_service_with_connect_info::<SocketAddr>(),
                )
                .with_graceful_shutdown(async {
                    let _ = rx.await;
                });
                if let Err(e) = serve.await {
                    tracing::warn!("server on {ip}:{port} stopped: {e}");
                }
            });
            stop.push(tx);
            opened.push(ip);
        }
        if opened.is_empty() {
            return Err(if last_err.is_empty() {
                "no network to share on: connect this computer to Wi-Fi or a cable network".into()
            } else {
                format!("cannot open port {port} ({last_err}); another app may be using it, choose another port in Settings")
            });
        }
        {
            let mut g = self.lock();
            g.pairing = Some(Pairing::new()?);
            g.running = Some(Running {
                port,
                addresses: opened,
                stop,
            });
        }
        self.changed();
        Ok(())
    }

    /// Stop listening and disconnect every receiver.
    pub fn stop(&self) {
        {
            let mut g = self.lock();
            if let Some(r) = g.running.take() {
                for s in r.stop {
                    let _ = s.send(());
                }
            }
            let bye = json(&Outbound::Bye {
                reason: "Sharing was turned off on the computer.",
            });
            for (_, s) in g.sessions.drain() {
                let _ = s.tx.send(bye.clone());
            }
            for r in g.requests.drain(..) {
                let _ = r.answer.send(Decision {
                    allow: false,
                    control: false,
                });
            }
            g.pairing = None;
        }
        self.changed();
    }

    /// A fresh QR link and code (the old ones stop working).
    pub fn new_pairing(&self) -> Result<(), String> {
        {
            let mut g = self.lock();
            if g.running.is_none() {
                return Err("start sharing first".into());
            }
            g.pairing = Some(Pairing::new()?);
        }
        self.changed();
        Ok(())
    }

    /// The user's answer to "Allow this device?".
    pub fn answer(&self, request: &str, decision: Decision) -> Result<(), String> {
        let r = {
            let mut g = self.lock();
            let i = g
                .requests
                .iter()
                .position(|r| r.id == request)
                .ok_or("that request has gone (the device gave up or disconnected)")?;
            g.requests.remove(i)
        };
        let _ = r.answer.send(decision);
        self.changed();
        Ok(())
    }

    pub fn devices(&self) -> Vec<devices::Device> {
        self.lock().devices.list()
    }

    /// Forget a device; it is disconnected and needs pairing again.
    pub fn remove_device(&self, id: &str) -> Result<(), String> {
        {
            let mut g = self.lock();
            g.devices.remove(id)?;
            let bye = json(&Outbound::Bye {
                reason: "This device was removed on the computer.",
            });
            g.sessions.retain(|_, s| {
                if s.device_id == id {
                    let _ = s.tx.send(bye.clone());
                    false
                } else {
                    true
                }
            });
        }
        self.changed();
        Ok(())
    }

    pub fn set_control(&self, id: &str, control: bool) -> Result<(), String> {
        self.lock().devices.set_control(id, control)?;
        self.changed();
        Ok(())
    }

    pub fn rename_device(&self, id: &str, name: &str) -> Result<(), String> {
        self.lock().devices.rename(id, name)?;
        self.changed();
        Ok(())
    }

    /// The media engine takes the receivers' signalling from here.
    #[allow(dead_code)] // media engine: next step of milestone 1
    pub fn take_signals(&self) -> mpsc::UnboundedReceiver<(u64, Inbound)> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.lock().signals = Some(tx);
        rx
    }

    /// Send a signalling message to one receiver. False if it has gone.
    #[allow(dead_code)] // media engine: next step of milestone 1
    pub fn send(&self, session: u64, msg: &Outbound<'_>) -> bool {
        let g = self.lock();
        match g.sessions.get(&session) {
            Some(s) => s.tx.send(json(msg)).is_ok(),
            None => false,
        }
    }

    /// The pairing check for a new device. Ok = go on and ask the user.
    fn check_pairing(&self, auth: &Auth) -> Result<(), Outbound<'static>> {
        let mut g = self.lock();
        let Some(p) = g.pairing.as_mut().filter(|p| !p.expired()) else {
            return Err(Outbound::NeedCode);
        };
        let ok = match auth {
            Auth::Token { token } => secret::same(token, &p.token),
            Auth::Code { code } => secret::same(code.trim(), &p.code),
            Auth::Device { .. } => false,
        };
        if ok {
            // Used once: the next device needs a new link or code.
            match Pairing::new() {
                Ok(next) => g.pairing = Some(next),
                Err(e) => {
                    tracing::warn!("{e}");
                    g.pairing = None;
                }
            }
            drop(g);
            self.changed();
            return Ok(());
        }
        if let Auth::Token { .. } = auth {
            return Err(Outbound::NeedCode);
        }
        p.wrong += 1;
        let left = MAX_WRONG.saturating_sub(p.wrong);
        if left == 0 {
            match Pairing::new() {
                Ok(next) => g.pairing = Some(next),
                Err(e) => tracing::warn!("{e}"),
            }
            drop(g);
            self.changed();
        }
        Err(Outbound::WrongCode { left })
    }

    /// Show "Allow <name>?" on the computer and wait for the answer.
    async fn ask(&self, name: &str, address: &str) -> Decision {
        let (tx, rx) = oneshot::channel();
        let id = match secret::random_hex(8) {
            Ok(id) => id,
            Err(e) => {
                tracing::warn!("{e}");
                return Decision {
                    allow: false,
                    control: false,
                };
            }
        };
        self.lock().requests.push(Request {
            id: id.clone(),
            name: name.to_string(),
            address: address.to_string(),
            answer: tx,
        });
        self.changed();
        crate::tray::show_main(&self.app);
        crate::notify(
            &self.app,
            "A device wants to connect",
            &format!(
                "{name} ({address}) asks to use this computer's screen. Open DeskSpan to allow it."
            ),
        );
        let decision = match tokio::time::timeout(ASK_TIMEOUT, rx).await {
            Ok(Ok(d)) => d,
            _ => Decision {
                allow: false,
                control: false,
            },
        };
        // Timed out: take it off the list.
        let mut g = self.lock();
        g.requests.retain(|r| r.id != id);
        drop(g);
        self.changed();
        decision
    }
}

fn json(msg: &Outbound<'_>) -> String {
    serde_json::to_string(msg).unwrap_or_else(|_| r#"{"t":"bye","reason":"internal error"}"#.into())
}

fn url_for(ip: IpAddr, port: u16) -> String {
    match ip {
        IpAddr::V4(v4) => format!("http://{v4}:{port}/"),
        IpAddr::V6(v6) => format!("http://[{v6}]:{port}/"),
    }
}

fn qr_svg(text: &str) -> Option<String> {
    match qrcode::QrCode::new(text.as_bytes()) {
        Ok(code) => Some(
            code.render::<qrcode::render::svg::Color<'_>>()
                .min_dimensions(240, 240)
                .quiet_zone(true)
                .build(),
        ),
        Err(e) => {
            tracing::warn!("cannot make the QR code: {e}");
            None
        }
    }
}

/// Home/office network ranges; not Tailscale (100.64/10), not link-local.
pub fn is_private_lan(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    o[0] == 10 || (o[0] == 172 && (16..=31).contains(&o[1])) || (o[0] == 192 && o[1] == 168)
}

/// Where to listen: this computer's LAN addresses (+ loopback, for `adb
/// reverse` and testing), or everything.
fn listen_addresses(everywhere: bool) -> Result<Vec<IpAddr>, String> {
    let ifaces =
        if_addrs::get_if_addrs().map_err(|e| format!("cannot read the network setup: {e}"))?;
    let mut lan: Vec<IpAddr> = ifaces
        .iter()
        .filter_map(|i| match i.ip() {
            IpAddr::V4(v4) if !i.is_loopback() && (everywhere || is_private_lan(v4)) => {
                Some(IpAddr::V4(v4))
            }
            _ => None,
        })
        .collect();
    // 192.168.x first: what most homes use, so it's the QR code's address.
    lan.sort_by_key(|ip| match ip {
        IpAddr::V4(v4) if v4.octets()[0] == 192 => 0,
        IpAddr::V4(v4) if is_private_lan(*v4) => 1,
        _ => 2,
    });
    lan.dedup();
    lan.push(IpAddr::V4(Ipv4Addr::LOCALHOST));
    Ok(lan)
}

// ---------- HTTP: the receiver page ----------

/// Only the receiver's own files are served (dist/receiver in the app).
fn asset_key(path: &str) -> Option<String> {
    let p = path.trim_start_matches('/');
    if p.is_empty() || p == "index.html" {
        return Some("receiver/index.html".into());
    }
    let ok = (p.starts_with("assets/") || p == "favicon.png" || p == "manifest.webmanifest")
        && !p.contains("..")
        && !p.contains('\\')
        && p.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./".contains(&b));
    ok.then(|| format!("receiver/{p}"))
}

async fn page(AxState(hub): AxState<Arc<Hub>>, uri: Uri, headers: HeaderMap) -> Response {
    let Some(key) = asset_key(uri.path()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(asset) = hub.app.asset_resolver().get(key.clone()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let html = asset.mime_type.starts_with("text/html");
    // Tauri falls back to the app's own index.html for unknown files.
    if html != key.ends_with(".html") {
        return StatusCode::NOT_FOUND.into_response();
    }
    let mut res = asset.bytes.into_response();
    let h = res.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&asset.mime_type) {
        h.insert(header::CONTENT_TYPE, v);
    }
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    if html {
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
        // Older Safari doesn't count ws:// as 'self', so name the host too.
        let host = headers
            .get(header::HOST)
            .and_then(|v| v.to_str().ok())
            .filter(|v| {
                v.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b".:[]-".contains(&b))
            })
            .unwrap_or("localhost");
        let csp = format!(
            "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
             img-src 'self' data:; media-src 'self' blob:; connect-src 'self' ws://{host}; \
             frame-ancestors 'none'; base-uri 'none'; form-action 'none'"
        );
        if let Ok(v) = HeaderValue::from_str(&csp) {
            h.insert(header::CONTENT_SECURITY_POLICY, v);
        }
    } else {
        h.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=31536000, immutable"),
        );
    }
    res
}

// ---------- WebSocket: pairing and signalling ----------

/// Browsers send Origin; it must be this server (no other web page may
/// drive the pairing from a visitor's browser).
fn same_origin(headers: &HeaderMap) -> bool {
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok());
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    match (host, origin) {
        (Some(h), Some(o)) => o.strip_prefix("http://") == Some(h),
        // Not a browser (tools, tests): nothing to forge.
        (Some(_), None) => true,
        _ => false,
    }
}

async fn ws_upgrade(
    AxState(hub): AxState<Arc<Hub>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if !same_origin(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    ws.max_message_size(64 * 1024)
        .on_upgrade(move |socket| receiver(hub, socket, addr))
}

async fn receiver(hub: Arc<Hub>, socket: WebSocket, addr: SocketAddr) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let writer = tauri::async_runtime::spawn(async move {
        while let Some(text) = rx.recv().await {
            let bye = text.starts_with(r#"{"t":"bye""#);
            if sink.send(Message::Text(text.into())).await.is_err() || bye {
                break;
            }
        }
        let _ = sink.close().await;
    });
    let address = addr.ip().to_string();
    let say = |m: &Outbound<'_>| {
        let _ = tx.send(json(m));
    };

    // 1. Hello (repeated after a wrong code) until the device is let in.
    let (device, key, screen) = loop {
        let msg = match tokio::time::timeout(HELLO_TIMEOUT, next_message(&mut stream)).await {
            Ok(Some(m)) => m,
            _ => {
                writer.abort();
                return;
            }
        };
        let Inbound::Hello { name, screen, auth } = msg else {
            continue;
        };
        if let Auth::Device { id, key } = &auth {
            let found = hub.lock().devices.check(id, key);
            match found {
                Some(d) => break (d, None, screen),
                None => {
                    say(&Outbound::NeedCode);
                    continue;
                }
            }
        }
        if let Err(reply) = hub.check_pairing(&auth) {
            say(&reply);
            continue;
        }
        say(&Outbound::Wait);
        let decision = hub.ask(&devices::clean_name(&name), &address).await;
        if !decision.allow {
            say(&Outbound::Denied);
            say(&Outbound::Bye {
                reason: "Not allowed on the computer.",
            });
            let _ = writer.await;
            return;
        }
        let added = hub.lock().devices.add(&name, decision.control);
        match added {
            Ok((id, key)) => {
                let d = hub.lock().devices.check(&id, &key);
                match d {
                    Some(d) => break (d, Some(key), screen),
                    None => {
                        writer.abort();
                        return;
                    }
                }
            }
            Err(e) => {
                tracing::warn!("{e}");
                say(&Outbound::Bye {
                    reason: "The computer couldn't save this device.",
                });
                let _ = writer.await;
                return;
            }
        }
    };

    // 2. In: register the session and say welcome.
    let host = gethostname::gethostname().to_string_lossy().into_owned();
    let session = {
        let mut g = hub.lock();
        let id = g.next_session;
        g.next_session += 1;
        g.sessions.insert(
            id,
            Session {
                device_id: device.id.clone(),
                name: device.name.clone(),
                address: address.clone(),
                since: devices::now(),
                screen,
                tx: tx.clone(),
            },
        );
        id
    };
    say(&Outbound::Welcome {
        id: &device.id,
        key: key.as_deref(),
        host: &host,
        control: device.control,
    });
    hub.changed();

    // 3. Signalling until the receiver goes.
    while let Some(msg) = next_message(&mut stream).await {
        if let Inbound::Screen { w, h, dpr } = msg {
            if let Some(s) = hub.lock().sessions.get_mut(&session) {
                s.screen = Some(ScreenInfo { w, h, dpr });
            }
            hub.changed();
        }
        let sink = hub.lock().signals.clone();
        if let Some(sink) = sink {
            if !matches!(msg, Inbound::Hello { .. }) {
                let _ = sink.send((session, msg));
            }
        }
    }
    hub.lock().sessions.remove(&session);
    writer.abort();
    hub.changed();
}

/// The next message we understand; None when the socket closes. Unknown or
/// broken messages are skipped.
async fn next_message(
    stream: &mut futures_util::stream::SplitStream<WebSocket>,
) -> Option<Inbound> {
    while let Some(frame) = stream.next().await {
        match frame {
            Ok(Message::Text(text)) => match serde_json::from_str::<Inbound>(text.as_str()) {
                Ok(m) => return Some(m),
                Err(e) => tracing::debug!("skipped a message: {e}"),
            },
            Ok(Message::Close(_)) | Err(_) => return None,
            Ok(_) => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_lan_ranges_are_private() {
        for ok in ["10.1.2.3", "172.16.0.1", "172.31.255.1", "192.168.1.67"] {
            assert!(is_private_lan(ok.parse().unwrap()), "{ok}");
        }
        for no in [
            "100.78.249.4",
            "172.32.0.1",
            "8.8.8.8",
            "169.254.1.1",
            "127.0.0.1",
        ] {
            assert!(!is_private_lan(no.parse().unwrap()), "{no}");
        }
    }

    #[test]
    fn only_receiver_files_are_served() {
        assert_eq!(asset_key("/").as_deref(), Some("receiver/index.html"));
        assert_eq!(
            asset_key("/assets/index-Ab1.js").as_deref(),
            Some("receiver/assets/index-Ab1.js")
        );
        for bad in [
            "/assets/../index.html",
            "/settings.json",
            "/assets/a%2e%2e",
            "/assets\\x",
        ] {
            assert_eq!(asset_key(bad), None, "{bad}");
        }
    }

    #[test]
    fn origin_must_match_host() {
        let mut h = HeaderMap::new();
        h.insert(header::HOST, HeaderValue::from_static("192.168.1.5:47800"));
        assert!(same_origin(&h));
        h.insert(
            header::ORIGIN,
            HeaderValue::from_static("http://192.168.1.5:47800"),
        );
        assert!(same_origin(&h));
        h.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://evil.example"),
        );
        assert!(!same_origin(&h));
    }

    #[test]
    fn messages_parse() {
        let m: Inbound = serde_json::from_str(
            r#"{"t":"hello","name":"iPad","screen":{"w":1024,"h":768,"dpr":2},"auth":{"kind":"code","code":"123456"}}"#,
        )
        .unwrap();
        assert!(matches!(
            m,
            Inbound::Hello {
                auth: Auth::Code { .. },
                ..
            }
        ));
        let out = json(&Outbound::Welcome {
            id: "a",
            key: None,
            host: "pc",
            control: false,
        });
        assert_eq!(
            out,
            r#"{"t":"welcome","id":"a","host":"pc","control":false}"#
        );
        assert_eq!(
            json(&Outbound::WrongCode { left: 2 }),
            r#"{"t":"wrong-code","left":2}"#
        );
    }
}
