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
use axum::body::Body;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, State as AxState};
use axum::extract::{DefaultBodyLimit, Query};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{mpsc, oneshot};

/// A QR link or code works this long, once.
const PAIRING_TTL: Duration = Duration::from_secs(10 * 60);
/// Wrong codes before a new code is made.
const MAX_WRONG: u32 = 5;
/// How long the user has to answer "Allow this device?".
const ASK_TIMEOUT: Duration = Duration::from_secs(120);
/// How often to look for a new network or cable while sharing.
const ADDRESS_CHECK: Duration = Duration::from_secs(3);
/// The largest single file a phone may send (8 GiB).
const MAX_UPLOAD: u64 = 8 << 30;

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
    /// "Use as screen" tapped: start the picture (and an extend display).
    ScreenStart,
    /// Back to the receiver's home page: stop the picture.
    ScreenStop,
    /// Files were sent with /api/send; one notification for the batch.
    Sent {
        count: u32,
    },
}

/// To a receiver. (Our ICE candidates travel inside the offer.)
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
    /// Something the receiver should show (e.g. why there's no picture).
    Notice {
        text: &'a str,
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
    /// The picture is on (the receiver chose "Use as screen").
    screen_on: bool,
    /// Came over the USB cable (adb forwards to this computer's localhost).
    via_usb: bool,
    tx: mpsc::UnboundedSender<String>,
}

struct Running {
    port: u16,
    everywhere: bool,
    /// One server per address; a cable plugged in later adds one.
    listeners: Vec<(IpAddr, oneshot::Sender<()>)>,
    /// Which start this is, so an old address watcher stops itself.
    generation: u64,
}

impl Running {
    fn addresses(&self) -> Vec<IpAddr> {
        self.listeners.iter().map(|(ip, _)| *ip).collect()
    }
}

/// What the media engine (media.rs) hears about receivers.
#[derive(Debug)]
pub enum MediaEvent {
    /// Let in and welcomed: offer it the picture.
    Joined(crate::media::Join),
    /// Gone (closed, removed, or sharing stopped).
    Left { session: u64 },
    /// Its answer or ICE candidate.
    Signal { session: u64, msg: Inbound },
    /// Its screen changed size (full screen, rotated).
    Resized { session: u64, screen: ScreenInfo },
}

struct Inner {
    devices: Devices,
    pairing: Option<Pairing>,
    requests: Vec<Request>,
    sessions: HashMap<u64, Session>,
    next_session: u64,
    running: Option<Running>,
    /// Counts starts (see Running::generation).
    generation: u64,
    /// Where signalling from receivers goes (the media engine), if listening.
    signals: Option<mpsc::UnboundedSender<MediaEvent>>,
}

pub struct Hub {
    pub(crate) app: AppHandle,
    data_dir: std::path::PathBuf,
    inner: Mutex<Inner>,
    /// Android phones on a USB cable (adb.rs).
    usb: crate::adb::Usb,
}

#[derive(Debug, Serialize)]
pub struct SessionView {
    pub session: u64,
    pub device_id: String,
    pub name: String,
    pub address: String,
    pub since: u64,
    pub screen: Option<ScreenInfo>,
    /// Showing the picture (not just connected).
    pub screen_on: bool,
    /// Connected by USB cable.
    pub via_usb: bool,
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
    /// Android phones on a USB cable.
    pub usb: crate::adb::UsbView,
}

impl Hub {
    pub fn new(app: AppHandle, data_dir: &Path) -> Arc<Hub> {
        Arc::new(Hub {
            app,
            data_dir: data_dir.to_path_buf(),
            usb: crate::adb::Usb::default(),
            inner: Mutex::new(Inner {
                devices: Devices::load(data_dir),
                pairing: None,
                requests: Vec::new(),
                sessions: HashMap::new(),
                next_session: 1,
                running: None,
                generation: 0,
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
            Some(r) => (true, r.port, r.addresses()),
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
            usb: self.usb.view(),
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
                    screen_on: s.screen_on,
                    via_usb: s.via_usb,
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
        let mut listeners = Vec::new();
        let mut last_err = String::new();
        for ip in wanted {
            match self.listen_on(ip, port).await {
                Ok(stop) => listeners.push((ip, stop)),
                Err(e) => last_err = e,
            }
        }
        if listeners.is_empty() {
            return Err(if last_err.is_empty() {
                "no network to share on: connect this computer to Wi-Fi or a cable network".into()
            } else {
                format!("cannot open port {port} ({last_err}); another app may be using it, choose another port in Settings")
            });
        }
        let generation = {
            let mut g = self.lock();
            g.generation += 1;
            g.pairing = Some(Pairing::new()?);
            g.running = Some(Running {
                port,
                everywhere,
                listeners,
                generation: g.generation,
            });
            g.generation
        };
        self.changed();
        // A cable plugged in (or unplugged) while sharing: follow it.
        let me = self.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(ADDRESS_CHECK).await;
                if !me.refresh_addresses(generation).await {
                    break;
                }
                me.usb_tick().await;
            }
        });
        Ok(())
    }

    /// Serve on one address; the sender stops it.
    async fn listen_on(
        self: &Arc<Self>,
        ip: IpAddr,
        port: u16,
    ) -> Result<oneshot::Sender<()>, String> {
        let listener = tokio::net::TcpListener::bind(SocketAddr::new(ip, port))
            .await
            .map_err(|e| {
                tracing::warn!("cannot listen on {ip}:{port}: {e}");
                format!("{ip}:{port}: {e}")
            })?;
        let (tx, rx) = oneshot::channel::<()>();
        let router = Router::new()
            .route("/ws", get(ws_upgrade))
            .route(
                "/api/send",
                post(upload)
                    .options(upload_preflight)
                    .layer(DefaultBodyLimit::disable()),
            )
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
        Ok(tx)
    }

    /// Android phones on a USB cable: set them up (blocking adb calls).
    pub fn usb(&self) -> &crate::adb::Usb {
        &self.usb
    }

    /// Find or download adb (the "Set up" button). The status shows progress.
    pub async fn usb_setup(self: &Arc<Self>) -> Result<(), String> {
        let me = self.clone();
        let task = tauri::async_runtime::spawn_blocking(move || {
            me.changed();
            let r = me.usb.setup(&me.data_dir);
            me.changed();
            r
        });
        task.await
            .map_err(|e| format!("cannot set up USB: {e}"))??;
        self.usb_tick().await;
        Ok(())
    }

    pub fn usb_off(&self) {
        self.usb.turn_off();
        self.changed();
    }

    /// One look at the cable while sharing.
    async fn usb_tick(self: &Arc<Self>) {
        if !self.usb.ready() {
            return;
        }
        let (port, link) = {
            let g = self.lock();
            let port = g.running.as_ref().map(|r| r.port);
            let link = g
                .pairing
                .as_ref()
                .filter(|p| !p.expired())
                .map(|p| format!("#t={}", p.token));
            (port, link)
        };
        let before = self.usb.view().phones;
        let me = self.clone();
        let _ = tauri::async_runtime::spawn_blocking(move || me.usb.tick(port, link)).await;
        if self.usb.view().phones != before {
            self.changed();
        }
    }

    /// Add servers for new addresses (a cable was plugged in) and stop those
    /// whose address went away. False once this sharing session is over.
    async fn refresh_addresses(self: &Arc<Self>, generation: u64) -> bool {
        let (port, everywhere, have) = {
            let g = self.lock();
            match &g.running {
                Some(r) if r.generation == generation => (r.port, r.everywhere, r.addresses()),
                _ => return false,
            }
        };
        let Ok(wanted) = listen_addresses(everywhere) else {
            return true;
        };
        let mut added = Vec::new();
        for ip in wanted.iter().filter(|ip| !have.contains(ip)) {
            if let Ok(stop) = self.listen_on(*ip, port).await {
                tracing::info!("now also sharing on {ip} (new network or cable)");
                added.push((*ip, stop));
            }
        }
        let gone: Vec<IpAddr> = have.into_iter().filter(|ip| !wanted.contains(ip)).collect();
        if added.is_empty() && gone.is_empty() {
            return true;
        }
        {
            let mut g = self.lock();
            match g.running.as_mut() {
                Some(r) if r.generation == generation => {
                    r.listeners.extend(added);
                    let (keep, stop): (Vec<_>, Vec<_>) = r
                        .listeners
                        .drain(..)
                        .partition(|(ip, _)| !gone.contains(ip));
                    r.listeners = keep;
                    for (ip, tx) in stop {
                        tracing::info!("stopped sharing on {ip} (network or cable gone)");
                        let _ = tx.send(());
                    }
                }
                // Stopped meanwhile: the new servers go too.
                _ => {
                    for (_, tx) in added {
                        let _ = tx.send(());
                    }
                    return false;
                }
            }
        }
        self.changed();
        true
    }

    /// Stop listening and disconnect every receiver.
    pub fn stop(&self) {
        let gone: Vec<u64> = {
            let mut g = self.lock();
            if let Some(r) = g.running.take() {
                for (_, s) in r.listeners {
                    let _ = s.send(());
                }
            }
            let bye = json(&Outbound::Bye {
                reason: "Sharing was turned off on the computer.",
            });
            let gone = g
                .sessions
                .drain()
                .map(|(id, s)| {
                    let _ = s.tx.send(bye.clone());
                    id
                })
                .collect();
            for r in g.requests.drain(..) {
                let _ = r.answer.send(Decision {
                    allow: false,
                    control: false,
                });
            }
            g.pairing = None;
            gone
        };
        // Phones on the cable: their page connection is gone too.
        self.usb.tick(None, None);
        for session in gone {
            self.media(MediaEvent::Left { session });
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
        let mut gone = Vec::new();
        {
            let mut g = self.lock();
            g.devices.remove(id)?;
            let bye = json(&Outbound::Bye {
                reason: "This device was removed on the computer.",
            });
            g.sessions.retain(|session, s| {
                if s.device_id == id {
                    let _ = s.tx.send(bye.clone());
                    gone.push(*session);
                    false
                } else {
                    true
                }
            });
        }
        for session in gone {
            self.media(MediaEvent::Left { session });
        }
        self.changed();
        Ok(())
    }

    pub fn set_control(&self, id: &str, control: bool) -> Result<(), String> {
        self.lock().devices.set_control(id, control)?;
        self.changed();
        Ok(())
    }

    /// Takes effect the next time the device connects.
    pub fn set_extend(&self, id: &str, extend: bool) -> Result<(), String> {
        self.lock().devices.set_extend(id, extend)?;
        self.changed();
        Ok(())
    }

    pub fn rename_device(&self, id: &str, name: &str) -> Result<(), String> {
        self.lock().devices.rename(id, name)?;
        self.changed();
        Ok(())
    }

    /// The media engine takes the receivers' signalling from here.
    pub fn take_signals(&self) -> mpsc::UnboundedReceiver<MediaEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.lock().signals = Some(tx);
        rx
    }

    /// Tell the media engine (if it's listening).
    fn media(&self, ev: MediaEvent) {
        let sink = self.lock().signals.clone();
        if let Some(sink) = sink {
            let _ = sink.send(ev);
        }
    }

    /// The addresses the server listens on (the picture uses the same).
    pub fn listen_ips(&self) -> Vec<IpAddr> {
        self.lock()
            .running
            .as_ref()
            .map(|r| r.addresses())
            .unwrap_or_default()
    }

    /// Send a signalling message to one receiver. False if it has gone.
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

/// Home/office network ranges, plus link-local 169.254/16: a Thunderbolt or
/// USB-C cable between two computers gives both such an address. Not
/// Tailscale (100.64/10).
pub fn is_private_lan(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    o[0] == 10
        || (o[0] == 172 && (16..=31).contains(&o[1]))
        || (o[0] == 192 && o[1] == 168)
        || (o[0] == 169 && o[1] == 254)
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

// ---------- Files from a phone (DeskLab Link) ----------

#[derive(Deserialize)]
struct SendQuery {
    name: String,
}

/// POST /api/send?name=… with the file as the body, from an allowed device
/// (its id and key in the X-DeskSpan-Device / X-DeskSpan-Key headers).
/// Streamed into Downloads under a free name; never overwrites.
async fn upload(
    AxState(hub): AxState<Arc<Hub>>,
    Query(q): Query<SendQuery>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    if !same_origin(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let get = |n: &str| headers.get(n).and_then(|v| v.to_str().ok()).unwrap_or("");
    let (id, key) = (get("x-deskspan-device"), get("x-deskspan-key"));
    if hub.lock().devices.check(id, key).is_none() {
        return (StatusCode::UNAUTHORIZED, "this device isn't allowed").into_response();
    }
    let Ok(dir) = hub.app.path().download_dir() else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "no Downloads folder").into_response();
    };
    let allow_origin = headers
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .filter(|o| app_origin(o))
        .and_then(|o| HeaderValue::from_str(o).ok());
    match save_upload(&dir, &safe_name(&q.name), body).await {
        Ok(name) => {
            let mut res = (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "application/json")],
                serde_json::json!({ "saved": name }).to_string(),
            )
                .into_response();
            if let Some(o) = allow_origin {
                res.headers_mut()
                    .insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, o);
            }
            res
        }
        Err(e) => {
            tracing::warn!("receiving a file: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, e).into_response()
        }
    }
}

/// Write the body to `dir` under a free version of `name` (through a hidden
/// part file, so a broken transfer leaves no half file). Returns the name.
async fn save_upload(dir: &Path, name: &str, body: Body) -> Result<String, String> {
    use tokio::io::AsyncWriteExt;
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|e| format!("cannot open {}: {e}", dir.display()))?;
    let part = dir.join(format!(".{name}.deskspan-part"));
    let mut file = tokio::fs::File::create(&part)
        .await
        .map_err(|e| format!("cannot save {name}: {e}"))?;
    let mut stream = body.into_data_stream();
    let mut size: u64 = 0;
    let result: Result<(), String> = async {
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| format!("the transfer broke off: {e}"))?;
            size += chunk.len() as u64;
            if size > MAX_UPLOAD {
                return Err("the file is larger than 8 GB".into());
            }
            file.write_all(&chunk)
                .await
                .map_err(|e| format!("cannot save {name}: {e}"))?;
        }
        file.flush()
            .await
            .map_err(|e| format!("cannot save {name}: {e}"))
    }
    .await;
    drop(file);
    if let Err(e) = result {
        let _ = tokio::fs::remove_file(&part).await;
        return Err(e);
    }
    let dest = free_path(dir, name);
    tokio::fs::rename(&part, &dest)
        .await
        .map_err(|e| format!("cannot save {name}: {e}"))?;
    Ok(dest
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| name.to_string()))
}

/// A file name from a phone, made safe: no folders, no control or reserved
/// characters, not hidden, not empty, at most 120 characters.
pub fn safe_name(raw: &str) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = base
        .chars()
        .filter(|c| !c.is_control() && !"<>:\"|?*".contains(*c))
        .take(120)
        .collect();
    let cleaned = cleaned
        .trim()
        .trim_start_matches('.')
        .trim_end_matches(['.', ' ']);
    if cleaned.is_empty() {
        "file from phone".into()
    } else {
        cleaned.to_string()
    }
}

/// `name`, or `name (2)`, `name (3)`… keeping the extension, whichever is free.
fn free_path(dir: &Path, name: &str) -> std::path::PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    (2..10_000)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .unwrap_or(first)
}

// ---------- WebSocket: pairing and signalling ----------

/// Browsers send Origin; it must be this server (no other web page may
/// drive the pairing from a visitor's browser).
fn same_origin(headers: &HeaderMap) -> bool {
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok());
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    match (host, origin) {
        (Some(_), Some(o)) if app_origin(o) => true,
        (Some(h), Some(o)) => o.strip_prefix("http://") == Some(h),
        // Not a browser (tools, tests): nothing to forge.
        (Some(_), None) => true,
        _ => false,
    }
}

/// The DeskSpan Screen app (iOS: tauri://localhost, Android:
/// http(s)://tauri.localhost). Pairing still needs the code and the user's
/// OK, and uploads the device key, so this only lets the app in the door.
fn app_origin(origin: &str) -> bool {
    matches!(
        origin,
        "tauri://localhost" | "http://tauri.localhost" | "https://tauri.localhost"
    )
}

/// CORS answer for the app's upload (its page isn't served by us).
async fn upload_preflight(headers: HeaderMap) -> Response {
    let origin = headers
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .filter(|o| app_origin(o));
    let Some(origin) = origin else {
        return StatusCode::FORBIDDEN.into_response();
    };
    let mut res = StatusCode::NO_CONTENT.into_response();
    let h = res.headers_mut();
    if let Ok(v) = HeaderValue::from_str(origin) {
        h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, v);
    }
    h.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("POST"),
    );
    h.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("x-deskspan-device, x-deskspan-key, content-type"),
    );
    h.insert(
        header::ACCESS_CONTROL_MAX_AGE,
        HeaderValue::from_static("600"),
    );
    res
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
                screen_on: false,
                via_usb: addr.ip().is_loopback(),
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
        match msg {
            Inbound::Screen { w, h, dpr } => {
                if let Some(s) = hub.lock().sessions.get_mut(&session) {
                    s.screen = Some(ScreenInfo { w, h, dpr });
                }
                hub.changed();
                // An extend receiver's display follows its screen.
                hub.media(MediaEvent::Resized {
                    session,
                    screen: ScreenInfo { w, h, dpr },
                });
            }
            Inbound::Hello { .. } => {}
            Inbound::ScreenStart => {
                // Its current screen and extend choice (may have changed).
                let start = {
                    let mut g = hub.lock();
                    let extend = g
                        .devices
                        .list()
                        .into_iter()
                        .find(|d| d.id == device.id)
                        .map(|d| d.extend)
                        .unwrap_or(device.extend);
                    match g.sessions.get_mut(&session) {
                        Some(s) if !s.screen_on => {
                            s.screen_on = true;
                            Some((s.screen, extend, s.via_usb))
                        }
                        _ => None,
                    }
                };
                if let Some((screen, extend, usb)) = start {
                    hub.media(MediaEvent::Joined(crate::media::Join {
                        session,
                        device_id: device.id.clone(),
                        name: device.name.clone(),
                        screen,
                        extend,
                        usb,
                    }));
                    hub.changed();
                }
            }
            Inbound::ScreenStop => {
                let was_on = hub
                    .lock()
                    .sessions
                    .get_mut(&session)
                    .map(|s| std::mem::replace(&mut s.screen_on, false))
                    .unwrap_or(false);
                if was_on {
                    hub.media(MediaEvent::Left { session });
                    hub.changed();
                }
            }
            Inbound::Sent { count } => {
                let what = if count == 1 {
                    "1 file".to_string()
                } else {
                    format!("{count} files")
                };
                crate::notify(
                    &hub.app,
                    &format!("{what} from {}", device.name),
                    "Saved in Downloads.",
                );
                let _ = hub.app.emit("files-received", (&device.name, count));
            }
            msg => hub.media(MediaEvent::Signal { session, msg }),
        }
    }
    hub.lock().sessions.remove(&session);
    hub.media(MediaEvent::Left { session });
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
        for ok in [
            "10.1.2.3",
            "172.16.0.1",
            "172.31.255.1",
            "192.168.1.67",
            "169.254.3.9",
        ] {
            assert!(is_private_lan(ok.parse().unwrap()), "{ok}");
        }
        for no in ["100.78.249.4", "172.32.0.1", "8.8.8.8", "127.0.0.1"] {
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
    fn phone_file_names_are_made_safe() {
        assert_eq!(safe_name("IMG_0001.HEIC"), "IMG_0001.HEIC");
        assert_eq!(safe_name("../../etc/passwd"), "passwd");
        assert_eq!(safe_name("C:\\Windows\\a:b?.txt"), "ab.txt");
        assert_eq!(safe_name(".hidden"), "hidden");
        assert_eq!(safe_name("   "), "file from phone");
        assert_eq!(safe_name(&"x".repeat(300)).len(), 120);
    }

    #[test]
    fn free_names_never_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(free_path(dir.path(), "a.jpg"), dir.path().join("a.jpg"));
        std::fs::write(dir.path().join("a.jpg"), b"x").unwrap();
        assert_eq!(free_path(dir.path(), "a.jpg"), dir.path().join("a (2).jpg"));
        std::fs::write(dir.path().join("notes"), b"x").unwrap();
        assert_eq!(free_path(dir.path(), "notes"), dir.path().join("notes (2)"));
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
        // The DeskSpan Screen app.
        h.insert(
            header::ORIGIN,
            HeaderValue::from_static("tauri://localhost"),
        );
        assert!(same_origin(&h));
        h.insert(
            header::ORIGIN,
            HeaderValue::from_static("http://tauri.localhost"),
        );
        assert!(same_origin(&h));
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
