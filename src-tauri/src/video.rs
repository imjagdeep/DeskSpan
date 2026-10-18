//! The picture: capture the main screen, shrink very large screens, convert
//! to YUV and encode H.264 (Cisco's OpenH264), then hand each frame to every
//! connected receiver (broadcast channel).
//!
//! One capture thread serves all receivers. It starts with the first one and
//! stops a few seconds after the last one leaves.

use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

/// Target frames per second (a ceiling; big screens may capture slower).
const FPS: u32 = 30;
/// Wider than this is halved before encoding (4K → 1920×1080).
const MAX_WIDTH: u32 = 2560;
/// Bits per second for a 1080p-ish desktop: sharp text, moderate Wi-Fi load.
const BITRATE: u32 = 8_000_000;
/// The thread stops this long after the last receiver leaves.
const LINGER: Duration = Duration::from_secs(3);

/// One encoded picture (Annex B NAL units).
pub struct Frame {
    pub data: Arc<[u8]>,
    /// When it was captured.
    pub at: Instant,
    /// 90 kHz time since the stream started.
    pub rtp: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct VideoStatus {
    /// "idle", "preparing" (fetching the encoder), "streaming", "error"
    pub state: &'static str,
    pub width: u32,
    pub height: u32,
    /// Frames per second over the last second.
    pub fps: u32,
    pub error: Option<String>,
}

struct Inner {
    data_dir: PathBuf,
    /// The checked encoder library, once known.
    library: Option<PathBuf>,
    status: VideoStatus,
}

#[derive(Clone)]
pub struct Video {
    /// The display to capture: None = the main screen (mirror), Some(id) =
    /// that display (an extend receiver's virtual display).
    target: Option<u32>,
    tx: broadcast::Sender<Arc<Frame>>,
    keyframe: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
    inner: Arc<Mutex<Inner>>,
}

impl Video {
    /// The main screen (mirror), shared by every mirroring receiver.
    pub fn new(data_dir: PathBuf) -> Video {
        Video::for_target(data_dir, None)
    }

    /// One display by its ID (an extend receiver's own virtual display).
    pub fn for_display(data_dir: PathBuf, display_id: u32) -> Video {
        Video::for_target(data_dir, Some(display_id))
    }

    fn for_target(data_dir: PathBuf, target: Option<u32>) -> Video {
        let (tx, _) = broadcast::channel(4);
        Video {
            target,
            tx,
            keyframe: Arc::new(AtomicBool::new(false)),
            running: Arc::new(AtomicBool::new(false)),
            inner: Arc::new(Mutex::new(Inner {
                data_dir,
                library: None,
                status: VideoStatus {
                    state: "idle",
                    ..VideoStatus::default()
                },
            })),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn status(&self) -> VideoStatus {
        self.lock().status.clone()
    }

    fn set_state(&self, state: &'static str, error: Option<String>) {
        let mut g = self.lock();
        g.status.state = state;
        g.status.error = error;
    }

    /// The next frame will be a full picture (new receiver, lost packets).
    pub fn request_keyframe(&self) {
        self.keyframe.store(true, Ordering::Relaxed);
    }

    /// Frames from now on; starts capturing if nobody was watching.
    pub async fn subscribe(&self) -> Result<broadcast::Receiver<Arc<Frame>>, String> {
        let rx = self.tx.subscribe();
        self.request_keyframe();
        if self.running.load(Ordering::Acquire) {
            return Ok(rx);
        }
        let (library, data_dir) = {
            let g = self.lock();
            (g.library.clone(), g.data_dir.clone())
        };
        let library = match library {
            Some(l) => l,
            None => {
                self.set_state("preparing", None);
                match crate::h264lib::ensure(&data_dir).await {
                    Ok(l) => {
                        self.lock().library = Some(l.clone());
                        l
                    }
                    Err(e) => {
                        self.set_state("error", Some(e.clone()));
                        return Err(e);
                    }
                }
            }
        };
        // Another receiver may have started it meanwhile.
        if self.running.swap(true, Ordering::AcqRel) {
            return Ok(rx);
        }
        let me = self.clone();
        std::thread::Builder::new()
            .name("deskspan-video".into())
            .spawn(move || {
                let result = me.run(&library);
                me.running.store(false, Ordering::Release);
                match result {
                    Ok(()) => me.set_state("idle", None),
                    Err(e) => {
                        tracing::warn!("video stopped: {e}");
                        me.set_state("error", Some(e));
                    }
                }
            })
            .map_err(|e| {
                self.running.store(false, Ordering::Release);
                format!("cannot start the screen capture: {e}")
            })?;
        Ok(rx)
    }

    /// The capture + encode loop (own thread).
    fn run(&self, library: &std::path::Path) -> Result<(), String> {
        use openh264::encoder::{
            BitRate, Encoder, EncoderConfig, FrameRate, IntraFramePeriod, Profile, RateControlMode,
            UsageType,
        };
        use openh264::formats::YUVBuffer;

        let api = crate::h264lib::load(library)?;
        let config = EncoderConfig::new()
            .usage_type(UsageType::ScreenContentRealTime)
            .profile(Profile::Baseline)
            .rate_control_mode(RateControlMode::Bitrate)
            .bitrate(BitRate::from_bps(BITRATE))
            .max_frame_rate(FrameRate::from_hz(FPS as f32))
            .intra_frame_period(IntraFramePeriod::from_num_frames(FPS * 10));
        let mut encoder = Encoder::with_api_config(api, config)
            .map_err(|e| format!("the H.264 encoder didn't start: {e}"))?;
        let monitor = monitor_for(self.target)?;
        self.set_state("streaming", None);

        let start = Instant::now();
        let frame_time = Duration::from_secs(1) / FPS;
        let mut alone_since: Option<Instant> = None;
        let mut counted = 0u32;
        let mut count_from = Instant::now();
        loop {
            let began = Instant::now();
            if self.tx.receiver_count() == 0 {
                let t = *alone_since.get_or_insert(began);
                if t.elapsed() > LINGER {
                    return Ok(());
                }
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            alone_since = None;

            let (rgba, sw, sh) = grab(&monitor)?;
            let scale = if sw > MAX_WIDTH { 2 } else { 1 };
            let (yuv, w, h) = rgba_to_i420(&rgba, sw, sh, scale);
            if w == 0 || h == 0 {
                return Err("the screen reported no size".into());
            }
            let buffer = YUVBuffer::from_vec(yuv, w as usize, h as usize);
            if self.keyframe.swap(false, Ordering::Relaxed) {
                encoder.force_intra_frame();
            }
            let bits = encoder
                .encode(&buffer)
                .map_err(|e| format!("encoding failed: {e}"))?;
            let data = bits.to_vec();
            if !data.is_empty() {
                let frame = Frame {
                    data: data.into(),
                    at: began,
                    rtp: (began - start).as_micros() as u64 * 9 / 100,
                };
                // No receivers left between the check and now: fine.
                let _ = self.tx.send(Arc::new(frame));
            }

            counted += 1;
            if count_from.elapsed() >= Duration::from_secs(1) {
                let mut g = self.lock();
                g.status.fps = counted;
                g.status.width = w;
                g.status.height = h;
                counted = 0;
                count_from = Instant::now();
            }
            if let Some(rest) = frame_time.checked_sub(began.elapsed()) {
                std::thread::sleep(rest);
            }
        }
    }
}

/// One picture of the screen as RGBA. macOS 14+: ScreenCaptureKit, asked
/// for at most MAX_WIDTH wide (xcap's older API returns nothing on macOS
/// 27); xcap elsewhere.
fn grab(monitor: &xcap::Monitor) -> Result<(Vec<u8>, u32, u32), String> {
    #[cfg(target_os = "macos")]
    if crate::sck::available() {
        let id = monitor.id().map_err(|e| format!("screen id: {e}"))?;
        let scale = monitor.scale_factor().unwrap_or(1.0) as f64;
        let mut w =
            (monitor.width().map_err(|e| format!("screen size: {e}"))? as f64 * scale) as u32;
        let mut h =
            (monitor.height().map_err(|e| format!("screen size: {e}"))? as f64 * scale) as u32;
        if w > MAX_WIDTH {
            w /= 2;
            h /= 2;
        }
        return crate::sck::capture_display(id, w, h);
    }
    let shot = monitor
        .capture_image()
        .map_err(|e| format!("cannot capture the screen: {e}"))?;
    let (w, h) = (shot.width(), shot.height());
    Ok((shot.into_raw(), w, h))
}

/// The screen to capture: the main one (or the first), or the display with
/// this ID. A just-made virtual display can take a moment to be listed.
fn monitor_for(target: Option<u32>) -> Result<xcap::Monitor, String> {
    for attempt in 0..15 {
        let all = xcap::Monitor::all().map_err(|e| format!("cannot list the screens: {e}"))?;
        let found = match target {
            None => all
                .iter()
                .position(|m| m.is_primary().unwrap_or(false))
                .or(if all.is_empty() { None } else { Some(0) }),
            Some(id) => all.iter().position(|m| m.id().ok() == Some(id)),
        };
        if let Some(i) = found {
            return all
                .into_iter()
                .nth(i)
                .ok_or_else(|| "no screen found".to_string());
        }
        if target.is_none() || attempt == 14 {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Err(match target {
        None => "no screen found".to_string(),
        Some(id) => format!("the extra display ({id}) didn't appear"),
    })
}

/// RGBA pixels → I420 (BT.601, limited range), optionally shrunk by an
/// integer `scale` (each output pixel averages a scale×scale block). The
/// result's width and height are even, as H.264 needs.
pub fn rgba_to_i420(src: &[u8], sw: u32, sh: u32, scale: u32) -> (Vec<u8>, u32, u32) {
    let scale = scale.max(1);
    let w = (sw / scale) & !1;
    let h = (sh / scale) & !1;
    if w == 0 || h == 0 || src.len() < (sw * sh * 4) as usize {
        return (Vec::new(), 0, 0);
    }
    let (wu, hu) = (w as usize, h as usize);
    let mut out = vec![0u8; wu * hu * 3 / 2];
    let (y_plane, uv) = out.split_at_mut(wu * hu);
    let (u_plane, v_plane) = uv.split_at_mut(wu * hu / 4);
    let stride = sw as usize * 4;
    let s = scale as usize;
    let n = (s * s) as u32;

    // One output pixel: the average of its block.
    let rgb = |ox: usize, oy: usize| -> (i32, i32, i32) {
        let (mut r, mut g, mut b) = (0u32, 0u32, 0u32);
        for dy in 0..s {
            let row = (oy * s + dy) * stride;
            for dx in 0..s {
                let i = row + (ox * s + dx) * 4;
                r += src[i] as u32;
                g += src[i + 1] as u32;
                b += src[i + 2] as u32;
            }
        }
        ((r / n) as i32, (g / n) as i32, (b / n) as i32)
    };

    for cy in 0..hu / 2 {
        for cx in 0..wu / 2 {
            let (mut sr, mut sg, mut sb) = (0i32, 0i32, 0i32);
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let (x, y) = (cx * 2 + dx, cy * 2 + dy);
                let (r, g, b) = rgb(x, y);
                y_plane[y * wu + x] = (((66 * r + 129 * g + 25 * b + 128) >> 8) + 16) as u8;
                sr += r;
                sg += g;
                sb += b;
            }
            let (r, g, b) = (sr / 4, sg / 4, sb / 4);
            let ci = cy * (wu / 2) + cx;
            u_plane[ci] = (((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128).clamp(0, 255) as u8;
            v_plane[ci] = (((112 * r - 94 * g - 18 * b + 128) >> 8) + 128).clamp(0, 255) as u8;
        }
    }
    (out, w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, rgba: [u8; 4]) -> Vec<u8> {
        rgba.iter()
            .copied()
            .cycle()
            .take((w * h * 4) as usize)
            .collect()
    }

    #[test]
    fn white_black_and_red() {
        let (out, w, h) = rgba_to_i420(&solid(4, 4, [255, 255, 255, 255]), 4, 4, 1);
        assert_eq!((w, h, out.len()), (4, 4, 24));
        assert_eq!(out[0], 235); // white Y
        assert_eq!(out[16], 128); // neutral U
        let (out, ..) = rgba_to_i420(&solid(4, 4, [0, 0, 0, 255]), 4, 4, 1);
        assert_eq!(out[0], 16); // black Y
        let (out, ..) = rgba_to_i420(&solid(2, 2, [255, 0, 0, 255]), 2, 2, 1);
        assert_eq!((out[0], out[4], out[5]), (82, 90, 240)); // red Y U V
    }

    #[test]
    fn halves_and_keeps_even_sizes() {
        let (out, w, h) = rgba_to_i420(&solid(3840, 2160, [10, 20, 30, 255]), 3840, 2160, 2);
        assert_eq!((w, h), (1920, 1080));
        assert_eq!(out.len(), 1920 * 1080 * 3 / 2);
        let (_, w, h) = rgba_to_i420(&solid(1367, 769, [0, 0, 0, 255]), 1367, 769, 1);
        assert_eq!((w, h), (1366, 768));
    }

    #[test]
    fn averages_blocks_when_shrinking() {
        // 2×2 → 1×1 is too small (odd), so 4×4 → 2×2: left half black, right white.
        let mut px = Vec::new();
        for _y in 0..4 {
            for x in 0..4 {
                let v = if x < 2 { 0 } else { 255 };
                px.extend_from_slice(&[v, v, v, 255]);
            }
        }
        let (out, w, _) = rgba_to_i420(&px, 4, 4, 2);
        assert_eq!(w, 2);
        assert_eq!((out[0], out[1]), (16, 235));
    }

    #[test]
    fn short_input_gives_nothing() {
        assert_eq!(rgba_to_i420(&[0; 10], 4, 4, 1).1, 0);
    }
}
