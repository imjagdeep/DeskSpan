//! One receiver's WebRTC connection (str0m, Sans I/O): we offer a send-only
//! H.264 video track and an "input" data channel, the receiver answers, and
//! this task moves UDP packets, timers and video frames through the `Rtc`.
//!
//! ICE-lite with host candidates on our LAN addresses: the browser does the
//! connectivity checks, so its own (often mDNS-hidden) addresses don't need
//! resolving.

use crate::server::{Hub, Inbound, Outbound};
use crate::video::{Frame, Video};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;
use str0m::change::SdpAnswer;
use str0m::format::Codec;
use str0m::media::{Direction, Frequency, MediaKind, MediaTime, Mid, Pt};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, IceConnectionState, Input, Output, Rtc};
use tokio::net::UdpSocket;
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

struct Packet {
    data: Vec<u8>,
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
) -> Ended {
    // Our UDP sockets: one per address the server listens on.
    let mut sockets: Vec<Arc<UdpSocket>> = Vec::new();
    for ip in hub.listen_ips() {
        if ip.is_ipv4() {
            match UdpSocket::bind(SocketAddr::new(ip, 0)).await {
                Ok(s) => sockets.push(Arc::new(s)),
                Err(e) => tracing::warn!("no UDP socket on {ip}: {e}"),
            }
        }
    }
    if sockets.is_empty() {
        return Ended::Dropped("no network address for the picture".into());
    }

    let mut rtc = Rtc::builder()
        .clear_codecs()
        .enable_h264(true)
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

    // The offer: video to the receiver, input back from it.
    let mut api = rtc.sdp_api();
    let mid = api.add_media(
        MediaKind::Video,
        Direction::SendOnly,
        Some("deskspan".into()),
        Some("screen".into()),
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
                    source,
                    destination: local,
                };
                if tx.send(p).await.is_err() {
                    break;
                }
            }
        }));
    }
    drop(net_tx);

    let mut frames: Option<broadcast::Receiver<Arc<Frame>>> = None;
    let mut pt: Option<Pt> = None;
    let ended = 'outer: loop {
        // Drain everything the engine wants to do (the str0m contract).
        let timeout = loop {
            match rtc.poll_output() {
                Ok(Output::Timeout(t)) => break t,
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
                    }
                    Event::KeyframeRequest(_) => video.request_keyframe(),
                    Event::IceConnectionStateChange(IceConnectionState::Disconnected) => {
                        break 'outer Ended::Dropped("connection lost".into());
                    }
                    Event::ChannelData(d) => {
                        // Touch, pen and mouse: acted on in milestone 4.
                        tracing::trace!("input {} bytes", d.data.len());
                    }
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
                    Receive { proto: Protocol::Udp, source: p.source, destination: p.destination, contents },
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
        }
    };

    rtc.disconnect();
    for r in readers {
        r.abort();
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
