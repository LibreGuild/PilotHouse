//! Media Foundation's few plain functions, loaded when first needed rather
//! than linked: Windows N and KN editions have no Media Foundation until the
//! Media Feature Pack is installed, and PilotHouse still starts there (a
//! video then says what to install). Its COM interfaces need no loading.
//! The player here and Studio's video reading and writing share these.

use std::ffi::c_void;
use std::sync::OnceLock;

use windows::Win32::Foundation::{HANDLE, HMODULE};
use windows::Win32::System::LibraryLoader::{
    GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};
use windows::core::{GUID, HRESULT, PCSTR, PCWSTR, w};

/// The functions, each as the DLL exports it.
pub struct Mf {
    pub startup: unsafe extern "system" fn(u32, u32) -> HRESULT,
    pub shutdown: unsafe extern "system" fn() -> HRESULT,
    pub create_attributes: unsafe extern "system" fn(*mut *mut c_void, u32) -> HRESULT,
    pub create_media_type: unsafe extern "system" fn(*mut *mut c_void) -> HRESULT,
    pub create_memory_buffer: unsafe extern "system" fn(u32, *mut *mut c_void) -> HRESULT,
    pub create_sample: unsafe extern "system" fn(*mut *mut c_void) -> HRESULT,
    pub create_source_reader_from_url:
        unsafe extern "system" fn(PCWSTR, *mut c_void, *mut *mut c_void) -> HRESULT,
    pub create_sink_writer_from_url:
        unsafe extern "system" fn(PCWSTR, *mut c_void, *mut c_void, *mut *mut c_void) -> HRESULT,
    /// `MFTEnumEx`: the codecs of a kind (the type infos are
    /// `MFT_REGISTER_TYPE_INFO`, the list `IMFActivate` pointers).
    pub enum_transforms: unsafe extern "system" fn(
        GUID,
        u32,
        *const c_void,
        *const c_void,
        *mut *mut *mut c_void,
        *mut u32,
    ) -> HRESULT,
}

pub const MISSING: &str = "Video needs Windows' Media Feature Pack, which this edition of \
     Windows does not have. Install it (Settings, Apps, Optional features) and open the file again.";

/// Media Foundation's functions, loaded the first time (from System32
/// only).
pub fn mf() -> Result<&'static Mf, String> {
    static MF: OnceLock<Result<Mf, String>> = OnceLock::new();
    MF.get_or_init(|| unsafe { load() })
        .as_ref()
        .map_err(Clone::clone)
}

// Each address is transmuted to the type of the field it fills.
#[allow(clippy::missing_transmute_annotations)]
unsafe fn load() -> Result<Mf, String> {
    let open = |name: PCWSTR| {
        unsafe { LoadLibraryExW(name, HANDLE::default(), LOAD_LIBRARY_SEARCH_SYSTEM32) }
            .map_err(|_| MISSING.to_string())
    };
    // Kept loaded for good.
    let plat: HMODULE = open(w!("mfplat.dll"))?;
    let readwrite: HMODULE = open(w!("mfreadwrite.dll"))?;
    macro_rules! symbol {
        ($module:expr, $name:literal) => {{
            let address = unsafe { GetProcAddress($module, PCSTR(concat!($name, "\0").as_ptr())) }
                .ok_or_else(|| format!("Windows' Media Foundation has no {}.", $name))?;
            // SAFETY: the export has the signature of the field it fills.
            unsafe { std::mem::transmute::<unsafe extern "system" fn() -> isize, _>(address) }
        }};
    }
    Ok(Mf {
        startup: symbol!(plat, "MFStartup"),
        shutdown: symbol!(plat, "MFShutdown"),
        create_attributes: symbol!(plat, "MFCreateAttributes"),
        create_media_type: symbol!(plat, "MFCreateMediaType"),
        create_memory_buffer: symbol!(plat, "MFCreateMemoryBuffer"),
        create_sample: symbol!(plat, "MFCreateSample"),
        create_source_reader_from_url: symbol!(readwrite, "MFCreateSourceReaderFromURL"),
        create_sink_writer_from_url: symbol!(readwrite, "MFCreateSinkWriterFromURL"),
        enum_transforms: symbol!(plat, "MFTEnumEx"),
    })
}
