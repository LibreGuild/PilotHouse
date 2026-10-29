//! A phone's screen as H.264 video: `screenrecord --output-format=h264`
//! has the phone's hardware encoder compress what changes on screen, as
//! Android Studio's mirroring does, so the phone does little work and the
//! cable carries kilobytes, not megabytes, per frame. The computer decodes
//! it with the system's own decoder (VideoToolbox on macOS, Media
//! Foundation on Windows). Elsewhere, or if the decoder cannot start, the
//! screen falls back to raw frames (see `screen.rs`).
//!
//! The stream is Annex B: NAL units after `00 00 01` start codes, with no
//! framing between pictures. [`Assembler`] cuts it into access units (the
//! NAL units of one picture) as bytes arrive.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::Decoder;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::Decoder;

#[cfg(not(any(target_os = "macos", windows)))]
mod none;
#[cfg(not(any(target_os = "macos", windows)))]
pub use none::Decoder;

/// NAL unit types (the low five bits of the first byte).
const SLICE: u8 = 1;
const IDR_SLICE: u8 = 5;

pub(crate) fn nal_type(nal: &[u8]) -> u8 {
    nal.first().map_or(0, |byte| byte & 0x1f)
}

fn is_slice(nal: &[u8]) -> bool {
    matches!(nal_type(nal), SLICE | IDR_SLICE)
}

/// Whether slice `nal` starts a picture: its `first_mb_in_slice` (the first
/// Exp-Golomb number after the header byte) is 0, written as a single 1 bit.
fn starts_picture(nal: &[u8]) -> bool {
    nal.get(1).is_some_and(|byte| byte & 0x80 != 0)
}

/// Turns the byte stream into pictures' NAL units.
#[derive(Default)]
pub struct Assembler {
    /// Bytes not yet cut into NAL units (from the last start code on).
    pending: Vec<u8>,
    /// The picture being gathered: parameter sets and slices.
    unit: Vec<Vec<u8>>,
}

impl Assembler {
    /// Adds bytes read from the stream; returns the pictures they complete.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Vec<Vec<u8>>> {
        self.pending.extend_from_slice(bytes);
        let starts = start_codes(&self.pending);
        let mut done = Vec::new();
        // Every NAL unit but the last is whole: the next start code ends it.
        for pair in starts.windows(2) {
            let (start, end) = (pair[0], pair[1]);
            let nal = trim_zeros(&self.pending[start.1..end.0]).to_vec();
            self.add(nal, &mut done);
        }
        if let Some(&(at, _)) = starts.last() {
            self.pending.drain(..at);
        }
        done
    }

    /// The stream paused: what is held ends a picture (the phone writes
    /// each one whole), so it and any before it are complete.
    pub fn flush(&mut self) -> Vec<Vec<Vec<u8>>> {
        let mut done = Vec::new();
        if let Some(&(_, from)) = start_codes(&self.pending).first() {
            let nal = trim_zeros(&self.pending[from..]).to_vec();
            self.add(nal, &mut done);
        }
        self.pending.clear();
        if self.unit.iter().any(|nal| is_slice(nal)) {
            done.push(std::mem::take(&mut self.unit));
        }
        done
    }

    fn add(&mut self, nal: Vec<u8>, done: &mut Vec<Vec<Vec<u8>>>) {
        if nal.is_empty() {
            return;
        }
        let has_slice = self.unit.iter().any(|nal| is_slice(nal));
        // A new picture begins with parameter sets or its first slice.
        let begins = !is_slice(&nal) || starts_picture(&nal);
        if has_slice && begins {
            done.push(std::mem::take(&mut self.unit));
        }
        self.unit.push(nal);
    }
}

/// Each start code's position and where its NAL unit begins.
fn start_codes(bytes: &[u8]) -> Vec<(usize, usize)> {
    let mut found = Vec::new();
    let mut at = 0;
    while at + 3 <= bytes.len() {
        if bytes[at] == 0 && bytes[at + 1] == 0 && bytes[at + 2] == 1 {
            // A four-byte start code's extra zero belongs to it.
            let begin = if at > 0 && bytes[at - 1] == 0 {
                at - 1
            } else {
                at
            };
            found.push((begin, at + 3));
            at += 3;
        } else {
            at += 1;
        }
    }
    found
}

/// A NAL unit without the zero bytes that may pad its end.
fn trim_zeros(nal: &[u8]) -> &[u8] {
    let end = nal
        .iter()
        .rposition(|byte| *byte != 0)
        .map_or(0, |at| at + 1);
    &nal[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPS_NAL: [u8; 4] = [0x67, 0x64, 0x00, 0x1f];
    const PPS_NAL: [u8; 3] = [0x68, 0xee, 0x0d];
    const IDR_NAL: [u8; 4] = [0x65, 0xb8, 0x4f, 0xb4];
    const P1: [u8; 3] = [0x41, 0x9a, 0x01];
    const P2: [u8; 3] = [0x41, 0x9a, 0x02];

    fn stream(nals: &[&[u8]]) -> Vec<u8> {
        nals.iter()
            .flat_map(|nal| [&[0u8, 0, 0, 1][..], nal].concat())
            .collect()
    }

    #[test]
    fn pictures_come_out_whole_however_the_bytes_arrive() {
        let bytes = stream(&[&SPS_NAL, &PPS_NAL, &IDR_NAL, &P1, &P2]);
        for chunk in [1, 2, 3, 5, 7, bytes.len()] {
            let mut assembler = Assembler::default();
            let mut pictures = Vec::new();
            for piece in bytes.chunks(chunk) {
                pictures.extend(assembler.push(piece));
            }
            pictures.extend(assembler.flush());
            assert_eq!(
                pictures,
                vec![
                    vec![SPS_NAL.to_vec(), PPS_NAL.to_vec(), IDR_NAL.to_vec()],
                    vec![P1.to_vec()],
                    vec![P2.to_vec()],
                ],
                "chunks of {chunk}"
            );
        }
    }

    /// Decodes a recording (`PH_H264_SAMPLE`, from `adb exec-out
    /// screenrecord --output-format=h264 -`) with the system's decoder.
    #[test]
    #[ignore = "needs a recording and a decoder"]
    fn a_recording_decodes() {
        let path = std::env::var("PH_H264_SAMPLE").expect("PH_H264_SAMPLE");
        let bytes = std::fs::read(path).unwrap();
        let mut decoder = Decoder::new().unwrap();
        let mut assembler = Assembler::default();
        let mut pictures = assembler.push(&bytes);
        pictures.extend(assembler.flush());
        let frames: Vec<_> = pictures
            .iter()
            .filter_map(|picture| decoder.decode(picture).unwrap())
            .collect();
        assert!(!frames.is_empty());
        let frame = &frames[0];
        eprintln!(
            "{} pictures, {} frames, {}x{}",
            pictures.len(),
            frames.len(),
            frame.width,
            frame.height
        );
        assert!(
            frame.bgra().chunks(4).any(|pixel| pixel[..3] != [0, 0, 0]),
            "not all black"
        );
    }

    #[test]
    fn a_pause_completes_the_last_picture() {
        let mut assembler = Assembler::default();
        assert!(
            assembler
                .push(&stream(&[&SPS_NAL, &PPS_NAL, &IDR_NAL]))
                .is_empty()
        );
        let pictures = assembler.flush();
        assert_eq!(pictures.len(), 1);
        assert_eq!(pictures[0].len(), 3);
        assert!(assembler.flush().is_empty(), "nothing twice");
        // Two pictures held at a pause both come out.
        assert!(assembler.push(&stream(&[&P1, &P2])).is_empty());
        assert_eq!(
            assembler.flush(),
            vec![vec![P1.to_vec()], vec![P2.to_vec()]]
        );
    }
}
