//! Video on Windows through Media Foundation: the system's own decoders
//! and encoders (hardware where there is one), so no codec ships with
//! PilotHouse. A source reader reads frames, converted to RGB32 by its
//! video processor, and sound as PCM; a sink writer writes H.264 or HEVC
//! (from NV12 frames) with AAC sound into MP4.
//!
//! Media Foundation pads frames to its block size (a 1080p H.264 video
//! decodes as 1088 rows): only the display area its media type names is
//! kept. A phone's recording carries its turn as metadata; frames are
//! turned upright here. Media Foundation itself is loaded when first used
//! (see `ph_preview::video::mf`).

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context as _, Result, anyhow, bail};
use image::RgbaImage;
use ph_preview::video::mf::{Mf, mf};
use windows::Win32::Media::MediaFoundation::{
    CODECAPI_AVEncMPVGOPSize, IMF2DBuffer2, IMFActivate, IMFAttributes, IMFMediaBuffer,
    IMFMediaSink, IMFMediaType, IMFSample, IMFSinkWriter, IMFSourceReader, MF_E_INVALIDMEDIATYPE,
    MF_E_TOPO_CODEC_NOT_FOUND, MF_E_UNSUPPORTED_BYTESTREAM_TYPE, MF_MPEG4SINK_MOOV_BEFORE_MDAT,
    MF_MT_AAC_AUDIO_PROFILE_LEVEL_INDICATION, MF_MT_ALL_SAMPLES_INDEPENDENT,
    MF_MT_AUDIO_AVG_BYTES_PER_SECOND, MF_MT_AUDIO_BITS_PER_SAMPLE, MF_MT_AUDIO_BLOCK_ALIGNMENT,
    MF_MT_AUDIO_CHANNEL_MASK, MF_MT_AUDIO_NUM_CHANNELS, MF_MT_AUDIO_SAMPLES_PER_SECOND,
    MF_MT_AVG_BITRATE, MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE,
    MF_MT_GEOMETRIC_APERTURE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE,
    MF_MT_MINIMUM_DISPLAY_APERTURE, MF_MT_MPEG2_PROFILE, MF_MT_PAN_SCAN_APERTURE,
    MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE, MF_MT_VIDEO_NOMINAL_RANGE, MF_MT_VIDEO_ROTATION,
    MF_MT_YUV_MATRIX, MF_PD_DURATION, MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS,
    MF_SINK_WRITER_MEDIASINK, MF_SOURCE_READER_ALL_STREAMS,
    MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING,
    MF_SOURCE_READER_FIRST_AUDIO_STREAM, MF_SOURCE_READER_FIRST_VIDEO_STREAM,
    MF_SOURCE_READER_MEDIASOURCE, MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED,
    MF_SOURCE_READERF_ENDOFSTREAM, MF_SOURCE_READERF_ERROR, MF_TRANSCODE_CONTAINERTYPE, MF_VERSION,
    MF2DBuffer_LockFlags_Read, MFAudioFormat_AAC, MFAudioFormat_Float, MFAudioFormat_PCM,
    MFMediaType_Audio, MFMediaType_Video, MFNominalRange_16_235, MFSTARTUP_NOSOCKET,
    MFT_CATEGORY_VIDEO_ENCODER, MFT_REGISTER_TYPE_INFO, MFTranscodeContainerType_MPEG4,
    MFVideoFormat_H264, MFVideoFormat_HEVC, MFVideoFormat_NV12, MFVideoFormat_RGB32,
    MFVideoInterlace_Progressive, MFVideoTransferMatrix_BT601, MFVideoTransferMatrix_BT709,
    eAVEncH264VProfile_High,
};
use windows::Win32::System::Com::{
    COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemFree, CoUninitialize,
};
use windows::core::{GUID, HSTRING, Interface, PCWSTR, PROPVARIANT};

use super::frames::{Raw, Seeked};
use super::media::{self, Codec, Decoder, Encoder, Info, Source, Target};
use super::sound::{Layout, PcmReader, Track, rate_for};
use super::yuv;
use crate::edit::Orientation;

/// Media Foundation's times are in 100 ns units.
const UNITS: f64 = 10_000_000.;
const FIRST_VIDEO: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
const FIRST_AUDIO: u32 = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
const ALL_STREAMS: u32 = MF_SOURCE_READER_ALL_STREAMS.0 as u32;
const MEDIA_SOURCE: u32 = MF_SOURCE_READER_MEDIASOURCE.0 as u32;
/// Reads in a row that give nothing (stream ticks) before giving up.
const EMPTY_READS: usize = 10_000;
/// AAC at 128 kbit/s.
const AAC_BYTES_PER_SECOND: u32 = 16_000;

/// COM and Media Foundation, started on this thread while it is kept. It
/// is the last field of whatever holds Media Foundation objects, so they
/// are released before it stops.
struct Session {
    mf: &'static Mf,
    com: bool,
}

impl Session {
    fn start() -> Result<Session> {
        let mf = mf().map_err(|err| anyhow!(err))?;
        // SAFETY: COM and Media Foundation start-up, undone in Drop. A
        // thread set up for COM another way keeps it (that is no error).
        unsafe {
            let com = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
            if let Err(err) = (mf.startup)(MF_VERSION, MFSTARTUP_NOSOCKET).ok() {
                if com {
                    CoUninitialize();
                }
                bail!("Media Foundation could not start: {}", err.message());
            }
            Ok(Session { mf, com })
        }
    }

    fn attributes(&self, count: u32) -> Result<IMFAttributes> {
        let mut raw = std::ptr::null_mut();
        unsafe { (self.mf.create_attributes)(&mut raw, count) }.ok()?;
        owned(raw)
    }

    fn media_type(&self) -> Result<IMFMediaType> {
        let mut raw = std::ptr::null_mut();
        unsafe { (self.mf.create_media_type)(&mut raw) }.ok()?;
        owned(raw)
    }

    fn sample(&self) -> Result<IMFSample> {
        let mut raw = std::ptr::null_mut();
        unsafe { (self.mf.create_sample)(&mut raw) }.ok()?;
        owned(raw)
    }

    /// A sample holding `bytes`, from second `time` for `duration`.
    fn sample_of(
        &self,
        bytes: usize,
        time: i64,
        duration: i64,
        fill: impl FnOnce(&mut [u8]),
    ) -> Result<IMFSample> {
        let length = u32::try_from(bytes).context("a frame too large")?;
        let mut raw = std::ptr::null_mut();
        unsafe { (self.mf.create_memory_buffer)(length, &mut raw) }.ok()?;
        let buffer: IMFMediaBuffer = owned(raw)?;
        unsafe {
            let (mut data, mut room) = (std::ptr::null_mut(), 0u32);
            buffer.Lock(&mut data, Some(&mut room), None)?;
            if data.is_null() || room < length {
                let _ = buffer.Unlock();
                bail!("Media Foundation gave too little memory");
            }
            fill(std::slice::from_raw_parts_mut(data, bytes));
            buffer.Unlock()?;
            buffer.SetCurrentLength(length)?;
        }
        let sample = self.sample()?;
        unsafe {
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(time)?;
            sample.SetSampleDuration(duration)?;
        }
        Ok(sample)
    }

    /// A source reader for `path`; with `processing`, one that converts
    /// frames (advanced: also from 10-bit, scaled and deinterlaced).
    fn reader(&self, path: &Path, processing: Option<bool>) -> Result<IMFSourceReader> {
        let attributes = self.attributes(1)?;
        unsafe {
            match processing {
                Some(true) => {
                    attributes.SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1)?
                }
                Some(false) => {
                    attributes.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)?
                }
                None => {}
            }
        }
        let url = HSTRING::from(path.as_os_str());
        let mut raw = std::ptr::null_mut();
        unsafe {
            (self.mf.create_source_reader_from_url)(
                PCWSTR(url.as_ptr()),
                attributes.as_raw(),
                &mut raw,
            )
        }
        .ok()
        .map_err(|err| failed("Windows cannot open it", err))?;
        owned(raw)
    }

    /// A reader of the first video stream giving RGB32 frames, and what
    /// they are like. Fails when Windows has no decoder for it.
    fn video_reader(&self, path: &Path) -> Result<(IMFSourceReader, Format)> {
        let mut error = None;
        // The advanced processor where it can, else the basic one.
        for advanced in [true, false] {
            let reader = self.reader(path, Some(advanced))?;
            unsafe {
                reader.SetStreamSelection(ALL_STREAMS, false)?;
                reader
                    .SetStreamSelection(FIRST_VIDEO, true)
                    .map_err(|_| anyhow!("Windows finds no video in it"))?;
            }
            let output = self.media_type()?;
            unsafe {
                output.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
                output.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
            }
            match unsafe { reader.SetCurrentMediaType(FIRST_VIDEO, None, &output) } {
                Ok(()) => {
                    let format = Format::read(&reader)?;
                    return Ok((reader, format));
                }
                Err(err) => error = Some(err),
            }
        }
        Err(failed(
            "Windows cannot decode this video",
            error.unwrap_or_else(windows::core::Error::empty),
        ))
    }

    fn writer(&self, path: &Path, attributes: &IMFAttributes) -> Result<IMFSinkWriter> {
        let url = HSTRING::from(path.as_os_str());
        let mut raw = std::ptr::null_mut();
        unsafe {
            (self.mf.create_sink_writer_from_url)(
                PCWSTR(url.as_ptr()),
                std::ptr::null_mut(),
                attributes.as_raw(),
                &mut raw,
            )
        }
        .ok()
        .map_err(|err| failed("Windows cannot write the file", err))?;
        owned(raw)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // SAFETY: undoes `start`, after everything made under it is gone.
        unsafe {
            let _ = (self.mf.shutdown)();
            if self.com {
                CoUninitialize();
            }
        }
    }
}

/// Takes over a reference Media Foundation handed out.
fn owned<T: Interface>(raw: *mut c_void) -> Result<T> {
    if raw.is_null() {
        bail!("Media Foundation gave nothing back");
    }
    // SAFETY: a reference to a `T`, given to us to release.
    Ok(unsafe { T::from_raw(raw) })
}

/// `what` failed, said for people.
fn failed(what: &str, err: windows::core::Error) -> anyhow::Error {
    let code = err.code();
    let reason = if code == MF_E_UNSUPPORTED_BYTESTREAM_TYPE {
        "it is a kind of file Windows does not read".to_string()
    } else if code == MF_E_TOPO_CODEC_NOT_FOUND || code == MF_E_INVALIDMEDIATYPE {
        "Windows has no codec for it".to_string()
    } else {
        let message = err.message();
        let message = message.trim();
        if message.is_empty() {
            format!("error {:#010x}", code.0)
        } else {
            message.to_string()
        }
    };
    anyhow!("{what}: {reason}")
}

fn pack(high: u32, low: u32) -> u64 {
    (high as u64) << 32 | low as u64
}

/// A size or ratio stored as two halves of a number.
fn pair(media: &IMFMediaType, key: &GUID) -> Option<(u32, u32)> {
    let value = unsafe { media.GetUINT64(key) }.ok()?;
    Some(((value >> 32) as u32, value as u32))
}

/// The display area a media type names (x, y, width, height), if any.
fn aperture(media: &IMFMediaType) -> Option<(u32, u32, u32, u32)> {
    for key in [
        MF_MT_MINIMUM_DISPLAY_APERTURE,
        MF_MT_GEOMETRIC_APERTURE,
        MF_MT_PAN_SCAN_APERTURE,
    ] {
        // An MFVideoArea: two MFOffsets ({fract u16, value i16}), then a
        // SIZE ({cx i32, cy i32}).
        let mut area = [0u8; 16];
        let mut size = 0u32;
        if unsafe { media.GetBlob(&key, &mut area, Some(&mut size)) }.is_err() || size < 16 {
            continue;
        }
        let x = i16::from_le_bytes([area[2], area[3]]).max(0) as u32;
        let y = i16::from_le_bytes([area[6], area[7]]).max(0) as u32;
        let width = i32::from_le_bytes([area[8], area[9], area[10], area[11]]);
        let height = i32::from_le_bytes([area[12], area[13], area[14], area[15]]);
        if width > 0 && height > 0 {
            return Some((x, y, width as u32, height as u32));
        }
    }
    None
}

/// What the decoded frames are like.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Format {
    width: u32,
    height: u32,
    /// Bytes from one row to the next when the buffer gives no pitch
    /// (negative: rows bottom up).
    stride: i32,
    /// The part shown: x, y, width, height.
    area: (u32, u32, u32, u32),
}

impl Format {
    fn read(reader: &IMFSourceReader) -> Result<Format> {
        let current = unsafe { reader.GetCurrentMediaType(FIRST_VIDEO) }?;
        let (width, height) = pair(&current, &MF_MT_FRAME_SIZE)
            .filter(|(width, height)| *width > 0 && *height > 0)
            .context("Windows gives no frame size")?;
        let stride = unsafe { current.GetUINT32(&MF_MT_DEFAULT_STRIDE) }
            .map(|stride| stride as i32)
            .ok()
            .filter(|stride| stride.unsigned_abs() >= width * 4)
            .unwrap_or(width as i32 * 4);
        let native = unsafe { reader.GetNativeMediaType(FIRST_VIDEO, 0) }.ok();
        let area = aperture(&current)
            .or_else(|| native.as_ref().and_then(aperture))
            .map(|(x, y, w, h)| {
                let (x, y) = (x.min(width - 1), y.min(height - 1));
                (x, y, w.min(width - x), h.min(height - y))
            })
            .unwrap_or((0, 0, width, height));
        Ok(Format {
            width,
            height,
            stride,
            area,
        })
    }

    fn shown(&self) -> (u32, u32) {
        (self.area.2, self.area.3)
    }
}

/// The shown part of `sample` as RGBA (from BGRX).
fn pixels(sample: &IMFSample, format: &Format) -> Result<RgbaImage> {
    let buffer = unsafe { sample.ConvertToContiguousBuffer() }?;
    // SAFETY: every row read is checked to lie in the locked memory.
    unsafe {
        // The 2D lock gives the top row and the real pitch.
        if let Ok(planar) = buffer.cast::<IMF2DBuffer2>() {
            let (mut top, mut pitch, mut start, mut length) =
                (std::ptr::null_mut(), 0i32, std::ptr::null_mut(), 0u32);
            if planar
                .Lock2DSize(
                    MF2DBuffer_LockFlags_Read,
                    &mut top,
                    &mut pitch,
                    &mut start,
                    &mut length,
                )
                .is_ok()
            {
                let image = copy_area(top, pitch as isize, start, length as usize, format);
                let _ = planar.Unlock2D();
                return image;
            }
        }
        let (mut data, mut length) = (std::ptr::null_mut(), 0u32);
        buffer.Lock(&mut data, None, Some(&mut length))?;
        let stride = format.stride as isize;
        let top = if stride < 0 {
            data.offset(-stride * (format.height as isize - 1))
        } else {
            data
        };
        let image = copy_area(top, stride, data, length as usize, format);
        let _ = buffer.Unlock();
        image
    }
}

/// Copies the shown area of a locked BGRX frame whose first row starts at
/// `top`, rows `pitch` bytes apart, all within `length` bytes from `start`.
unsafe fn copy_area(
    top: *const u8,
    pitch: isize,
    start: *const u8,
    length: usize,
    format: &Format,
) -> Result<RgbaImage> {
    let (x, y, width, height) = format.area;
    if top.is_null() || start.is_null() || pitch.unsigned_abs() < format.width as usize * 4 {
        bail!("Media Foundation gave an unreadable frame");
    }
    let (low, high) = (start as usize, start as usize + length);
    let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
    for row in y..y + height {
        let line = (top as isize + row as isize * pitch + x as isize * 4) as usize;
        if line < low || line + width as usize * 4 > high {
            bail!("Media Foundation gave a frame smaller than it said");
        }
        // SAFETY: inside the locked memory, as just checked.
        let line = unsafe { std::slice::from_raw_parts(line as *const u8, width as usize * 4) };
        for bgrx in line.as_chunks::<4>().0 {
            pixels.extend_from_slice(&[bgrx[2], bgrx[1], bgrx[0], 255]);
        }
    }
    RgbaImage::from_raw(width, height, pixels).context("a frame")
}

fn upright(image: RgbaImage, turns: u8) -> RgbaImage {
    if turns == 0 {
        return image;
    }
    crate::render::orient(
        &image,
        Orientation {
            turns,
            flipped: false,
        },
    )
    .into_owned()
}

fn seek(reader: &IMFSourceReader, seconds: f64) -> Result<()> {
    let position = PROPVARIANT::from((seconds.max(0.) * UNITS).round() as i64);
    unsafe { reader.SetCurrentPosition(&GUID::zeroed(), &position) }
        .map_err(|err| failed("Windows cannot go to that time", err))
}

pub fn probe(path: &Path) -> Result<Info> {
    let session = Session::start()?;
    // Opening it to decode checks Windows has a decoder for it; the first
    // frame says its real size (decoders often change it once they start).
    let (reader, format) = session.video_reader(path)?;
    let mut reading = Reading {
        reader,
        format,
        ended: false,
    };
    let format = match reading.read()? {
        Some(first) => first.frame.1,
        None => bail!("Windows gives no frame of it"),
    };
    let reader = reading.reader;
    let native = unsafe { reader.GetNativeMediaType(FIRST_VIDEO, 0) }?;
    let duration = unsafe { reader.GetPresentationAttribute(MEDIA_SOURCE, &MF_PD_DURATION) }
        .ok()
        .and_then(|value| u64::try_from(&value).ok())
        .map(|units| units as f64 / UNITS)
        .filter(|duration| duration.is_finite() && *duration > 0.)
        .context("Windows cannot tell how long it is")?;
    // Stored turned this far counter-clockwise: turned back clockwise.
    let rotation = unsafe { native.GetUINT32(&MF_MT_VIDEO_ROTATION) }.unwrap_or(0);
    let mut turns = ((rotation / 90) % 4) as u8;
    let (width, height) = format.shown();
    let stored = aperture(&native)
        .map(|area| (area.2, area.3))
        .or_else(|| pair(&native, &MF_MT_FRAME_SIZE))
        .unwrap_or((width, height));
    // Should the processor have turned it already, it is not turned again.
    if turns % 2 == 1 && width != height && (width, height) == (stored.1, stored.0) {
        turns = 0;
    }
    let fps = pair(&native, &MF_MT_FRAME_RATE)
        .filter(|(_, den)| *den > 0)
        .map(|(num, den)| num as f64 / den as f64)
        .filter(|fps| *fps > 0. && *fps < 1000.)
        .unwrap_or(30.);
    let audio = unsafe { reader.GetNativeMediaType(FIRST_AUDIO, 0) }.is_ok();
    drop((native, reader));
    drop(session);
    // Warms what the export dialog asks.
    let _ = writes_hevc();
    Ok(Info {
        duration,
        size: if turns % 2 == 1 {
            (height, width)
        } else {
            (width, height)
        },
        fps,
        audio,
        source: Source::Mf { turns },
    })
}

/// A frame's sample as read, and how it is laid out.
type Read = (IMFSample, Format);

/// The reading side of a decoder.
struct Reading {
    reader: IMFSourceReader,
    format: Format,
    ended: bool,
}

impl Reading {
    fn read(&mut self) -> Result<Option<Raw<Read>>> {
        if self.ended {
            return Ok(None);
        }
        for _ in 0..EMPTY_READS {
            let (mut flags, mut time, mut sample) = (0u32, 0i64, None::<IMFSample>);
            unsafe {
                self.reader.ReadSample(
                    FIRST_VIDEO,
                    0,
                    None,
                    Some(&mut flags),
                    Some(&mut time),
                    Some(&mut sample),
                )
            }
            .map_err(|err| failed("reading the video failed", err))?;
            if flags & MF_SOURCE_READERF_ERROR.0 as u32 != 0 {
                bail!("reading the video failed");
            }
            if flags & MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED.0 as u32 != 0 {
                self.format = Format::read(&self.reader)?;
            }
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                self.ended = true;
            }
            if let Some(sample) = sample {
                let start = time as f64 / UNITS;
                let end = unsafe { sample.GetSampleDuration() }
                    .ok()
                    .filter(|duration| *duration > 0)
                    .map(|duration| start + duration as f64 / UNITS);
                return Ok(Some(Raw {
                    start,
                    end,
                    frame: (sample, self.format),
                }));
            }
            if self.ended {
                return Ok(None);
            }
        }
        bail!("Windows stopped giving frames")
    }
}

/// The frames of `from`..`to`, decoded, upright.
pub struct MfDecoder {
    reading: Reading,
    seeked: Seeked<Read>,
    turns: u8,
    _session: Session,
}

pub fn decoder(path: &Path, turns: u8, from: f64, to: f64) -> Result<MfDecoder> {
    let session = Session::start()?;
    let (reader, format) = session.video_reader(path)?;
    if from > 0. {
        seek(&reader, from)?;
    }
    Ok(MfDecoder {
        reading: Reading {
            reader,
            format,
            ended: false,
        },
        seeked: Seeked::new(from, to),
        turns,
        _session: session,
    })
}

impl Decoder for MfDecoder {
    fn next(&mut self) -> Result<Option<(f64, RgbaImage)>> {
        let reading = &mut self.reading;
        let Some((start, (sample, format))) = self.seeked.next(|| reading.read())? else {
            return Ok(None);
        };
        Ok(Some((
            start,
            upright(pixels(&sample, &format)?, self.turns),
        )))
    }
}

/// The frame shown at `t`.
pub fn frame_at(path: &Path, turns: u8, t: f64) -> Result<RgbaImage> {
    let mut decoder = decoder(path, turns, t, t)?;
    decoder
        .next()?
        .map(|(_, frame)| frame)
        .context("no frame at that time")
}

/// When each frame starts, read without decoding.
pub fn frame_times(path: &Path) -> Result<Vec<f64>> {
    let session = Session::start()?;
    let reader = session.reader(path, None)?;
    unsafe {
        reader.SetStreamSelection(ALL_STREAMS, false)?;
        reader.SetStreamSelection(FIRST_VIDEO, true)?;
    }
    let mut times = Vec::new();
    let mut empty = 0;
    loop {
        let (mut flags, mut time, mut sample) = (0u32, 0i64, None::<IMFSample>);
        unsafe {
            reader.ReadSample(
                FIRST_VIDEO,
                0,
                None,
                Some(&mut flags),
                Some(&mut time),
                Some(&mut sample),
            )
        }
        .map_err(|err| failed("reading the video failed", err))?;
        if flags & MF_SOURCE_READERF_ERROR.0 as u32 != 0 {
            bail!("reading the video failed");
        }
        if sample.is_some() {
            times.push(time as f64 / UNITS);
            empty = 0;
        } else {
            empty += 1;
        }
        if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 || empty > EMPTY_READS {
            break;
        }
    }
    drop(reader);
    times.sort_by(f64::total_cmp);
    times.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
    Ok(times)
}

/// Whether Windows has an HEVC encoder here (it comes with the HEVC Video
/// Extensions, or a graphics card that has one). Asked once.
pub fn writes_hevc() -> bool {
    static HEVC: OnceLock<bool> = OnceLock::new();
    *HEVC.get_or_init(|| has_encoder(&MFVideoFormat_HEVC).unwrap_or(false))
}

fn has_encoder(subtype: &GUID) -> Result<bool> {
    let session = Session::start()?;
    let output = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: *subtype,
    };
    // Synchronous, asynchronous and hardware ones, sorted.
    const FLAGS: u32 = 0x1 | 0x2 | 0x4 | 0x40;
    let (mut list, mut count) = (std::ptr::null_mut(), 0u32);
    unsafe {
        (session.mf.enum_transforms)(
            MFT_CATEGORY_VIDEO_ENCODER,
            FLAGS,
            std::ptr::null(),
            (&output as *const MFT_REGISTER_TYPE_INFO).cast(),
            &mut list,
            &mut count,
        )
        .ok()?;
        if !list.is_null() {
            // Each entry is a reference to release; the list is COM memory.
            for ix in 0..count as usize {
                let entry = *list.add(ix);
                if !entry.is_null() {
                    drop(IMFActivate::from_raw(entry));
                }
            }
            CoTaskMemFree(Some(list as *const c_void));
        }
    }
    Ok(count > 0)
}

/// The source's sound, decoded by Media Foundation.
struct MfPcm {
    reader: IMFSourceReader,
    layout: Layout,
    float: bool,
    ended: bool,
}

impl MfPcm {
    fn open(session: &Session, path: &Path) -> Result<MfPcm> {
        let reader = session.reader(path, None)?;
        unsafe {
            reader.SetStreamSelection(ALL_STREAMS, false)?;
            reader
                .SetStreamSelection(FIRST_AUDIO, true)
                .map_err(|_| anyhow!("Windows finds no sound in it"))?;
        }
        // 16-bit PCM, else floats.
        let mut set = None;
        for (subtype, bits) in [(MFAudioFormat_PCM, 16), (MFAudioFormat_Float, 32)] {
            let output = session.media_type()?;
            unsafe {
                output.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
                output.SetGUID(&MF_MT_SUBTYPE, &subtype)?;
                output.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, bits)?;
            }
            match unsafe { reader.SetCurrentMediaType(FIRST_AUDIO, None, &output) } {
                Ok(()) => {
                    set = Some(Ok(()));
                    break;
                }
                Err(err) => set = Some(Err(err)),
            }
        }
        if let Some(Err(err)) = set {
            return Err(failed("Windows cannot decode this video's sound", err));
        }
        let current = unsafe { reader.GetCurrentMediaType(FIRST_AUDIO) }?;
        let read = |key: &GUID| unsafe { current.GetUINT32(key) }.ok();
        let float = unsafe { current.GetGUID(&MF_MT_SUBTYPE) }? == MFAudioFormat_Float;
        let rate = read(&MF_MT_AUDIO_SAMPLES_PER_SECOND).unwrap_or(0);
        let channels = read(&MF_MT_AUDIO_NUM_CHANNELS).unwrap_or(0) as usize;
        let bits = read(&MF_MT_AUDIO_BITS_PER_SAMPLE).unwrap_or(0);
        if !(1..=384_000).contains(&rate)
            || !(1..=32).contains(&channels)
            || bits != if float { 32 } else { 16 }
        {
            bail!(
                "Windows decodes this video's sound in a form {} does not take",
                ph_brand::NAME
            );
        }
        Ok(MfPcm {
            reader,
            layout: Layout {
                rate,
                channels,
                mask: read(&MF_MT_AUDIO_CHANNEL_MASK),
            },
            float,
            ended: false,
        })
    }
}

impl PcmReader for MfPcm {
    fn layout(&self) -> Layout {
        self.layout
    }

    fn seek(&mut self, from: f64, _: f64) -> Result<()> {
        self.ended = false;
        seek(&self.reader, from)
    }

    fn read(&mut self) -> Result<Option<(f64, Vec<i16>)>> {
        if self.ended {
            return Ok(None);
        }
        for _ in 0..EMPTY_READS {
            let (mut flags, mut time, mut sample) = (0u32, 0i64, None::<IMFSample>);
            unsafe {
                self.reader.ReadSample(
                    FIRST_AUDIO,
                    0,
                    None,
                    Some(&mut flags),
                    Some(&mut time),
                    Some(&mut sample),
                )
            }
            .map_err(|err| failed("reading the sound failed", err))?;
            if flags & MF_SOURCE_READERF_ERROR.0 as u32 != 0 {
                bail!("reading the sound failed");
            }
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                self.ended = true;
            }
            if let Some(sample) = sample {
                let buffer = unsafe { sample.ConvertToContiguousBuffer() }?;
                let samples = unsafe {
                    let (mut data, mut length) = (std::ptr::null_mut(), 0u32);
                    buffer.Lock(&mut data, None, Some(&mut length))?;
                    let bytes = if data.is_null() {
                        &[][..]
                    } else {
                        std::slice::from_raw_parts(data, length as usize)
                    };
                    let samples: Vec<i16> = if self.float {
                        bytes
                            .as_chunks::<4>()
                            .0
                            .iter()
                            .map(|value| {
                                let value = f32::from_le_bytes(*value);
                                (value * 32767.).round().clamp(-32768., 32767.) as i16
                            })
                            .collect()
                    } else {
                        bytes
                            .as_chunks::<2>()
                            .0
                            .iter()
                            .map(|value| i16::from_le_bytes(*value))
                            .collect()
                    };
                    let _ = buffer.Unlock();
                    samples
                };
                return Ok(Some((time as f64 / UNITS, samples)));
            }
            if self.ended {
                return Ok(None);
            }
        }
        bail!("Windows stopped giving sound")
    }
}

/// Writes H.264 or HEVC (and AAC sound) with a sink writer.
pub struct MfEncoder {
    writer: Option<IMFSinkWriter>,
    video: u32,
    sound: Option<(u32, Track<MfPcm>)>,
    out: PathBuf,
    target: Target,
    hardware: bool,
    /// One frame's length, 100 ns units.
    frame: i64,
    frames: u64,
    /// How far ahead of the pictures sound is fed.
    lead: f64,
    session: Session,
}

/// Opens a sink writer for `target` at `out`: its video stream, and its
/// sound stream at `rate` when there is one.
fn open_writer(
    session: &Session,
    out: &Path,
    target: &Target,
    rate: Option<u32>,
    hardware: bool,
) -> Result<(IMFSinkWriter, u32, Option<u32>)> {
    let attributes = session.attributes(3)?;
    unsafe {
        attributes.SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &MFTranscodeContainerType_MPEG4)?;
        attributes.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, hardware as u32)?;
        // The index first, so a player can start before all of it came.
        attributes.SetUINT32(&MF_MPEG4SINK_MOOV_BEFORE_MDAT, 1)?;
    }
    let writer = session.writer(out, &attributes)?;
    // A writer set up only partly still holds the file: shut it, so the
    // next try (or the caller removing it) can have it.
    match add_streams(session, &writer, target, rate) {
        Ok((video, audio)) => Ok((writer, video, audio)),
        Err(err) => {
            close(&writer);
            Err(err)
        }
    }
}

/// Adds the video stream (and the sound's at `rate`) to `writer` and
/// starts it: the streams' indexes.
fn add_streams(
    session: &Session,
    writer: &IMFSinkWriter,
    target: &Target,
    rate: Option<u32>,
) -> Result<(u32, Option<u32>)> {
    let hevc = target.codec == Codec::Hevc;
    let (width, height) = target.size;
    let (num, den) = media::rational(target.fps);
    let matrix = if yuv::is_hd(height) {
        MFVideoTransferMatrix_BT709
    } else {
        MFVideoTransferMatrix_BT601
    };
    let video_type = |subtype: &GUID| -> Result<IMFMediaType> {
        let media = session.media_type()?;
        unsafe {
            media.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            media.SetGUID(&MF_MT_SUBTYPE, subtype)?;
            media.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            media.SetUINT64(&MF_MT_FRAME_SIZE, pack(width, height))?;
            media.SetUINT64(&MF_MT_FRAME_RATE, pack(num, den))?;
            media.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
            media.SetUINT32(&MF_MT_YUV_MATRIX, matrix.0 as u32)?;
            media.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32)?;
        }
        Ok(media)
    };
    let codec_name = if hevc { "HEVC" } else { "H.264" };
    let output = video_type(if hevc {
        &MFVideoFormat_HEVC
    } else {
        &MFVideoFormat_H264
    })?;
    unsafe {
        output.SetUINT32(&MF_MT_AVG_BITRATE, target.bitrate.max(1))?;
        if !hevc {
            output.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_High.0 as u32)?;
        }
    }
    let video = unsafe { writer.AddStream(&output) }
        .map_err(|err| failed(&format!("Windows cannot write {codec_name}"), err))?;
    let input = video_type(&MFVideoFormat_NV12)?;
    unsafe {
        input.SetUINT32(&MF_MT_DEFAULT_STRIDE, width)?;
        input.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)?;
    }
    // A key frame every two seconds, for seeking; encoders that do not take
    // the setting choose their own.
    let parameters = session.attributes(1)?;
    unsafe {
        parameters.SetUINT32(
            &CODECAPI_AVEncMPVGOPSize,
            ((target.fps * 2.).round() as u32).max(1),
        )?;
        if writer
            .SetInputMediaType(video, &input, &parameters)
            .is_err()
        {
            writer
                .SetInputMediaType(video, &input, None::<&IMFAttributes>)
                .map_err(|err| {
                    let what = if hevc {
                        "Windows has no HEVC encoder here (it comes with the HEVC Video \
                         Extensions, or a graphics card that has one)"
                            .to_string()
                    } else {
                        format!("Windows cannot write {codec_name} at {width}×{height}")
                    };
                    failed(&what, err)
                })?;
        }
    }
    let audio = match rate {
        Some(rate) => {
            let audio_type = |subtype: &GUID| -> Result<IMFMediaType> {
                let media = session.media_type()?;
                unsafe {
                    media.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
                    media.SetGUID(&MF_MT_SUBTYPE, subtype)?;
                    media.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
                    media.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, rate)?;
                    media.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2)?;
                }
                Ok(media)
            };
            let aac = audio_type(&MFAudioFormat_AAC)?;
            let pcm = audio_type(&MFAudioFormat_PCM)?;
            unsafe {
                aac.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, AAC_BYTES_PER_SECOND)?;
                aac.SetUINT32(&MF_MT_AAC_AUDIO_PROFILE_LEVEL_INDICATION, 0x29)?;
                pcm.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 4)?;
                pcm.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, rate * 4)?;
                pcm.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)?;
            }
            let stream = unsafe { writer.AddStream(&aac) }
                .map_err(|err| failed("Windows cannot write AAC sound", err))?;
            unsafe { writer.SetInputMediaType(stream, &pcm, None::<&IMFAttributes>) }
                .map_err(|err| failed("Windows cannot write AAC sound", err))?;
            Some(stream)
        }
        None => None,
    };
    unsafe { writer.BeginWriting() }
        .map_err(|err| failed(&format!("Windows cannot write {codec_name}"), err))?;
    Ok((video, audio))
}

/// Shuts the writer's file, written or not (releasing a writer alone may
/// keep it open a while, and the caller removes a broken one).
fn close(writer: &IMFSinkWriter) {
    let mut raw = std::ptr::null_mut();
    unsafe {
        let found = writer.GetServiceForStream(
            MF_SINK_WRITER_MEDIASINK.0,
            &GUID::zeroed(),
            &IMFMediaSink::IID,
            &mut raw,
        );
        if found.is_ok()
            && let Ok(sink) = owned::<IMFMediaSink>(raw)
        {
            let _ = sink.Shutdown();
        }
    }
}

pub fn encoder(source: &Path, out: &Path, target: &Target) -> Result<MfEncoder> {
    let session = Session::start()?;
    let sound = if target.audio.is_empty() {
        None
    } else {
        let reader = MfPcm::open(&session, source)?;
        let rate = rate_for(reader.layout.rate);
        Some(Track::new(
            reader,
            target.audio.clone(),
            target.seconds,
            rate,
        ))
    };
    let rate = sound.as_ref().map(|track| track.rate());
    // Hardware where the driver's encoder takes this; Windows' own if not.
    let (writer, video, audio, hardware) = match open_writer(&session, out, target, rate, true) {
        Ok((writer, video, audio)) => (writer, video, audio, true),
        Err(_) => {
            let (writer, video, audio) = open_writer(&session, out, target, rate, false)?;
            (writer, video, audio, false)
        }
    };
    Ok(MfEncoder {
        writer: Some(writer),
        video,
        sound: audio.zip(sound),
        out: out.to_path_buf(),
        target: target.clone(),
        hardware,
        frame: (UNITS / target.fps.max(1.)).round() as i64,
        frames: 0,
        lead: (2. / target.fps.max(1.)).max(0.5),
        session,
    })
}

impl MfEncoder {
    fn writer(&self) -> Result<&IMFSinkWriter> {
        self.writer.as_ref().context("the video is already written")
    }

    /// Opens the file again without hardware (nothing is written yet).
    fn reopen_without_hardware(&mut self) -> Result<()> {
        if let Some(writer) = self.writer.take() {
            close(&writer);
        }
        let rate = self.sound.as_ref().map(|(_, track)| track.rate());
        let (writer, video, audio) =
            open_writer(&self.session, &self.out, &self.target, rate, false)?;
        self.writer = Some(writer);
        self.video = video;
        if let (Some((stream, _)), Some(audio)) = (&mut self.sound, audio) {
            *stream = audio;
        }
        self.hardware = false;
        Ok(())
    }

    /// Writes the result's sound up to second `until`.
    fn feed(&mut self, until: f64) -> Result<()> {
        let Self {
            writer,
            sound,
            session,
            ..
        } = self;
        let (Some(writer), Some((stream, track))) = (writer.as_ref(), sound.as_mut()) else {
            return Ok(());
        };
        let rate = track.rate() as u64;
        while let Some(block) = track.next(until)? {
            if block.is_empty() {
                continue;
            }
            let frames = block.len() as u64 / 2;
            let first = track.given() - frames;
            let units = |frames: u64| (frames * UNITS as u64 / rate) as i64;
            let sample = session.sample_of(
                block.len() * 2,
                units(first),
                units(first + frames) - units(first),
                |bytes| {
                    for (to, from) in bytes.as_chunks_mut::<2>().0.iter_mut().zip(&block) {
                        to.copy_from_slice(&from.to_le_bytes());
                    }
                },
            )?;
            unsafe { writer.WriteSample(*stream, &sample) }
                .map_err(|err| failed("writing the sound failed", err))?;
        }
        Ok(())
    }
}

impl Encoder for MfEncoder {
    fn push(&mut self, frame: &RgbaImage, time: f64) -> Result<()> {
        if frame.dimensions() != self.target.size {
            bail!("a frame did not fit the encoder");
        }
        let (width, height) = self.target.size;
        let sample = self.session.sample_of(
            yuv::nv12_len(width, height),
            (time * UNITS).round() as i64,
            self.frame,
            |bytes| yuv::nv12(frame, bytes),
        )?;
        let written = unsafe { self.writer()?.WriteSample(self.video, &sample) };
        if let Err(err) = written {
            // Some drivers' encoders take the setup but not the frames:
            // then Windows' own, from the start.
            if self.frames == 0 && self.hardware {
                self.reopen_without_hardware()?;
                unsafe { self.writer()?.WriteSample(self.video, &sample) }
                    .map_err(|err| failed("writing a frame failed", err))?;
            } else {
                return Err(failed("writing a frame failed", err));
            }
        }
        self.frames += 1;
        self.feed(time + self.lead)
    }

    fn finish(mut self: Box<Self>) -> Result<()> {
        self.feed(f64::INFINITY)?;
        let writer = self.writer.take().context("the video is already written")?;
        let finished = unsafe { writer.Finalize() };
        if finished.is_err() {
            close(&writer);
        }
        finished.map_err(|err| failed("finishing the video failed", err))
    }

    fn abort(self: Box<Self>) {
        // Drop shuts the file.
    }
}

impl Drop for MfEncoder {
    fn drop(&mut self) {
        if let Some(writer) = self.writer.take() {
            close(&writer);
        }
    }
}
