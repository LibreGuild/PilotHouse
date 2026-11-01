//! The result's sound, made from the source's for the native writers
//! (Media Foundation, GStreamer): each kept part read in turn and placed at
//! its time in the result, sample for sample, silence between them and to
//! the end, in stereo at one rate. A writer takes it a little at a time,
//! ahead of the pictures, so the two interleave.

use std::collections::VecDeque;

use anyhow::Result;

use super::media::AudioPiece;

/// Frames given at a time, at most.
const BLOCK: u64 = 4096;

/// How the source's decoded sound comes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Layout {
    pub rate: u32,
    pub channels: usize,
    /// Which speakers the channels are, as WAVE's channel mask says (in
    /// that order), when known.
    pub mask: Option<u32>,
}

/// The source's sound, decoded to 16-bit samples.
pub(crate) trait PcmReader {
    fn layout(&self) -> Layout;
    /// Reads on from source second `from` (the first block may start a
    /// little before); `to` is where the part ends.
    fn seek(&mut self, from: f64, to: f64) -> Result<()>;
    /// The next block: when it starts (source seconds) and its samples,
    /// interleaved. `None` at the end.
    fn read(&mut self) -> Result<Option<(f64, Vec<i16>)>>;
}

/// The rate the result is written at: the source's when a writer takes it
/// (AAC takes 44.1 and 48 kHz), else 48 kHz.
#[cfg(any(windows, test))]
pub(crate) fn rate_for(source: u32) -> u32 {
    if source == 44_100 { 44_100 } else { 48_000 }
}

pub(crate) struct Track<R> {
    reader: R,
    layout: Layout,
    /// Each source channel's share of the left and right.
    gains: Vec<(f32, f32)>,
    pieces: VecDeque<AudioPiece>,
    rate: u32,
    /// Frames in the whole result.
    total: u64,
    /// Frames given so far.
    given: u64,
    part: Option<Part>,
}

/// A kept part being read.
struct Part {
    piece: AudioPiece,
    /// The result's frames it fills.
    end: u64,
    /// Source frames read and not yet used, in stereo; `first` is the
    /// source frame (at the source's rate) of the first.
    buffer: VecDeque<[f32; 2]>,
    first: i64,
    /// A block was read since the seek.
    reading: bool,
    ended: bool,
}

impl<R: PcmReader> Track<R> {
    /// Sound for a result `seconds` long at `rate`, from `pieces` of the
    /// source read by `reader`.
    pub fn new(reader: R, pieces: Vec<AudioPiece>, seconds: f64, rate: u32) -> Self {
        let layout = reader.layout();
        let mut pieces: Vec<AudioPiece> = pieces
            .into_iter()
            .filter(|piece| piece.to > piece.from)
            .collect();
        pieces.sort_by(|a, b| a.at.total_cmp(&b.at));
        Self {
            gains: stereo_gains(layout.channels.max(1), layout.mask),
            layout,
            reader,
            pieces: pieces.into(),
            rate,
            total: (seconds.max(0.) * rate as f64).round() as u64,
            given: 0,
            part: None,
        }
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Frames given so far.
    pub fn given(&self) -> u64 {
        self.given
    }

    /// Result second `t` as a frame.
    fn frame(&self, t: f64) -> u64 {
        (t.max(0.) * self.rate as f64).round() as u64
    }

    /// The next block of the result up to result second `until` (all of
    /// it: `f64::INFINITY`), interleaved stereo, starting at frame
    /// [`Self::given`] as it was; `None` when that far is given.
    pub fn next(&mut self, until: f64) -> Result<Option<Vec<i16>>> {
        let until = if until.is_finite() {
            ((until.max(0.) * self.rate as f64).ceil() as u64).min(self.total)
        } else {
            self.total
        };
        if self.given >= until {
            return Ok(None);
        }
        let limit = until.min(self.given + BLOCK);
        if self.part.is_none() {
            self.start_part()?;
        }
        let out = match self.part.take() {
            Some(mut part) => {
                let stop = limit.min(part.end);
                let out = self.read_part(&mut part, stop)?;
                if stop < part.end {
                    self.part = Some(part);
                }
                out
            }
            None => {
                let next = self
                    .pieces
                    .front()
                    .map_or(u64::MAX, |piece| self.frame(piece.at));
                vec![0; (limit.min(next).max(self.given + 1) - self.given) as usize * 2]
            }
        };
        self.given += out.len() as u64 / 2;
        Ok(Some(out))
    }

    /// Starts reading the part due now, if one is.
    fn start_part(&mut self) -> Result<()> {
        // Parts already over (they overlap ones before) are passed over.
        while let Some(piece) = self.pieces.front() {
            if self.frame(piece.at + (piece.to - piece.from)) > self.given {
                break;
            }
            self.pieces.pop_front();
        }
        let Some(piece) = self.pieces.front().copied() else {
            return Ok(());
        };
        if self.frame(piece.at) > self.given {
            return Ok(());
        }
        self.pieces.pop_front();
        // Reading picks up where this frame is in the source.
        let from = piece.from + (self.given as f64 / self.rate as f64 - piece.at).max(0.);
        self.reader.seek(from, piece.to)?;
        self.part = Some(Part {
            piece,
            end: self
                .frame(piece.at + (piece.to - piece.from))
                .min(self.total),
            buffer: VecDeque::new(),
            first: 0,
            reading: false,
            ended: false,
        });
        Ok(())
    }

    /// The result's frames from `given` up to `stop`, from `part`.
    fn read_part(&mut self, part: &mut Part, stop: u64) -> Result<Vec<i16>> {
        let source_rate = self.layout.rate.max(1) as f64;
        let same_rate = self.layout.rate == self.rate;
        let mut out = Vec::with_capacity((stop - self.given) as usize * 2);
        for n in self.given..stop {
            let seconds = part.piece.from + (n as f64 / self.rate as f64 - part.piece.at);
            let position = seconds * source_rate;
            let (index, fraction) = if same_rate {
                (position.round() as i64, 0.)
            } else {
                let floor = position.floor();
                (floor as i64, (position - floor) as f32)
            };
            let needed = index + i64::from(fraction > 0.);
            loop {
                // Frames before this one are used up (a reader landing well
                // before the part never fills memory).
                while part.first < index && !part.buffer.is_empty() {
                    part.buffer.pop_front();
                    part.first += 1;
                }
                if part.ended || part.first + part.buffer.len() as i64 > needed {
                    break;
                }
                self.fill(part)?;
            }
            let at = |ix: i64| -> [f32; 2] {
                if ix < part.first {
                    return [0.; 2];
                }
                part.buffer
                    .get((ix - part.first) as usize)
                    .copied()
                    .unwrap_or([0.; 2])
            };
            let a = at(index);
            let frame = if fraction > 0. {
                let b = at(index + 1);
                [
                    a[0] + (b[0] - a[0]) * fraction,
                    a[1] + (b[1] - a[1]) * fraction,
                ]
            } else {
                a
            };
            out.extend(frame.map(|sample| sample.round().clamp(-32768., 32767.) as i16));
        }
        Ok(out)
    }

    /// Reads the next block into `part`'s buffer, where its time says it
    /// goes: after a gap the buffer starts again there (what is missing
    /// reads as silence), what overlaps is dropped.
    fn fill(&mut self, part: &mut Part) -> Result<()> {
        let Some((start, samples)) = self.reader.read()? else {
            part.ended = true;
            return Ok(());
        };
        let channels = self.layout.channels.max(1);
        let frames = samples.chunks_exact(channels).map(|frame| {
            let (mut left, mut right) = (0f32, 0f32);
            for (sample, (to_left, to_right)) in frame.iter().zip(&self.gains) {
                left += *sample as f32 * to_left;
                right += *sample as f32 * to_right;
            }
            [left, right]
        });
        let rate = self.layout.rate.max(1) as f64;
        let index = (start * rate).round() as i64;
        let expected = part.first + part.buffer.len() as i64;
        // Up to 10 ms off is the timestamps' rounding, not a gap.
        let slack = (rate / 100.).max(1.) as i64;
        if !part.reading || index > expected + slack {
            part.reading = true;
            part.buffer.clear();
            part.first = index;
            part.buffer.extend(frames);
        } else {
            // Frames overlapping what was read are had already.
            let skip = if index < expected - slack {
                (expected - index) as usize
            } else {
                0
            };
            part.buffer.extend(frames.skip(skip));
        }
        Ok(())
    }
}

/// Each channel's share of the left and right, for `channels` laid out as
/// `mask` says (else the usual layout for that many). More than two are
/// mixed down the usual way (centre and surrounds at -3 dB, no LFE), then
/// scaled so the loudest side cannot clip.
fn stereo_gains(channels: usize, mask: Option<u32>) -> Vec<(f32, f32)> {
    const FRONT_LEFT: u32 = 0x1;
    const FRONT_RIGHT: u32 = 0x2;
    const FRONT_CENTER: u32 = 0x4;
    const LOW_FREQUENCY: u32 = 0x8;
    const BACK_LEFT: u32 = 0x10;
    const BACK_RIGHT: u32 = 0x20;
    const FRONT_LEFT_OF_CENTER: u32 = 0x40;
    const FRONT_RIGHT_OF_CENTER: u32 = 0x80;
    const BACK_CENTER: u32 = 0x100;
    const SIDE_LEFT: u32 = 0x200;
    const SIDE_RIGHT: u32 = 0x400;
    const HALF: f32 = std::f32::consts::FRAC_1_SQRT_2;
    match channels {
        1 => return vec![(1., 1.)],
        2 if mask.is_none_or(|mask| mask.count_ones() != 2 || mask == 0x3) => {
            return vec![(1., 0.), (0., 1.)];
        }
        _ => {}
    }
    let usual = match channels {
        2 => FRONT_LEFT | FRONT_RIGHT,
        3 => FRONT_LEFT | FRONT_RIGHT | FRONT_CENTER,
        4 => FRONT_LEFT | FRONT_RIGHT | BACK_LEFT | BACK_RIGHT,
        5 => FRONT_LEFT | FRONT_RIGHT | FRONT_CENTER | BACK_LEFT | BACK_RIGHT,
        6 => FRONT_LEFT | FRONT_RIGHT | FRONT_CENTER | LOW_FREQUENCY | BACK_LEFT | BACK_RIGHT,
        7 => {
            FRONT_LEFT
                | FRONT_RIGHT
                | FRONT_CENTER
                | LOW_FREQUENCY
                | BACK_CENTER
                | SIDE_LEFT
                | SIDE_RIGHT
        }
        _ => {
            FRONT_LEFT
                | FRONT_RIGHT
                | FRONT_CENTER
                | LOW_FREQUENCY
                | BACK_LEFT
                | BACK_RIGHT
                | SIDE_LEFT
                | SIDE_RIGHT
        }
    };
    let mask = mask
        .filter(|mask| mask.count_ones() as usize == channels)
        .unwrap_or(usual);
    // The speakers in mask order; channels past them alternate sides.
    let mut speakers = (0..32).map(|bit| 1u32 << bit).filter(|bit| mask & bit != 0);
    let mut gains: Vec<(f32, f32)> = (0..channels)
        .map(|ix| match speakers.next() {
            Some(FRONT_LEFT | FRONT_LEFT_OF_CENTER) => (1., 0.),
            Some(FRONT_RIGHT | FRONT_RIGHT_OF_CENTER) => (0., 1.),
            Some(FRONT_CENTER | BACK_CENTER) => (HALF, HALF),
            Some(LOW_FREQUENCY) => (0., 0.),
            Some(BACK_LEFT | SIDE_LEFT) => (HALF, 0.),
            Some(BACK_RIGHT | SIDE_RIGHT) => (0., HALF),
            Some(_) | None if ix % 2 == 0 => (HALF, 0.),
            Some(_) | None => (0., HALF),
        })
        .collect();
    let loudest = gains
        .iter()
        .fold((0f32, 0f32), |sum, gain| (sum.0 + gain.0, sum.1 + gain.1));
    let scale = 1. / loudest.0.max(loudest.1).max(1.);
    for gain in &mut gains {
        gain.0 *= scale;
        gain.1 *= scale;
    }
    gains
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A source whose every sample is its frame's number (left) and its
    /// negative (right), at `rate`, read in blocks of 1000.
    struct Ramp {
        rate: u32,
        position: i64,
        end: i64,
        /// Times seeked to.
        seeks: Vec<f64>,
    }

    impl Ramp {
        fn new(rate: u32, seconds: f64) -> Self {
            Self {
                rate,
                position: 0,
                end: (seconds * rate as f64) as i64,
                seeks: Vec::new(),
            }
        }
    }

    impl PcmReader for Ramp {
        fn layout(&self) -> Layout {
            Layout {
                rate: self.rate,
                channels: 2,
                mask: None,
            }
        }

        fn seek(&mut self, from: f64, _: f64) -> Result<()> {
            self.seeks.push(from);
            // Lands a little early, as decoders do.
            self.position = ((from - 0.05).max(0.) * self.rate as f64) as i64;
            Ok(())
        }

        fn read(&mut self) -> Result<Option<(f64, Vec<i16>)>> {
            if self.position >= self.end {
                return Ok(None);
            }
            let start = self.position;
            let count = 1000.min(self.end - start);
            self.position += count;
            let samples = (start..start + count)
                .flat_map(|frame| {
                    let value = (frame % 30000) as i16;
                    [value, -value]
                })
                .collect();
            Ok(Some((start as f64 / self.rate as f64, samples)))
        }
    }

    fn whole(track: &mut Track<Ramp>) -> Vec<i16> {
        let mut out = Vec::new();
        while let Some(block) = track.next(f64::INFINITY).unwrap() {
            out.extend(block);
        }
        out
    }

    #[test]
    fn places_each_part_sample_for_sample() {
        let rate = 48_000;
        let pieces = vec![
            AudioPiece {
                from: 0.,
                to: 0.5,
                at: 0.,
            },
            // A part sped up was here: silence from 0.5 to 0.75.
            AudioPiece {
                from: 2.,
                to: 2.25,
                at: 0.75,
            },
        ];
        let mut track = Track::new(Ramp::new(rate, 3.), pieces, 1.25, rate);
        let out = whole(&mut track);
        assert_eq!(out.len(), 60_000 * 2, "1.25 s of stereo");
        let left = |frame: usize| out[frame * 2];
        // The first part as it is.
        assert_eq!(left(0), 0);
        assert_eq!(left(23_999), 23_999);
        assert_eq!(out[23_999 * 2 + 1], -23_999);
        // Silence, then the second part from 2 s (frame 96000).
        assert!((24_000..36_000).all(|frame| left(frame) == 0));
        assert_eq!(left(36_000), (96_000 % 30_000) as i16);
        assert_eq!(left(47_999), (107_999 % 30_000) as i16);
        // Silence to the end.
        assert!((48_000..60_000).all(|frame| left(frame) == 0));
        assert_eq!(track.reader.seeks, [0., 2.]);
    }

    #[test]
    fn stops_where_asked_and_resumes() {
        let rate = 44_100;
        let pieces = vec![AudioPiece {
            from: 1.,
            to: 2.,
            at: 0.,
        }];
        let mut track = Track::new(Ramp::new(rate, 3.), pieces, 1., rate);
        let mut first = Vec::new();
        while let Some(block) = track.next(0.5).unwrap() {
            first.extend(block);
        }
        assert_eq!(first.len(), 22_050 * 2);
        assert_eq!(track.rate(), rate);
        assert_eq!(track.given(), 22_050);
        let rest = whole(&mut track);
        assert_eq!(rest.len(), 22_050 * 2);
        assert_eq!(rest[0], ((44_100 + 22_050) % 30_000) as i16);
        assert!(track.next(f64::INFINITY).unwrap().is_none(), "all given");
    }

    #[test]
    fn resamples_to_the_result_rate() {
        // 24 kHz to 48 kHz: each source frame lands on every other result
        // frame, the ones between halfway.
        let pieces = vec![AudioPiece {
            from: 0.,
            to: 1.,
            at: 0.,
        }];
        let mut track = Track::new(Ramp::new(24_000, 1.), pieces, 1., 48_000);
        let out = whole(&mut track);
        assert_eq!(out.len(), 48_000 * 2);
        assert_eq!(out[100 * 2], 50);
        assert!((50..=51).contains(&out[101 * 2]), "halfway");
        assert_eq!(out[102 * 2], 51);
        assert_eq!(out[47_998 * 2], 23_999);
    }

    #[test]
    fn mixes_down_to_stereo() {
        assert_eq!(stereo_gains(1, None), [(1., 1.)]);
        assert_eq!(stereo_gains(2, None), [(1., 0.), (0., 1.)]);
        // 5.1: the centre and back to both sides at -3 dB, no LFE, scaled
        // so a side's shares add up to one.
        let gains = stereo_gains(6, None);
        let left: f32 = gains.iter().map(|gain| gain.0).sum();
        let right: f32 = gains.iter().map(|gain| gain.1).sum();
        assert!((left - 1.).abs() < 1e-5 && (right - 1.).abs() < 1e-5);
        assert_eq!(gains[3], (0., 0.));
        assert!(gains[0].0 > gains[2].0 && gains[2].0 == gains[2].1);
        assert_eq!(rate_for(44_100), 44_100);
        assert_eq!(rate_for(22_050), 48_000);
    }
}
