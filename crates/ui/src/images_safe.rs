//! Pictures in documents (Markdown, notebooks), loaded so that a drawing
//! cannot read files: GPUI draws any bytes that are not a known raster
//! format as SVG, and its SVG renderer reads every path an `<image>` names
//! (a device such as /dev/zero never ends; a network path on Windows sends
//! the user's sign-in there). Here an SVG that links files is refused,
//! wherever it comes from: a file beside the document, a web address, or a
//! `data:` URL. A picture attached to a chat is held tighter still: only the
//! raster kinds are drawn ([`AttachedImage`]).

use std::path::Path;
use std::sync::Arc;

use gpui_kit::{
    App, Asset, Global, Image, ImageCacheError, ImageFormat, ImageSource, RenderImage, Resource,
};

/// The most a picture's file or download may weigh.
const MOST: u64 = 64 * 1024 * 1024;
/// The most the frames of a GIF may take decoded. GPUI keeps every frame,
/// each as large as the whole picture: a GIF of a few kilobytes can ask
/// for gigabytes, and an ordinary screen recording for more than one.
const GIF_FRAMES: u64 = 256 * 1024 * 1024;
/// The longest side of a GIF that is decoded.
pub const GIF_SIDE: u32 = 8192;

pub enum DocumentImage {}

/// `resource` as a picture source that goes through [`DocumentImage`].
pub fn source(resource: Resource) -> ImageSource {
    ImageSource::Custom(Arc::new(move |window, cx| {
        window.use_asset::<DocumentImage>(&resource, cx)
    }))
}

fn failed(message: impl Into<String>) -> ImageCacheError {
    ImageCacheError::Other(Arc::new(anyhow::anyhow!(message.into())))
}

impl Asset for DocumentImage {
    type Source = Resource;
    type Output = Result<Arc<RenderImage>, ImageCacheError>;

    fn load(
        source: Self::Source,
        cx: &mut App,
    ) -> impl std::future::Future<Output = Self::Output> + Send + 'static {
        let client = cx.http_client();
        let svg_renderer = cx.svg_renderer();
        async move {
            let bytes = match &source {
                Resource::Path(path) => {
                    use std::io::Read as _;
                    let mut bytes = Vec::new();
                    std::fs::File::open(path.as_ref())
                        .and_then(|file| file.take(MOST + 1).read_to_end(&mut bytes))
                        .map_err(|err| ImageCacheError::Io(Arc::new(err)))?;
                    bytes
                }
                Resource::Uri(uri) if uri.as_ref().starts_with("data:") => {
                    data_url(uri.as_ref()).ok_or_else(|| failed("not a picture"))?
                }
                Resource::Uri(uri) => {
                    use futures::AsyncReadExt as _;
                    let mut response = client
                        .get(uri.as_ref(), ().into(), true)
                        .await
                        .map_err(|err| failed(format!("{err:#}")))?;
                    if !response.status().is_success() {
                        return Err(failed(format!("{}", response.status())));
                    }
                    let mut bytes = Vec::new();
                    response
                        .body_mut()
                        .take(MOST + 1)
                        .read_to_end(&mut bytes)
                        .await
                        .map_err(|err| ImageCacheError::Io(Arc::new(err)))?;
                    bytes
                }
                Resource::Embedded(_) => return Err(failed("not a document picture")),
            };
            if bytes.len() as u64 > MOST {
                return Err(failed("the picture is too large"));
            }
            let format = match ::image::guess_format(&bytes) {
                Ok(::image::ImageFormat::Png) => ImageFormat::Png,
                Ok(::image::ImageFormat::Jpeg) => ImageFormat::Jpeg,
                Ok(::image::ImageFormat::Gif) => ImageFormat::Gif,
                Ok(::image::ImageFormat::WebP) => ImageFormat::Webp,
                Ok(::image::ImageFormat::Bmp) => ImageFormat::Bmp,
                Ok(::image::ImageFormat::Tiff) => ImageFormat::Tiff,
                Ok(::image::ImageFormat::Ico) => ImageFormat::Ico,
                Ok(_) => return Err(failed("this picture format is not shown")),
                Err(_) => {
                    if let Some(reason) = drawing_refused(&bytes) {
                        return Err(failed(reason));
                    }
                    ImageFormat::Svg
                }
            };
            if format == ImageFormat::Gif {
                return decode_gif(&bytes).map(|(frames, _)| frames).map_err(failed);
            }
            Image::from_bytes(format, bytes)
                .to_image_data(svg_renderer)
                .map_err(|err| failed(format!("{err:#}")))
        }
    }
}

/// A picture attached to a chat or given back by a tool, from its file:
/// drawn only when the file starts as a PNG, JPEG, GIF or WebP (the kinds
/// agents take). The file's name and the kind it was said to be decide
/// nothing, since either can lie, and nothing here is ever drawn as SVG
/// (see the top of this file).
pub enum AttachedImage {}

/// The attached pictures GPUI holds decoded or is loading. Kept so that
/// letting go of one that was never drawn does not load it to find out.
#[derive(Default)]
struct Held(std::collections::HashSet<Arc<Path>>);

impl Global for Held {}

/// The picture in the file at `path`, through [`AttachedImage`].
pub fn attached(path: &Path) -> ImageSource {
    let path: Arc<Path> = path.into();
    ImageSource::Custom(Arc::new(move |window, cx| {
        window.use_asset::<AttachedImage>(&path, cx)
    }))
}

/// The kind of picture `bytes` start as, when it is one an attachment is
/// drawn as.
pub fn raster_format(bytes: &[u8]) -> Option<ImageFormat> {
    match ::image::guess_format(bytes).ok()? {
        ::image::ImageFormat::Png => Some(ImageFormat::Png),
        ::image::ImageFormat::Jpeg => Some(ImageFormat::Jpeg),
        ::image::ImageFormat::Gif => Some(ImageFormat::Gif),
        ::image::ImageFormat::WebP => Some(ImageFormat::Webp),
        _ => None,
    }
}

impl Asset for AttachedImage {
    type Source = Arc<Path>;
    type Output = Result<Arc<RenderImage>, ImageCacheError>;

    fn load(
        source: Self::Source,
        cx: &mut App,
    ) -> impl std::future::Future<Output = Self::Output> + Send + 'static {
        cx.default_global::<Held>().0.insert(source.clone());
        let svg_renderer = cx.svg_renderer();
        async move {
            use std::io::Read as _;
            // A device or a pipe under a picture's name is not opened: one
            // never ends, the other may never start.
            if !std::fs::metadata(&source).is_ok_and(|meta| meta.is_file()) {
                return Err(failed("not a file"));
            }
            let mut bytes = Vec::new();
            std::fs::File::open(&source)
                .and_then(|file| file.take(MOST + 1).read_to_end(&mut bytes))
                .map_err(|err| ImageCacheError::Io(Arc::new(err)))?;
            if bytes.len() as u64 > MOST {
                return Err(failed("the picture is too large"));
            }
            let format = raster_format(&bytes)
                .ok_or_else(|| failed("not a PNG, JPEG, GIF or WebP picture"))?;
            // A GIF within limits: GPUI's own decoding of one has none, and
            // a file of a few dozen bytes can ask for gigabytes.
            if matches!(format, ImageFormat::Gif) {
                return decode_gif(&bytes).map(|(image, _)| image).map_err(failed);
            }
            Image::from_bytes(format, bytes)
                .to_image_data(svg_renderer)
                .map_err(|err| failed(format!("{err:#}")))
        }
    }
}

/// Forgets the attached picture of `path` and gives back what was decoded
/// of it, for its textures to be dropped ([`crate::images::release_paths`]).
/// `None` when it was never drawn, or is still being read.
pub(crate) fn forget_attached(path: &Path, cx: &mut App) -> Option<Arc<RenderImage>> {
    let path: Arc<Path> = path.into();
    if !cx.try_global::<Held>()?.0.contains(&path) {
        return None;
    }
    let decoded = cx.fetch_asset::<AttachedImage>(&path);
    cx.remove_asset::<AttachedImage>(&path);
    cx.global_mut::<Held>().0.remove(&path);
    decoded?.ok()
}

/// Why bytes that are no raster picture are not drawn as an SVG, if they
/// are not.
fn drawing_refused(bytes: &[u8]) -> Option<&'static str> {
    // The renderer unpacks gzip by itself (`.svgz`), after the check for
    // linked files has seen only the packed bytes: a packed drawing could
    // link files unseen, and a small one unpack to gigabytes.
    if bytes.starts_with(&[0x1f, 0x8b]) {
        return Some("compressed drawings are not shown");
    }
    links_files(&String::from_utf8_lossy(bytes)).then_some("the drawing links to files")
}

/// A GIF's frames as GPUI draws them, decoded within limits (GPUI's own
/// decoding has none). One whose frames together pass [`GIF_FRAMES`] is
/// shown still, as its first frame (`true` then); one with a side longer
/// than [`GIF_SIDE`] is refused.
pub fn decode_gif(bytes: &[u8]) -> Result<(Arc<RenderImage>, bool), String> {
    gif_frames(bytes, GIF_FRAMES)
}

/// [`decode_gif`], its frames taking `room` bytes at most.
fn gif_frames(bytes: &[u8], room: u64) -> Result<(Arc<RenderImage>, bool), String> {
    use ::image::{AnimationDecoder as _, ImageDecoder as _};
    let decoder = ::image::codecs::gif::GifDecoder::new(std::io::Cursor::new(bytes))
        .map_err(|err| err.to_string())?;
    let (width, height) = decoder.dimensions();
    if width > GIF_SIDE || height > GIF_SIDE {
        return Err(format!("the GIF is too large ({width} × {height} pixels)"));
    }
    // Every frame is the whole picture, at four bytes a pixel (and a
    // little more, which counts when the picture is a few pixels and the
    // frames are millions).
    let frame = width as u64 * height as u64 * 4 + 128;
    let most = (room / frame).max(1) as usize;
    let mut frames = Vec::new();
    let mut still = false;
    for decoded in decoder.into_frames() {
        // What follows a damaged frame is not read: the frames before it
        // are the picture.
        let Ok(mut decoded) = decoded else {
            break;
        };
        if frames.len() == most {
            frames.truncate(1);
            still = true;
            break;
        }
        // GPUI draws BGRA.
        for pixel in decoded.buffer_mut().as_chunks_mut::<4>().0 {
            pixel.swap(0, 2);
        }
        frames.push(decoded);
    }
    if frames.is_empty() {
        return Err("the GIF could not be decoded".into());
    }
    Ok((Arc::new(RenderImage::new(frames)), still))
}

/// The bytes of a `data:` URL (Base64, or percent-encoded text).
fn data_url(url: &str) -> Option<Vec<u8>> {
    use base64::Engine as _;
    let (meta, data) = url.strip_prefix("data:")?.split_once(',')?;
    if meta.ends_with(";base64") {
        let data: String = data.chars().filter(|c| !c.is_whitespace()).collect();
        base64::engine::general_purpose::STANDARD.decode(data).ok()
    } else {
        Some(percent_decode(data))
    }
}

/// Whether an SVG document links a file (an `<image>` or `<use>` naming a
/// path, on this computer or another, or written with character
/// references): drawing it would read that file. A device such as
/// /dev/zero is read forever, and on Windows reading a network path sends
/// the user's sign-in there.
pub fn links_files(svg: &str) -> bool {
    svg.match_indices("href").any(|(at, _)| {
        let rest = svg[at + 4..].trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            return false;
        };
        let rest = rest.trim_start();
        let value = match rest.chars().next() {
            Some(quote @ ('"' | '\'')) => rest[1..].split(quote).next().unwrap_or(""),
            _ => return false,
        };
        let value = value.trim();
        // Pictures inside it (data:) and links within it (#id) are drawn;
        // anything else would be read from disk by the renderer (a device
        // such as /dev/zero never ends; a network path leaks credentials).
        let lower = value.to_ascii_lowercase();
        !(lower.starts_with("data:")
            || lower.starts_with("http://")
            || lower.starts_with("https://")
            || value.starts_with('#')
            || value.is_empty())
    })
}

/// Bytes of percent-encoded text (`%3C` is `<`); a `%` not followed by two
/// hex digits stays as it is.
fn percent_decode(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let hex = |byte: u8| (byte as char).to_digit(16);
    let mut out = Vec::with_capacity(bytes.len());
    let mut ix = 0;
    while ix < bytes.len() {
        if bytes[ix] == b'%'
            && let (Some(high), Some(low)) = (
                bytes.get(ix + 1).copied().and_then(hex),
                bytes.get(ix + 2).copied().and_then(hex),
            )
        {
            out.push((high * 16 + low) as u8);
            ix += 3;
            continue;
        }
        out.push(bytes[ix]);
        ix += 1;
    }
    out
}

/// For Markdown an agent wrote (chat replies): web and `data:` pictures
/// through [`DocumentImage`]; files on this computer are not read.
pub fn web_only(uri: &gpui_kit::SharedUri) -> ImageSource {
    let lower = uri.as_ref().to_ascii_lowercase();
    if lower.starts_with("https://") || lower.starts_with("data:") {
        source(Resource::Uri(uri.clone()))
    } else {
        ImageSource::Custom(Arc::new(|_, _| Some(Err(failed("not loaded")))))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_data_urls() {
        assert_eq!(
            super::data_url("data:image/png;base64,AAEC").unwrap(),
            [0, 1, 2]
        );
        assert_eq!(
            super::data_url("data:image/svg+xml,%3Csvg%3E").unwrap(),
            b"<svg>"
        );
        assert!(super::data_url("data:nothing").is_none());
    }

    #[test]
    fn packed_drawings_and_drawings_that_link_files_are_refused() {
        // The start of any gzip stream: what is inside is never looked at.
        assert!(super::drawing_refused(&[0x1f, 0x8b, 8, 0, 0, 0, 0, 0]).is_some());
        assert!(super::drawing_refused(br#"<svg><image href="/dev/zero"/></svg>"#).is_some());
        assert!(super::drawing_refused(br#"<svg><circle r="4"/></svg>"#).is_none());
    }

    /// A GIF of `frames` frames, each all of a `side` × `side` picture.
    fn gif(side: u16, frames: usize) -> Vec<u8> {
        use ::image::codecs::gif::{GifEncoder, Repeat};
        let mut bytes = Vec::new();
        {
            let mut encoder = GifEncoder::new(&mut bytes);
            encoder.set_repeat(Repeat::Infinite).unwrap();
            for shade in 0..frames {
                let pixels = ::image::RgbaImage::from_pixel(
                    side as u32,
                    side as u32,
                    ::image::Rgba([shade as u8, 0, 255, 255]),
                );
                encoder.encode_frame(::image::Frame::new(pixels)).unwrap();
            }
        }
        bytes
    }

    #[test]
    fn gifs_are_decoded_within_limits() {
        // Small: all its frames, blue and red swapped for GPUI.
        let (image, still) = super::decode_gif(&gif(4, 3)).unwrap();
        assert_eq!((image.frame_count(), still), (3, false));
        assert_eq!(&image.as_bytes(0).unwrap()[..4], [255, 0, 0, 255]);
        // Room for three frames: three play, four are too many and the
        // first is shown still.
        let room = 3 * (4 * 4 * 4 + 128);
        let (image, still) = super::gif_frames(&gif(4, 3), room).unwrap();
        assert_eq!((image.frame_count(), still), (3, false));
        let (image, still) = super::gif_frames(&gif(4, 4), room).unwrap();
        assert_eq!((image.frame_count(), still), (1, true));
        // A GIF that says it is 10,000 pixels wide is refused before any
        // frame is decoded.
        let mut wide = gif(4, 1);
        wide[6..8].copy_from_slice(&10_000u16.to_le_bytes());
        assert!(super::decode_gif(&wide).unwrap_err().contains("too large"));
        assert!(super::decode_gif(b"GIF89a").is_err());
    }

    /// An attachment is drawn by what its bytes are: a drawing under a
    /// picture's name (which would be read as SVG, and read the files it
    /// links) is none, and neither is a kind agents do not take.
    #[test]
    fn attached_pictures_are_told_by_their_first_bytes() {
        use gpui_kit::ImageFormat;
        let kind = super::raster_format;
        assert_eq!(kind(b"\x89PNG\r\n\x1a\n...."), Some(ImageFormat::Png));
        assert_eq!(kind(b"\xFF\xD8\xFF\xE0...."), Some(ImageFormat::Jpeg));
        assert_eq!(kind(b"GIF89a...."), Some(ImageFormat::Gif));
        assert_eq!(kind(b"RIFF\0\0\0\0WEBPVP8 "), Some(ImageFormat::Webp));
        assert_eq!(
            kind(br#"<svg xmlns="http://www.w3.org/2000/svg"><image href="/dev/zero"/></svg>"#),
            None
        );
        assert_eq!(kind(b"<?xml version=\"1.0\"?><svg/>"), None);
        assert_eq!(kind(b"BM\0\0\0\0\0\0\0\0"), None, "BMP");
        assert_eq!(kind(b"II*\0\0\0\0\0"), None, "TIFF");
        assert_eq!(kind(b""), None);
    }
}
