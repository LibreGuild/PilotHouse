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
    IMFMediaEngineNotify, IMFMediaEngineNotify_Impl, IMFSourceReader, MF_MEDIA_ENGINE_CALLBACK,
    MF_MEDIA_ENGINE_READY_HAVE_METADATA, MF_MEDIA_ENGINE_VIDEO_OUTPUT_FORMAT, MF_MT_FRAME_SIZE,
    MF_MT_VIDEO_ROTATION, MF_SOURCE_READER_FIRST_VIDEO_STREAM, MF_VERSION, MFSTARTUP_FULL,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::core::{BSTR, HSTRING, Interface as _, PCWSTR, implement};

use super::mf::{Mf, mf};
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
    mf: &'static Mf,
    engine: IMFMediaEngine,
    images: IWICImagingFactory,
    /// The time of the last frame given, so a paused video's picture is
    /// copied once, not at every check.
    shown: std::cell::Cell<Option<i64>>,
    /// Quarter turns clockwise that stand the stored frames upright (a
    /// phone's recording), and the stored size, to tell whether the engine
    /// turned them already.
    turns: u8,
    stored: Option<(u32, u32)>,
}

/// How a file's video is stored turned: quarter turns clockwise to stand
/// it upright, and its stored size. Read from its first video stream's
/// native type, as Studio's reader reads it.
unsafe fn stored_turn(mf: &Mf, path: &Path) -> (u8, Option<(u32, u32)>) {
    // SAFETY: Media Foundation as documented; the reader is let go here.
    unsafe {
        let url = HSTRING::from(path.as_os_str());
        let mut raw = std::ptr::null_mut();
        if (mf.create_source_reader_from_url)(PCWSTR(url.as_ptr()), std::ptr::null_mut(), &mut raw)
            .is_err()
            || raw.is_null()
        {
            return (0, None);
        }
        let reader = IMFSourceReader::from_raw(raw);
        let Ok(native) = reader.GetNativeMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, 0)
        else {
            return (0, None);
        };
        let rotation = native.GetUINT32(&MF_MT_VIDEO_ROTATION).unwrap_or(0);
        let stored = native
            .GetUINT64(&MF_MT_FRAME_SIZE)
            .ok()
            .map(|packed| ((packed >> 32) as u32, packed as u32));
        (((rotation / 90) % 4) as u8, stored)
    }
}

impl Engine {
    pub fn open(path: &Path) -> Result<Self, String> {
        // SAFETY: Media Foundation and WIC as documented; the engine is used
        // from the thread that made it.
        unsafe {
            let mf = mf()?;
            // The UI thread may already be set up for COM; that is fine.
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            (mf.startup)(MF_VERSION, MFSTARTUP_FULL)
                .ok()
                .map_err(failed)?;
            let engine = Self::create(mf, path);
            if engine.is_err() {
                let _ = (mf.shutdown)();
            }
            engine
        }
    }

    /// The engine, once Media Foundation has started.
    unsafe fn create(mf: &'static Mf, path: &Path) -> Result<Self, String> {
        // SAFETY: as in `open`.
        unsafe {
            let factory: IMFMediaEngineClassFactory =
                CoCreateInstance(&CLSID_MFMediaEngineClassFactory, None, CLSCTX_INPROC_SERVER)
                    .map_err(failed)?;
            let mut attributes = std::ptr::null_mut();
            (mf.create_attributes)(&mut attributes, 2)
                .ok()
                .map_err(failed)?;
            if attributes.is_null() {
                return Err("Windows could not open the video.".into());
            }
            let attributes = IMFAttributes::from_raw(attributes);
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
            let (turns, stored) = stored_turn(mf, path);
            Ok(Self {
                mf,
                engine,
                images,
                shown: None.into(),
                turns,
                stored,
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

    /// The frames' size as the engine gives them.
    fn engine_size(&self) -> Option<(u32, u32)> {
        let (mut width, mut height) = (0u32, 0u32);
        unsafe {
            self.engine
                .GetNativeVideoSize(Some(&mut width), Some(&mut height))
                .ok()?;
        }
        (width > 0 && height > 0).then_some((width, height))
    }

    /// Quarter turns still to make to frames the engine gives: none when
    /// it turned them itself (its size is the stored one turned).
    fn turns_left(&self, size: (u32, u32)) -> u8 {
        match self.stored {
            Some(stored)
                if self.turns % 2 == 1 && size.0 != size.1 && size == (stored.1, stored.0) =>
            {
                0
            }
            _ => self.turns,
        }
    }

    pub fn size(&self) -> Option<(f32, f32)> {
        let (width, height) = self.engine_size()?;
        Some(if self.turns_left((width, height)) % 2 == 1 {
            (height as f32, width as f32)
        } else {
            (width as f32, height as f32)
        })
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
        let (width, height) = self.engine_size()?;
        let turns = self.turns_left((width, height));
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
            let buffer = match turns {
                1 => ::image::imageops::rotate90(&buffer),
                2 => ::image::imageops::rotate180(&buffer),
                3 => ::image::imageops::rotate270(&buffer),
                _ => buffer,
            };
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
            let _ = (self.mf.shutdown)();
        }
    }
}
