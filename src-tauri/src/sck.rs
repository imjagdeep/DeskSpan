//! macOS screen pictures with ScreenCaptureKit (macOS 14+). Same as
//! DeskShot's sck.rs, returning raw RGBA.
//!
//! xcap uses CGWindowListCreateImage, which Apple has deprecated and which
//! stopped returning pictures on macOS 27. On macOS 14 and later the picture
//! comes from ScreenCaptureKit instead; older systems keep using xcap.
//! (A continuous SCStream is the faster way; that's milestone 2.)

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyClass;
use objc2::AnyThread;
use objc2_core_graphics::{CGDataProvider, CGImage};
use objc2_foundation::{NSArray, NSError};
use objc2_screen_capture_kit::{
    SCContentFilter, SCDisplay, SCScreenshotManager, SCShareableContent, SCStreamConfiguration,
};
use std::sync::mpsc;
use std::time::Duration;

/// Longer than this means something is stuck (e.g. a permission prompt).
const WAIT: Duration = Duration::from_secs(10);

/// True when this Mac has the screenshot API (macOS 14+).
pub fn available() -> bool {
    AnyClass::get(c"SCScreenshotManager").is_some()
}

/// Picture of the display with this CGDirectDisplayID, `w`×`h` pixels.
pub fn capture_display(display_id: u32, w: u32, h: u32) -> Result<Rgba, String> {
    let display = find_display(display_id)?;
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
        // A screen for someone to work on: the cursor belongs in it.
        config.setShowsCursor(true);
    }

    let (tx, rx) = mpsc::channel::<Result<Rgba, String>>();
    let handler = RcBlock::new(move |image: *mut CGImage, error: *mut NSError| {
        let result = match unsafe { image.as_ref() } {
            Some(img) => to_rgba(img),
            None => Err(describe(error, "macOS returned no picture")),
        };
        let _ = tx.send(result);
    });
    unsafe {
        SCScreenshotManager::captureImageWithFilter_configuration_completionHandler(
            &filter,
            &config,
            Some(&*handler),
        );
    }
    rx.recv_timeout(WAIT)
        .map_err(|_| "macOS didn't hand over the screen picture in time".to_string())?
}

/// The main display (for the sound capture, which needs one to attach to).
pub fn main_display() -> Result<Retained<SCDisplay>, String> {
    find_display(objc2_core_graphics::CGMainDisplayID())
}

pub fn find_display(display_id: u32) -> Result<Retained<SCDisplay>, String> {
    let (tx, rx) = mpsc::channel::<Result<Retained<SCDisplay>, String>>();
    let handler = RcBlock::new(
        move |content: *mut SCShareableContent, error: *mut NSError| {
            let result = match unsafe { content.as_ref() } {
            Some(c) => {
                let displays = unsafe { c.displays() };
                displays
                    .iter()
                    .find(|d| unsafe { d.displayID() } == display_id)
                    .ok_or_else(|| "that screen isn't available to capture".to_string())
            }
            // Usually: Screen Recording permission is off for DeskSpan.
            None => Err(describe(
                error,
                "macOS didn't allow the capture; turn on DeskSpan in System Settings › Privacy & Security › Screen Recording",
            )),
        };
            let _ = tx.send(result);
        },
    );
    unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&handler) };
    rx.recv_timeout(WAIT)
        .map_err(|_| "macOS didn't list the screens in time".to_string())?
}

fn describe(error: *mut NSError, fallback: &str) -> String {
    match unsafe { error.as_ref() } {
        Some(e) => format!("{fallback} ({})", e.localizedDescription()),
        None => fallback.to_string(),
    }
}

/// RGBA pixels, width, height.
pub type Rgba = (Vec<u8>, u32, u32);

/// CGImage (BGRA, possibly padded rows) → RGBA. Same steps as xcap's own
/// macOS capture.
fn to_rgba(img: &CGImage) -> Result<Rgba, String> {
    let width = CGImage::width(Some(img));
    let height = CGImage::height(Some(img));
    let bytes_per_row = CGImage::bytes_per_row(Some(img));
    let provider = CGImage::data_provider(Some(img));
    let data = CGDataProvider::data(provider.as_deref())
        .ok_or("cannot read the screen picture")?
        .to_vec();
    if width == 0 || height == 0 || bytes_per_row < width * 4 {
        return Err("the screen picture is empty".into());
    }
    let mut buffer = Vec::with_capacity(width * height * 4);
    for row in data.chunks_exact(bytes_per_row).take(height) {
        buffer.extend_from_slice(&row[..width * 4]);
    }
    for bgra in buffer.as_chunks_mut::<4>().0 {
        bgra.swap(0, 2);
    }
    if buffer.len() != width * height * 4 {
        return Err("the screen picture has the wrong size".into());
    }
    Ok((buffer, width as u32, height as u32))
}
