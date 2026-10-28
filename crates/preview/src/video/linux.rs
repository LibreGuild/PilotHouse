//! Video on Linux: GStreamer's `playbin` decodes and plays (sound
//! included), and an `appsink` hands each frame over as BGRA pixels.
//!
//! GStreamer is loaded when the first video opens, not linked: PilotHouse
//! starts on a system without it, and a video then says what to install.

use std::ffi::{CStr, CString, c_char, c_int, c_uint, c_void};
use std::path::Path;
use std::sync::{Arc, OnceLock};

use gpui_kit::RenderImage;
use libloading::Library;

use super::{Frame, Status};

type Gboolean = c_int;
const GST_STATE_NULL: c_int = 1;
const GST_STATE_PAUSED: c_int = 3;
const GST_STATE_PLAYING: c_int = 4;
const GST_FORMAT_TIME: c_int = 3;
const GST_SEEK_FLAG_FLUSH: c_int = 1 << 0;
const GST_SEEK_FLAG_ACCURATE: c_int = 1 << 1;
const GST_SEEK_TYPE_NONE: c_int = 0;
const GST_SEEK_TYPE_SET: c_int = 1;
const GST_MESSAGE_ERROR: c_uint = 1 << 1;
const GST_MAP_READ: c_uint = 1;
const NANOS: f64 = 1_000_000_000.;

#[repr(C)]
struct GError {
    domain: u32,
    code: c_int,
    message: *const c_char,
}

#[repr(C)]
struct GstMapInfo {
    memory: *mut c_void,
    flags: c_uint,
    data: *const u8,
    size: usize,
    maxsize: usize,
    user_data: [*mut c_void; 4],
    reserved: [*mut c_void; 4],
}

/// The GStreamer calls the player makes, loaded once.
struct Gst {
    _libraries: Vec<Library>,
    element_factory_make: unsafe extern "C" fn(*const c_char, *const c_char) -> *mut c_void,
    object_set: unsafe extern "C" fn(*mut c_void, *const c_char, ...),
    object_unref: unsafe extern "C" fn(*mut c_void),
    caps_from_string: unsafe extern "C" fn(*const c_char) -> *mut c_void,
    filename_to_uri: unsafe extern "C" fn(*const c_char, *mut *mut GError) -> *mut c_char,
    free: unsafe extern "C" fn(*mut c_void),
    error_free: unsafe extern "C" fn(*mut GError),
    set_state: unsafe extern "C" fn(*mut c_void, c_int) -> c_int,
    query_duration: unsafe extern "C" fn(*mut c_void, c_int, *mut i64) -> Gboolean,
    query_position: unsafe extern "C" fn(*mut c_void, c_int, *mut i64) -> Gboolean,
    seek: unsafe extern "C" fn(*mut c_void, f64, c_int, c_int, c_int, i64, c_int, i64) -> Gboolean,
    get_bus: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    bus_pop_filtered: unsafe extern "C" fn(*mut c_void, c_uint) -> *mut c_void,
    message_parse_error: unsafe extern "C" fn(*mut c_void, *mut *mut GError, *mut *mut c_char),
    mini_object_unref: unsafe extern "C" fn(*mut c_void),
    try_pull_sample: unsafe extern "C" fn(*mut c_void, u64) -> *mut c_void,
    try_pull_preroll: unsafe extern "C" fn(*mut c_void, u64) -> *mut c_void,
    sample_get_buffer: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    sample_get_caps: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    caps_get_structure: unsafe extern "C" fn(*mut c_void, c_uint) -> *mut c_void,
    structure_get_int: unsafe extern "C" fn(*mut c_void, *const c_char, *mut c_int) -> Gboolean,
    buffer_map: unsafe extern "C" fn(*mut c_void, *mut GstMapInfo, c_uint) -> Gboolean,
    buffer_unmap: unsafe extern "C" fn(*mut c_void, *mut GstMapInfo),
}

// SAFETY: plain function pointers into libraries kept loaded for good.
unsafe impl Send for Gst {}
unsafe impl Sync for Gst {}

const MISSING: &str = "Video needs GStreamer, which is not installed. Install it (for example \
     `sudo apt install gstreamer1.0-plugins-good gstreamer1.0-libav`) and open the file again.";

fn gst() -> Result<&'static Gst, String> {
    static GST: OnceLock<Result<Gst, String>> = OnceLock::new();
    GST.get_or_init(|| unsafe { load() })
        .as_ref()
        .map_err(Clone::clone)
}

unsafe fn load() -> Result<Gst, String> {
    let open = |name: &str| unsafe { Library::new(name) }.map_err(|_| MISSING.to_string());
    let gstreamer = open("libgstreamer-1.0.so.0")?;
    let app = open("libgstapp-1.0.so.0")?;
    let gobject = open("libgobject-2.0.so.0")?;
    let glib = open("libglib-2.0.so.0")?;
    macro_rules! symbol {
        ($library:expr, $name:literal) => {
            *unsafe { $library.get(concat!($name, "\0").as_bytes()) }
                .map_err(|_| format!("GStreamer is too old (no {}).", $name))?
        };
    }
    let init: unsafe extern "C" fn(
        *mut c_int,
        *mut *mut *mut c_char,
        *mut *mut GError,
    ) -> Gboolean = symbol!(gstreamer, "gst_init_check");
    if unsafe {
        init(
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err("GStreamer could not start.".into());
    }
    let gst = Gst {
        element_factory_make: symbol!(gstreamer, "gst_element_factory_make"),
        object_set: symbol!(gobject, "g_object_set"),
        object_unref: symbol!(gstreamer, "gst_object_unref"),
        caps_from_string: symbol!(gstreamer, "gst_caps_from_string"),
        filename_to_uri: symbol!(gstreamer, "gst_filename_to_uri"),
        free: symbol!(glib, "g_free"),
        error_free: symbol!(glib, "g_error_free"),
        set_state: symbol!(gstreamer, "gst_element_set_state"),
        query_duration: symbol!(gstreamer, "gst_element_query_duration"),
        query_position: symbol!(gstreamer, "gst_element_query_position"),
        seek: symbol!(gstreamer, "gst_element_seek"),
        get_bus: symbol!(gstreamer, "gst_element_get_bus"),
        bus_pop_filtered: symbol!(gstreamer, "gst_bus_pop_filtered"),
        message_parse_error: symbol!(gstreamer, "gst_message_parse_error"),
        mini_object_unref: symbol!(gstreamer, "gst_mini_object_unref"),
        try_pull_sample: symbol!(app, "gst_app_sink_try_pull_sample"),
        try_pull_preroll: symbol!(app, "gst_app_sink_try_pull_preroll"),
        sample_get_buffer: symbol!(gstreamer, "gst_sample_get_buffer"),
        sample_get_caps: symbol!(gstreamer, "gst_sample_get_caps"),
        caps_get_structure: symbol!(gstreamer, "gst_caps_get_structure"),
        structure_get_int: symbol!(gstreamer, "gst_structure_get_int"),
        buffer_map: symbol!(gstreamer, "gst_buffer_map"),
        buffer_unmap: symbol!(gstreamer, "gst_buffer_unmap"),
        _libraries: vec![gstreamer, app, gobject, glib],
    };
    Ok(gst)
}

fn c(text: &str) -> CString {
    CString::new(text).unwrap_or_default()
}

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
            let mut error: *mut GError = std::ptr::null_mut();
            let mut debug: *mut c_char = std::ptr::null_mut();
            (self.gst.message_parse_error)(message, &mut error, &mut debug);
            let text = if error.is_null() || (*error).message.is_null() {
                "GStreamer could not play it.".to_string()
            } else {
                CStr::from_ptr((*error).message)
                    .to_string_lossy()
                    .into_owned()
            };
            let lower = text.to_lowercase();
            let text =
                if lower.contains("plugin") || lower.contains("decoder") || lower.contains("codec")
                {
                    format!("{text} Install the codec (for example `gstreamer1.0-libav`).")
                } else {
                    text
                };
            if !error.is_null() {
                (self.gst.error_free)(error);
            }
            if !debug.is_null() {
                (self.gst.free)(debug.cast());
            }
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
