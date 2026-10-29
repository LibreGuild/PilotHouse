//! H.264 decoding with Media Foundation's H.264 decoder (part of Windows),
//! from NV12 into BGRA.

use std::mem::ManuallyDrop;

use anyhow::{Context as _, Result};
use ph_media::Frame;
use windows::Win32::Media::MediaFoundation::{
    CLSID_MSH264DecoderMFT, IMFSample, IMFTransform, MF_E_NOTACCEPTING,
    MF_E_TRANSFORM_NEED_MORE_INPUT, MF_E_TRANSFORM_STREAM_CHANGE, MF_MT_DEFAULT_STRIDE,
    MF_MT_FRAME_SIZE, MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE, MF_VERSION, MFCreateMediaType,
    MFCreateMemoryBuffer, MFCreateSample, MFMediaType_Video, MFSTARTUP_NOSOCKET, MFShutdown,
    MFStartup, MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, MFT_MESSAGE_NOTIFY_START_OF_STREAM,
    MFT_OUTPUT_DATA_BUFFER, MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES,
    MFT_OUTPUT_STREAM_PROVIDES_SAMPLES, MFVideoFormat_H264, MFVideoFormat_NV12,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
};

pub struct Decoder {
    transform: IMFTransform,
    /// Width, height and row stride of the decoder's pictures.
    layout: Option<(u32, u32, u32)>,
    /// The next sample's time (the decoder wants them to increase).
    time: i64,
}

impl Decoder {
    pub fn new() -> Result<Self> {
        // SAFETY: COM and Media Foundation as documented; this thread (the
        // screen's) owns the decoder.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET).context("starting Media Foundation")?;
            match Self::create() {
                Ok(decoder) => Ok(decoder),
                Err(err) => {
                    let _ = MFShutdown();
                    Err(err)
                }
            }
        }
    }

    unsafe fn create() -> Result<Self> {
        // SAFETY: as in `new`.
        unsafe {
            let transform: IMFTransform =
                CoCreateInstance(&CLSID_MSH264DecoderMFT, None, CLSCTX_INPROC_SERVER)
                    .context("Windows has no H.264 decoder")?;
            let input = MFCreateMediaType()?;
            input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            input.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
            transform.SetInputType(0, &input, 0)?;
            let mut decoder = Self {
                transform,
                layout: None,
                time: 0,
            };
            decoder.choose_output()?;
            decoder
                .transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            decoder
                .transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
            Ok(decoder)
        }
    }

    /// Picks NV12 output, and reads its size (known once the stream's
    /// parameters are).
    fn choose_output(&mut self) -> Result<()> {
        // SAFETY: the transform's own output types.
        unsafe {
            let mut index = 0;
            loop {
                let kind = self
                    .transform
                    .GetOutputAvailableType(0, index)
                    .context("the decoder offers no NV12 output")?;
                if kind.GetGUID(&MF_MT_SUBTYPE)? == MFVideoFormat_NV12 {
                    self.transform.SetOutputType(0, &kind, 0)?;
                    self.layout = kind.GetUINT64(&MF_MT_FRAME_SIZE).ok().map(|size| {
                        let (width, height) = ((size >> 32) as u32, size as u32);
                        let stride = kind
                            .GetUINT32(&MF_MT_DEFAULT_STRIDE)
                            .map(|stride| (stride as i32).unsigned_abs())
                            .unwrap_or(width);
                        (width, height, stride.max(width))
                    });
                    return Ok(());
                }
                index += 1;
            }
        }
    }

    /// Decodes one picture's NAL units (no start codes).
    pub fn decode(&mut self, nals: &[Vec<u8>]) -> Result<Option<Frame>> {
        let mut annex_b = Vec::new();
        for nal in nals {
            annex_b.extend_from_slice(&[0, 0, 0, 1]);
            annex_b.extend_from_slice(nal);
        }
        if annex_b.is_empty() {
            return Ok(None);
        }
        // SAFETY: Media Foundation samples and buffers made and used here.
        unsafe {
            let sample = sample_of(&annex_b)?;
            sample.SetSampleTime(self.time)?;
            sample.SetSampleDuration(166_666)?;
            self.time += 166_666;
            let mut frame = None;
            if let Err(err) = self.transform.ProcessInput(0, &sample, 0) {
                if err.code() != MF_E_NOTACCEPTING {
                    return Err(err.into());
                }
                // Full: take its pictures, then it takes this one.
                frame = self.drain()?;
                self.transform.ProcessInput(0, &sample, 0)?;
            }
            Ok(self.drain()?.or(frame))
        }
    }

    /// Takes every picture the decoder has ready; the newest.
    unsafe fn drain(&mut self) -> Result<Option<Frame>> {
        let mut newest = None;
        // SAFETY: as in `decode`; each output buffer is taken back once.
        unsafe {
            loop {
                let info = self.transform.GetOutputStreamInfo(0)?;
                let provides = info.dwFlags
                    & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0
                        | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0) as u32
                    != 0;
                let given = if provides {
                    None
                } else {
                    Some(sample_of(&vec![0u8; info.cbSize.max(1) as usize])?)
                };
                let mut output = [MFT_OUTPUT_DATA_BUFFER {
                    dwStreamID: 0,
                    pSample: ManuallyDrop::new(given),
                    dwStatus: 0,
                    pEvents: ManuallyDrop::new(None),
                }];
                let mut status = 0;
                let result = self.transform.ProcessOutput(0, &mut output, &mut status);
                let sample = ManuallyDrop::take(&mut output[0].pSample);
                drop(ManuallyDrop::take(&mut output[0].pEvents));
                match result {
                    Ok(()) => {
                        if let (Some(sample), Some(layout)) = (sample, self.layout) {
                            newest = Some(to_frame(&sample, layout)?);
                        }
                    }
                    Err(err) if err.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(newest),
                    Err(err) if err.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                        self.choose_output()?
                    }
                    Err(err) => return Err(err.into()),
                }
            }
        }
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        // SAFETY: pairs the `MFStartup` in `new`.
        unsafe {
            let _ = MFShutdown();
        }
    }
}

/// A sample holding a copy of `bytes`.
unsafe fn sample_of(bytes: &[u8]) -> Result<IMFSample> {
    // SAFETY: the buffer is locked for the copy and unlocked after.
    unsafe {
        let buffer = MFCreateMemoryBuffer(bytes.len() as u32)?;
        let mut data = std::ptr::null_mut();
        buffer.Lock(&mut data, None, None)?;
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), data, bytes.len());
        buffer.Unlock()?;
        buffer.SetCurrentLength(bytes.len() as u32)?;
        let sample = MFCreateSample()?;
        sample.AddBuffer(&buffer)?;
        Ok(sample)
    }
}

/// An NV12 picture as BGRA (BT.601, video range).
unsafe fn to_frame(sample: &IMFSample, (width, height, stride): (u32, u32, u32)) -> Result<Frame> {
    // SAFETY: the buffer is locked while read and unlocked after.
    unsafe {
        let buffer = sample.ConvertToContiguousBuffer()?;
        let mut data = std::ptr::null_mut();
        let mut length = 0;
        buffer.Lock(&mut data, None, Some(&mut length))?;
        let bytes = std::slice::from_raw_parts(data, length as usize);
        let frame = nv12_to_bgra(bytes, width as usize, height as usize, stride as usize);
        buffer.Unlock()?;
        frame.context("the decoder's picture is smaller than its size")
    }
}

fn nv12_to_bgra(nv12: &[u8], width: usize, height: usize, stride: usize) -> Option<Frame> {
    let chroma = stride * height;
    if nv12.len() < chroma + stride * height.div_ceil(2) {
        return None;
    }
    let mut pixels = vec![255u8; width * height * 4];
    for y in 0..height {
        let luma = &nv12[y * stride..y * stride + width];
        let uv = &nv12[chroma + (y / 2) * stride..];
        for x in 0..width {
            let c = i32::from(luma[x]) - 16;
            let d = i32::from(uv[x & !1]) - 128;
            let e = i32::from(uv[(x & !1) + 1]) - 128;
            let clamp = |value: i32| (value >> 8).clamp(0, 255) as u8;
            let out = (y * width + x) * 4;
            pixels[out] = clamp(298 * c + 516 * d + 128);
            pixels[out + 1] = clamp(298 * c - 100 * d - 208 * e + 128);
            pixels[out + 2] = clamp(298 * c + 409 * e + 128);
        }
    }
    Some(Frame::from_bgra(width as u32, height as u32, pixels))
}
