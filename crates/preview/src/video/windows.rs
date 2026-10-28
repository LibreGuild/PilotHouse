//! Video on Windows: the Media Foundation media engine decodes and plays
//! (sound included) with the codecs Windows has; each new frame is copied
//! into a bitmap PilotHouse draws.

use std::path::Path;
use std::sync::Arc;

use gpui_kit::RenderImage;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppBGRA, IWICImagingFactory, WICBitmapCacheOnLoad,
};
use windows::Win32::Media::MediaFoundation::{
    CLSID_MFMediaEngineClassFactory, IMFAttributes, IMFMediaEngine, IMFMediaEngineClassFactory,
    IMFMediaEngineNotify, IMFMediaEngineNotify_Impl, MF_MEDIA_ENGINE_CALLBACK,
    MF_MEDIA_ENGINE_READY_HAVE_METADATA, MF_MEDIA_ENGINE_VIDEO_OUTPUT_FORMAT, MF_VERSION,
    MFCreateAttributes, MFSTARTUP_FULL, MFShutdown, MFStartup,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::core::{BSTR, implement};

use super::{Frame, Status};

/// The engine asks to be told of events; the player reads its state
/// instead, so they are let go.
#[implement(IMFMediaEngineNotify)]
struct Quiet;

impl IMFMediaEngineNotify_Impl for Quiet_Impl {
    fn EventNotify(&self, _: u32, _: usize, _: u32) -> windows::core::Result<()> {
        Ok(())
    }
}

fn failed(err: windows::core::Error) -> String {
    format!("Windows could not open the video: {err}")
}

pub struct Engine {
    engine: IMFMediaEngine,
    images: IWICImagingFactory,
    /// The time of the last frame given, so a paused video's picture is
    /// copied once, not at every check.
    shown: std::cell::Cell<Option<i64>>,
}

impl Engine {
    pub fn open(path: &Path) -> Result<Self, String> {
        // SAFETY: Media Foundation and WIC as documented; the engine is used
        // from the thread that made it.
        unsafe {
            // The UI thread may already be set up for COM; that is fine.
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            MFStartup(MF_VERSION, MFSTARTUP_FULL).map_err(failed)?;
            let engine = Self::create(path);
            if engine.is_err() {
                let _ = MFShutdown();
            }
            engine
        }
    }

    /// The engine, once Media Foundation has started.
    unsafe fn create(path: &Path) -> Result<Self, String> {
        // SAFETY: as in `open`.
        unsafe {
            let factory: IMFMediaEngineClassFactory =
                CoCreateInstance(&CLSID_MFMediaEngineClassFactory, None, CLSCTX_INPROC_SERVER)
                    .map_err(failed)?;
            let mut attributes: Option<IMFAttributes> = None;
            MFCreateAttributes(&mut attributes, 2).map_err(failed)?;
            let attributes = attributes.ok_or("Windows could not open the video.")?;
            let notify: IMFMediaEngineNotify = Quiet.into();
            attributes
                .SetUnknown(&MF_MEDIA_ENGINE_CALLBACK, &notify)
                .map_err(failed)?;
            attributes
                .SetUINT32(
                    &MF_MEDIA_ENGINE_VIDEO_OUTPUT_FORMAT,
                    DXGI_FORMAT_B8G8R8A8_UNORM.0 as u32,
                )
                .map_err(failed)?;
            let engine = factory.CreateInstance(0, &attributes).map_err(failed)?;
            engine
                .SetSource(&BSTR::from(path.to_string_lossy().as_ref()))
                .map_err(failed)?;
            let images: IWICImagingFactory =
                CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
                    .map_err(failed)?;
            Ok(Self {
                engine,
                images,
                shown: None.into(),
            })
        }
    }

    pub fn status(&self) -> Status {
        unsafe {
            if let Ok(error) = self.engine.GetError() {
                return Status::Failed(format!(
                    "Windows has no codec for it, or the file is damaged (error {}).",
                    error.GetErrorCode()
                ));
            }
            if self.engine.GetReadyState() >= MF_MEDIA_ENGINE_READY_HAVE_METADATA.0 as u16 {
                Status::Ready
            } else {
                Status::Loading
            }
        }
    }

    pub fn duration(&self) -> Option<f64> {
        let seconds = unsafe { self.engine.GetDuration() };
        (seconds.is_finite() && seconds > 0.).then_some(seconds)
    }

    pub fn position(&self) -> f64 {
        unsafe { self.engine.GetCurrentTime() }.max(0.)
    }

    pub fn size(&self) -> Option<(f32, f32)> {
        let (mut width, mut height) = (0u32, 0u32);
        unsafe {
            self.engine
                .GetNativeVideoSize(Some(&mut width), Some(&mut height))
                .ok()?;
        }
        (width > 0 && height > 0).then_some((width as f32, height as f32))
    }

    pub fn play(&self, rate: f32) {
        unsafe {
            let _ = self.engine.SetPlaybackRate(rate as f64);
            let _ = self.engine.Play();
        }
    }

    pub fn pause(&self) {
        unsafe {
            let _ = self.engine.Pause();
        }
    }

    pub fn is_playing(&self) -> bool {
        unsafe { !self.engine.IsPaused().as_bool() && !self.engine.IsEnded().as_bool() }
    }

    pub fn set_rate(&self, rate: f32) {
        unsafe {
            let _ = self.engine.SetPlaybackRate(rate as f64);
        }
    }

    pub fn seek(&self, seconds: f64) {
        self.shown.set(None);
        unsafe {
            let _ = self.engine.SetCurrentTime(seconds.max(0.));
        }
    }

    pub fn set_muted(&self, muted: bool) {
        unsafe {
            let _ = self.engine.SetMuted(muted);
        }
    }

    /// The newest frame, when the engine has one it has not given yet.
    pub fn frame(&self) -> Option<Frame> {
        let (width, height) = self.size()?;
        let (width, height) = (width as u32, height as u32);
        unsafe {
            // The frame's time; the same as last time means no new frame.
            let time = self.engine.OnVideoStreamTick().ok()?;
            if self.shown.get() == Some(time) {
                return None;
            }
            let bitmap = self
                .images
                .CreateBitmap(
                    width,
                    height,
                    &GUID_WICPixelFormat32bppBGRA,
                    WICBitmapCacheOnLoad,
                )
                .ok()?;
            let target = RECT {
                left: 0,
                top: 0,
                right: width as i32,
                bottom: height as i32,
            };
            self.engine
                .TransferVideoFrame(&bitmap, None, &target, None)
                .ok()?;
            let stride = width * 4;
            let mut pixels = vec![0u8; (stride * height) as usize];
            bitmap
                .CopyPixels(std::ptr::null(), stride, &mut pixels)
                .ok()?;
            // Video is opaque, whatever the alpha bytes say.
            for alpha in pixels.iter_mut().skip(3).step_by(4) {
                *alpha = 255;
            }
            let buffer = ::image::RgbaImage::from_raw(width, height, pixels)?;
            self.shown.set(Some(time));
            Some(Frame::Image(Arc::new(RenderImage::new(vec![
                ::image::Frame::new(buffer),
            ]))))
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        unsafe {
            let _ = self.engine.Shutdown();
            // Each open started Media Foundation once.
            let _ = MFShutdown();
        }
    }
}
