//! Video on macOS through AVFoundation: the system's own decoders and
//! encoders (hardware where there is one), so no codec ships with
//! PilotHouse. AVAssetReader reads frames and sound, AVAssetWriter writes
//! H.264 or HEVC with AAC sound.
//!
//! Frames come as BGRA pixel buffers, turned upright here (a phone's
//! recording carries its turn as metadata), and leave as BGRA buffers from
//! the writer's pool. The result's sound is made by `sound::Track` from the
//! source's, decoded to PCM, and handed over in blocks: the writer packs
//! the sound it is given end to end, so silence between kept parts has to
//! be written too.

use std::path::Path;
use std::ptr::NonNull;
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use image::RgbaImage;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_av_foundation::{
    AVAsset, AVAssetReader, AVAssetReaderStatus, AVAssetReaderTrackOutput, AVAssetTrack,
    AVAssetWriter, AVAssetWriterInput, AVAssetWriterInputPixelBufferAdaptor, AVAssetWriterStatus,
    AVFileTypeMPEG4, AVMediaTypeAudio, AVMediaTypeVideo, AVURLAsset, AVVideoAverageBitRateKey,
    AVVideoCodecKey, AVVideoCodecTypeH264, AVVideoCodecTypeHEVC, AVVideoCompressionPropertiesKey,
    AVVideoExpectedSourceFrameRateKey, AVVideoHeightKey, AVVideoMaxKeyFrameIntervalKey,
    AVVideoWidthKey,
};
use objc2_core_audio_types::{
    AudioStreamBasicDescription, kAudioFormatFlagIsPacked, kAudioFormatFlagIsSignedInteger,
    kAudioFormatLinearPCM,
};
use objc2_core_foundation::CFRetained;
use objc2_core_media::{
    CMAudioFormatDescriptionCreate, CMAudioSampleBufferCreateReadyWithPacketDescriptions,
    CMBlockBuffer, CMFormatDescription, CMSampleBuffer, CMTime, CMTimeRange,
    kCMBlockBufferAssureMemoryNowFlag,
};
use objc2_core_video::{
    CVPixelBuffer, CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow,
    CVPixelBufferGetHeight, CVPixelBufferGetWidth, CVPixelBufferLockBaseAddress,
    CVPixelBufferLockFlags, CVPixelBufferPool, CVPixelBufferUnlockBaseAddress,
    kCVPixelBufferHeightKey, kCVPixelBufferPixelFormatTypeKey, kCVPixelBufferWidthKey,
};
use objc2_foundation::{NSCopying as _, NSDictionary, NSNumber, NSString, NSURL};

use super::media::{Codec, Decoder, Encoder, Info, Source, Target};
use super::sound::{Layout, PcmReader, Track};
use crate::edit::Orientation;

/// 'BGRA': the pixel format read and written.
const BGRA: u32 = 0x4247_5241;
/// 'lpcm' and 'aac ' audio formats.
const LINEAR_PCM: u32 = 0x6C70_636D;
const AAC: u32 = 0x6161_6320;
const SAMPLE_RATE: f64 = 44_100.;
/// Times are written in this many parts of a second.
const TIMESCALE: i32 = 90_000;

fn url(path: &Path) -> Retained<NSURL> {
    NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()))
}

fn cm(seconds: f64) -> CMTime {
    unsafe { CMTime::with_seconds(seconds, TIMESCALE) }
}

fn seconds(time: CMTime) -> f64 {
    unsafe { time.seconds() }
}

/// A Core Foundation string constant as the Foundation string it is
/// toll-free bridged to.
fn ns_key(key: &'static objc2_core_foundation::CFString) -> &'static NSString {
    // SAFETY: CFString and NSString are toll-free bridged.
    unsafe { &*(key as *const objc2_core_foundation::CFString as *const NSString) }
}

/// A settings dictionary from key and value pairs.
fn settings(
    pairs: Vec<(Retained<NSString>, Retained<AnyObject>)>,
) -> Retained<NSDictionary<NSString, AnyObject>> {
    let (keys, values): (Vec<_>, Vec<_>) = pairs.into_iter().unzip();
    let keys: Vec<&NSString> = keys.iter().map(|key| &**key).collect();
    NSDictionary::from_retained_objects(&keys, &values)
}

fn key(name: &'static NSString) -> Retained<NSString> {
    name.copy()
}

/// An AVFoundation audio settings key, by its documented name (they live in
/// AVFAudio, whose value is the name).
fn audio_key(name: &str) -> Retained<NSString> {
    NSString::from_str(name)
}

fn number(value: f64) -> Retained<AnyObject> {
    NSNumber::new_f64(value).into()
}

fn int(value: i64) -> Retained<AnyObject> {
    NSNumber::new_i64(value).into()
}

fn asset(path: &Path) -> Retained<AVURLAsset> {
    unsafe { AVURLAsset::URLAssetWithURL_options(&url(path), None) }
}

#[allow(deprecated)]
fn track(asset: &AVAsset, video: bool) -> Option<Retained<AVAssetTrack>> {
    // SAFETY: the media types are AVFoundation's constants.
    unsafe {
        let media = if video {
            AVMediaTypeVideo
        } else {
            AVMediaTypeAudio
        }?;
        asset.tracksWithMediaType(media).firstObject()
    }
}

/// The quarter turns a track's transform asks for.
fn turns(track: &AVAssetTrack) -> u8 {
    let transform = unsafe { track.preferredTransform() };
    match (transform.a.round() as i32, transform.b.round() as i32) {
        (0, 1) => 1,
        (-1, 0) => 2,
        (0, -1) => 3,
        _ => 0,
    }
}

pub fn probe(path: &Path) -> Result<Info> {
    let asset = asset(path);
    let video = track(&asset, true).context("AVFoundation finds no video in it")?;
    let duration = seconds(unsafe { asset.duration() });
    if !(duration.is_finite() && duration > 0.) {
        bail!("AVFoundation cannot read how long it is");
    }
    let natural = unsafe { video.naturalSize() };
    let turns = turns(&video);
    let (width, height) = (natural.width.round() as u32, natural.height.round() as u32);
    let size = if turns % 2 == 1 {
        (height, width)
    } else {
        (width, height)
    };
    let fps = unsafe { video.nominalFrameRate() } as f64;
    Ok(Info {
        duration,
        size,
        fps: if fps > 0. { fps } else { 30. },
        audio: track(&asset, false).is_some(),
        source: Source::Av { turns },
    })
}

/// Reads `track` from `from` for `length` seconds, as `output` settings say
/// (`None`: as stored, undecoded).
fn reader(
    asset: &AVAsset,
    track: &AVAssetTrack,
    output: Option<&NSDictionary<NSString, AnyObject>>,
    from: f64,
    length: f64,
) -> Result<(Retained<AVAssetReader>, Retained<AVAssetReaderTrackOutput>)> {
    unsafe {
        let reader = AVAssetReader::assetReaderWithAsset_error(asset)
            .map_err(|err| anyhow!("{}", err.localizedDescription()))?;
        reader.setTimeRange(CMTimeRange {
            start: cm(from.max(0.)),
            duration: cm(length.max(0.001)),
        });
        let output =
            AVAssetReaderTrackOutput::assetReaderTrackOutputWithTrack_outputSettings(track, output);
        output.setAlwaysCopiesSampleData(false);
        if !reader.canAddOutput(&output) {
            bail!("AVFoundation cannot read this video");
        }
        reader.addOutput(&output);
        if !reader.startReading() {
            let reason = reader
                .error()
                .map(|err| err.localizedDescription().to_string())
                .unwrap_or_default();
            bail!("AVFoundation cannot read this video: {reason}");
        }
        Ok((reader, output))
    }
}

fn bgra_output() -> Retained<NSDictionary<NSString, AnyObject>> {
    settings(vec![(
        ns_key(unsafe { kCVPixelBufferPixelFormatTypeKey }).copy(),
        NSNumber::new_u32(BGRA).into(),
    )])
}

/// A BGRA pixel buffer's pixels as RGBA.
fn read_pixels(buffer: &CVPixelBuffer) -> Option<RgbaImage> {
    unsafe {
        CVPixelBufferLockBaseAddress(buffer, CVPixelBufferLockFlags::ReadOnly);
        let (width, height) = (
            CVPixelBufferGetWidth(buffer),
            CVPixelBufferGetHeight(buffer),
        );
        let stride = CVPixelBufferGetBytesPerRow(buffer);
        let base = CVPixelBufferGetBaseAddress(buffer) as *const u8;
        let image = (!base.is_null()).then(|| {
            let mut pixels = Vec::with_capacity(width * height * 4);
            for row in 0..height {
                let line = std::slice::from_raw_parts(base.add(row * stride), width * 4);
                for bgra in line.as_chunks::<4>().0 {
                    pixels.extend_from_slice(&[bgra[2], bgra[1], bgra[0], bgra[3]]);
                }
            }
            RgbaImage::from_raw(width as u32, height as u32, pixels)
        });
        CVPixelBufferUnlockBaseAddress(buffer, CVPixelBufferLockFlags::ReadOnly);
        image.flatten()
    }
}

/// Writes `image` (RGBA) into a BGRA pixel buffer of the same size.
fn write_pixels(buffer: &CVPixelBuffer, image: &RgbaImage) -> Result<()> {
    unsafe {
        CVPixelBufferLockBaseAddress(buffer, CVPixelBufferLockFlags::empty());
        let (width, height) = (
            CVPixelBufferGetWidth(buffer),
            CVPixelBufferGetHeight(buffer),
        );
        let stride = CVPixelBufferGetBytesPerRow(buffer);
        let base = CVPixelBufferGetBaseAddress(buffer) as *mut u8;
        let fits =
            !base.is_null() && (width, height) == (image.width() as usize, image.height() as usize);
        if fits {
            for (row, line) in image.as_raw().chunks_exact(width * 4).enumerate() {
                let out = std::slice::from_raw_parts_mut(base.add(row * stride), width * 4);
                for (to, from) in out
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .zip(line.as_chunks::<4>().0)
                {
                    *to = [from[2], from[1], from[0], from[3]];
                }
            }
        }
        CVPixelBufferUnlockBaseAddress(buffer, CVPixelBufferLockFlags::empty());
        if fits {
            Ok(())
        } else {
            bail!("a frame did not fit the encoder")
        }
    }
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

/// The frames of `from`..`to`, decoded, upright.
pub struct AvDecoder {
    reader: Retained<AVAssetReader>,
    output: Retained<AVAssetReaderTrackOutput>,
    turns: u8,
    to: f64,
}

pub fn decoder(path: &Path, turns: u8, from: f64, to: f64) -> Result<AvDecoder> {
    let asset = asset(path);
    let video = track(&asset, true).context("no video in it")?;
    let (reader, output) = reader(&asset, &video, Some(&bgra_output()), from, to - from)?;
    Ok(AvDecoder {
        reader,
        output,
        turns,
        to,
    })
}

impl Decoder for AvDecoder {
    fn next(&mut self) -> Result<Option<(f64, RgbaImage)>> {
        loop {
            let Some(sample) = (unsafe { self.output.copyNextSampleBuffer() }) else {
                if unsafe { self.reader.status() } == AVAssetReaderStatus::Failed {
                    let reason = unsafe { self.reader.error() }
                        .map(|err| err.localizedDescription().to_string())
                        .unwrap_or_default();
                    bail!("reading the video failed: {reason}");
                }
                return Ok(None);
            };
            let time = seconds(unsafe { sample.presentation_time_stamp() });
            if time > self.to + 0.001 {
                return Ok(None);
            }
            // Samples without a picture (markers) are passed over.
            let Some(buffer) = (unsafe { sample.image_buffer() }) else {
                continue;
            };
            let Some(image) = read_pixels(&buffer) else {
                continue;
            };
            return Ok(Some((time, upright(image, self.turns))));
        }
    }
}

impl Drop for AvDecoder {
    fn drop(&mut self) {
        unsafe { self.reader.cancelReading() }
    }
}

/// The frame shown at `t`: the last one starting at or before it.
pub fn frame_at(path: &Path, turns: u8, t: f64, fps: f64) -> Result<RgbaImage> {
    let frame = 1. / fps.max(1.);
    // From a little before, so `t` between two frames finds the earlier.
    let mut decoder = decoder(path, turns, (t - frame * 2.).max(0.), t + frame * 2.)?;
    let mut best = None;
    while let Some((time, image)) = decoder.next()? {
        if time <= t + 0.0005 || best.is_none() {
            best = Some(image);
        }
        if time > t {
            break;
        }
    }
    best.context("no frame at that time")
}

/// When each frame starts, read without decoding.
pub fn frame_times(path: &Path, duration: f64) -> Result<Vec<f64>> {
    let asset = asset(path);
    let video = track(&asset, true).context("no video in it")?;
    let (reader, output) = reader(&asset, &video, None, 0., duration + 1.)?;
    let mut times = Vec::new();
    while let Some(sample) = unsafe { output.copyNextSampleBuffer() } {
        let count = unsafe { sample.num_samples() };
        let time = seconds(unsafe { sample.presentation_time_stamp() });
        if count > 0 && time.is_finite() {
            times.push(time);
        }
    }
    unsafe { reader.cancelReading() };
    times.sort_by(f64::total_cmp);
    times.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
    // Stored frames carry their times before the track's edit list (an
    // MP4 with B-frames starts a frame or two in); decoded ones, after it.
    // The first decoded frame's time says how far to move them.
    if let Some(&stored) = times.first()
        && let Ok(Some((decoded, _))) =
            decoder(path, 0, 0., stored.max(0.5)).and_then(|mut decoder| decoder.next())
    {
        let shift = decoded - stored;
        if shift.abs() > 1e-6 {
            for time in &mut times {
                *time += shift;
            }
            times.retain(|time| *time > -1e-6);
        }
    }
    Ok(times)
}

/// Writes H.264 or HEVC (and AAC sound) with AVAssetWriter.
pub struct AvEncoder {
    writer: Retained<AVAssetWriter>,
    video: Retained<AVAssetWriterInput>,
    adaptor: Retained<AVAssetWriterInputPixelBufferAdaptor>,
    audio: Option<AudioFeed>,
}

/// The result's sound, fed to the writer a little ahead of the pictures
/// so it can interleave them.
struct AudioFeed {
    input: Retained<AVAssetWriterInput>,
    track: Track<AvPcm>,
    format: CFRetained<CMFormatDescription>,
    done: bool,
}

/// The settings the source's sound is read and written as: 16-bit
/// interleaved stereo PCM.
fn pcm_settings() -> Retained<NSDictionary<NSString, AnyObject>> {
    settings(vec![
        (
            audio_key("AVFormatIDKey"),
            NSNumber::new_u32(LINEAR_PCM).into(),
        ),
        (audio_key("AVLinearPCMBitDepthKey"), int(16)),
        (
            audio_key("AVLinearPCMIsFloatKey"),
            NSNumber::new_bool(false).into(),
        ),
        (
            audio_key("AVLinearPCMIsBigEndianKey"),
            NSNumber::new_bool(false).into(),
        ),
        (
            audio_key("AVLinearPCMIsNonInterleaved"),
            NSNumber::new_bool(false).into(),
        ),
        (audio_key("AVSampleRateKey"), number(SAMPLE_RATE)),
        (audio_key("AVNumberOfChannelsKey"), int(2)),
    ])
}

/// The format of the PCM handed to the writer (as `pcm_settings` reads it).
fn pcm_format() -> Result<CFRetained<CMFormatDescription>> {
    let description = AudioStreamBasicDescription {
        mSampleRate: SAMPLE_RATE,
        mFormatID: kAudioFormatLinearPCM,
        mFormatFlags: kAudioFormatFlagIsSignedInteger | kAudioFormatFlagIsPacked,
        mBytesPerPacket: 4,
        mFramesPerPacket: 1,
        mBytesPerFrame: 4,
        mChannelsPerFrame: 2,
        mBitsPerChannel: 16,
        mReserved: 0,
    };
    let mut format: *const CMFormatDescription = std::ptr::null();
    let status = unsafe {
        CMAudioFormatDescriptionCreate(
            None,
            NonNull::from(&description),
            0,
            std::ptr::null(),
            0,
            std::ptr::null(),
            None,
            NonNull::from(&mut format),
        )
    };
    let format = NonNull::new(format as *mut CMFormatDescription)
        .filter(|_| status == 0)
        .context("the sound's format could not be made")?;
    // SAFETY: created for us, ours to release.
    Ok(unsafe { CFRetained::from_raw(format) })
}

/// A sample buffer of `block` (interleaved stereo) starting at frame
/// `first` of the result, at `rate`.
fn pcm_sample(
    format: &CMFormatDescription,
    block: &[i16],
    first: u64,
    rate: u32,
) -> Result<CFRetained<CMSampleBuffer>> {
    let bytes = std::mem::size_of_val(block);
    unsafe {
        let mut memory: *mut CMBlockBuffer = std::ptr::null_mut();
        let status = CMBlockBuffer::create_with_memory_block(
            None,
            std::ptr::null_mut(),
            bytes,
            None,
            std::ptr::null(),
            0,
            bytes,
            kCMBlockBufferAssureMemoryNowFlag,
            NonNull::from(&mut memory),
        );
        let memory = NonNull::new(memory)
            .filter(|_| status == 0)
            .context("no memory for the sound")?;
        let memory: CFRetained<CMBlockBuffer> = CFRetained::from_raw(memory);
        let source = NonNull::new(block.as_ptr() as *mut std::ffi::c_void).context("no sound")?;
        if CMBlockBuffer::replace_data_bytes(source, &memory, 0, bytes) != 0 {
            bail!("the sound could not be copied");
        }
        let mut sample: *mut CMSampleBuffer = std::ptr::null_mut();
        let status = CMAudioSampleBufferCreateReadyWithPacketDescriptions(
            None,
            &memory,
            format,
            (block.len() / 2) as isize,
            CMTime::new(first as i64, rate as i32),
            std::ptr::null(),
            NonNull::from(&mut sample),
        );
        let sample = NonNull::new(sample)
            .filter(|_| status == 0)
            .context("the sound could not be packed")?;
        Ok(CFRetained::from_raw(sample))
    }
}

/// The source's sound, decoded to 16-bit stereo at 44.1 kHz.
struct AvPcm {
    asset: Retained<AVURLAsset>,
    track: Retained<AVAssetTrack>,
    reading: Option<(Retained<AVAssetReader>, Retained<AVAssetReaderTrackOutput>)>,
}

impl AvPcm {
    fn stop(&mut self) {
        if let Some((reader, _)) = self.reading.take() {
            unsafe { reader.cancelReading() };
        }
    }
}

impl PcmReader for AvPcm {
    fn layout(&self) -> Layout {
        Layout {
            rate: SAMPLE_RATE as u32,
            channels: 2,
            mask: None,
        }
    }

    fn seek(&mut self, from: f64, to: f64) -> Result<()> {
        self.stop();
        self.reading = Some(reader(
            &self.asset,
            &self.track,
            Some(&pcm_settings()),
            from,
            (to - from).max(0.) + 0.05,
        )?);
        Ok(())
    }

    fn read(&mut self) -> Result<Option<(f64, Vec<i16>)>> {
        let Some((reader, output)) = &self.reading else {
            return Ok(None);
        };
        loop {
            let Some(sample) = (unsafe { output.copyNextSampleBuffer() }) else {
                if unsafe { reader.status() } == AVAssetReaderStatus::Failed {
                    let reason = unsafe { reader.error() }
                        .map(|err| err.localizedDescription().to_string())
                        .unwrap_or_default();
                    bail!("reading the sound failed: {reason}");
                }
                return Ok(None);
            };
            let time = seconds(unsafe { sample.presentation_time_stamp() });
            let Some(data) = (unsafe { sample.data_buffer() }) else {
                continue;
            };
            let length = unsafe { data.data_length() };
            if length < 2 || !time.is_finite() {
                continue;
            }
            let mut bytes = vec![0u8; length];
            let copied =
                unsafe { data.copy_data_bytes(0, length, NonNull::from(&mut bytes[..]).cast()) };
            if copied != 0 {
                bail!("reading the sound failed");
            }
            let samples = bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|value| i16::from_le_bytes(*value))
                .collect();
            return Ok(Some((time, samples)));
        }
    }
}

impl Drop for AvPcm {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn encoder(source: &Path, out: &Path, target: &Target) -> Result<AvEncoder> {
    let (size, fps, bitrate) = (target.size, target.fps, target.bitrate);
    let (hevc, audio) = (target.codec == Codec::Hevc, target.audio.clone());
    unsafe {
        let file_type = AVFileTypeMPEG4.context("MP4 is not available")?;
        let writer = AVAssetWriter::assetWriterWithURL_fileType_error(&url(out), file_type)
            .map_err(|err| anyhow!("{}", err.localizedDescription()))?;
        writer.setShouldOptimizeForNetworkUse(true);
        let codec = if hevc {
            AVVideoCodecTypeHEVC
        } else {
            AVVideoCodecTypeH264
        }
        .context("the codec is not available")?;
        let compression = settings(vec![
            (
                key(AVVideoAverageBitRateKey.context("no bitrate key")?),
                int(bitrate as i64),
            ),
            (
                key(AVVideoMaxKeyFrameIntervalKey.context("no key frame key")?),
                int((fps * 2.).round() as i64),
            ),
            (
                key(AVVideoExpectedSourceFrameRateKey.context("no frame rate key")?),
                number(fps),
            ),
        ]);
        let video_settings = settings(vec![
            (
                key(AVVideoCodecKey.context("no codec key")?),
                codec.copy().into(),
            ),
            (
                key(AVVideoWidthKey.context("no width key")?),
                int(size.0 as i64),
            ),
            (
                key(AVVideoHeightKey.context("no height key")?),
                int(size.1 as i64),
            ),
            (
                key(AVVideoCompressionPropertiesKey.context("no compression key")?),
                compression.into(),
            ),
        ]);
        let video_type = AVMediaTypeVideo.context("no video type")?;
        let video = AVAssetWriterInput::assetWriterInputWithMediaType_outputSettings(
            video_type,
            Some(&video_settings),
        );
        video.setExpectsMediaDataInRealTime(false);
        let pool_settings = settings(vec![
            (
                ns_key(kCVPixelBufferPixelFormatTypeKey).copy(),
                NSNumber::new_u32(BGRA).into(),
            ),
            (ns_key(kCVPixelBufferWidthKey).copy(), int(size.0 as i64)),
            (ns_key(kCVPixelBufferHeightKey).copy(), int(size.1 as i64)),
        ]);
        let adaptor =
            AVAssetWriterInputPixelBufferAdaptor::assetWriterInputPixelBufferAdaptorWithAssetWriterInput_sourcePixelBufferAttributes(
                &video,
                Some(&pool_settings),
            );
        if !writer.canAddInput(&video) {
            bail!("AVFoundation cannot write this video");
        }
        writer.addInput(&video);

        let source_asset = asset(source);
        let audio = match (audio.is_empty(), track(&source_asset, false)) {
            (false, Some(source_track)) => {
                let audio_settings = settings(vec![
                    (audio_key("AVFormatIDKey"), NSNumber::new_u32(AAC).into()),
                    (audio_key("AVNumberOfChannelsKey"), int(2)),
                    (audio_key("AVSampleRateKey"), number(SAMPLE_RATE)),
                    (audio_key("AVEncoderBitRateKey"), int(128_000)),
                ]);
                let audio_type = AVMediaTypeAudio.context("no audio type")?;
                let input = AVAssetWriterInput::assetWriterInputWithMediaType_outputSettings(
                    audio_type,
                    Some(&audio_settings),
                );
                input.setExpectsMediaDataInRealTime(false);
                if writer.canAddInput(&input) {
                    writer.addInput(&input);
                    let reader = AvPcm {
                        asset: source_asset,
                        track: source_track,
                        reading: None,
                    };
                    Some(AudioFeed {
                        input,
                        track: Track::new(reader, audio, target.seconds, SAMPLE_RATE as u32),
                        format: pcm_format()?,
                        done: false,
                    })
                } else {
                    None
                }
            }
            _ => None,
        };

        if !writer.startWriting() {
            let reason = writer
                .error()
                .map(|err| err.localizedDescription().to_string())
                .unwrap_or_default();
            bail!("AVFoundation cannot write the file: {reason}");
        }
        writer.startSessionAtSourceTime(cm(0.));
        Ok(AvEncoder {
            writer,
            video,
            adaptor,
            audio,
        })
    }
}

/// Waits (briefly sleeping) until `input` takes more, or the writer fails.
fn wait_ready(writer: &AVAssetWriter, input: &AVAssetWriterInput) -> Result<()> {
    loop {
        unsafe {
            if input.isReadyForMoreMediaData() {
                return Ok(());
            }
            if writer.status() == AVAssetWriterStatus::Failed {
                let reason = writer
                    .error()
                    .map(|err| err.localizedDescription().to_string())
                    .unwrap_or_default();
                bail!("writing the video failed: {reason}");
            }
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

impl AudioFeed {
    /// The next block of sound up to result second `until`, packed.
    fn next(&mut self, until: f64) -> Result<Option<CFRetained<CMSampleBuffer>>> {
        let Some(block) = self.track.next(until)? else {
            return Ok(None);
        };
        let first = self.track.given() - block.len() as u64 / 2;
        pcm_sample(&self.format, &block, first, self.track.rate()).map(Some)
    }

    fn append(&self, writer: &AVAssetWriter, sample: &CMSampleBuffer) -> Result<()> {
        if !unsafe { self.input.appendSampleBuffer(sample) } {
            let reason = unsafe { writer.error() }
                .map(|err| err.localizedDescription().to_string())
                .unwrap_or_default();
            bail!("writing the sound failed: {reason}");
        }
        Ok(())
    }

    /// Feeds the writer sound up to result time `until`, as far as it
    /// takes it now (it may want pictures first).
    fn feed_until(&mut self, writer: &AVAssetWriter, until: f64) -> Result<()> {
        while !self.done && unsafe { self.input.isReadyForMoreMediaData() } {
            match self.next(until)? {
                Some(sample) => self.append(writer, &sample)?,
                None => return Ok(()),
            }
        }
        Ok(())
    }

    /// Feeds one more block, if the writer takes one now; whether it did.
    fn feed_one(&mut self, writer: &AVAssetWriter) -> Result<bool> {
        if self.done || !unsafe { self.input.isReadyForMoreMediaData() } {
            return Ok(false);
        }
        match self.next(f64::INFINITY)? {
            Some(sample) => {
                self.append(writer, &sample)?;
                Ok(true)
            }
            None => {
                self.done = true;
                unsafe { self.input.markAsFinished() };
                Ok(false)
            }
        }
    }
}

impl Encoder for AvEncoder {
    fn push(&mut self, frame: &RgbaImage, time: f64) -> Result<()> {
        if let Some(audio) = &mut self.audio {
            audio.feed_until(&self.writer, time + 0.5)?;
        }
        // The writer interleaves: while it holds pictures back it wants
        // more sound.
        loop {
            if unsafe { self.video.isReadyForMoreMediaData() } {
                break;
            }
            let fed = match &mut self.audio {
                Some(audio) => audio.feed_one(&self.writer)?,
                None => false,
            };
            if !fed {
                wait_ready(&self.writer, &self.video)?;
                break;
            }
        }
        unsafe {
            let pool: Retained<CVPixelBufferPool> = self
                .adaptor
                .pixelBufferPool()
                .context("the encoder has no frames to fill")?;
            let mut buffer: *mut CVPixelBuffer = std::ptr::null_mut();
            CVPixelBufferPool::create_pixel_buffer(None, &pool, NonNull::from(&mut buffer));
            let buffer = NonNull::new(buffer).context("no frame from the encoder")?;
            let buffer: CFRetained<CVPixelBuffer> = CFRetained::from_raw(buffer);
            write_pixels(&buffer, frame)?;
            if !self
                .adaptor
                .appendPixelBuffer_withPresentationTime(&buffer, cm(time))
            {
                let reason = self
                    .writer
                    .error()
                    .map(|err| err.localizedDescription().to_string())
                    .unwrap_or_default();
                bail!("writing a frame failed: {reason}");
            }
        }
        Ok(())
    }

    fn finish(mut self: Box<Self>) -> Result<()> {
        unsafe { self.video.markAsFinished() };
        if let Some(audio) = &mut self.audio {
            while !audio.done {
                wait_ready(&self.writer, &audio.input)?;
                audio.feed_one(&self.writer)?;
            }
        }
        let (done, wait) = std::sync::mpsc::channel();
        let handler = block2::RcBlock::new(move || {
            let _ = done.send(());
        });
        unsafe { self.writer.finishWritingWithCompletionHandler(&handler) };
        wait.recv_timeout(Duration::from_secs(120))
            .map_err(|_| anyhow!("the video took too long to finish"))?;
        if unsafe { self.writer.status() } != AVAssetWriterStatus::Completed {
            let reason = unsafe { self.writer.error() }
                .map(|err| err.localizedDescription().to_string())
                .unwrap_or_default();
            bail!("finishing the video failed: {reason}");
        }
        Ok(())
    }

    fn abort(self: Box<Self>) {
        unsafe { self.writer.cancelWriting() }
    }
}
