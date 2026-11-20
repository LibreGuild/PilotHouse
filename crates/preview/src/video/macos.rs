//! Video on macOS: AVFoundation decodes and plays (sound included), and
//! hands each frame over as a pixel buffer GPUI draws without copying.

use std::path::Path;
use std::ptr::NonNull;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AnyThread as _, MainThreadMarker};
use objc2_av_foundation::{
    AVAsset, AVMediaTypeVideo, AVPlayer, AVPlayerItem, AVPlayerItemStatus, AVPlayerItemVideoOutput,
    AVURLAsset, AVVideoComposition,
};
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

/// What opening reads from the file, which can take a while (a slow or
/// network disk, a file not downloaded yet): made on any thread by
/// [`Engine::prepare`], then given to [`Engine::open_prepared`] on the main
/// one.
pub struct Prepared {
    asset: Retained<AVURLAsset>,
    /// The video is stored turned (a phone's recording).
    turned: bool,
}

/// Whether the asset's video is stored turned. Waits until the file's
/// tracks are read.
#[allow(deprecated)]
unsafe fn stored_turned(asset: &AVAsset) -> bool {
    // SAFETY: AVFoundation's documented calls, which may be made on any
    // thread.
    unsafe {
        let Some(video) = AVMediaTypeVideo else {
            return false;
        };
        let Some(track) = asset.tracksWithMediaType(video).firstObject() else {
            return false;
        };
        let t = track.preferredTransform();
        let identity = (t.a - 1.).abs() < 1e-6
            && t.b.abs() < 1e-6
            && t.c.abs() < 1e-6
            && (t.d - 1.).abs() < 1e-6;
        // What standing it upright needs is read here too, not on the
        // main thread later.
        if !identity {
            _ = asset.duration();
        }
        !identity
    }
}

impl Engine {
    /// Opens the video, waiting for its file to be read: [`Engine::prepare`]
    /// off the main thread and then [`Engine::open_prepared`] do not hold
    /// the window up.
    pub fn open(path: &Path) -> Result<Self, String> {
        Self::open_prepared(Self::prepare(path))
    }

    /// Reads what opening needs from the file. On any thread.
    pub fn prepare(path: &Path) -> Prepared {
        // A pool of its own: the thread this runs on may have none.
        objc2::rc::autoreleasepool(|_| {
            let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
            // SAFETY: AVFoundation's documented calls; an asset is made and
            // read on any thread.
            unsafe {
                let asset = AVURLAsset::URLAssetWithURL_options(&url, None);
                let turned = stored_turned(&asset);
                Prepared { asset, turned }
            }
        })
    }

    /// The player for a video [`Engine::prepare`] has read. On the main
    /// thread.
    pub fn open_prepared(prepared: Prepared) -> Result<Self, String> {
        let main = MainThreadMarker::new().ok_or("Video opens on the main thread.")?;
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
            let item = AVPlayerItem::playerItemWithAsset(&prepared.asset, main);
            let output = AVPlayerItemVideoOutput::initWithPixelBufferAttributes(
                AVPlayerItemVideoOutput::alloc(),
                Some(&attributes),
            );
            item.addOutput(&output);
            // A video recorded turned (a phone's) plays upright: its frames
            // are handed over as they are stored unless a composition turns
            // them. The asset's tracks are read already, so this does not
            // wait for the file.
            if prepared.turned {
                #[allow(deprecated)]
                let composition =
                    AVVideoComposition::videoCompositionWithPropertiesOfAsset(&prepared.asset);
                item.setVideoComposition(Some(&composition));
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_is_read_off_the_main_thread() {
        // Tests run off the main thread, as the player's reading does: no
        // call in it may be one of the main thread's. A file that is not
        // there has no video to turn.
        let prepared = Engine::prepare(Path::new("/nonexistent/ph-preview-video.mov"));
        assert!(!prepared.turned);
    }
}
