//! Frames read after a seek. Media Foundation and GStreamer land on the key
//! frame before the time asked for and decode on from there: frames over
//! before `from` are passed over (never converted), the one showing at
//! `from` is kept, and reading stops after `to`.

use anyhow::Result;

/// Times this close count as the same (a frame's start rounds).
const SAME: f64 = 1e-4;

/// A frame as read: when it starts and ends (seconds, when known) and its
/// pixels in whatever form the reader has them.
pub(crate) struct Raw<F> {
    pub start: f64,
    pub end: Option<f64>,
    pub frame: F,
}

pub(crate) struct Seeked<F> {
    from: f64,
    to: f64,
    /// The latest frame starting at or before `from`, while it is not yet
    /// known whether a later one does too.
    held: Option<(f64, F)>,
    /// A frame to give after the held one.
    queued: Option<(f64, F)>,
    started: bool,
    done: bool,
}

impl<F> Seeked<F> {
    pub fn new(from: f64, to: f64) -> Self {
        Self {
            from,
            to: to.max(from),
            held: None,
            queued: None,
            started: false,
            done: false,
        }
    }

    /// The next frame of `from`..`to` and when it starts, reading with
    /// `read` (`None`: the reader is at its end) as far as needed.
    pub fn next(
        &mut self,
        mut read: impl FnMut() -> Result<Option<Raw<F>>>,
    ) -> Result<Option<(f64, F)>> {
        if let Some(frame) = self.queued.take() {
            return Ok(Some(frame));
        }
        if self.done {
            return Ok(None);
        }
        loop {
            let Some(raw) = read()? else {
                self.done = true;
                return Ok(self.held.take());
            };
            if self.started {
                if raw.start > self.to + SAME {
                    self.done = true;
                    return Ok(None);
                }
                return Ok(Some((raw.start, raw.frame)));
            }
            if raw.start <= self.from + SAME {
                // One lasting past `from` is the one showing then.
                if raw.end.is_some_and(|end| end > self.from + SAME) {
                    self.started = true;
                    self.held = None;
                    return Ok(Some((raw.start, raw.frame)));
                }
                self.held = Some((raw.start, raw.frame));
                continue;
            }
            self.started = true;
            match self.held.take() {
                Some(held) => {
                    if raw.start > self.to + SAME {
                        self.done = true;
                    } else {
                        self.queued = Some((raw.start, raw.frame));
                    }
                    return Ok(Some(held));
                }
                // Nothing at or before `from` (the reader landed later):
                // the first there is, even past `to`, rather than none.
                None => {
                    if raw.start > self.to + SAME {
                        self.done = true;
                    }
                    return Ok(Some((raw.start, raw.frame)));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Frames every 0.1 s from `first`, with or without their ends.
    fn frames(first: f64, count: usize, ends: bool) -> impl FnMut() -> Result<Option<Raw<u32>>> {
        let mut ix = 0;
        move || {
            if ix == count {
                return Ok(None);
            }
            let start = first + ix as f64 * 0.1;
            ix += 1;
            Ok(Some(Raw {
                start,
                end: ends.then_some(start + 0.1),
                frame: ix as u32,
            }))
        }
    }

    fn all(
        mut seeked: Seeked<u32>,
        mut read: impl FnMut() -> Result<Option<Raw<u32>>>,
    ) -> Vec<u32> {
        let mut out = Vec::new();
        while let Some((_, frame)) = seeked.next(&mut read).unwrap() {
            out.push(frame);
        }
        out
    }

    #[test]
    fn starts_with_the_frame_showing() {
        // Key frame at 0, frames 1..=11 at 0.0..1.0; from 0.45 the one
        // showing started at 0.4 (frame 5).
        for ends in [false, true] {
            let got = all(Seeked::new(0.45, 0.75), frames(0., 11, ends));
            assert_eq!(got, [5, 6, 7, 8], "ends known: {ends}");
        }
        // Exactly on a frame: that frame.
        assert_eq!(all(Seeked::new(0.5, 0.5), frames(0., 11, true)), [6]);
        assert_eq!(all(Seeked::new(0.5, 0.5), frames(0., 11, false)), [6]);
    }

    #[test]
    fn edges() {
        // Past the end: the last frame.
        assert_eq!(all(Seeked::new(5., 6.), frames(0., 11, false)), [11]);
        // The reader landed after `from`: its first frame, even past `to`.
        assert_eq!(all(Seeked::new(0.1, 0.2), frames(0.5, 3, false)), [1]);
        // Nothing at all.
        assert!(all(Seeked::new(0., 1.), frames(0., 0, false)).is_empty());
    }
}
