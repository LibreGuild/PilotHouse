//! What a video edit is: which parts of the source are kept and how fast
//! they play, the crop and size, marks shown for a while, and the sound.
//!
//! Like a picture's edit it is a description: the source is only read. Times
//! are seconds of the source, so a mark stays on what it points at when an
//! earlier part is cut; the result's own times follow from the kept parts.

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::edit::{MarkKind, Rect, Size, merge};

/// The most kept parts and marks.
const MAX_SEGMENTS: usize = 500;
const MAX_OVERLAYS: usize = 300;
/// The shortest part kept (a frame or two).
const MIN_LENGTH: f64 = 0.04;
/// Speeds a part plays at.
pub const SPEEDS: [f32; 6] = [0.5, 1., 2., 4., 8., 16.];

/// A part of the source kept, from `start` to `end` (seconds of the
/// source), played `speed` times as fast.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Segment {
    pub start: f64,
    pub end: f64,
    #[serde(default = "one")]
    pub speed: f32,
}

fn one() -> f32 {
    1.
}

impl Segment {
    pub fn new(start: f64, end: f64, speed: f32) -> Segment {
        Segment { start, end, speed }
    }

    /// How long it lasts in the result.
    pub fn length(&self) -> f64 {
        (self.end - self.start) / self.speed as f64
    }

    pub fn contains(&self, t: f64) -> bool {
        t >= self.start && t < self.end
    }
}

/// A mark shown from `start` to `end` (seconds of the source), fading in
/// and out over `fade` seconds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Overlay {
    #[serde(default)]
    pub id: u32,
    pub start: f64,
    pub end: f64,
    #[serde(default)]
    pub fade: f32,
    #[serde(flatten)]
    pub kind: MarkKind,
}

impl Overlay {
    /// How visible it is at source time `t` (0 when not shown).
    pub fn alpha(&self, t: f64) -> f32 {
        if t < self.start || t > self.end {
            return 0.;
        }
        if self.fade <= 0. {
            return 1.;
        }
        let fade = self.fade as f64;
        ((t - self.start) / fade)
            .min((self.end - t) / fade)
            .clamp(0., 1.) as f32
    }
}

/// Everything done to one video. Its default changes nothing.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct VideoEdit {
    /// The parts kept, in order (all of it, at its own speed, when empty).
    pub segments: Vec<Segment>,
    /// The part of the frame kept, in pixels of the (upright) source.
    pub crop: Option<Rect>,
    /// The result's size (the crop's when `None`).
    pub resize: Option<Size>,
    pub overlays: Vec<Overlay>,
    /// No sound in the result.
    pub mute: bool,
}

impl VideoEdit {
    pub fn is_unchanged(&self) -> bool {
        *self == VideoEdit::default()
    }

    /// The parts kept, for a source `duration` seconds long.
    pub fn pieces(&self, duration: f64) -> Vec<Segment> {
        if self.segments.is_empty() {
            vec![Segment::new(0., duration, 1.)]
        } else {
            self.segments.clone()
        }
    }

    /// How long the result lasts.
    pub fn output_duration(&self, duration: f64) -> f64 {
        self.pieces(duration).iter().map(Segment::length).sum()
    }

    /// Where source time `t` is in the result (`None`: cut).
    pub fn to_output(&self, duration: f64, t: f64) -> Option<f64> {
        let mut before = 0.;
        for piece in self.pieces(duration) {
            if piece.contains(t) || (t == piece.end && t >= duration) {
                return Some(before + (t - piece.start) / piece.speed as f64);
            }
            before += piece.length();
        }
        None
    }

    /// The source time at result time `t`, and the part it is in.
    pub fn to_source(&self, duration: f64, t: f64) -> (f64, usize) {
        let pieces = self.pieces(duration);
        let mut before = 0.;
        for (ix, piece) in pieces.iter().enumerate() {
            let length = piece.length();
            if t < before + length || ix == pieces.len() - 1 {
                let into = (t - before).clamp(0., length);
                return ((piece.start + into * piece.speed as f64).min(piece.end), ix);
            }
            before += length;
        }
        (0., 0)
    }

    /// The source's crop rectangle, all of a `size` frame when none.
    pub fn crop_rect(&self, size: (u32, u32)) -> Rect {
        let all = Rect::new(0., 0., size.0 as f32, size.1 as f32);
        self.crop
            .map_or(all, |crop| crop.clamped(size.0 as f32, size.1 as f32))
    }

    /// The result's frame size: the resize, else the crop's, made even
    /// (video encoders want even sides).
    pub fn output_size(&self, size: (u32, u32)) -> (u32, u32) {
        let (width, height) = match self.resize {
            Some(resize) => (resize.width, resize.height),
            None => {
                let crop = self.crop_rect(size);
                (crop.width.round() as u32, crop.height.round() as u32)
            }
        };
        (even(width), even(height))
    }

    // Changes to the kept parts. Each keeps them in order, apart and
    // longer than a frame.

    /// Splits the part holding `t` there.
    pub fn split(&mut self, duration: f64, t: f64) {
        let mut pieces = self.pieces(duration);
        if let Some(ix) = pieces
            .iter()
            .position(|piece| t - piece.start > MIN_LENGTH && piece.end - t > MIN_LENGTH)
        {
            let piece = pieces[ix];
            pieces[ix].end = t;
            pieces.insert(ix + 1, Segment::new(t, piece.end, piece.speed));
            self.segments = pieces;
        }
    }

    /// Removes the source from `from` to `to`.
    pub fn cut(&mut self, duration: f64, from: f64, to: f64) {
        let (from, to) = (from.min(to).max(0.), from.max(to).min(duration));
        let mut kept = Vec::new();
        for piece in self.pieces(duration) {
            if piece.end <= from || piece.start >= to {
                kept.push(piece);
                continue;
            }
            if piece.start < from {
                kept.push(Segment::new(piece.start, from, piece.speed));
            }
            if piece.end > to {
                kept.push(Segment::new(to, piece.end, piece.speed));
            }
        }
        self.set_pieces(kept, duration);
    }

    /// Keeps only the source from `from` to `to`.
    pub fn trim(&mut self, duration: f64, from: f64, to: f64) {
        let (from, to) = (from.min(to), from.max(to));
        self.cut(duration, to, duration);
        self.cut(duration, 0., from);
    }

    /// Plays the source from `from` to `to` at `speed`.
    pub fn speed_range(&mut self, duration: f64, from: f64, to: f64, speed: f32) {
        let (from, to) = (from.min(to).max(0.), from.max(to).min(duration));
        let mut pieces = Vec::new();
        for piece in self.pieces(duration) {
            let (a, b) = (
                from.clamp(piece.start, piece.end),
                to.clamp(piece.start, piece.end),
            );
            let cuts = [piece.start, a, b, piece.end];
            for window in cuts.windows(2) {
                let (start, end) = (window[0], window[1]);
                if end - start <= 0. {
                    continue;
                }
                let inside = start >= from && end <= to;
                pieces.push(Segment::new(
                    start,
                    end,
                    if inside { speed } else { piece.speed },
                ));
            }
        }
        self.set_pieces(pieces, duration);
    }

    /// Removes kept part `ix`.
    pub fn delete_piece(&mut self, duration: f64, ix: usize) {
        let mut pieces = self.pieces(duration);
        if ix < pieces.len() && pieces.len() > 1 {
            pieces.remove(ix);
            self.set_pieces(pieces, duration);
        }
    }

    /// Sets kept part `ix`'s speed.
    pub fn set_piece_speed(&mut self, duration: f64, ix: usize, speed: f32) {
        let mut pieces = self.pieces(duration);
        if let Some(piece) = pieces.get_mut(ix) {
            piece.speed = speed;
            self.set_pieces(pieces, duration);
        }
    }

    /// Moves kept part `ix`'s start (`end` false) or end to source time
    /// `t`, within the source and its neighbours. Where it touches a
    /// neighbour (a split), the split moves: both change.
    pub fn move_edge(&mut self, duration: f64, ix: usize, end: bool, t: f64) {
        let mut pieces = self.pieces(duration);
        let Some(piece) = pieces.get(ix).copied() else {
            return;
        };
        let touching = if end {
            pieces
                .get(ix + 1)
                .filter(|next| (next.start - piece.end).abs() < 1e-6)
                .map(|_| ix + 1)
        } else {
            ix.checked_sub(1)
                .filter(|&before| (pieces[before].end - piece.start).abs() < 1e-6)
        };
        if let Some(other) = touching {
            let (left, right) = if end { (ix, other) } else { (other, ix) };
            let at = t.clamp(
                pieces[left].start + MIN_LENGTH,
                pieces[right].end - MIN_LENGTH,
            );
            pieces[left].end = at;
            pieces[right].start = at;
            self.set_pieces(pieces, duration);
            return;
        }
        if end {
            let limit = pieces.get(ix + 1).map_or(duration, |next| next.start);
            pieces[ix].end = t.clamp(piece.start + MIN_LENGTH, limit);
        } else {
            let limit = if ix > 0 { pieces[ix - 1].end } else { 0. };
            pieces[ix].start = t.clamp(limit, piece.end - MIN_LENGTH);
        }
        self.set_pieces(pieces, duration);
    }

    /// Takes `pieces` as the kept parts: in order, without overlaps or
    /// slivers; all of it at its own speed is no change. Parts that only
    /// touch stay apart (a split is kept), overlapping ones of one speed
    /// join.
    fn set_pieces(&mut self, mut pieces: Vec<Segment>, duration: f64) {
        pieces.retain(|piece| piece.end - piece.start >= MIN_LENGTH);
        pieces.sort_by(|a, b| a.start.total_cmp(&b.start));
        let mut joined: Vec<Segment> = Vec::new();
        for piece in pieces {
            let last = joined.last().copied();
            match last {
                Some(last) if piece.start < last.end - 1e-6 && piece.speed == last.speed => {
                    if let Some(last) = joined.last_mut() {
                        last.end = last.end.max(piece.end);
                    }
                }
                // Overlapping another speed: starts where it ends.
                Some(last) if piece.start < last.end => {
                    joined.push(Segment::new(last.end, piece.end.max(last.end), piece.speed))
                }
                _ => joined.push(piece),
            }
        }
        joined.retain(|piece| piece.end - piece.start >= MIN_LENGTH);
        self.segments = match joined.as_slice() {
            [whole] if whole.start <= 1e-6 && whole.end >= duration - 1e-6 && whole.speed == 1. => {
                Vec::new()
            }
            // Nothing left would be no video: keep what there was.
            [] => return,
            _ => joined,
        };
    }

    pub fn overlay(&self, id: u32) -> Option<&Overlay> {
        self.overlays.iter().find(|overlay| overlay.id == id)
    }

    pub fn overlay_mut(&mut self, id: u32) -> Option<&mut Overlay> {
        self.overlays.iter_mut().find(|overlay| overlay.id == id)
    }

    /// Adds `overlay`, its sizes made to suit a `size` frame; its id.
    pub fn add_overlay(&mut self, mut overlay: Overlay, size: (u32, u32)) -> Result<u32> {
        if self.overlays.len() >= MAX_OVERLAYS {
            bail!("a video has at most {MAX_OVERLAYS} marks");
        }
        overlay.kind.check()?;
        overlay.kind.fill_sizes(size.0.max(size.1) as f32);
        if let MarkKind::Step { number, .. } = &mut overlay.kind
            && *number == 0
        {
            *number = self
                .overlays
                .iter()
                .filter_map(|o| match o.kind {
                    MarkKind::Step { number, .. } => Some(number),
                    _ => None,
                })
                .max()
                .unwrap_or(0)
                + 1;
        }
        overlay.id = self.overlays.iter().map(|o| o.id).max().unwrap_or(0) + 1;
        let id = overlay.id;
        self.overlays.push(overlay);
        Ok(id)
    }

    /// Applies an agent's changes (see the `studio_edit` tool): `set`
    /// (crop, resize, mute), `cut`, `trim`, `speed`, `split`,
    /// `reset_timeline`, and marks with `add`, `update`, `remove`.
    /// `duration` and `size` are the source's.
    pub fn apply(
        &mut self,
        request: &Value,
        duration: f64,
        size: (u32, u32),
    ) -> Result<Vec<String>> {
        let mut next = self.clone();
        let mut done = Vec::new();
        let range = |value: &Value, what: &str| -> Result<(f64, f64)> {
            let from = value.get("from").and_then(Value::as_f64);
            let to = value.get("to").and_then(Value::as_f64);
            match (from, to) {
                (Some(from), Some(to)) if from.is_finite() && to.is_finite() => Ok((from, to)),
                _ => bail!("{what}: give from and to, in seconds of the source"),
            }
        };
        if request.get("reset_timeline").and_then(Value::as_bool) == Some(true) {
            next.segments.clear();
            done.push("kept all of it again".to_string());
        }
        if let Some(trim) = request.get("trim").filter(|trim| !trim.is_null()) {
            let (from, to) = range(trim, "trim")?;
            next.trim(duration, from, to);
            done.push(format!("kept {from:.2}s to {to:.2}s"));
        }
        for cut in request
            .get("cut")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let (from, to) = range(cut, "cut")?;
            next.cut(duration, from, to);
            done.push(format!("cut {from:.2}s to {to:.2}s"));
        }
        for speed in request
            .get("speed")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let (from, to) = range(speed, "speed")?;
            let factor = speed
                .get("speed")
                .and_then(Value::as_f64)
                .filter(|factor| (0.25..=32.).contains(factor))
                .ok_or_else(|| anyhow::anyhow!("speed: give speed, 0.25 to 32"))?
                as f32;
            next.speed_range(duration, from, to, factor);
            done.push(format!("{from:.2}s to {to:.2}s at {factor}×"));
        }
        for at in request
            .get("split")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let at = at
                .as_f64()
                .ok_or_else(|| anyhow::anyhow!("split: seconds"))?;
            next.split(duration, at);
        }
        if let Some(changes) = request.get("set").filter(|changes| !changes.is_null()) {
            if !changes.is_object()
                || changes.get("overlays").is_some()
                || changes.get("segments").is_some()
            {
                bail!(
                    "set: crop, resize or mute; change marks with add, update, remove and the timeline with cut, trim, speed"
                );
            }
            let mut changes = changes.clone();
            let resize = changes
                .as_object_mut()
                .and_then(|changes| changes.remove("resize"));
            let mut value = serde_json::to_value(&next)?;
            merge(&mut value, &changes);
            next = serde_json::from_value(value).map_err(|err| anyhow::anyhow!("set: {err}"))?;
            match resize {
                Some(Value::Null) => next.resize = None,
                Some(resize) => {
                    let crop = next.crop_rect(size);
                    let side = |key: &str| resize.get(key).and_then(Value::as_f64);
                    let ratio = (crop.width / crop.height) as f64;
                    let (width, height) = match (side("width"), side("height")) {
                        (Some(w), Some(h)) => (w, h),
                        (Some(w), None) => (w, w / ratio),
                        (None, Some(h)) => (h * ratio, h),
                        (None, None) => bail!("resize: give width, height or both"),
                    };
                    if !(16. ..=8192.).contains(&width) || !(16. ..=8192.).contains(&height) {
                        bail!("resize: 16 to 8192 pixels a side");
                    }
                    next.resize = Some(Size {
                        width: width.round() as u32,
                        height: height.round() as u32,
                    });
                }
                None => {}
            }
            done.push("changed".into());
        }
        if let Some(remove) = request.get("remove") {
            match remove {
                Value::String(all) if all == "all" => next.overlays.clear(),
                _ => {
                    let ids: Vec<u32> = serde_json::from_value(remove.clone())
                        .map_err(|_| anyhow::anyhow!("remove: a list of mark ids, or \"all\""))?;
                    next.overlays.retain(|overlay| !ids.contains(&overlay.id));
                }
            }
            done.push("removed marks".into());
        }
        for update in request
            .get("update")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let id = update
                .get("id")
                .and_then(Value::as_u64)
                .ok_or_else(|| anyhow::anyhow!("update: each needs the mark's id"))?
                as u32;
            let overlay = next
                .overlay_mut(id)
                .ok_or_else(|| anyhow::anyhow!("update: no mark {id}"))?;
            let mut value = serde_json::to_value(&*overlay)?;
            merge(&mut value, update);
            let mut updated: Overlay = serde_json::from_value(value)
                .map_err(|err| anyhow::anyhow!("update mark {id}: {err}"))?;
            updated.kind.check()?;
            updated.id = id;
            *overlay = updated;
            done.push(format!("updated mark {id}"));
        }
        for add in request
            .get("add")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let mut overlay: Overlay =
                serde_json::from_value(add.clone()).map_err(|err| anyhow::anyhow!("add: {err}"))?;
            overlay.id = 0;
            let id = next.add_overlay(overlay, size)?;
            done.push(format!("added mark {id}"));
        }
        next.sanitize(duration, size)?;
        *self = next;
        Ok(done)
    }

    /// Clamps every number to what can be made.
    pub fn sanitize(&mut self, duration: f64, size: (u32, u32)) -> Result<()> {
        if self.segments.len() > MAX_SEGMENTS {
            bail!("at most {MAX_SEGMENTS} parts");
        }
        for piece in &mut self.segments {
            if !(piece.start.is_finite() && piece.end.is_finite()) {
                bail!("a part's times are not numbers");
            }
            piece.start = piece.start.clamp(0., duration);
            piece.end = piece.end.clamp(0., duration);
            piece.speed = piece.speed.clamp(0.25, 32.);
        }
        let pieces = std::mem::take(&mut self.segments);
        if !pieces.is_empty() {
            self.set_pieces(pieces, duration);
        }
        if let Some(crop) = self.crop {
            let clamped = crop.clamped(size.0 as f32, size.1 as f32);
            self.crop =
                (clamped != Rect::new(0., 0., size.0 as f32, size.1 as f32)).then_some(clamped);
        }
        if let Some(resize) = &mut self.resize {
            resize.width = resize.width.clamp(16, 8192);
            resize.height = resize.height.clamp(16, 8192);
        }
        for overlay in &mut self.overlays {
            if !(overlay.start.is_finite() && overlay.end.is_finite()) {
                bail!("a mark's times are not numbers");
            }
            overlay.start = overlay.start.clamp(0., duration);
            overlay.end = overlay.end.clamp(overlay.start, duration);
            overlay.fade = overlay.fade.clamp(0., 5.);
            overlay.kind.check()?;
        }
        Ok(())
    }
}

/// `value` rounded down to an even number, at least 2.
fn even(value: u32) -> u32 {
    (value & !1).max(2)
}

/// How a video is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VideoFormat {
    /// H.264 in MP4: plays everywhere.
    #[default]
    Mp4,
    /// HEVC in MP4: about half the size, newer players.
    Hevc,
    /// An animated GIF, for pull requests and chats.
    Gif,
    /// VP9 in WebM (ffmpeg writes it; on Linux GStreamer can too).
    Webm,
}

impl VideoFormat {
    pub const ALL: [VideoFormat; 4] = [
        VideoFormat::Mp4,
        VideoFormat::Hevc,
        VideoFormat::Gif,
        VideoFormat::Webm,
    ];

    pub fn label(self) -> &'static str {
        match self {
            VideoFormat::Mp4 => "MP4",
            VideoFormat::Hevc => "MP4 (HEVC)",
            VideoFormat::Gif => "GIF",
            VideoFormat::Webm => "WebM",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            VideoFormat::Mp4 => "Plays everywhere",
            VideoFormat::Hevc => "About half the size; newer players",
            VideoFormat::Gif => "No sound; for pull requests and chats",
            VideoFormat::Webm => "For the web",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            VideoFormat::Mp4 | VideoFormat::Hevc => "mp4",
            VideoFormat::Gif => "gif",
            VideoFormat::Webm => "webm",
        }
    }

    pub fn from_extension(extension: &str) -> Option<VideoFormat> {
        Some(match extension.to_ascii_lowercase().as_str() {
            "mp4" | "m4v" | "mov" => VideoFormat::Mp4,
            "gif" => VideoFormat::Gif,
            "webm" => VideoFormat::Webm,
            _ => return None,
        })
    }

    pub fn has_sound(self) -> bool {
        self != VideoFormat::Gif
    }
}

/// How the result is written.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct VideoExport {
    pub format: VideoFormat,
    /// The most megabytes the file may take.
    pub max_mb: Option<u32>,
    /// Frames a second (the source's, at most 60, when `None`; GIFs 12).
    pub fps: Option<u32>,
    /// A GIF's width at most.
    pub gif_width: u32,
}

impl Default for VideoExport {
    fn default() -> Self {
        VideoExport {
            format: VideoFormat::Mp4,
            max_mb: None,
            fps: None,
            gif_width: 720,
        }
    }
}

impl VideoExport {
    pub fn sanitize(&mut self) {
        self.max_mb = self.max_mb.filter(|mb| *mb > 0).map(|mb| mb.min(100_000));
        self.fps = self.fps.map(|fps| fps.clamp(1, 120));
        self.gif_width = self.gif_width.clamp(64, 2048);
    }

    /// Frames a second written, for a source of `source_fps`.
    pub fn frame_rate(&self, source_fps: f64) -> f64 {
        match (self.fps, self.format) {
            (Some(fps), _) => fps as f64,
            (None, VideoFormat::Gif) => 12.,
            (None, _) => source_fps.clamp(1., 60.).round(),
        }
    }

    /// The video bitrate (bits a second) for `seconds` of `width` ×
    /// `height` at `fps`: enough for crisp screen recordings, or what fits
    /// the size limit (sound taking `audio` bits a second).
    pub fn bitrate(&self, width: u32, height: u32, fps: f64, seconds: f64, audio: u32) -> u32 {
        let codec = if self.format == VideoFormat::Hevc {
            0.6
        } else {
            1.
        };
        let natural =
            (width as f64 * height as f64 * fps * 0.075 * codec).clamp(400_000., 40_000_000.);
        match self.max_mb {
            Some(mb) => {
                // A little under, for the container.
                let budget = mb as f64 * 1024. * 1024. * 8. * 0.94 / seconds.max(0.1);
                (budget - audio as f64).clamp(100_000., natural) as u32
            }
            None => natural as u32,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const DURATION: f64 = 60.;
    const SIZE: (u32, u32) = (1920, 1080);

    #[test]
    fn untouched_keeps_everything() {
        let edit = VideoEdit::default();
        assert_eq!(edit.output_duration(DURATION), 60.);
        assert_eq!(edit.to_output(DURATION, 12.5), Some(12.5));
        assert_eq!(edit.to_source(DURATION, 12.5), (12.5, 0));
        assert_eq!(edit.output_size(SIZE), (1920, 1080));
    }

    #[test]
    fn cutting_and_speeding_map_times() {
        let mut edit = VideoEdit::default();
        edit.cut(DURATION, 10., 20.);
        assert_eq!(
            edit.segments,
            [Segment::new(0., 10., 1.), Segment::new(20., 60., 1.)]
        );
        assert_eq!(edit.output_duration(DURATION), 50.);
        assert_eq!(edit.to_output(DURATION, 15.), None, "cut");
        assert_eq!(edit.to_output(DURATION, 25.), Some(15.));
        edit.speed_range(DURATION, 30., 50., 4.);
        assert_eq!(
            edit.segments,
            [
                Segment::new(0., 10., 1.),
                Segment::new(20., 30., 1.),
                Segment::new(30., 50., 4.),
                Segment::new(50., 60., 1.),
            ]
        );
        // 0–10, 20–30, 30–50 at 4× (5 s), 50–60.
        assert_eq!(edit.output_duration(DURATION), 10. + 10. + 5. + 10.);
        assert_eq!(edit.to_output(DURATION, 40.), Some(20. + 2.5));
        let (source, ix) = edit.to_source(DURATION, 22.5);
        assert_eq!((source, ix), (40., 2));
    }

    #[test]
    fn parts_stay_tidy() {
        let mut edit = VideoEdit::default();
        edit.split(DURATION, 30.);
        assert_eq!(edit.segments.len(), 2);
        // A split stays, even at one speed, so either half can change.
        edit.set_piece_speed(DURATION, 0, 1.);
        assert_eq!(edit.segments.len(), 2);
        edit.sanitize(DURATION, SIZE).unwrap();
        assert_eq!(edit.segments.len(), 2, "kept through the checks");
        edit.set_piece_speed(DURATION, 1, 2.);
        edit.delete_piece(DURATION, 0);
        assert_eq!(edit.segments, [Segment::new(30., 60., 2.)]);
        // The last part is never deleted.
        edit.delete_piece(DURATION, 0);
        assert_eq!(edit.segments.len(), 1);
        edit.move_edge(DURATION, 0, false, 10.);
        assert_eq!(edit.segments, [Segment::new(10., 60., 2.)]);
        // A split's edge moves the split.
        edit.split(DURATION, 30.);
        edit.move_edge(DURATION, 0, true, 40.);
        assert_eq!(
            edit.segments,
            [Segment::new(10., 40., 2.), Segment::new(40., 60., 2.)]
        );
        edit.trim(DURATION, 20., 40.);
        assert_eq!(edit.segments, [Segment::new(20., 40., 2.)]);
    }

    #[test]
    fn overlays_fade() {
        let overlay = Overlay {
            id: 1,
            start: 10.,
            end: 14.,
            fade: 1.,
            kind: crate::edit::MarkType::Rect.mark([0., 0.], [10., 10.], crate::edit::Color::RED),
        };
        assert_eq!(overlay.alpha(9.), 0.);
        assert_eq!(overlay.alpha(10.5), 0.5);
        assert_eq!(overlay.alpha(12.), 1.);
        assert_eq!(overlay.alpha(13.75), 0.25);
    }

    #[test]
    fn an_agent_edits_the_timeline_and_marks() {
        let mut edit = VideoEdit::default();
        edit.apply(
            &json!({
                "cut": [{ "from": 0, "to": 5 }],
                "speed": [{ "from": 20, "to": 40, "speed": 8 }],
                "add": [{ "type": "text", "x": 40, "y": 40, "text": "Login", "start": 6, "end": 9, "fade": 0.3 }],
                "set": { "mute": true, "resize": { "width": 1280 } }
            }),
            DURATION,
            SIZE,
        )
        .unwrap();
        assert_eq!(edit.segments[0].start, 5.);
        assert_eq!(edit.output_duration(DURATION), 15. + 2.5 + 20.);
        assert!(edit.mute);
        assert_eq!(edit.output_size(SIZE), (1280, 720));
        assert_eq!(edit.overlays[0].id, 1);
        edit.apply(
            &json!({ "update": [{ "id": 1, "end": 12 }] }),
            DURATION,
            SIZE,
        )
        .unwrap();
        assert_eq!(edit.overlays[0].end, 12.);
        let before = edit.clone();
        assert!(
            edit.apply(&json!({ "cut": [{ "from": 1 }] }), DURATION, SIZE)
                .is_err()
        );
        assert_eq!(edit, before, "a failed change changes nothing");
        edit.apply(&json!({ "reset_timeline": true }), DURATION, SIZE)
            .unwrap();
        assert!(edit.segments.is_empty());
    }

    #[test]
    fn a_size_limit_sets_the_bitrate() {
        let export = VideoExport {
            max_mb: Some(50),
            ..Default::default()
        };
        // 10 minutes in 50 MB: about 0.66 Mbit/s with 128 kbit/s of sound.
        let bitrate = export.bitrate(1920, 1080, 30., 600., 128_000);
        assert!((500_000..700_000).contains(&bitrate), "{bitrate}");
        let free = VideoExport::default().bitrate(1920, 1080, 30., 600., 128_000);
        assert!(free > 3_000_000);
    }
}
