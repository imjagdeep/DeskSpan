//! Sound: what the computer is playing, to every receiver.
//!
//! One capture for everyone, started when the first receiver subscribes and
//! stopped a few seconds after the last one leaves. Samples are turned into
//! 48 kHz stereo and encoded as Opus in 20 ms packets (what every browser
//! plays). Windows: WASAPI loopback (cpal). macOS 13+: ScreenCaptureKit's
//! audio (scaudio.rs), which needs the same Screen Recording permission as
//! the picture. Linux: not yet.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

pub const RATE: u32 = 48_000;
/// 20 ms of stereo at 48 kHz.
const FRAME: usize = 960;

/// One Opus packet and where it falls in time.
pub struct Packet {
    pub data: Vec<u8>,
    /// When it was captured.
    pub at: Instant,
    /// RTP time: samples at 48 kHz since the capture started.
    pub rtp: u64,
}

/// The one capture for the whole app.
pub fn shared() -> &'static Audio {
    static AUDIO: std::sync::OnceLock<Audio> = std::sync::OnceLock::new();
    AUDIO.get_or_init(Audio::default)
}

/// The capture shared by all receivers.
#[derive(Clone, Default)]
pub struct Audio {
    tx: Arc<Mutex<Option<broadcast::Sender<Arc<Packet>>>>>,
}

impl Audio {
    /// Packets from now on; starts the capture if it isn't running.
    /// None where the computer's sound can't be captured.
    pub fn subscribe(&self) -> Option<broadcast::Receiver<Arc<Packet>>> {
        if !SUPPORTED {
            return None;
        }
        let mut g = self.tx.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(tx) = g.as_ref() {
            return Some(tx.subscribe());
        }
        let (tx, rx) = broadcast::channel(64);
        *g = Some(tx.clone());
        let me = self.clone();
        std::thread::Builder::new()
            .name("deskspan-audio".into())
            .spawn(move || {
                if let Err(e) = run(&tx) {
                    tracing::warn!("sound: {e}");
                }
                // Next subscriber starts a fresh capture.
                *me.tx.lock().unwrap_or_else(|p| p.into_inner()) = None;
            })
            .ok()?;
        Some(rx)
    }
}

#[cfg(any(windows, target_os = "macos"))]
const SUPPORTED: bool = true;
#[cfg(not(any(windows, target_os = "macos")))]
const SUPPORTED: bool = false;

/// Capture and encode until nobody has listened for a while.
fn run(tx: &broadcast::Sender<Arc<Packet>>) -> Result<(), String> {
    let (pcm_tx, pcm_rx) = std::sync::mpsc::sync_channel::<Chunk>(64);
    let _source = Source::start(pcm_tx)?;
    let mut enc = opus::Encoder::new(RATE, opus::Channels::Stereo, opus::Application::LowDelay)
        .map_err(|e| format!("no Opus encoder: {e}"))?;
    let _ = enc.set_bitrate(opus::Bitrate::Bits(128_000));
    let mut shaper = Shaper::default();
    let mut out = vec![0u8; 4000];
    let mut rtp: u64 = 0;
    let mut lonely_since: Option<Instant> = None;
    loop {
        if tx.receiver_count() == 0 {
            let since = *lonely_since.get_or_insert_with(Instant::now);
            if since.elapsed() > Duration::from_secs(5) {
                return Ok(());
            }
        } else {
            lonely_since = None;
        }
        let chunk = match pcm_rx.recv_timeout(Duration::from_millis(500)) {
            Ok(c) => c,
            // Nothing playing (Windows sends nothing in silence): keep waiting.
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return Err("the sound capture stopped".into())
            }
        };
        for frame in shaper.push(&chunk) {
            let n = enc
                .encode_float(&frame, &mut out)
                .map_err(|e| format!("Opus: {e}"))?;
            let _ = tx.send(Arc::new(Packet {
                data: out[..n].to_vec(),
                at: Instant::now(),
                rtp,
            }));
            rtp += FRAME as u64;
        }
    }
}

/// Samples as the system delivers them: interleaved, any rate and channel count.
pub struct Chunk {
    pub samples: Vec<f32>,
    pub channels: u16,
    pub rate: u32,
}

/// Turns chunks into 20 ms stereo frames at 48 kHz (interleaved, 1920 values).
#[derive(Default)]
pub struct Shaper {
    pending: Vec<f32>,
    /// Resampling position between input frames.
    pos: f64,
    /// The last input frame (for interpolating across chunks).
    last: [f32; 2],
}

impl Shaper {
    pub fn push(&mut self, c: &Chunk) -> Vec<Vec<f32>> {
        let ch = usize::from(c.channels.max(1));
        let frames = c.samples.len() / ch;
        // Down or up to stereo: mono is copied to both sides, extra channels dropped.
        let stereo = (0..frames).map(|i| {
            let l = c.samples[i * ch];
            let r = if ch > 1 { c.samples[i * ch + 1] } else { l };
            [l, r]
        });
        if c.rate == RATE || c.rate == 0 {
            for [l, r] in stereo {
                self.pending.push(l);
                self.pending.push(r);
            }
        } else {
            // Linear resampling: plenty for a second screen's sound.
            let step = f64::from(c.rate) / f64::from(RATE);
            let input: Vec<[f32; 2]> = std::iter::once(self.last).chain(stereo).collect();
            while self.pos + 1.0 < input.len() as f64 {
                let i = self.pos.floor() as usize;
                let t = (self.pos - i as f64) as f32;
                let (a, b) = (input[i], input[i + 1]);
                self.pending.push(a[0] + (b[0] - a[0]) * t);
                self.pending.push(a[1] + (b[1] - a[1]) * t);
                self.pos += step;
            }
            self.pos -= (input.len() - 1) as f64;
            if let Some(l) = input.last() {
                self.last = *l;
            }
        }
        let mut out = Vec::new();
        while self.pending.len() >= FRAME * 2 {
            out.push(self.pending.drain(..FRAME * 2).collect());
        }
        out
    }
}

// ---------- where the samples come from ----------

#[cfg(windows)]
struct Source {
    _stream: cpal::Stream,
}

#[cfg(windows)]
impl Source {
    /// Loopback: an input stream on the default output device records what it plays.
    fn start(tx: std::sync::mpsc::SyncSender<Chunk>) -> Result<Source, String> {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or("no speakers or headphones to listen to")?;
        let config = device
            .default_output_config()
            .map_err(|e| format!("can't read the sound settings: {e}"))?;
        let channels = config.channels();
        let rate = config.sample_rate();
        let on_error = |e: cpal::Error| tracing::warn!("sound capture: {e}");
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => device.build_input_stream(
                config.into(),
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    let _ = tx.try_send(Chunk {
                        samples: data.to_vec(),
                        channels,
                        rate,
                    });
                },
                on_error,
                None,
            ),
            cpal::SampleFormat::I16 => device.build_input_stream(
                config.into(),
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    let samples = data.iter().map(|&s| f32::from(s) / 32768.0).collect();
                    let _ = tx.try_send(Chunk {
                        samples,
                        channels,
                        rate,
                    });
                },
                on_error,
                None,
            ),
            other => return Err(format!("unsupported sound format {other:?}")),
        }
        .map_err(|e| format!("can't listen to the computer's sound: {e}"))?;
        stream
            .play()
            .map_err(|e| format!("can't start the sound capture: {e}"))?;
        Ok(Source { _stream: stream })
    }
}

#[cfg(target_os = "macos")]
struct Source {
    _capture: crate::scaudio::Capture,
}

#[cfg(target_os = "macos")]
impl Source {
    fn start(tx: std::sync::mpsc::SyncSender<Chunk>) -> Result<Source, String> {
        Ok(Source {
            _capture: crate::scaudio::Capture::start(tx)?,
        })
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
struct Source;

#[cfg(not(any(windows, target_os = "macos")))]
impl Source {
    fn start(_tx: std::sync::mpsc::SyncSender<Chunk>) -> Result<Source, String> {
        Err("sound isn't available on Linux yet".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(frames: usize, channels: u16, rate: u32, value: f32) -> Chunk {
        Chunk {
            samples: vec![value; frames * usize::from(channels)],
            channels,
            rate,
        }
    }

    #[test]
    fn stereo_48k_becomes_20ms_frames() {
        let mut s = Shaper::default();
        assert!(s.push(&chunk(500, 2, 48_000, 0.5)).is_empty());
        let out = s.push(&chunk(500, 2, 48_000, 0.5));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].len(), 1920);
        assert_eq!(s.pending.len(), 80); // 40 frames left over
    }

    #[test]
    fn mono_is_copied_and_extra_channels_dropped() {
        let mut s = Shaper::default();
        let out = s.push(&Chunk {
            samples: (0..960).map(|i| i as f32).collect(),
            channels: 1,
            rate: 48_000,
        });
        assert_eq!(&out[0][..4], &[0.0, 0.0, 1.0, 1.0]);

        let mut s = Shaper::default();
        let six: Vec<f32> = (0..960)
            .flat_map(|_| [1.0, 2.0, 9.0, 9.0, 9.0, 9.0])
            .collect();
        let out = s.push(&Chunk {
            samples: six,
            channels: 6,
            rate: 48_000,
        });
        assert_eq!(&out[0][..2], &[1.0, 2.0]);
    }

    #[test]
    fn other_rates_are_resampled_to_48k() {
        let mut s = Shaper::default();
        // One second at 44.1 kHz → about one second at 48 kHz (50 frames of 20 ms).
        let mut frames = 0;
        for _ in 0..10 {
            frames += s.push(&chunk(4410, 2, 44_100, 0.25)).len();
        }
        assert!((49..=50).contains(&frames), "{frames}");
    }
}
