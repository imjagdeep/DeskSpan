//! macOS 13+: the computer's sound through ScreenCaptureKit.
//!
//! An audio-only SCStream of the main display (a 2×2, once-a-second picture
//! that's thrown away), asking for 48 kHz stereo and leaving out DeskSpan's
//! own sounds. Samples arrive as 32-bit float; non-interleaved buffers (one
//! channel after the other, which is what macOS sends) are interleaved here.

use crate::audio::Chunk;
use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass};
use objc2_core_media::{CMSampleBuffer, CMTime};
use objc2_foundation::{NSArray, NSObject, NSObjectProtocol};
use objc2_screen_capture_kit::{
    SCContentFilter, SCStream, SCStreamConfiguration, SCStreamOutput, SCStreamOutputType,
};
use std::ptr::NonNull;
use std::sync::mpsc::SyncSender;

/// kAudioFormatFlagIsNonInterleaved
const NON_INTERLEAVED: u32 = 1 << 5;

struct Ivars {
    tx: SyncSender<Chunk>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "DeskSpanSoundSink"]
    #[ivars = Ivars]
    struct SoundSink;

    unsafe impl NSObjectProtocol for SoundSink {}

    unsafe impl SCStreamOutput for SoundSink {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        fn stream_did_output(
            &self,
            _stream: &SCStream,
            sample: &CMSampleBuffer,
            kind: SCStreamOutputType,
        ) {
            if kind != SCStreamOutputType::Audio {
                return;
            }
            if let Some(chunk) = unsafe { to_chunk(sample) } {
                // Full: the encoder is behind; a dropped 10 ms beats a growing delay.
                let _ = self.ivars().tx.try_send(chunk);
            }
        }
    }
);

impl SoundSink {
    fn new(tx: SyncSender<Chunk>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(Ivars { tx });
        unsafe { msg_send![super(this), init] }
    }
}

/// A running sound capture. Dropping it stops the stream.
pub struct Capture {
    stream: Retained<SCStream>,
    _sink: Retained<SoundSink>,
    _queue: DispatchRetained<DispatchQueue>,
}

impl Capture {
    pub fn start(tx: SyncSender<Chunk>) -> Result<Capture, String> {
        let display = crate::sck::main_display()?;
        let filter = unsafe {
            SCContentFilter::initWithDisplay_excludingWindows(
                SCContentFilter::alloc(),
                &display,
                &NSArray::new(),
            )
        };
        let config = unsafe { SCStreamConfiguration::new() };
        unsafe {
            config.setCapturesAudio(true);
            config.setSampleRate(crate::audio::RATE as isize);
            config.setChannelCount(2);
            config.setExcludesCurrentProcessAudio(true);
            // The picture of this stream isn't used: keep it tiny and rare.
            config.setWidth(2);
            config.setHeight(2);
            config.setMinimumFrameInterval(CMTime::new(1, 1));
        }
        let sink = SoundSink::new(tx);
        let queue = DispatchQueue::new("app.deskspan.sound", None);
        let stream = unsafe {
            SCStream::initWithFilter_configuration_delegate(
                SCStream::alloc(),
                &filter,
                &config,
                None,
            )
        };
        unsafe {
            stream.addStreamOutput_type_sampleHandlerQueue_error(
                ProtocolObject::from_ref(&*sink),
                SCStreamOutputType::Audio,
                Some(&*queue),
            )
        }
        .map_err(|e| format!("cannot capture the sound: {}", e.localizedDescription()))?;
        crate::scstream::wait_for(|handler| unsafe {
            stream.startCaptureWithCompletionHandler(Some(handler))
        })
        .map_err(|e| format!("macOS didn't start the sound capture: {e}"))?;
        Ok(Capture {
            stream,
            _sink: sink,
            _queue: queue,
        })
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        let _ = crate::scstream::wait_for(|handler| unsafe {
            self.stream.stopCaptureWithCompletionHandler(Some(handler))
        });
    }
}

/// Float samples from one audio sample buffer, interleaved.
unsafe fn to_chunk(sample: &CMSampleBuffer) -> Option<Chunk> {
    let frames = usize::try_from(sample.num_samples())
        .ok()
        .filter(|&n| n > 0)?;
    let desc = sample.format_description()?;
    let asbd =
        objc2_core_media::CMAudioFormatDescriptionGetStreamBasicDescription(&desc).as_ref()?;
    let channels = asbd.mChannelsPerFrame.max(1) as usize;
    if asbd.mBitsPerChannel != 32 {
        return None;
    }
    let block = sample.data_buffer()?;
    let len = block.data_length();
    let mut raw = vec![0f32; len / 4];
    if raw.is_empty()
        || block.copy_data_bytes(0, raw.len() * 4, NonNull::new(raw.as_mut_ptr().cast())?) != 0
    {
        return None;
    }
    let samples = if asbd.mFormatFlags & NON_INTERLEAVED != 0 && channels > 1 {
        // One channel's frames after the other's.
        let per = raw.len() / channels;
        let n = frames.min(per);
        let mut out = Vec::with_capacity(n * channels);
        for i in 0..n {
            for c in 0..channels {
                out.push(raw[c * per + i]);
            }
        }
        out
    } else {
        raw
    };
    Some(Chunk {
        samples,
        channels: channels as u16,
        rate: asbd.mSampleRate as u32,
    })
}
