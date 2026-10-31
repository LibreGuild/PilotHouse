//! GStreamer, loaded when first needed, not linked: PilotHouse starts on a
//! system without it, and a video then says what to install. The player
//! here and Studio's video reading and writing share these calls.
//!
//! Only calls GStreamer 1.10 (2016) already had are used. Buffer times are
//! read and written through `GstBuffer`'s public fields, as GStreamer's own
//! macros do (their layout is fixed for all of 1.x).

use std::ffi::{CStr, CString, c_char, c_int, c_uint, c_ulong, c_void};
use std::ptr::NonNull;
use std::sync::OnceLock;

use libloading::Library;

pub type Gboolean = c_int;
pub const GST_STATE_NULL: c_int = 1;
pub const GST_STATE_PAUSED: c_int = 3;
pub const GST_STATE_PLAYING: c_int = 4;
pub const GST_STATE_CHANGE_FAILURE: c_int = 0;
pub const GST_FORMAT_TIME: c_int = 3;
pub const GST_SEEK_FLAG_FLUSH: c_int = 1 << 0;
pub const GST_SEEK_FLAG_ACCURATE: c_int = 1 << 1;
pub const GST_SEEK_TYPE_NONE: c_int = 0;
pub const GST_SEEK_TYPE_SET: c_int = 1;
pub const GST_MESSAGE_EOS: c_uint = 1 << 0;
pub const GST_MESSAGE_ERROR: c_uint = 1 << 1;
pub const GST_MAP_READ: c_uint = 1;
pub const GST_MAP_WRITE: c_uint = 2;
pub const GST_PAD_LINK_OK: c_int = 0;
pub const GST_FLOW_OK: c_int = 0;
/// An unknown time.
pub const GST_CLOCK_TIME_NONE: u64 = u64::MAX;
pub const NANOS: f64 = 1_000_000_000.;

#[repr(C)]
pub struct GError {
    pub domain: u32,
    pub code: c_int,
    pub message: *const c_char,
}

#[repr(C)]
pub struct GstMapInfo {
    pub memory: *mut c_void,
    pub flags: c_uint,
    pub data: *mut u8,
    pub size: usize,
    pub maxsize: usize,
    pub user_data: [*mut c_void; 4],
    pub reserved: [*mut c_void; 4],
}

#[repr(C)]
pub struct GstMiniObject {
    pub type_: usize,
    pub refcount: c_int,
    pub lockstate: c_int,
    pub flags: c_uint,
    pub copy: *mut c_void,
    pub dispose: *mut c_void,
    pub free: *mut c_void,
    pub priv_uint: c_uint,
    pub priv_pointer: *mut c_void,
}

/// The start of `GstBuffer`: its times.
#[repr(C)]
pub struct GstBuffer {
    pub mini_object: GstMiniObject,
    pub pool: *mut c_void,
    pub pts: u64,
    pub dts: u64,
    pub duration: u64,
    pub offset: u64,
    pub offset_end: u64,
}

/// The start of `GstMessage`: its type.
#[repr(C)]
pub struct GstMessage {
    pub mini_object: GstMiniObject,
    pub type_: c_uint,
}

/// A signal handler, cast to the type it really has when connected.
pub type GCallback = unsafe extern "C" fn();

/// The GStreamer calls PilotHouse makes, loaded once.
pub struct Gst {
    _libraries: Vec<Library>,
    pub element_factory_make: unsafe extern "C" fn(*const c_char, *const c_char) -> *mut c_void,
    pub element_factory_find: unsafe extern "C" fn(*const c_char) -> *mut c_void,
    pub parse_launch: unsafe extern "C" fn(*const c_char, *mut *mut GError) -> *mut c_void,
    pub bin_get_by_name: unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void,
    pub object_set: unsafe extern "C" fn(*mut c_void, *const c_char, ...),
    pub object_get: unsafe extern "C" fn(*mut c_void, *const c_char, ...),
    pub object_class_find_property: unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void,
    pub util_set_object_arg: unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char),
    pub signal_connect_data: unsafe extern "C" fn(
        *mut c_void,
        *const c_char,
        GCallback,
        *mut c_void,
        *mut c_void,
        c_int,
    ) -> c_ulong,
    pub object_ref_sink: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    pub object_unref: unsafe extern "C" fn(*mut c_void),
    pub caps_from_string: unsafe extern "C" fn(*const c_char) -> *mut c_void,
    pub filename_to_uri: unsafe extern "C" fn(*const c_char, *mut *mut GError) -> *mut c_char,
    pub free: unsafe extern "C" fn(*mut c_void),
    pub error_free: unsafe extern "C" fn(NonNull<GError>),
    pub set_state: unsafe extern "C" fn(*mut c_void, c_int) -> c_int,
    pub get_state: unsafe extern "C" fn(*mut c_void, *mut c_int, *mut c_int, u64) -> c_int,
    pub query_duration: unsafe extern "C" fn(*mut c_void, c_int, *mut i64) -> Gboolean,
    pub query_position: unsafe extern "C" fn(*mut c_void, c_int, *mut i64) -> Gboolean,
    pub seek:
        unsafe extern "C" fn(*mut c_void, f64, c_int, c_int, c_int, i64, c_int, i64) -> Gboolean,
    pub get_bus: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    pub bus_pop: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    pub bus_pop_filtered: unsafe extern "C" fn(*mut c_void, c_uint) -> *mut c_void,
    pub bus_timed_pop_filtered: unsafe extern "C" fn(*mut c_void, u64, c_uint) -> *mut c_void,
    // The outputs as `Option<NonNull<_>>`, the same as nullable pointers:
    // no null pointer is ever read.
    pub message_parse_error: unsafe extern "C" fn(
        *mut c_void,
        *mut Option<NonNull<GError>>,
        *mut Option<NonNull<c_char>>,
    ),
    pub mini_object_unref: unsafe extern "C" fn(*mut c_void),
    pub element_get_static_pad: unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void,
    pub pad_link: unsafe extern "C" fn(*mut c_void, *mut c_void) -> c_int,
    pub pad_is_linked: unsafe extern "C" fn(*mut c_void) -> Gboolean,
    pub pad_get_current_caps: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    pub pad_query_caps: unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void,
    pub try_pull_sample: unsafe extern "C" fn(*mut c_void, u64) -> *mut c_void,
    pub try_pull_preroll: unsafe extern "C" fn(*mut c_void, u64) -> *mut c_void,
    pub app_sink_is_eos: unsafe extern "C" fn(*mut c_void) -> Gboolean,
    pub app_src_push_buffer: unsafe extern "C" fn(*mut c_void, *mut c_void) -> c_int,
    pub app_src_end_of_stream: unsafe extern "C" fn(*mut c_void) -> c_int,
    pub buffer_new_allocate: unsafe extern "C" fn(*mut c_void, usize, *mut c_void) -> *mut c_void,
    pub sample_get_buffer: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    pub sample_get_caps: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    pub sample_get_segment: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    /// `gst_segment_to_stream_time_full`: 1 or -1 (the time's sign), 0
    /// when it cannot.
    pub segment_to_stream_time_full:
        unsafe extern "C" fn(*mut c_void, c_int, u64, *mut u64) -> c_int,
    pub caps_get_structure: unsafe extern "C" fn(*mut c_void, c_uint) -> *mut c_void,
    pub structure_get_name: unsafe extern "C" fn(*mut c_void) -> *const c_char,
    pub structure_get_int: unsafe extern "C" fn(*mut c_void, *const c_char, *mut c_int) -> Gboolean,
    pub structure_get_fraction:
        unsafe extern "C" fn(*mut c_void, *const c_char, *mut c_int, *mut c_int) -> Gboolean,
    pub buffer_map: unsafe extern "C" fn(*mut c_void, *mut GstMapInfo, c_uint) -> Gboolean,
    pub buffer_unmap: unsafe extern "C" fn(*mut c_void, *mut GstMapInfo),
}

// SAFETY: plain function pointers into libraries kept loaded for good.
unsafe impl Send for Gst {}
unsafe impl Sync for Gst {}

pub const MISSING: &str = "Video needs GStreamer, which is not installed. Install it (for example \
     `sudo apt install gstreamer1.0-plugins-good gstreamer1.0-libav`) and open the file again.";

/// GStreamer, loaded and started the first time.
pub fn gst() -> Result<&'static Gst, String> {
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
        element_factory_find: symbol!(gstreamer, "gst_element_factory_find"),
        parse_launch: symbol!(gstreamer, "gst_parse_launch"),
        bin_get_by_name: symbol!(gstreamer, "gst_bin_get_by_name"),
        object_set: symbol!(gobject, "g_object_set"),
        object_get: symbol!(gobject, "g_object_get"),
        object_class_find_property: symbol!(gobject, "g_object_class_find_property"),
        util_set_object_arg: symbol!(gstreamer, "gst_util_set_object_arg"),
        signal_connect_data: symbol!(gobject, "g_signal_connect_data"),
        object_ref_sink: symbol!(gstreamer, "gst_object_ref_sink"),
        object_unref: symbol!(gstreamer, "gst_object_unref"),
        caps_from_string: symbol!(gstreamer, "gst_caps_from_string"),
        filename_to_uri: symbol!(gstreamer, "gst_filename_to_uri"),
        free: symbol!(glib, "g_free"),
        error_free: symbol!(glib, "g_error_free"),
        set_state: symbol!(gstreamer, "gst_element_set_state"),
        get_state: symbol!(gstreamer, "gst_element_get_state"),
        query_duration: symbol!(gstreamer, "gst_element_query_duration"),
        query_position: symbol!(gstreamer, "gst_element_query_position"),
        seek: symbol!(gstreamer, "gst_element_seek"),
        get_bus: symbol!(gstreamer, "gst_element_get_bus"),
        bus_pop: symbol!(gstreamer, "gst_bus_pop"),
        bus_pop_filtered: symbol!(gstreamer, "gst_bus_pop_filtered"),
        bus_timed_pop_filtered: symbol!(gstreamer, "gst_bus_timed_pop_filtered"),
        message_parse_error: symbol!(gstreamer, "gst_message_parse_error"),
        mini_object_unref: symbol!(gstreamer, "gst_mini_object_unref"),
        element_get_static_pad: symbol!(gstreamer, "gst_element_get_static_pad"),
        pad_link: symbol!(gstreamer, "gst_pad_link"),
        pad_is_linked: symbol!(gstreamer, "gst_pad_is_linked"),
        pad_get_current_caps: symbol!(gstreamer, "gst_pad_get_current_caps"),
        pad_query_caps: symbol!(gstreamer, "gst_pad_query_caps"),
        try_pull_sample: symbol!(app, "gst_app_sink_try_pull_sample"),
        try_pull_preroll: symbol!(app, "gst_app_sink_try_pull_preroll"),
        app_sink_is_eos: symbol!(app, "gst_app_sink_is_eos"),
        app_src_push_buffer: symbol!(app, "gst_app_src_push_buffer"),
        app_src_end_of_stream: symbol!(app, "gst_app_src_end_of_stream"),
        buffer_new_allocate: symbol!(gstreamer, "gst_buffer_new_allocate"),
        sample_get_buffer: symbol!(gstreamer, "gst_sample_get_buffer"),
        sample_get_caps: symbol!(gstreamer, "gst_sample_get_caps"),
        sample_get_segment: symbol!(gstreamer, "gst_sample_get_segment"),
        segment_to_stream_time_full: symbol!(gstreamer, "gst_segment_to_stream_time_full"),
        caps_get_structure: symbol!(gstreamer, "gst_caps_get_structure"),
        structure_get_name: symbol!(gstreamer, "gst_structure_get_name"),
        structure_get_int: symbol!(gstreamer, "gst_structure_get_int"),
        structure_get_fraction: symbol!(gstreamer, "gst_structure_get_fraction"),
        buffer_map: symbol!(gstreamer, "gst_buffer_map"),
        buffer_unmap: symbol!(gstreamer, "gst_buffer_unmap"),
        _libraries: vec![gstreamer, app, gobject, glib],
    };
    Ok(gst)
}

/// `text` as a C string (one with a NUL in it, which no name has, is empty).
pub fn c(text: &str) -> CString {
    CString::new(text).unwrap_or_default()
}

impl Gst {
    /// Sets property `name` of `object` from text, when it has one by that
    /// name (elements gain properties over the versions). Whether it did.
    ///
    /// # Safety
    /// `object` is a live GObject.
    pub unsafe fn set_if_known(&self, object: *mut c_void, name: &str, value: &str) -> bool {
        let name = c(name);
        unsafe {
            // A GObject starts with its class.
            let class = *(object as *const *mut c_void);
            if (self.object_class_find_property)(class, name.as_ptr()).is_null() {
                return false;
            }
            (self.util_set_object_arg)(object, name.as_ptr(), c(value).as_ptr());
        }
        true
    }

    /// Whether this GStreamer has the element `name`.
    pub fn has(&self, name: &str) -> bool {
        unsafe {
            let factory = (self.element_factory_find)(c(name).as_ptr());
            if factory.is_null() {
                return false;
            }
            (self.object_unref)(factory);
        }
        true
    }

    /// The text of an error message, freeing what was parsed (not the
    /// message).
    ///
    /// # Safety
    /// `message` is a live error message.
    pub unsafe fn error_text(&self, message: *mut c_void) -> String {
        unsafe {
            let (mut error, mut debug) = (None, None);
            (self.message_parse_error)(message, &mut error, &mut debug);
            let text = error
                .map(|error| error.as_ref().message)
                .filter(|message| !message.is_null())
                .map(|message| CStr::from_ptr(message).to_string_lossy().into_owned())
                .unwrap_or_else(|| "GStreamer failed.".to_string());
            if let Some(error) = error {
                (self.error_free)(error);
            }
            if let Some(debug) = debug {
                (self.free)(debug.as_ptr().cast());
            }
            text
        }
    }

    /// The name of a caps' first structure (`video/x-raw`), or empty.
    ///
    /// # Safety
    /// `caps` is live caps or null.
    pub unsafe fn caps_name(&self, caps: *mut c_void) -> String {
        if caps.is_null() {
            return String::new();
        }
        unsafe {
            let structure = (self.caps_get_structure)(caps, 0);
            if structure.is_null() {
                return String::new();
            }
            let name = (self.structure_get_name)(structure);
            if name.is_null() {
                return String::new();
            }
            CStr::from_ptr(name).to_string_lossy().into_owned()
        }
    }
}

/// Adds the install hint to an error that says a plugin or codec is
/// missing.
pub fn with_hint(text: String) -> String {
    let lower = text.to_lowercase();
    if lower.contains("plugin") || lower.contains("decoder") || lower.contains("codec") {
        format!("{text} Install the codec (for example `gstreamer1.0-libav`).")
    } else {
        text
    }
}
