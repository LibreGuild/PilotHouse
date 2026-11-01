//! Video on Linux through GStreamer: whatever codecs the system has
//! (gst-libav, x264, VA-API hardware and so on), so none ships with
//! PilotHouse. GStreamer is loaded when first used (see
//! `ph_preview::video::gst`).
//!
//! Reading: `parsebin` splits the file without decoding; only the stream
//! wanted goes on to `decodebin` (pictures are never decoded for the
//! sound, nor sound for the pictures), then `videoflip` stands a phone's
//! recording upright as its metadata asks and `videoconvert` makes RGBA;
//! sound becomes 48 kHz stereo. Writing: frames and the result's sound go
//! in through `appsrc`, to the first encoders this GStreamer has that pass
//! a short trial (hardware ones can be listed and still fail), into MP4 or
//! WebM.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::os::unix::ffi::OsStrExt as _;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow, bail};
use image::RgbaImage;
use ph_preview::video::gst::{
    GCallback, GError, GST_CLOCK_TIME_NONE, GST_FLOW_OK, GST_FORMAT_TIME, GST_MAP_READ,
    GST_MAP_WRITE, GST_MESSAGE_EOS, GST_MESSAGE_ERROR, GST_PAD_LINK_OK, GST_SEEK_FLAG_ACCURATE,
    GST_SEEK_FLAG_FLUSH, GST_SEEK_TYPE_NONE, GST_SEEK_TYPE_SET, GST_STATE_CHANGE_FAILURE,
    GST_STATE_NULL, GST_STATE_PAUSED, GST_STATE_PLAYING, Gst, GstBuffer, GstMapInfo, GstMessage,
    NANOS, c, gst, with_hint,
};

use super::frames::{Raw, Seeked};
use super::media::{self, Codec, Decoder, Encoder, Info, Source, Target};
use super::sound::{Layout, PcmReader, Track};

/// `GST_STATE_CHANGE_ASYNC`: still changing when the wait ran out.
const STATE_ASYNC: c_int = 2;
/// How long opening a file may take (a network drive is slow).
const OPEN: Duration = Duration::from_secs(20);
/// How long one frame or block may take to come.
const NEXT: Duration = Duration::from_secs(30);
/// How long writing may take to finish after the last frame.
const FINISH: Duration = Duration::from_secs(120);
/// How often a wait looks for errors.
const POLL: Duration = Duration::from_millis(100);
/// The sound read and written: 16-bit stereo at 48 kHz (GStreamer converts).
const RATE: u32 = 48_000;
const AAC_BITRATE: &str = "128000";

fn nanos(duration: Duration) -> u64 {
    duration.as_nanos().min(u64::MAX as u128) as u64
}

fn seconds_to_nanos(seconds: f64) -> u64 {
    (seconds.max(0.) * NANOS).round() as u64
}

fn loaded() -> Result<&'static Gst> {
    gst().map_err(|err| anyhow!(err))
}

/// What a `pad-added` handler links: the first pad of the kind wanted to
/// `target`, noting which kinds turned up and when there are no more.
struct Link {
    gst: &'static Gst,
    /// A sink pad, its reference held.
    target: *mut c_void,
    video: bool,
    linked: AtomicBool,
    saw_video: AtomicBool,
    saw_audio: AtomicBool,
    all_pads: AtomicBool,
}

impl Drop for Link {
    fn drop(&mut self) {
        unsafe { (self.gst.object_unref)(self.target) };
    }
}

unsafe extern "C" fn pad_added(_: *mut c_void, pad: *mut c_void, data: *mut c_void) {
    // SAFETY: `data` is the pipeline's `Link`, which outlives its threads;
    // `pad` is the new pad.
    unsafe {
        let link = &*(data as *const Link);
        let gst = link.gst;
        let mut caps = (gst.pad_get_current_caps)(pad);
        if caps.is_null() {
            caps = (gst.pad_query_caps)(pad, std::ptr::null_mut());
        }
        let name = gst.caps_name(caps);
        if !caps.is_null() {
            (gst.mini_object_unref)(caps);
        }
        let video = name.starts_with("video/") || name.starts_with("image/");
        let audio = name.starts_with("audio/");
        if video {
            link.saw_video.store(true, Ordering::SeqCst);
        }
        if audio {
            link.saw_audio.store(true, Ordering::SeqCst);
        }
        let wanted = if link.video { video } else { audio };
        if wanted
            && !link.linked.swap(true, Ordering::SeqCst)
            && (gst.pad_link)(pad, link.target) != GST_PAD_LINK_OK
        {
            // Another pad of the kind may link instead.
            link.linked.store(false, Ordering::SeqCst);
        }
    }
}

unsafe extern "C" fn no_more_pads(_: *mut c_void, data: *mut c_void) {
    // SAFETY: as in `pad_added`.
    unsafe {
        (*(data as *const Link))
            .all_pads
            .store(true, Ordering::SeqCst);
    }
}

/// A pipeline, stopped and released when dropped.
struct Pipeline {
    gst: &'static Gst,
    element: *mut c_void,
    bus: *mut c_void,
    /// Elements looked up by name, their references held.
    held: Vec<*mut c_void>,
    /// What its `pad-added` handler links (freed after it stops).
    link: Option<Box<Link>>,
}

impl Pipeline {
    fn launch(description: &str) -> Result<Pipeline> {
        let gst = loaded()?;
        unsafe {
            let mut error: *mut GError = std::ptr::null_mut();
            let element = (gst.parse_launch)(c(description).as_ptr(), &mut error);
            if !error.is_null() {
                let message = (*error).message;
                let text = if message.is_null() {
                    String::new()
                } else {
                    CStr::from_ptr(message).to_string_lossy().into_owned()
                };
                (gst.error_free)(std::ptr::NonNull::new_unchecked(error));
                if !element.is_null() {
                    (gst.object_ref_sink)(element);
                    (gst.object_unref)(element);
                }
                bail!("{}", with_hint(format!("GStreamer cannot do it: {text}.")));
            }
            if element.is_null() {
                bail!("GStreamer cannot do it");
            }
            // Ours, not floating.
            let element = (gst.object_ref_sink)(element);
            Ok(Pipeline {
                gst,
                element,
                bus: (gst.get_bus)(element),
                held: Vec::new(),
                link: None,
            })
        }
    }

    /// The element named `name` in it.
    fn named(&mut self, name: &str) -> Result<*mut c_void> {
        let found = unsafe { (self.gst.bin_get_by_name)(self.element, c(name).as_ptr()) };
        if found.is_null() {
            bail!("GStreamer lost its {name}");
        }
        self.held.push(found);
        Ok(found)
    }

    /// Links the first pad `splitter` adds of the kind wanted to `target`'s
    /// sink pad.
    fn link(&mut self, splitter: *mut c_void, target: *mut c_void, video: bool) -> Result<()> {
        let gst = self.gst;
        let pad = unsafe { (gst.element_get_static_pad)(target, c("sink").as_ptr()) };
        if pad.is_null() {
            bail!("GStreamer cannot link it");
        }
        let link = Box::new(Link {
            gst,
            target: pad,
            video,
            linked: AtomicBool::new(false),
            saw_video: AtomicBool::new(false),
            saw_audio: AtomicBool::new(false),
            all_pads: AtomicBool::new(false),
        });
        let data = &*link as *const Link as *mut c_void;
        unsafe {
            let added: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) = pad_added;
            let done: unsafe extern "C" fn(*mut c_void, *mut c_void) = no_more_pads;
            (gst.signal_connect_data)(
                splitter,
                c("pad-added").as_ptr(),
                std::mem::transmute::<
                    unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void),
                    GCallback,
                >(added),
                data,
                std::ptr::null_mut(),
                0,
            );
            (gst.signal_connect_data)(
                splitter,
                c("no-more-pads").as_ptr(),
                std::mem::transmute::<unsafe extern "C" fn(*mut c_void, *mut c_void), GCallback>(
                    done,
                ),
                data,
                std::ptr::null_mut(),
                0,
            );
        }
        self.link = Some(link);
        Ok(())
    }

    /// GStreamer's reason, if it failed. The other messages waiting are
    /// let go, so they never pile up over a long run (the end of writing is
    /// waited for apart, in `wait_end`).
    fn error(&self) -> Option<String> {
        let mut error = None;
        unsafe {
            loop {
                let message = (self.gst.bus_pop)(self.bus);
                if message.is_null() {
                    break;
                }
                if error.is_none() && (*(message as *const GstMessage)).type_ == GST_MESSAGE_ERROR {
                    error = Some(with_hint(self.gst.error_text(message)));
                }
                (self.gst.mini_object_unref)(message);
            }
        }
        error
    }

    fn fail(&self, otherwise: &str) -> anyhow::Error {
        anyhow!(self.error().unwrap_or_else(|| otherwise.to_string()))
    }

    fn set_state(&self, state: c_int) -> Result<()> {
        if unsafe { (self.gst.set_state)(self.element, state) } == GST_STATE_CHANGE_FAILURE {
            return Err(self.fail("GStreamer cannot read it"));
        }
        Ok(())
    }

    /// What was not there, once the file is split (or failed, `failed`,
    /// after streams of the other kind came): the stream wanted.
    fn missing(&self, failed: bool) -> Option<&'static str> {
        let link = self.link.as_ref()?;
        let other = if link.video {
            &link.saw_audio
        } else {
            &link.saw_video
        };
        let split =
            link.all_pads.load(Ordering::SeqCst) || (failed && other.load(Ordering::SeqCst));
        (split && !link.linked.load(Ordering::SeqCst)).then_some(if link.video {
            "GStreamer finds no video in it"
        } else {
            "GStreamer finds no sound in it"
        })
    }

    /// Paused with its first frame ready: the file is open.
    fn open(&self) -> Result<()> {
        self.set_state(GST_STATE_PAUSED)?;
        let started = Instant::now();
        loop {
            let (mut state, mut pending) = (0, 0);
            let change = unsafe {
                (self.gst.get_state)(self.element, &mut state, &mut pending, nanos(POLL))
            };
            let error = (change == GST_STATE_CHANGE_FAILURE)
                .then(|| self.fail("GStreamer cannot read it").to_string())
                .or_else(|| self.error());
            // An unlinked stream fails the file too: say what was missing.
            if let Some(missing) = self.missing(error.is_some()) {
                bail!(missing);
            }
            if let Some(error) = error {
                bail!(error);
            }
            if change != STATE_ASYNC {
                return Ok(());
            }
            if started.elapsed() > OPEN {
                bail!("GStreamer took too long to open it");
            }
        }
    }

    /// Goes to `from` (exactly, not the key frame before), reading up to
    /// `to` when given.
    fn seek(&self, from: f64, to: Option<f64>) -> Result<()> {
        let (stop_type, stop) = match to {
            Some(to) => (GST_SEEK_TYPE_SET, seconds_to_nanos(to) as i64),
            None => (GST_SEEK_TYPE_NONE, -1),
        };
        let done = unsafe {
            (self.gst.seek)(
                self.element,
                1.,
                GST_FORMAT_TIME,
                GST_SEEK_FLAG_FLUSH | GST_SEEK_FLAG_ACCURATE,
                GST_SEEK_TYPE_SET,
                seconds_to_nanos(from) as i64,
                stop_type,
                stop,
            )
        };
        if done == 0 {
            return Err(self.fail("GStreamer cannot go to that time"));
        }
        Ok(())
    }

    /// The next sample from `sink`, or `None` at the end.
    fn pull(&self, sink: *mut c_void) -> Result<Option<Sample>> {
        let started = Instant::now();
        loop {
            let sample = unsafe { (self.gst.try_pull_sample)(sink, nanos(POLL)) };
            if !sample.is_null() {
                return Ok(Some(Sample {
                    gst: self.gst,
                    sample,
                }));
            }
            if let Some(error) = self.error() {
                bail!(error);
            }
            if unsafe { (self.gst.app_sink_is_eos)(sink) } != 0 {
                return Ok(None);
            }
            if started.elapsed() > NEXT {
                bail!("GStreamer stopped giving frames");
            }
        }
    }

    /// Waits for the end of what was written, or an error.
    fn wait_end(&self, wait: Duration) -> Result<()> {
        unsafe {
            let message = (self.gst.bus_timed_pop_filtered)(
                self.bus,
                nanos(wait),
                GST_MESSAGE_EOS | GST_MESSAGE_ERROR,
            );
            if message.is_null() {
                bail!("GStreamer took too long to finish the video");
            }
            let error = (*(message as *const GstMessage)).type_ == GST_MESSAGE_ERROR;
            let text = error.then(|| with_hint(self.gst.error_text(message)));
            (self.gst.mini_object_unref)(message);
            match text {
                Some(text) => bail!(text),
                None => Ok(()),
            }
        }
    }
}

impl Drop for Pipeline {
    fn drop(&mut self) {
        unsafe {
            // Stopping joins its threads, so nothing runs on after this.
            (self.gst.set_state)(self.element, GST_STATE_NULL);
            for element in self.held.drain(..) {
                (self.gst.object_unref)(element);
            }
            (self.gst.object_unref)(self.bus);
            (self.gst.object_unref)(self.element);
        }
    }
}

/// A sample pulled from an `appsink`, released when dropped.
struct Sample {
    gst: &'static Gst,
    sample: *mut c_void,
}

impl Sample {
    fn buffer(&self) -> *mut c_void {
        unsafe { (self.gst.sample_get_buffer)(self.sample) }
    }

    /// When it starts and ends in the file's own time (seconds, when
    /// known): a buffer's time is its stream's, which an MP4's edit list
    /// shifts; the segment it came in says how.
    fn times(&self) -> (Option<f64>, Option<f64>) {
        let buffer = self.buffer();
        if buffer.is_null() {
            return (None, None);
        }
        // SAFETY: a live buffer; its times are public fields.
        let (pts, duration) = unsafe {
            let fields = &*(buffer as *const GstBuffer);
            (fields.pts, fields.duration)
        };
        if pts == GST_CLOCK_TIME_NONE {
            return (None, None);
        }
        let segment = unsafe { (self.gst.sample_get_segment)(self.sample) };
        let mut stream = 0u64;
        let nanos = match segment.is_null() {
            true => pts as f64,
            false => match unsafe {
                (self.gst.segment_to_stream_time_full)(segment, GST_FORMAT_TIME, pts, &mut stream)
            } {
                1 => stream as f64,
                -1 => -(stream as f64),
                _ => pts as f64,
            },
        };
        let start = Some(nanos / NANOS);
        let end = start
            .zip((duration != GST_CLOCK_TIME_NONE && duration > 0).then_some(duration))
            .map(|(start, duration)| start + duration as f64 / NANOS);
        (start, end)
    }

    /// Its caps' `width` and `height`, and frame rate.
    fn video_format(&self) -> Option<(u32, u32, Option<f64>)> {
        unsafe {
            let caps = (self.gst.sample_get_caps)(self.sample);
            if caps.is_null() {
                return None;
            }
            let structure = (self.gst.caps_get_structure)(caps, 0);
            if structure.is_null() {
                return None;
            }
            let (mut width, mut height) = (0, 0);
            (self.gst.structure_get_int)(structure, c("width").as_ptr(), &mut width);
            (self.gst.structure_get_int)(structure, c("height").as_ptr(), &mut height);
            let (mut num, mut den) = (0, 0);
            let rate = ((self.gst.structure_get_fraction)(
                structure,
                c("framerate").as_ptr(),
                &mut num,
                &mut den,
            ) != 0
                && num > 0
                && den > 0)
                .then(|| num as f64 / den as f64);
            (width > 0 && height > 0).then_some((width as u32, height as u32, rate))
        }
    }

    /// Reads its bytes.
    fn with_bytes<T>(&self, read: impl FnOnce(&[u8]) -> T) -> Result<T> {
        let buffer = self.buffer();
        if buffer.is_null() {
            bail!("GStreamer gave an empty frame");
        }
        unsafe {
            let mut map: GstMapInfo = std::mem::zeroed();
            if (self.gst.buffer_map)(buffer, &mut map, GST_MAP_READ) == 0 {
                bail!("GStreamer gave an unreadable frame");
            }
            let bytes = if map.data.is_null() {
                &[][..]
            } else {
                std::slice::from_raw_parts(map.data, map.size)
            };
            let value = read(bytes);
            (self.gst.buffer_unmap)(buffer, &mut map);
            Ok(value)
        }
    }

    /// As RGBA pixels.
    fn image(&self) -> Result<RgbaImage> {
        let (width, height, _) = self
            .video_format()
            .context("GStreamer gave a frame of no size")?;
        let row = width as usize * 4;
        self.with_bytes(|bytes| {
            let stride = (bytes.len() / height as usize).max(row);
            if stride * (height as usize - 1) + row > bytes.len() {
                bail!("GStreamer gave a frame smaller than it said");
            }
            let mut pixels = Vec::with_capacity(row * height as usize);
            for line in bytes.chunks(stride).take(height as usize) {
                pixels.extend_from_slice(&line[..row]);
            }
            RgbaImage::from_raw(width, height, pixels).context("a frame")
        })?
    }
}

impl Drop for Sample {
    fn drop(&mut self) {
        unsafe { (self.gst.mini_object_unref)(self.sample) };
    }
}

/// `path` as filesrc and filesink take it.
fn location(path: &Path) -> Result<CString> {
    CString::new(path.as_os_str().as_bytes()).context("the path cannot be given to GStreamer")
}

/// A pipeline reading `path`'s first video (`video`) or sound stream
/// through `tail` (elements after `decodebin`, ending in `appsink
/// name=sink`; empty: the stream undecoded straight to the sink), opened.
fn reading(path: &Path, video: bool, tail: &str) -> Result<(Pipeline, *mut c_void)> {
    let description = if tail.is_empty() {
        "filesrc name=src ! parsebin name=split  appsink name=sink sync=false max-buffers=16 enable-last-sample=false".to_string()
    } else {
        format!("filesrc name=src ! parsebin name=split  decodebin name=decode ! {tail}")
    };
    let mut pipeline = Pipeline::launch(&description)?;
    let gst = pipeline.gst;
    let src = pipeline.named("src")?;
    let location = location(path)?;
    unsafe {
        (gst.object_set)(
            src,
            c("location").as_ptr(),
            location.as_ptr(),
            std::ptr::null::<c_char>(),
        );
    }
    let split = pipeline.named("split")?;
    let sink = pipeline.named("sink")?;
    let target = if tail.is_empty() {
        sink
    } else {
        pipeline.named("decode")?
    };
    pipeline.link(split, target, video)?;
    if let Ok(flip) = pipeline.named("flip") {
        unsafe { gst.set_if_known(flip, "video-direction", "auto") };
    }
    if let Ok(convert) = pipeline.named("convert") {
        // As many threads as it likes (GStreamer 1.20 on).
        unsafe { gst.set_if_known(convert, "n-threads", "0") };
    }
    pipeline.open()?;
    Ok((pipeline, sink))
}

/// Decoded frames as RGBA, upright.
fn video_reading(path: &Path) -> Result<(Pipeline, *mut c_void)> {
    let gst = loaded()?;
    let flip = if gst.has("videoflip") {
        "videoflip name=flip ! "
    } else {
        ""
    };
    reading(
        path,
        true,
        &format!(
            "{flip}videoconvert name=convert ! video/x-raw,format=RGBA ! appsink name=sink sync=false max-buffers=4 enable-last-sample=false"
        ),
    )
}

pub fn probe(path: &Path) -> Result<Info> {
    let (pipeline, sink) = video_reading(path)?;
    let sample = unsafe { (pipeline.gst.try_pull_preroll)(sink, nanos(NEXT)) };
    if sample.is_null() {
        return Err(pipeline.fail("GStreamer gives no frame of it"));
    }
    let sample = Sample {
        gst: pipeline.gst,
        sample,
    };
    let (width, height, fps) = sample
        .video_format()
        .context("GStreamer cannot tell the frame size")?;
    let mut duration = 0i64;
    let known =
        unsafe { (pipeline.gst.query_duration)(pipeline.element, GST_FORMAT_TIME, &mut duration) };
    if known == 0 || duration <= 0 {
        bail!("GStreamer cannot tell how long it is");
    }
    let audio = pipeline
        .link
        .as_ref()
        .is_some_and(|link| link.saw_audio.load(Ordering::SeqCst));
    Ok(Info {
        duration: duration as f64 / NANOS,
        size: (width, height),
        fps: fps.filter(|fps| *fps < 1000.).unwrap_or(30.),
        audio,
        source: Source::Gst,
    })
}

/// The frames of `from`..`to`, decoded, upright.
pub struct GstDecoder {
    seeked: Seeked<Sample>,
    reading: VideoRead,
}

/// The reading side of a decoder.
struct VideoRead {
    pipeline: Pipeline,
    sink: *mut c_void,
    /// When the last frame started (for one without a time).
    last: Option<f64>,
}

impl VideoRead {
    fn read(&mut self) -> Result<Option<Raw<Sample>>> {
        let Some(sample) = self.pipeline.pull(self.sink)? else {
            return Ok(None);
        };
        let (start, end) = sample.times();
        let start = start.unwrap_or_else(|| self.last.map_or(0., |last| last + 1. / 30.));
        self.last = Some(start);
        Ok(Some(Raw {
            start,
            end,
            frame: sample,
        }))
    }
}

pub fn decoder(path: &Path, from: f64, to: f64) -> Result<GstDecoder> {
    let (pipeline, sink) = video_reading(path)?;
    // A little past `to`, so a frame starting right at it comes.
    pipeline.seek(from, Some(to.max(from) + 0.002))?;
    pipeline.set_state(GST_STATE_PLAYING)?;
    Ok(GstDecoder {
        seeked: Seeked::new(from, to),
        reading: VideoRead {
            pipeline,
            sink,
            last: None,
        },
    })
}

impl Decoder for GstDecoder {
    fn next(&mut self) -> Result<Option<(f64, RgbaImage)>> {
        let reading = &mut self.reading;
        let Some((start, sample)) = self.seeked.next(|| reading.read())? else {
            return Ok(None);
        };
        Ok(Some((start, sample.image()?)))
    }
}

/// The frame shown at `t`.
pub fn frame_at(path: &Path, t: f64) -> Result<RgbaImage> {
    if let Some((_, frame)) = decoder(path, t, t)?.next()? {
        return Ok(frame);
    }
    // Past the last frame (the sound can run longer): the last one, read
    // from a little before, only it made into pixels.
    let (pipeline, sink) = video_reading(path)?;
    pipeline.seek((t - 2.).max(0.), None)?;
    pipeline.set_state(GST_STATE_PLAYING)?;
    let mut last = None;
    while let Some(sample) = pipeline.pull(sink)? {
        last = Some(sample);
    }
    last.context("no frame at that time")?.image()
}

/// When each frame starts, read without decoding.
pub fn frame_times(path: &Path) -> Result<Vec<f64>> {
    let (pipeline, sink) = reading(path, true, "")?;
    pipeline.set_state(GST_STATE_PLAYING)?;
    let mut times = Vec::new();
    while let Some(sample) = pipeline.pull(sink)? {
        // Frames before the start (an edit list cuts them) never show.
        if let (Some(start), _) = sample.times()
            && start > -1e-6
        {
            times.push(start.max(0.));
        }
    }
    times.sort_by(f64::total_cmp);
    times.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
    Ok(times)
}

/// The source's sound, decoded to 48 kHz stereo.
struct GstPcm {
    pipeline: Pipeline,
    sink: *mut c_void,
    playing: bool,
}

impl GstPcm {
    fn open(path: &Path) -> Result<GstPcm> {
        let (pipeline, sink) = reading(
            path,
            false,
            &format!(
                "audioconvert ! audioresample ! audio/x-raw,format=S16LE,layout=interleaved,rate={RATE},channels=2 ! appsink name=sink sync=false max-buffers=16 enable-last-sample=false"
            ),
        )?;
        Ok(GstPcm {
            pipeline,
            sink,
            playing: false,
        })
    }
}

impl PcmReader for GstPcm {
    fn layout(&self) -> Layout {
        Layout {
            rate: RATE,
            channels: 2,
            mask: None,
        }
    }

    fn seek(&mut self, from: f64, to: f64) -> Result<()> {
        self.pipeline.seek(from, Some(to + 0.05))?;
        if !self.playing {
            self.pipeline.set_state(GST_STATE_PLAYING)?;
            self.playing = true;
        }
        Ok(())
    }

    fn read(&mut self) -> Result<Option<(f64, Vec<i16>)>> {
        loop {
            let Some(sample) = self.pipeline.pull(self.sink)? else {
                return Ok(None);
            };
            // A block without a time cannot be placed.
            let (Some(start), _) = sample.times() else {
                continue;
            };
            let samples = sample.with_bytes(|bytes| {
                bytes
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|value| i16::from_le_bytes(*value))
                    .collect()
            })?;
            return Ok(Some((start, samples)));
        }
    }
}

/// An encoder element and the raw format it takes.
type VideoEncoder = (&'static str, &'static str);

/// The encoders tried for `codec`, in order: software ones first (they
/// work wherever they are installed), then hardware ones.
fn video_encoders(codec: Codec) -> &'static [VideoEncoder] {
    match codec {
        Codec::H264 => &[
            ("x264enc", "I420"),
            ("openh264enc", "I420"),
            ("vah264enc", "NV12"),
            ("vaapih264enc", "NV12"),
            ("nvh264enc", "NV12"),
        ],
        Codec::Hevc => &[
            ("x265enc", "I420"),
            ("vah265enc", "NV12"),
            ("vaapih265enc", "NV12"),
            ("nvh265enc", "NV12"),
        ],
        Codec::Vp9 => &[("vp9enc", "I420")],
    }
}

/// How an encoder's settings are given.
#[derive(Clone, Copy)]
enum Setting {
    Kilobits,
    Bits,
    /// Frames between key frames.
    KeyFrames,
    Threads,
    Text(&'static str),
}

/// Each encoder's settings: its rate, key frames, speed. Encoders name and
/// scale these differently, so each is set as that one takes it.
fn settings(encoder: &str) -> &'static [(&'static str, Setting)] {
    use Setting::*;
    match encoder {
        "x264enc" | "x265enc" => &[
            ("bitrate", Kilobits),
            ("speed-preset", Text("veryfast")),
            ("key-int-max", KeyFrames),
        ],
        "openh264enc" => &[
            ("rate-control", Text("bitrate")),
            ("bitrate", Bits),
            ("gop-size", KeyFrames),
        ],
        "vah264enc" | "vah265enc" => &[
            ("rate-control", Text("vbr")),
            ("bitrate", Kilobits),
            ("key-int-max", KeyFrames),
        ],
        "vaapih264enc" | "vaapih265enc" => &[
            ("rate-control", Text("vbr")),
            ("bitrate", Kilobits),
            ("keyframe-period", KeyFrames),
        ],
        "nvh264enc" | "nvh265enc" => &[
            ("rc-mode", Text("vbr")),
            ("bitrate", Kilobits),
            ("gop-size", KeyFrames),
        ],
        // Real time, as ffmpeg writes WebM here.
        "vp9enc" => &[
            ("target-bitrate", Bits),
            ("deadline", Text("1")),
            ("cpu-used", Text("6")),
            ("keyframe-max-dist", KeyFrames),
            ("row-mt", Text("true")),
            ("threads", Threads),
        ],
        // The sound encoders all take bit/s.
        _ => &[("bitrate", Text(AAC_BITRATE))],
    }
}

/// What a container needs: its muxer, the parser before it and the caps
/// it takes for the pictures, and its sound encoders with the caps each
/// gives (and the parser before the muxer).
struct Container {
    muxer: &'static str,
    parser: Option<&'static str>,
    video_caps: &'static str,
    audio: &'static [(&'static str, &'static str)],
    audio_parser: Option<&'static str>,
}

fn container(codec: Codec) -> Container {
    const AAC: &str = "audio/mpeg,mpegversion=4,stream-format=raw";
    let aac = &[
        ("avenc_aac", AAC),
        ("fdkaacenc", AAC),
        ("voaacenc", AAC),
        ("faac", AAC),
    ];
    match codec {
        Codec::H264 => Container {
            muxer: "mp4mux",
            parser: Some("h264parse"),
            video_caps: "video/x-h264,stream-format=avc,alignment=au",
            audio: aac,
            audio_parser: Some("aacparse"),
        },
        Codec::Hevc => Container {
            muxer: "mp4mux",
            parser: Some("h265parse"),
            video_caps: "video/x-h265,stream-format=hvc1,alignment=au",
            audio: aac,
            audio_parser: Some("aacparse"),
        },
        Codec::Vp9 => Container {
            muxer: "webmmux",
            parser: None,
            video_caps: "video/x-vp9",
            audio: &[("opusenc", "audio/x-opus"), ("vorbisenc", "audio/x-vorbis")],
            audio_parser: None,
        },
    }
}

/// The silence an AAC encoder puts before the sound (its priming), in
/// frames. GStreamer's MP4 muxer keeps it (writing no edit list to skip it,
/// as ffmpeg and AVFoundation do), so the sound would play that much late:
/// as much is dropped from its start instead. Measured for libav's and
/// VisualOn's encoders; fdk-aac's and FAAC's are their documented delays.
/// Opus in WebM states its own (players skip it), so nothing is dropped.
fn priming(encoder: &str) -> u64 {
    match encoder {
        "avenc_aac" | "faac" => 1024,
        "voaacenc" => 1600,
        "fdkaacenc" => 2048,
        _ => 0,
    }
}

/// Whether GStreamer here writes `codec` (an encoder and the muxer).
pub fn writes(codec: Codec) -> bool {
    let Ok(gst) = gst() else {
        return false;
    };
    gst.has(container(codec).muxer)
        && video_encoders(codec)
            .iter()
            .any(|(encoder, _)| gst.has(encoder))
}

/// A parser, when this GStreamer has it, as a pipeline step.
fn step(gst: &Gst, element: Option<&str>) -> String {
    element
        .filter(|element| gst.has(element))
        .map(|element| format!("{element} ! "))
        .unwrap_or_default()
}

/// The pictures' part of a pipeline, from `appsrc name=video` to before the
/// muxer.
fn video_branch(gst: &Gst, encoder: VideoEncoder, container: &Container) -> String {
    let (element, format) = encoder;
    format!(
        "appsrc name=video ! videoconvert name=convert ! video/x-raw,format={format} ! {element} name=encode ! {}{} ! queue",
        step(gst, container.parser),
        container.video_caps
    )
}

/// The sound's part, from `appsrc name=audio` to before the muxer.
fn audio_branch(gst: &Gst, encoder: (&str, &str), container: &Container) -> String {
    let (element, caps) = encoder;
    format!(
        "appsrc name=audio ! audioconvert ! audioresample ! {element} name=audioencode ! {}{caps} ! queue",
        step(gst, container.audio_parser),
    )
}

/// Sets `element`'s settings for `target`.
fn set_up(gst: &Gst, element: *mut c_void, name: &str, target: &Target) {
    let key_frames = ((target.fps * 2.).round() as u32).max(1);
    let threads = std::thread::available_parallelism().map_or(4, |count| count.get().min(16));
    for (property, setting) in settings(name) {
        let value = match setting {
            Setting::Kilobits => (target.bitrate / 1000).max(1).to_string(),
            Setting::Bits => target.bitrate.max(1000).to_string(),
            Setting::KeyFrames => key_frames.to_string(),
            Setting::Threads => threads.to_string(),
            Setting::Text(text) => text.to_string(),
        };
        // SAFETY: a live element of the pipeline.
        unsafe { gst.set_if_known(element, property, &value) };
    }
}

/// Sets up a writing pipeline's elements: what goes in, and the encoders
/// (`video` and `audio`, by element name, when there).
fn configure(
    pipeline: &mut Pipeline,
    target: &Target,
    video: Option<&str>,
    audio: Option<&str>,
) -> Result<()> {
    let gst = pipeline.gst;
    unsafe {
        if let Some(name) = video {
            let source = pipeline.named("video")?;
            let (width, height) = target.size;
            let (num, den) = media::rational(target.fps);
            let caps = (gst.caps_from_string)(
                c(&format!(
                    "video/x-raw,format=RGBA,width={width},height={height},framerate={num}/{den},pixel-aspect-ratio=1/1,interlace-mode=progressive"
                ))
                .as_ptr(),
            );
            if caps.is_null() {
                bail!("GStreamer does not take {width}×{height} frames");
            }
            (gst.object_set)(source, c("caps").as_ptr(), caps, std::ptr::null::<c_char>());
            (gst.mini_object_unref)(caps);
            gst.set_if_known(source, "format", "time");
            // Pushing never blocks (an error further on would leave it
            // waiting for good); `GstEncoder::push` waits for room instead.
            gst.set_if_known(source, "block", "false");
            if let Ok(convert) = pipeline.named("convert") {
                gst.set_if_known(convert, "n-threads", "0");
            }
            let encoder = pipeline.named("encode")?;
            set_up(gst, encoder, name, target);
        }
        if let Some(name) = audio {
            let source = pipeline.named("audio")?;
            let caps = (gst.caps_from_string)(
                c(&format!(
                    "audio/x-raw,format=S16LE,layout=interleaved,rate={RATE},channels=2"
                ))
                .as_ptr(),
            );
            if caps.is_null() {
                bail!("GStreamer does not take the sound");
            }
            (gst.object_set)(source, c("caps").as_ptr(), caps, std::ptr::null::<c_char>());
            (gst.mini_object_unref)(caps);
            gst.set_if_known(source, "format", "time");
            // Sound never makes pushing wait: it is fed only a little ahead
            // of the pictures, so it stays small.
            gst.set_if_known(source, "block", "false");
            gst.set_if_known(source, "max-bytes", "268435456");
            let encoder = pipeline.named("audioencode")?;
            set_up(gst, encoder, name, target);
        }
    }
    Ok(())
}

/// A GStreamer buffer of `length` bytes filled by `fill`, starting at
/// `pts` for `duration` (nanoseconds).
fn buffer(
    gst: &Gst,
    length: usize,
    pts: u64,
    duration: u64,
    fill: impl FnOnce(&mut [u8]),
) -> Result<*mut c_void> {
    unsafe {
        let buffer = (gst.buffer_new_allocate)(std::ptr::null_mut(), length, std::ptr::null_mut());
        if buffer.is_null() {
            bail!("GStreamer gave no memory for a frame");
        }
        let mut map: GstMapInfo = std::mem::zeroed();
        if (gst.buffer_map)(buffer, &mut map, GST_MAP_WRITE) == 0
            || map.data.is_null()
            || map.size < length
        {
            if !map.data.is_null() {
                (gst.buffer_unmap)(buffer, &mut map);
            }
            (gst.mini_object_unref)(buffer);
            bail!("GStreamer gave too little memory for a frame");
        }
        fill(std::slice::from_raw_parts_mut(map.data, length));
        (gst.buffer_unmap)(buffer, &mut map);
        // A new buffer is ours alone, so its times can be set.
        let fields = &mut *(buffer as *mut GstBuffer);
        fields.pts = pts;
        fields.duration = duration;
        Ok(buffer)
    }
}

/// Waits until `source` holds less than `limit` bytes, or the pipeline
/// failed.
fn wait_room(pipeline: &Pipeline, source: *mut c_void, limit: u64) -> Result<()> {
    let started = Instant::now();
    loop {
        let mut level = 0u64;
        unsafe {
            (pipeline.gst.object_get)(
                source,
                c("current-level-bytes").as_ptr(),
                &mut level as *mut u64,
                std::ptr::null::<c_char>(),
            );
        }
        if level < limit {
            return Ok(());
        }
        if let Some(error) = pipeline.error() {
            bail!(error);
        }
        if started.elapsed() > NEXT {
            bail!("GStreamer stopped taking frames");
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Pushes `buffer` (handing it over) into `source`.
fn push(pipeline: &Pipeline, source: *mut c_void, buffer: *mut c_void, what: &str) -> Result<()> {
    let flow = unsafe { (pipeline.gst.app_src_push_buffer)(source, buffer) };
    if flow != GST_FLOW_OK {
        return Err(pipeline.fail(&format!("GStreamer stopped taking {what}")));
    }
    Ok(())
}

/// Whether a short run of a pipeline with `branch` (pictures when `video`,
/// else sound) ends without error: an encoder can be listed and still not
/// work here (a hardware one without its driver).
fn works(branch: &str, encoder: &str, target: &Target, video: bool) -> bool {
    let trial = || -> Result<()> {
        let mut pipeline = Pipeline::launch(&format!("{branch} ! fakesink sync=false"))?;
        if video {
            configure(&mut pipeline, target, Some(encoder), None)?;
        } else {
            configure(&mut pipeline, target, None, Some(encoder))?;
        }
        let source = pipeline.named(if video { "video" } else { "audio" })?;
        pipeline.set_state(GST_STATE_PLAYING)?;
        let gst = pipeline.gst;
        if video {
            let (width, height) = target.size;
            let length = width as usize * height as usize * 4;
            let frame = nanos(Duration::from_secs_f64(1. / target.fps.max(1.)));
            for index in 0..2 {
                let buffer = buffer(gst, length, index * frame, frame, |bytes| bytes.fill(0))?;
                push(&pipeline, source, buffer, "frames")?;
            }
        } else {
            let frames = RATE as usize / 10;
            let buffer = buffer(
                gst,
                frames * 4,
                0,
                nanos(Duration::from_millis(100)),
                |bytes| bytes.fill(0),
            )?;
            push(&pipeline, source, buffer, "sound")?;
        }
        unsafe { (gst.app_src_end_of_stream)(source) };
        pipeline.wait_end(Duration::from_secs(10))
    };
    trial().is_ok()
}

/// Writes H.264, HEVC or VP9 (and AAC, Opus or Vorbis sound) with GStreamer.
pub struct GstEncoder {
    video: *mut c_void,
    sound: Option<(*mut c_void, Track<GstPcm>)>,
    /// Frames of the sound dropped from its start (see [`priming`]).
    skip: u64,
    size: (u32, u32),
    /// One frame's length, nanoseconds.
    frame: u64,
    lead: f64,
    pipeline: Pipeline,
}

pub fn encoder(source: &Path, out: &Path, target: &Target) -> Result<GstEncoder> {
    let gst = loaded()?;
    let container = container(target.codec);
    let name = match target.codec {
        Codec::H264 => "H.264",
        Codec::Hevc => "HEVC",
        Codec::Vp9 => "VP9",
    };
    if !gst.has(container.muxer) {
        bail!(
            "GStreamer has no {} here: install `gstreamer1.0-plugins-good`",
            container.muxer
        );
    }
    let listed: Vec<VideoEncoder> = video_encoders(target.codec)
        .iter()
        .copied()
        .filter(|(encoder, _)| gst.has(encoder))
        .collect();
    if listed.is_empty() {
        bail!(match target.codec {
            Codec::H264 =>
                "GStreamer has no H.264 encoder here: install one (for example `gstreamer1.0-plugins-ugly` for x264)",
            Codec::Hevc =>
                "GStreamer has no HEVC encoder here: install one (for example `gstreamer1.0-plugins-bad` for x265)",
            Codec::Vp9 => "GStreamer has no VP9 encoder here: install `gstreamer1.0-plugins-good`",
        });
    }
    let video = listed
        .into_iter()
        .find(|encoder| {
            works(
                &video_branch(gst, *encoder, &container),
                encoder.0,
                target,
                true,
            )
        })
        .with_context(|| match container.parser.filter(|parser| !gst.has(parser)) {
            Some(parser) => format!(
                "GStreamer cannot write {name} here without {parser}: install `gstreamer1.0-plugins-bad`"
            ),
            None => format!("GStreamer's {name} encoders here do not work"),
        })?;
    let audio = if target.audio.is_empty() {
        None
    } else {
        let encoder = container
            .audio
            .iter()
            .copied()
            .filter(|(encoder, _)| gst.has(encoder))
            .find(|encoder| {
                works(
                    &audio_branch(gst, *encoder, &container),
                    encoder.0,
                    target,
                    false,
                )
            })
            .context(if target.codec == Codec::Vp9 {
                "GStreamer has no Opus or Vorbis encoder here: install `gstreamer1.0-plugins-base`, or turn Sound off in Export"
            } else {
                "GStreamer has no AAC encoder here: install `gstreamer1.0-libav`, or turn Sound off in Export"
            })?;
        Some(encoder)
    };
    // The sound's reader first: when it cannot read the sound, nothing is
    // written.
    let reader = match audio {
        Some(_) => Some(GstPcm::open(source)?),
        None => None,
    };
    let mut description = format!(
        "{} ! {} name=mux ! filesink name=out",
        video_branch(gst, video, &container),
        container.muxer
    );
    if let Some(encoder) = audio {
        description.push_str(&format!(
            "  {} ! mux.",
            audio_branch(gst, encoder, &container)
        ));
    }
    let mut pipeline = Pipeline::launch(&description)?;
    configure(
        &mut pipeline,
        target,
        Some(video.0),
        audio.map(|(encoder, _)| encoder),
    )?;
    let mux = pipeline.named("mux")?;
    let sink = pipeline.named("out")?;
    let location = location(out)?;
    unsafe {
        // The index first, so a player can start before all of it came.
        gst.set_if_known(mux, "faststart", "true");
        (gst.object_set)(
            sink,
            c("location").as_ptr(),
            location.as_ptr(),
            std::ptr::null::<c_char>(),
        );
    }
    let video_source = pipeline.named("video")?;
    let sound = match reader {
        Some(reader) => Some((
            pipeline.named("audio")?,
            Track::new(reader, target.audio.clone(), target.seconds, RATE),
        )),
        None => None,
    };
    pipeline.set_state(GST_STATE_PLAYING)?;
    Ok(GstEncoder {
        video: video_source,
        sound,
        skip: audio.map_or(0, |(encoder, _)| priming(encoder)),
        size: target.size,
        frame: (NANOS / target.fps.max(1.)).round() as u64,
        lead: (2. / target.fps.max(1.)).max(0.5),
        pipeline,
    })
}

impl GstEncoder {
    /// Pushes the result's sound up to second `until`.
    fn feed(&mut self, until: f64) -> Result<()> {
        let Some((source, track)) = &mut self.sound else {
            return Ok(());
        };
        let gst = self.pipeline.gst;
        // The sound is fed `skip` frames ahead (the encoder's priming makes
        // up for them).
        let until = until + self.skip as f64 / track.rate() as f64;
        while let Some(block) = track.next(until)? {
            let frames = block.len() as u64 / 2;
            let first = track.given() - frames;
            let dropped = self.skip.saturating_sub(first).min(frames);
            let block = &block[dropped as usize * 2..];
            if block.is_empty() {
                continue;
            }
            let first = first + dropped - self.skip;
            let frames = frames - dropped;
            let rate = track.rate() as u128;
            let at = |frame: u64| (frame as u128 * 1_000_000_000 / rate) as u64;
            let buffer = buffer(
                gst,
                block.len() * 2,
                at(first),
                at(first + frames) - at(first),
                |bytes| {
                    for (to, from) in bytes.as_chunks_mut::<2>().0.iter_mut().zip(block) {
                        to.copy_from_slice(&from.to_le_bytes());
                    }
                },
            )?;
            push(&self.pipeline, *source, buffer, "sound")?;
        }
        Ok(())
    }
}

impl Encoder for GstEncoder {
    fn push(&mut self, frame: &RgbaImage, time: f64) -> Result<()> {
        if frame.dimensions() != self.size {
            bail!("a frame did not fit the encoder");
        }
        let pixels = frame.as_raw();
        // Two frames waiting at most.
        wait_room(&self.pipeline, self.video, pixels.len() as u64 * 2)?;
        let buffer = buffer(
            self.pipeline.gst,
            pixels.len(),
            seconds_to_nanos(time),
            self.frame,
            |bytes| bytes.copy_from_slice(pixels),
        )?;
        push(&self.pipeline, self.video, buffer, "frames")?;
        self.feed(time + self.lead)
    }

    fn finish(mut self: Box<Self>) -> Result<()> {
        self.feed(f64::INFINITY)?;
        unsafe {
            (self.pipeline.gst.app_src_end_of_stream)(self.video);
            if let Some((source, _)) = &self.sound {
                (self.pipeline.gst.app_src_end_of_stream)(*source);
            }
        }
        self.pipeline.wait_end(FINISH)
    }

    fn abort(self: Box<Self>) {
        // Dropping stops it and closes the file.
    }
}
