//! H.264 decoding with VideoToolbox (the hardware decoder), into BGRA.

use std::ffi::c_void;
use std::ptr::{self, NonNull};
use std::sync::Mutex;

use anyhow::{Result, bail};
use ph_media::Frame;

use super::nal_type;

/// The parameter sets' NAL unit types.
const SPS: u8 = 7;
const PPS: u8 = 8;

type OSStatus = i32;
type CFTypeRef = *const c_void;

/// Core Media's time (unused values are passed as zero).
#[repr(C)]
#[derive(Clone, Copy)]
struct CMTime {
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
}

type OutputCallback = unsafe extern "C" fn(
    refcon: *mut c_void,
    frame_refcon: *mut c_void,
    status: OSStatus,
    info: u32,
    image: *mut c_void,
    presentation: CMTime,
    duration: CMTime,
);

#[repr(C)]
struct OutputCallbackRecord {
    callback: OutputCallback,
    refcon: *mut c_void,
}

/// An opaque C structure, for taking the address of a constant.
#[repr(C)]
struct Opaque {
    _private: [u8; 0],
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFTypeDictionaryKeyCallBacks: Opaque;
    static kCFTypeDictionaryValueCallBacks: Opaque;
    fn CFDictionaryCreate(
        allocator: CFTypeRef,
        keys: *const CFTypeRef,
        values: *const CFTypeRef,
        count: isize,
        key_callbacks: *const Opaque,
        value_callbacks: *const Opaque,
    ) -> CFTypeRef;
    fn CFNumberCreate(allocator: CFTypeRef, kind: isize, value: *const c_void) -> CFTypeRef;
    fn CFRelease(object: CFTypeRef);
}

#[link(name = "CoreVideo", kind = "framework")]
unsafe extern "C" {
    static kCVPixelBufferPixelFormatTypeKey: CFTypeRef;
    fn CVPixelBufferLockBaseAddress(buffer: *mut c_void, flags: u64) -> i32;
    fn CVPixelBufferUnlockBaseAddress(buffer: *mut c_void, flags: u64) -> i32;
    fn CVPixelBufferGetBaseAddress(buffer: *mut c_void) -> *mut u8;
    fn CVPixelBufferGetBytesPerRow(buffer: *mut c_void) -> usize;
    fn CVPixelBufferGetWidth(buffer: *mut c_void) -> usize;
    fn CVPixelBufferGetHeight(buffer: *mut c_void) -> usize;
}

#[link(name = "CoreMedia", kind = "framework")]
unsafe extern "C" {
    fn CMVideoFormatDescriptionCreateFromH264ParameterSets(
        allocator: CFTypeRef,
        count: usize,
        pointers: *const *const u8,
        sizes: *const usize,
        nal_length_size: i32,
        out: *mut *mut c_void,
    ) -> OSStatus;
    fn CMBlockBufferCreateWithMemoryBlock(
        structure_allocator: CFTypeRef,
        memory: *mut c_void,
        length: usize,
        block_allocator: CFTypeRef,
        custom_source: *const c_void,
        offset: usize,
        data_length: usize,
        flags: u32,
        out: *mut *mut c_void,
    ) -> OSStatus;
    fn CMBlockBufferReplaceDataBytes(
        source: *const c_void,
        destination: *mut c_void,
        offset: usize,
        length: usize,
    ) -> OSStatus;
    fn CMSampleBufferCreateReady(
        allocator: CFTypeRef,
        data: *mut c_void,
        format: *mut c_void,
        samples: isize,
        timings: isize,
        timing: *const c_void,
        sizes_count: isize,
        sizes: *const usize,
        out: *mut *mut c_void,
    ) -> OSStatus;
}

#[link(name = "VideoToolbox", kind = "framework")]
unsafe extern "C" {
    fn VTDecompressionSessionCreate(
        allocator: CFTypeRef,
        format: *mut c_void,
        decoder_specification: CFTypeRef,
        image_attributes: CFTypeRef,
        callback: *const OutputCallbackRecord,
        out: *mut *mut c_void,
    ) -> OSStatus;
    fn VTDecompressionSessionDecodeFrame(
        session: *mut c_void,
        sample: *mut c_void,
        flags: u32,
        frame_refcon: *mut c_void,
        info: *mut u32,
    ) -> OSStatus;
    fn VTDecompressionSessionWaitForAsynchronousFrames(session: *mut c_void) -> OSStatus;
    fn VTDecompressionSessionInvalidate(session: *mut c_void);
}

/// `kCVPixelFormatType_32BGRA`: the layout GPUI draws.
const BGRA: i32 = i32::from_be_bytes(*b"BGRA");
const NUMBER_SINT32: isize = 3;
const BLOCK_ASSURE_MEMORY: u32 = 1;
const LOCK_READ_ONLY: u64 = 1;

/// Pictures the decoder hands back, through its callback.
type Out = Mutex<Option<Frame>>;

/// A decoding session for one set of parameter sets.
struct Session {
    format: NonNull<c_void>,
    session: NonNull<c_void>,
}

impl Drop for Session {
    fn drop(&mut self) {
        // SAFETY: both were made by `Session::new` and are released once.
        unsafe {
            VTDecompressionSessionInvalidate(self.session.as_ptr());
            CFRelease(self.session.as_ptr());
            CFRelease(self.format.as_ptr());
        }
    }
}

pub struct Decoder {
    sps: Vec<u8>,
    pps: Vec<u8>,
    session: Option<Session>,
    /// Boxed so its address, given to VideoToolbox, stays put.
    out: Box<Out>,
}

impl Decoder {
    pub fn new() -> Result<Self> {
        Ok(Self {
            sps: Vec::new(),
            pps: Vec::new(),
            session: None,
            out: Box::new(Mutex::new(None)),
        })
    }

    /// Decodes one picture's NAL units (no start codes): its picture, once
    /// the parameter sets and a key frame have been seen.
    pub fn decode(&mut self, nals: &[Vec<u8>]) -> Result<Option<Frame>> {
        let mut avcc = Vec::new();
        for nal in nals {
            match nal_type(nal) {
                SPS if *nal != self.sps => {
                    self.sps = nal.clone();
                    self.session = None;
                }
                PPS if *nal != self.pps => {
                    self.pps = nal.clone();
                    self.session = None;
                }
                SPS | PPS => {}
                _ => {
                    // AVCC: each NAL unit after its length.
                    avcc.extend_from_slice(&(nal.len() as u32).to_be_bytes());
                    avcc.extend_from_slice(nal);
                }
            }
        }
        if avcc.is_empty() || self.sps.is_empty() || self.pps.is_empty() {
            return Ok(None);
        }
        if self.session.is_none() {
            self.session = Some(self.open()?);
        }
        let Some(session) = &self.session else {
            return Ok(None);
        };
        // SAFETY: the buffers are made, used and released here; the
        // session's callback writes to `self.out`, which outlives the call.
        unsafe {
            let mut block = ptr::null_mut();
            check(
                CMBlockBufferCreateWithMemoryBlock(
                    ptr::null(),
                    ptr::null_mut(),
                    avcc.len(),
                    ptr::null(),
                    ptr::null(),
                    0,
                    avcc.len(),
                    BLOCK_ASSURE_MEMORY,
                    &mut block,
                ),
                "making a block buffer",
            )?;
            let copied = CMBlockBufferReplaceDataBytes(avcc.as_ptr().cast(), block, 0, avcc.len());
            let mut sample = ptr::null_mut();
            let made = if copied == 0 {
                let size = avcc.len();
                CMSampleBufferCreateReady(
                    ptr::null(),
                    block,
                    session.format.as_ptr(),
                    1,
                    0,
                    ptr::null(),
                    1,
                    &size,
                    &mut sample,
                )
            } else {
                copied
            };
            CFRelease(block);
            check(made, "making a sample buffer")?;
            let mut info = 0;
            let status = VTDecompressionSessionDecodeFrame(
                session.session.as_ptr(),
                sample,
                0,
                ptr::null_mut(),
                &mut info,
            );
            VTDecompressionSessionWaitForAsynchronousFrames(session.session.as_ptr());
            CFRelease(sample);
            check(status, "decoding")?;
        }
        Ok(self.out.lock().ok().and_then(|mut out| out.take()))
    }

    /// A session for the current parameter sets.
    fn open(&self) -> Result<Session> {
        // SAFETY: Core Media, Core Video and VideoToolbox as documented;
        // every object made here is released here or by `Session`.
        unsafe {
            let sets = [self.sps.as_ptr(), self.pps.as_ptr()];
            let sizes = [self.sps.len(), self.pps.len()];
            let mut format = ptr::null_mut();
            check(
                CMVideoFormatDescriptionCreateFromH264ParameterSets(
                    ptr::null(),
                    2,
                    sets.as_ptr(),
                    sizes.as_ptr(),
                    4,
                    &mut format,
                ),
                "reading the stream's parameters",
            )?;
            let format = NonNull::new(format).ok_or_else(|| anyhow::anyhow!("no format"))?;
            let pixel_format =
                CFNumberCreate(ptr::null(), NUMBER_SINT32, (&BGRA as *const i32).cast());
            let keys = [kCVPixelBufferPixelFormatTypeKey];
            let values = [pixel_format];
            let attributes = CFDictionaryCreate(
                ptr::null(),
                keys.as_ptr(),
                values.as_ptr(),
                1,
                &kCFTypeDictionaryKeyCallBacks,
                &kCFTypeDictionaryValueCallBacks,
            );
            CFRelease(pixel_format);
            let record = OutputCallbackRecord {
                callback: decoded,
                refcon: (&*self.out as *const Out).cast_mut().cast(),
            };
            let mut session = ptr::null_mut();
            let status = VTDecompressionSessionCreate(
                ptr::null(),
                format.as_ptr(),
                ptr::null(),
                attributes,
                &record,
                &mut session,
            );
            CFRelease(attributes);
            let session = match (status, NonNull::new(session)) {
                (0, Some(session)) => session,
                _ => {
                    CFRelease(format.as_ptr());
                    bail!("VideoToolbox could not start a decoder (status {status})");
                }
            };
            Ok(Session { format, session })
        }
    }
}

fn check(status: OSStatus, what: &str) -> Result<()> {
    if status != 0 {
        bail!("{what} failed (status {status})");
    }
    Ok(())
}

/// VideoToolbox's callback: copies the decoded picture into a frame.
unsafe extern "C" fn decoded(
    refcon: *mut c_void,
    _: *mut c_void,
    status: OSStatus,
    _: u32,
    image: *mut c_void,
    _: CMTime,
    _: CMTime,
) {
    if status != 0 || image.is_null() || refcon.is_null() {
        return;
    }
    // SAFETY: `refcon` is the decoder's `Out`, alive while it decodes;
    // `image` is a locked BGRA pixel buffer for the copy.
    unsafe {
        let out = &*(refcon as *const Out);
        if CVPixelBufferLockBaseAddress(image, LOCK_READ_ONLY) != 0 {
            return;
        }
        let (width, height) = (CVPixelBufferGetWidth(image), CVPixelBufferGetHeight(image));
        let (base, stride) = (
            CVPixelBufferGetBaseAddress(image),
            CVPixelBufferGetBytesPerRow(image),
        );
        let row = width * 4;
        if !base.is_null() && stride >= row {
            let mut pixels = Vec::with_capacity(row * height);
            for y in 0..height {
                pixels.extend_from_slice(std::slice::from_raw_parts(base.add(y * stride), row));
            }
            if let Ok(mut out) = out.lock() {
                *out = Some(Frame::from_bgra(width as u32, height as u32, pixels));
            }
        }
        CVPixelBufferUnlockBaseAddress(image, LOCK_READ_ONLY);
    }
}
