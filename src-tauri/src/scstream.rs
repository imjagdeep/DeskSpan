//! macOS 14+: a continuous ScreenCaptureKit stream of one display.
//!
//! One screenshot per frame (sck.rs) took 60–100 ms each on a Retina
//! display, so the picture lagged at about 10 frames a second. An SCStream
//! pushes frames itself (only when something changed), already scaled and
//! already in NV12, which is one cheap repack away from the encoder's I420.
//! Frames arrive on our own dispatch queue and go to the encoder thread
//! through a short channel; when the encoder is busy, older frames are
//! dropped rather than queued (fresh beats complete).

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass};
use objc2_core_media::{CMSampleBuffer, CMTime};
use objc2_core_video::{
    CVPixelBufferGetBaseAddressOfPlane, CVPixelBufferGetBytesPerRowOfPlane, CVPixelBufferGetHeight,
    CVPixelBufferGetWidth, CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags,
    CVPixelBufferUnlockBaseAddress,
};
use objc2_foundation::{NSArray, NSError, NSObject, NSObjectProtocol};
use objc2_screen_capture_kit::{
    SCContentFilter, SCStream, SCStreamConfiguration, SCStreamOutput, SCStreamOutputType,
};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::time::Duration;

/// '420v': NV12, video range — what H.264 encoders eat.
const NV12: u32 = 0x3432_3076;

/// One picture as I420 (Y plane, then U, then V), even width and height.
pub struct Yuv {
    pub data: Vec<u8>,
    pub w: u32,
    pub h: u32,
}

struct Ivars {
    tx: SyncSender<Yuv>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "DeskSpanFrameSink"]
    #[ivars = Ivars]
    struct FrameSink;

    unsafe impl NSObjectProtocol for FrameSink {}

    unsafe impl SCStreamOutput for FrameSink {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        fn stream_did_output(
            &self,
            _stream: &SCStream,
            sample: &CMSampleBuffer,
            kind: SCStreamOutputType,
        ) {
            if kind != SCStreamOutputType::Screen {
                return;
            }
            // Idle/blank status frames carry no picture.
            if let Some(frame) = unsafe { to_i420(sample) } {
                // Full: the encoder is busy, this frame is skipped.
                let _ = self.ivars().tx.try_send(frame);
            }
        }
    }
);

impl FrameSink {
    fn new(tx: SyncSender<Yuv>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(Ivars { tx });
        unsafe { msg_send![super(this), init] }
    }
}

/// A running stream of one display. Dropping it stops the stream.
pub struct Capture {
    stream: Retained<SCStream>,
    config: Retained<SCStreamConfiguration>,
    _sink: Retained<FrameSink>,
    _queue: DispatchRetained<DispatchQueue>,
    rx: Receiver<Yuv>,
}

impl Capture {
    /// Stream display `display_id` at `w`×`h` pixels, at most `fps` frames a second.
    pub fn start(display_id: u32, w: u32, h: u32, fps: u32) -> Result<Capture, String> {
        let display = crate::sck::find_display(display_id)?;
        let filter = unsafe {
            SCContentFilter::initWithDisplay_excludingWindows(
                SCContentFilter::alloc(),
                &display,
                &NSArray::new(),
            )
        };
        let config = unsafe { SCStreamConfiguration::new() };
        unsafe {
            config.setWidth(w as usize);
            config.setHeight(h as usize);
            config.setShowsCursor(true);
            config.setPixelFormat(NV12);
            config.setMinimumFrameInterval(CMTime::new(1, fps.max(1) as i32));
            config.setQueueDepth(5);
        }
        // Two waiting frames at most: a busy encoder gets the newest.
        let (tx, rx) = mpsc::sync_channel(2);
        let sink = FrameSink::new(tx);
        let queue = DispatchQueue::new("app.deskspan.capture", None);
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
                SCStreamOutputType::Screen,
                Some(&*queue),
            )
        }
        .map_err(|e| format!("cannot stream the screen: {}", e.localizedDescription()))?;
        wait_for(|handler| unsafe { stream.startCaptureWithCompletionHandler(Some(handler)) })
            .map_err(|e| format!("macOS didn't start the screen stream: {e}"))?;
        Ok(Capture {
            stream,
            config,
            _sink: sink,
            _queue: queue,
            rx,
        })
    }

    /// The next new frame, or None if the screen didn't change in `wait`.
    pub fn next(&self, wait: Duration) -> Option<Yuv> {
        let mut frame = self.rx.recv_timeout(wait).ok()?;
        // Skip to the newest if more piled up.
        while let Ok(newer) = self.rx.try_recv() {
            frame = newer;
        }
        Some(frame)
    }

    /// The display changed size: stream it at the new size.
    pub fn resize(&self, w: u32, h: u32) -> Result<(), String> {
        unsafe {
            self.config.setWidth(w as usize);
            self.config.setHeight(h as usize);
        }
        wait_for(|handler| unsafe {
            self.stream
                .updateConfiguration_completionHandler(&self.config, Some(handler))
        })
        .map_err(|e| format!("cannot resize the screen stream: {e}"))
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        let _ = wait_for(|handler| unsafe {
            self.stream.stopCaptureWithCompletionHandler(Some(handler))
        });
    }
}

/// Call an API that reports back through a completion block, and wait.
pub(crate) fn wait_for(
    call: impl FnOnce(&block2::DynBlock<dyn Fn(*mut NSError)>),
) -> Result<(), String> {
    let (tx, rx) = mpsc::channel::<Result<(), String>>();
    let handler = RcBlock::new(move |error: *mut NSError| {
        let result = match unsafe { error.as_ref() } {
            Some(e) => Err(e.localizedDescription().to_string()),
            None => Ok(()),
        };
        let _ = tx.send(result);
    });
    call(&handler);
    rx.recv_timeout(Duration::from_secs(10))
        .map_err(|_| "no answer from macOS".to_string())?
}

/// NV12 sample → I420 copy (even size). None for status-only samples.
unsafe fn to_i420(sample: &CMSampleBuffer) -> Option<Yuv> {
    let buffer = sample.image_buffer()?;
    let pb = &*buffer;
    if CVPixelBufferLockBaseAddress(pb, CVPixelBufferLockFlags::ReadOnly) != 0 {
        return None;
    }
    let w = CVPixelBufferGetWidth(pb) & !1;
    let h = CVPixelBufferGetHeight(pb) & !1;
    let y_src = CVPixelBufferGetBaseAddressOfPlane(pb, 0) as *const u8;
    let uv_src = CVPixelBufferGetBaseAddressOfPlane(pb, 1) as *const u8;
    let y_stride = CVPixelBufferGetBytesPerRowOfPlane(pb, 0);
    let uv_stride = CVPixelBufferGetBytesPerRowOfPlane(pb, 1);
    let frame = if w == 0 || h == 0 || y_src.is_null() || uv_src.is_null() {
        None
    } else {
        let mut data = vec![0u8; w * h * 3 / 2];
        let (y_plane, uv) = data.split_at_mut(w * h);
        let (u_plane, v_plane) = uv.split_at_mut(w * h / 4);
        for row in 0..h {
            let src = std::slice::from_raw_parts(y_src.add(row * y_stride), w);
            y_plane[row * w..(row + 1) * w].copy_from_slice(src);
        }
        let cw = w / 2;
        for row in 0..h / 2 {
            let src = std::slice::from_raw_parts(uv_src.add(row * uv_stride), w);
            for (i, pair) in src.as_chunks::<2>().0.iter().enumerate() {
                u_plane[row * cw + i] = pair[0];
                v_plane[row * cw + i] = pair[1];
            }
        }
        Some(Yuv {
            data,
            w: w as u32,
            h: h as u32,
        })
    };
    CVPixelBufferUnlockBaseAddress(pb, CVPixelBufferLockFlags::ReadOnly);
    frame
}
