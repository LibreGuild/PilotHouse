//! Video on macOS: AVFoundation decodes and plays (sound included), and
//! hands each frame over as a pixel buffer GPUI draws without copying.

use std::path::Path;
use std::ptr::NonNull;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AnyThread as _, MainThreadMarker};
use objc2_av_foundation::{AVPlayer, AVPlayerItem, AVPlayerItemStatus, AVPlayerItemVideoOutput};
use objc2_core_foundation::{CFRetained, CFString};
use objc2_core_media::CMTime;
use objc2_core_video::{
    CVPixelBuffer, kCVPixelBufferIOSurfacePropertiesKey, kCVPixelBufferMetalCompatibilityKey,
    kCVPixelBufferPixelFormatTypeKey,
};
use objc2_foundation::{NSDictionary, NSNumber, NSString, NSURL};

use super::{Frame, Status};

/// Bi-planar 4:2:0 full range ('420f'): the format GPUI's surfaces draw.
const PIXEL_FORMAT: u32 = 0x3432_3066;

pub struct Engine {
    player: Retained<AVPlayer>,
    item: Retained<AVPlayerItem>,
    output: Retained<AVPlayerItemVideoOutput>,
}

/// A Core Foundation string constant as the Foundation string it is
/// toll-free bridged to.
fn ns_key(key: &'static CFString) -> &'static NSString {
    // SAFETY: CFString and NSString are toll-free bridged.
    unsafe { &*(key as *const CFString as *const NSString) }
}

impl Engine {
    pub fn open(path: &Path) -> Result<Self, String> {
        let main = MainThreadMarker::new().ok_or("Video opens on the main thread.")?;
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        let keys: [&NSString; 3] = unsafe {
            [
                ns_key(kCVPixelBufferPixelFormatTypeKey),
                ns_key(kCVPixelBufferMetalCompatibilityKey),
                ns_key(kCVPixelBufferIOSurfacePropertiesKey),
            ]
        };
        let values: [Retained<AnyObject>; 3] = [
            NSNumber::new_u32(PIXEL_FORMAT).into(),
            NSNumber::new_bool(true).into(),
            NSDictionary::<NSString, AnyObject>::new().into(),
        ];
        let attributes = NSDictionary::from_retained_objects(&keys, &values);
        // SAFETY: AVFoundation's documented calls, on the main thread.
        unsafe {
            let item = AVPlayerItem::playerItemWithURL(&url, main);
            let output = AVPlayerItemVideoOutput::initWithPixelBufferAttributes(
                AVPlayerItemVideoOutput::alloc(),
                Some(&attributes),
            );
            item.addOutput(&output);
            let player = AVPlayer::playerWithPlayerItem(Some(&item), main);
            Ok(Self {
                player,
                item,
                output,
            })
        }
    }

    pub fn status(&self) -> Status {
        // SAFETY: plain property reads.
        unsafe {
            match self.item.status() {
                AVPlayerItemStatus::ReadyToPlay => Status::Ready,
                AVPlayerItemStatus::Failed => Status::Failed(
                    self.item
                        .error()
                        .map(|err| err.localizedDescription().to_string())
                        .unwrap_or_else(|| "The video cannot be played.".into()),
                ),
                _ => Status::Loading,
            }
        }
    }

    /// Seconds, once known.
    pub fn duration(&self) -> Option<f64> {
        let seconds = unsafe { self.item.duration().seconds() };
        (seconds.is_finite() && seconds > 0.).then_some(seconds)
    }

    pub fn position(&self) -> f64 {
        let seconds = unsafe { self.player.currentTime().seconds() };
        if seconds.is_finite() {
            seconds.max(0.)
        } else {
            0.
        }
    }

    /// The picture's size in pixels, once known (none for sound alone).
    pub fn size(&self) -> Option<(f32, f32)> {
        let size = unsafe { self.item.presentationSize() };
        (size.width > 0. && size.height > 0.).then_some((size.width as f32, size.height as f32))
    }

    pub fn play(&self, rate: f32) {
        unsafe {
            self.player.play();
            self.player.setRate(rate);
        }
    }

    pub fn pause(&self) {
        unsafe { self.player.pause() }
    }

    pub fn is_playing(&self) -> bool {
        unsafe { self.player.rate() != 0. }
    }

    pub fn set_rate(&self, rate: f32) {
        if self.is_playing() {
            unsafe { self.player.setRate(rate) }
        }
    }

    pub fn seek(&self, seconds: f64) {
        unsafe {
            let time = CMTime::with_seconds(seconds.max(0.), 600);
            let zero = CMTime::with_seconds(0., 600);
            self.player
                .seekToTime_toleranceBefore_toleranceAfter(time, zero, zero);
        }
    }

    pub fn set_muted(&self, muted: bool) {
        unsafe { self.player.setMuted(muted) }
    }

    /// The frame for now, when it is a new one.
    pub fn frame(&self) -> Option<Frame> {
        unsafe {
            let now = self.player.currentTime();
            if !self.output.hasNewPixelBufferForItemTime(now) {
                return None;
            }
            let buffer = self
                .output
                .copyPixelBufferForItemTime_itemTimeForDisplay(now, std::ptr::null_mut())?;
            // The copy is ours (a +1 reference): hand it to a CFRetained.
            let raw = NonNull::new(Retained::into_raw(buffer).cast::<CVPixelBuffer>())?;
            Some(Frame::Surface(gpui_kit::SurfaceSource::from(
                CFRetained::from_raw(raw),
            )))
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        unsafe {
            self.player.pause();
            self.player.replaceCurrentItemWithPlayerItem(None);
        }
    }
}
