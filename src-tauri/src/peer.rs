//! One receiver's WebRTC connection (str0m, Sans I/O): we offer a send-only
//! H.264 video track and an "input" data channel, the receiver answers, and
//! this task moves UDP packets, timers and video frames through the `Rtc`.
//!
//! ICE-lite with host candidates on our LAN addresses: the browser does the
//! connectivity checks, so its own (often mDNS-hidden) addresses don't need
//! resolving.

use crate::server::{Hub, Inbound, Outbound};
use crate::video::{Frame, Video};
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use str0m::change::SdpAnswer;
use str0m::format::Codec;
use str0m::media::{Direction, Frequency, MediaKind, MediaTime, Mid, Pt};
use str0m::net::{Protocol, Receive, TcpType};
use str0m::{Candidate, Event, IceConnectionState, Input, Output, Rtc};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::{broadcast, mpsc};

/// Why a connection ended.
pub enum Ended {
    /// The receiver left (its socket closed).
    Left,
    /// The connection broke; worth offering again.
    Dropped(String),
    /// No point retrying (no encoder, no screen).
    Fatal(String),
}

/// Writers for the cable's TCP connections, by the phone's address.
type TcpOut = Arc<Mutex<HashMap<SocketAddr, mpsc::UnboundedSender<Vec<u8>>>>>;

struct Packet {
    data: Vec<u8>,
    proto: Protocol,
    source: SocketAddr,
    destination: SocketAddr,
}

/// Run one connection until it ends. `signals` carries the receiver's
/// answer and candidates; it closes when the receiver leaves.
pub async fn run(
    hub: &Arc<Hub>,
    session: u64,
    signals: &mut mpsc::UnboundedReceiver<Inbound>,
    video: &Video,
    usb: bool,
    control: &mut crate::input::Control,
) -> Ended {
    // Network: UDP sockets on the server's addresses. Over the USB cable
    // instead one loopback TCP port, which adb forwards to the phone's own
    // localhost (adb carries TCP only), so the picture stays on the cable.
    let mut sockets: Vec<Arc<UdpSocket>> = Vec::new();
    if !usb {
        for ip in hub.listen_ips() {
            if ip.is_ipv4() {
                match UdpSocket::bind(SocketAddr::new(ip, 0)).await {
                    Ok(s) => sockets.push(Arc::new(s)),
                    Err(e) => tracing::warn!("no UDP socket on {ip}: {e}"),
                }
            }
        }
    }
    let tcp = if usb {
        match TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await {
            Ok(l) => Some(l),
            Err(e) => return Ended::Dropped(format!("no port for the cable: {e}")),
        }
    } else {
        None
    };
    let tcp_addr = tcp.as_ref().and_then(|l| l.local_addr().ok());
    if sockets.is_empty() && tcp_addr.is_none() {
        return Ended::Dropped("no network address for the picture".into());
    }
    if let Some(addr) = tcp_addr {
        // Forward it to the phone before it learns about it.
        let hub2 = hub.clone();
        let port = addr.port();
        let _ = tauri::async_runtime::spawn_blocking(move || hub2.usb().add_port(port)).await;
    }

    let mut rtc = Rtc::builder()
        .clear_codecs()
        .enable_h264(true)
        .enable_opus(true, false)
        .set_ice_lite(true)
        .build(Instant::now());
    for s in &sockets {
        match s.local_addr().map(|a| Candidate::host(a, "udp")) {
            Ok(Ok(c)) => {
                rtc.add_local_candidate(c);
            }
            Ok(Err(e)) => tracing::warn!("bad local candidate: {e}"),
            Err(e) => tracing::warn!("socket address: {e}"),
        }
    }
    if let Some(addr) = tcp_addr {
        // The phone's browser connects (active), we listen (passive).
        match Candidate::builder()
            .tcp()
            .host(addr)
            .tcptype(TcpType::Passive)
            .build()
        {
            Ok(c) => {
                rtc.add_local_candidate(c);
            }
            Err(e) => tracing::warn!("bad cable candidate: {e}"),
        }
    }

    // The offer: video to the receiver, input back from it.
    let mut api = rtc.sdp_api();
    let mid = api.add_media(
        MediaKind::Video,
        Direction::SendOnly,
        Some("deskspan".into()),
        Some("screen".into()),
        None,
    );
    // The computer's sound, in the same stream as the picture.
    let audio_mid = api.add_media(
        MediaKind::Audio,
        Direction::SendOnly,
        Some("deskspan".into()),
        Some("sound".into()),
        None,
    );
    api.add_channel("input".into());
    let Some((offer, pending)) = api.apply() else {
        return Ended::Fatal("could not prepare the connection".into());
    };
    let mut pending = Some(pending);
    if !hub.send(
        session,
        &Outbound::Offer {
            sdp: &offer.to_sdp_string(),
        },
    ) {
        return Ended::Left;
    }

    // Packets from every socket into one queue.
    let (net_tx, mut net_rx) = mpsc::channel::<Packet>(256);
    let mut readers = Vec::new();
    for s in &sockets {
        let (s, tx) = (s.clone(), net_tx.clone());
        readers.push(tauri::async_runtime::spawn(async move {
            let Ok(local) = s.local_addr() else { return };
            let mut buf = vec![0u8; 2000];
            while let Ok((n, source)) = s.recv_from(&mut buf).await {
                let p = Packet {
                    data: buf[..n].to_vec(),
                    proto: Protocol::Udp,
                    source,
                    destination: local,
                };
                if tx.send(p).await.is_err() {
                    break;
                }
            }
        }));
    }
    // The cable: each TCP connection carries RFC 4571 frames (a two-byte
    // length before every packet), in and out.
    let tcp_out: TcpOut = Arc::new(Mutex::new(HashMap::new()));
    if let (Some(listener), Some(local)) = (tcp, tcp_addr) {
        let (tx, out) = (net_tx.clone(), tcp_out.clone());
        readers.push(tauri::async_runtime::spawn(async move {
            while let Ok((stream, remote)) = listener.accept().await {
                let _ = stream.set_nodelay(true);
                let (mut rd, mut wr) = stream.into_split();
                let (wtx, mut wrx) = mpsc::unbounded_channel::<Vec<u8>>();
                out.lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .insert(remote, wtx);
                tauri::async_runtime::spawn(async move {
                    while let Some(packet) = wrx.recv().await {
                        let mut framed = Vec::with_capacity(packet.len() + 2);
                        framed.extend_from_slice(&(packet.len() as u16).to_be_bytes());
                        framed.extend_from_slice(&packet);
                        if wr.write_all(&framed).await.is_err() {
                            break;
                        }
                    }
                });
                let tx = tx.clone();
                tauri::async_runtime::spawn(async move {
                    let mut len = [0u8; 2];
                    while rd.read_exact(&mut len).await.is_ok() {
                        let mut data = vec![0u8; u16::from_be_bytes(len) as usize];
                        if rd.read_exact(&mut data).await.is_err() {
                            break;
                        }
                        let p = Packet {
                            data,
                            proto: Protocol::Tcp,
                            source: remote,
                            destination: local,
                        };
                        if tx.send(p).await.is_err() {
                            break;
                        }
                    }
                });
            }
        }));
    }
    drop(net_tx);

    let mut frames: Option<broadcast::Receiver<Arc<Frame>>> = None;
    let mut pt: Option<Pt> = None;
    let mut sounds: Option<broadcast::Receiver<Arc<crate::audio::Packet>>> = None;
    let mut audio_pt: Option<Pt> = None;
    let ended = 'outer: loop {
        // Drain everything the engine wants to do (the str0m contract).
        let timeout = loop {
            match rtc.poll_output() {
                Ok(Output::Timeout(t)) => break t,
                Ok(Output::Transmit(t)) if t.proto == Protocol::Tcp => {
                    let out = tcp_out
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .get(&t.destination)
                        .cloned();
                    if let Some(out) = out {
                        let _ = out.send(t.contents.to_vec());
                    }
                }
                Ok(Output::Transmit(t)) => {
                    if let Some(s) = sockets
                        .iter()
                        .find(|s| s.local_addr().ok() == Some(t.source))
                    {
                        if let Err(e) = s.send_to(&t.contents, t.destination).await {
                            tracing::debug!("send to {}: {e}", t.destination);
                        }
                    }
                }
                Ok(Output::Event(e)) => match e {
                    Event::Connected => {
                        pt = pick_h264(&mut rtc, mid);
                        if pt.is_none() {
                            break 'outer Ended::Fatal(
                                "this browser can't show H.264 video".into(),
                            );
                        }
                        match video.subscribe().await {
                            Ok(rx) => frames = Some(rx),
                            Err(e) => break 'outer Ended::Fatal(e),
                        }
                        // Sound if the computer can capture it and it's on.
                        audio_pt = pick_opus(&mut rtc, audio_mid);
                        if audio_pt.is_some() && hub.sound_on() {
                            sounds = crate::audio::shared().subscribe();
                        }
                    }
                    Event::KeyframeRequest(_) => video.request_keyframe(),
                    Event::IceConnectionStateChange(IceConnectionState::Disconnected) => {
                        break 'outer Ended::Dropped("connection lost".into());
                    }
                    // Touch, pen, mouse and scroll back from the receiver.
                    Event::ChannelData(d) => control.handle(&d.data),
                    _ => {}
                },
                Err(e) => break 'outer Ended::Dropped(format!("connection error: {e}")),
            }
        };
        if !rtc.is_alive() {
            break Ended::Dropped("connection closed".into());
        }

        // Wait for ONE thing, then feed it in (one mutation per drain).
        let sleep = tokio::time::sleep_until(tokio::time::Instant::from_std(timeout));
        tokio::select! {
            p = net_rx.recv() => {
                let Some(p) = p else { break Ended::Dropped("network closed".into()) };
                let Ok(contents) = p.data.as_slice().try_into() else { continue };
                let input = Input::Receive(
                    Instant::now(),
                    Receive { proto: p.proto, source: p.source, destination: p.destination, contents },
                );
                if rtc.accepts(&input) {
                    if let Err(e) = rtc.handle_input(input) {
                        tracing::debug!("bad packet: {e}");
                    }
                }
            }
            _ = sleep => {
                if let Err(e) = rtc.handle_input(Input::Timeout(Instant::now())) {
                    break Ended::Dropped(format!("connection error: {e}"));
                }
            }
            sig = signals.recv() => match sig {
                None => break Ended::Left,
                Some(Inbound::Answer { sdp }) => {
                    let Some(p) = pending.take() else { continue };
                    let answer = match SdpAnswer::from_sdp_string(&sdp) {
                        Ok(a) => a,
                        Err(e) => break Ended::Fatal(format!("the receiver's answer wasn't understood: {e}")),
                    };
                    if let Err(e) = rtc.sdp_api().accept_answer(p, answer) {
                        break Ended::Fatal(format!("the receiver's answer didn't fit: {e}"));
                    }
                }
                Some(Inbound::Ice { c }) => {
                    // mDNS (.local) candidates don't parse; ICE-lite doesn't need them.
                    if let Some(Ok(cand)) = c.get("candidate").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(Candidate::from_sdp_string) {
                        rtc.add_remote_candidate(cand);
                    }
                }
                Some(_) => {}
            },
            f = recv_frame(&mut frames) => match f {
                Ok(frame) => {
                    if let (Some(pt), Some(w)) = (pt, rtc.writer(mid)) {
                        let t = MediaTime::new(frame.rtp, Frequency::NINETY_KHZ);
                        if let Err(e) = w.write(pt, frame.at, t, frame.data.clone()) {
                            tracing::debug!("write frame: {e}");
                        }
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => video.request_keyframe(),
                Err(broadcast::error::RecvError::Closed) => break Ended::Dropped("the picture stopped".into()),
            },
            p = recv_sound(&mut sounds) => match p {
                Ok(packet) => {
                    if let (Some(pt), Some(w)) = (audio_pt, rtc.writer(audio_mid)) {
                        let t = MediaTime::new(packet.rtp, Frequency::FORTY_EIGHT_KHZ);
                        if let Err(e) = w.write(pt, packet.at, t, packet.data.clone()) {
                            tracing::debug!("write sound: {e}");
                        }
                    }
                }
                // Behind: skip ahead (a click beats a growing delay).
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                // The capture stopped (e.g. no sound device): carry on without.
                Err(broadcast::error::RecvError::Closed) => sounds = None,
            },
        }
    };

    rtc.disconnect();
    for r in readers {
        r.abort();
    }
    // Closing the writers ends the cable's connections.
    tcp_out.lock().unwrap_or_else(|p| p.into_inner()).clear();
    if let Some(addr) = tcp_addr {
        let hub2 = hub.clone();
        let port = addr.port();
        let _ = tauri::async_runtime::spawn_blocking(move || hub2.usb().remove_port(port)).await;
    }
    ended
}

/// The next frame, or never while we aren't subscribed yet.
async fn recv_frame(
    frames: &mut Option<broadcast::Receiver<Arc<Frame>>>,
) -> Result<Arc<Frame>, broadcast::error::RecvError> {
    match frames {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

/// The next sound packet, or never without sound.
async fn recv_sound(
    sounds: &mut Option<broadcast::Receiver<Arc<crate::audio::Packet>>>,
) -> Result<Arc<crate::audio::Packet>, broadcast::error::RecvError> {
    match sounds {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

/// The negotiated Opus payload type, if the receiver takes sound.
fn pick_opus(rtc: &mut Rtc, mid: Mid) -> Option<Pt> {
    let w = rtc.writer(mid)?;
    for p in w.payload_params() {
        if p.spec().codec == Codec::Opus {
            return Some(p.pt());
        }
    }
    None
}

/// The negotiated H.264 payload type we can send: packetization mode 1,
/// Constrained Baseline preferred (what OpenH264 makes and every browser
/// decodes).
fn pick_h264(rtc: &mut Rtc, mid: Mid) -> Option<Pt> {
    let w = rtc.writer(mid)?;
    let mut best: Option<(u8, Pt)> = None;
    for p in w.payload_params() {
        let spec = p.spec();
        if spec.codec != Codec::H264 || spec.format.packetization_mode != Some(1) {
            continue;
        }
        let rank = match spec.format.profile_level_id.map(|v| v >> 8) {
            Some(0x42e0) => 0,
            Some(v) if v >> 8 == 0x42 => 1,
            _ => 2,
        };
        if best.is_none_or(|(r, _)| rank < r) {
            best = Some((rank, p.pt()));
        }
    }
    best.map(|(_, pt)| pt)
}
