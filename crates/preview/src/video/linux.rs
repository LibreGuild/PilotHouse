//! Video on Linux: GStreamer's `playbin` decodes and plays (sound
//! included), and an `appsink` hands each frame over as BGRA pixels. A
//! `videoflip` stands a phone's recording upright, as its metadata asks.

use std::ffi::{c_char, c_uint, c_void};
use std::path::Path;
use std::sync::Arc;

use gpui_kit::RenderImage;

use super::gst::{
    GST_FORMAT_TIME, GST_MAP_READ, GST_MESSAGE_ERROR, GST_SEEK_FLAG_ACCURATE, GST_SEEK_FLAG_FLUSH,
    GST_SEEK_TYPE_NONE, GST_SEEK_TYPE_SET, GST_STATE_NULL, GST_STATE_PAUSED, GST_STATE_PLAYING,
    Gboolean, Gst, GstMapInfo, MISSING, NANOS, c, gst, with_hint,
};
use super::{Frame, Status};

pub struct Engine {
    gst: &'static Gst,
    playbin: *mut c_void,
    sink: *mut c_void,
    bus: *mut c_void,
    playing: std::cell::Cell<bool>,
    rate: std::cell::Cell<f64>,
    error: std::cell::RefCell<Option<String>>,
    size: std::cell::Cell<Option<(f32, f32)>>,
    /// The paused picture (after opening or a seek) was taken.
    preroll_taken: std::cell::Cell<bool>,
}

impl Engine {
    pub fn open(path: &Path) -> Result<Self, String> {
        let gst = gst()?;
        unsafe {
            let playbin = (gst.element_factory_make)(c("playbin").as_ptr(), std::ptr::null());
            let sink = (gst.element_factory_make)(c("appsink").as_ptr(), std::ptr::null());
            if playbin.is_null() || sink.is_null() {
                for element in [playbin, sink] {
                    if !element.is_null() {
                        (gst.object_unref)(element);
                    }
                }
                return Err(MISSING.into());
            }
            let caps = (gst.caps_from_string)(c("video/x-raw,format=BGRA").as_ptr());
            (gst.object_set)(sink, c("caps").as_ptr(), caps, std::ptr::null::<c_char>());
            (gst.mini_object_unref)(caps);
            (gst.object_set)(
                sink,
                c("max-buffers").as_ptr(),
                2 as c_uint,
                std::ptr::null::<c_char>(),
            );
            (gst.object_set)(
                sink,
                c("drop").as_ptr(),
                1 as Gboolean,
                std::ptr::null::<c_char>(),
            );
            (gst.object_set)(
                playbin,
                c("video-sink").as_ptr(),
                sink,
                std::ptr::null::<c_char>(),
            );
            // Upright, when this GStreamer can (1.10 on).
            let flip = (gst.element_factory_make)(c("videoflip").as_ptr(), std::ptr::null());
            if !flip.is_null() {
                if gst.set_if_known(flip, "video-direction", "auto") {
                    (gst.object_set)(
                        playbin,
                        c("video-filter").as_ptr(),
                        flip,
                        std::ptr::null::<c_char>(),
                    );
                } else {
                    (gst.object_ref_sink)(flip);
                    (gst.object_unref)(flip);
                }
            }
            let file = c(&path.to_string_lossy());
            let uri = (gst.filename_to_uri)(file.as_ptr(), std::ptr::null_mut());
            if uri.is_null() {
                (gst.object_unref)(playbin);
                return Err("The video's path cannot be read.".into());
            }
            (gst.object_set)(playbin, c("uri").as_ptr(), uri, std::ptr::null::<c_char>());
            (gst.free)(uri.cast());
            let bus = (gst.get_bus)(playbin);
            // Paused: the first frame and the length come in.
            (gst.set_state)(playbin, GST_STATE_PAUSED);
            Ok(Self {
                gst,
                playbin,
                sink,
                bus,
                playing: false.into(),
                rate: 1.0.into(),
                error: None.into(),
                size: None.into(),
                preroll_taken: false.into(),
            })
        }
    }

    fn check_errors(&self) {
        unsafe {
            let message = (self.gst.bus_pop_filtered)(self.bus, GST_MESSAGE_ERROR);
            if message.is_null() {
                return;
            }
            let text = with_hint(self.gst.error_text(message));
            (self.gst.mini_object_unref)(message);
            *self.error.borrow_mut() = Some(text);
        }
    }

    pub fn status(&self) -> Status {
        self.check_errors();
        if let Some(error) = self.error.borrow().clone() {
            return Status::Failed(error);
        }
        if self.duration().is_some() {
            Status::Ready
        } else {
            Status::Loading
        }
    }

    pub fn duration(&self) -> Option<f64> {
        let mut nanos = 0i64;
        let known = unsafe { (self.gst.query_duration)(self.playbin, GST_FORMAT_TIME, &mut nanos) };
        (known != 0 && nanos > 0).then(|| nanos as f64 / NANOS)
    }

    pub fn position(&self) -> f64 {
        let mut nanos = 0i64;
        let known = unsafe { (self.gst.query_position)(self.playbin, GST_FORMAT_TIME, &mut nanos) };
        if known != 0 {
            nanos.max(0) as f64 / NANOS
        } else {
            0.
        }
    }

    pub fn size(&self) -> Option<(f32, f32)> {
        self.size.get()
    }

    pub fn play(&self, rate: f32) {
        self.playing.set(true);
        unsafe {
            (self.gst.set_state)(self.playbin, GST_STATE_PLAYING);
        }
        if (rate as f64 - self.rate.get()).abs() > f64::EPSILON {
            self.set_rate(rate);
        }
    }

    pub fn pause(&self) {
        self.playing.set(false);
        unsafe {
            (self.gst.set_state)(self.playbin, GST_STATE_PAUSED);
        }
    }

    pub fn is_playing(&self) -> bool {
        if !self.playing.get() {
            return false;
        }
        // At the end it stops by itself.
        match self.duration() {
            Some(duration) if self.position() >= duration - 0.05 => {
                self.pause();
                false
            }
            _ => true,
        }
    }

    pub fn set_rate(&self, rate: f32) {
        self.rate.set(rate as f64);
        self.seek_at(self.position(), rate as f64);
    }

    pub fn seek(&self, seconds: f64) {
        self.seek_at(seconds, self.rate.get());
    }

    fn seek_at(&self, seconds: f64, rate: f64) {
        self.preroll_taken.set(false);
        unsafe {
            (self.gst.seek)(
                self.playbin,
                rate,
                GST_FORMAT_TIME,
                GST_SEEK_FLAG_FLUSH | GST_SEEK_FLAG_ACCURATE,
                GST_SEEK_TYPE_SET,
                (seconds.max(0.) * NANOS) as i64,
                GST_SEEK_TYPE_NONE,
                -1,
            );
        }
    }

    pub fn set_muted(&self, muted: bool) {
        unsafe {
            (self.gst.object_set)(
                self.playbin,
                c("mute").as_ptr(),
                muted as Gboolean,
                std::ptr::null::<c_char>(),
            );
        }
    }

    /// The next decoded frame, if one is waiting.
    pub fn frame(&self) -> Option<Frame> {
        unsafe {
            let mut sample = (self.gst.try_pull_sample)(self.sink, 0);
            // Paused, the picture waiting is the preroll.
            if sample.is_null() && !self.playing.get() && !self.preroll_taken.get() {
                sample = (self.gst.try_pull_preroll)(self.sink, 0);
                self.preroll_taken.set(!sample.is_null());
            }
            if sample.is_null() {
                return None;
            }
            let frame = self.read_sample(sample);
            (self.gst.mini_object_unref)(sample);
            frame
        }
    }

    unsafe fn read_sample(&self, sample: *mut c_void) -> Option<Frame> {
        unsafe {
            let caps = (self.gst.sample_get_caps)(sample);
            let structure = (self.gst.caps_get_structure)(caps, 0);
            let (mut width, mut height) = (0, 0);
            (self.gst.structure_get_int)(structure, c("width").as_ptr(), &mut width);
            (self.gst.structure_get_int)(structure, c("height").as_ptr(), &mut height);
            if width <= 0 || height <= 0 {
                return None;
            }
            self.size.set(Some((width as f32, height as f32)));
            let buffer = (self.gst.sample_get_buffer)(sample);
            let mut map: GstMapInfo = std::mem::zeroed();
            if (self.gst.buffer_map)(buffer, &mut map, GST_MAP_READ) == 0 {
                return None;
            }
            let data = std::slice::from_raw_parts(map.data, map.size);
            let (width, height) = (width as usize, height as usize);
            let stride = (map.size / height).max(width * 4);
            let mut pixels = Vec::with_capacity(width * height * 4);
            for row in data.chunks(stride).take(height) {
                pixels.extend_from_slice(&row[..(width * 4).min(row.len())]);
            }
            (self.gst.buffer_unmap)(buffer, &mut map);
            pixels.resize(width * height * 4, 255);
            for alpha in pixels.iter_mut().skip(3).step_by(4) {
                *alpha = 255;
            }
            let buffer = ::image::RgbaImage::from_raw(width as u32, height as u32, pixels)?;
            Some(Frame::Image(Arc::new(RenderImage::new(vec![
                ::image::Frame::new(buffer),
            ]))))
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        unsafe {
            (self.gst.set_state)(self.playbin, GST_STATE_NULL);
            (self.gst.object_unref)(self.bus);
            // The playbin owns the sink.
            (self.gst.object_unref)(self.playbin);
        }
    }
}
