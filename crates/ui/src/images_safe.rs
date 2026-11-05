//! Pictures in documents (Markdown, notebooks), loaded so that a drawing
//! cannot read files: GPUI draws any bytes that are not a known raster
//! format as SVG, and its SVG renderer reads every path an `<image>` names
//! (a device such as /dev/zero never ends; a network path on Windows sends
//! the user's sign-in there). Here an SVG that links files is refused,
//! wherever it comes from: a file beside the document, a web address, or a
//! `data:` URL.

use std::sync::Arc;

use gpui_kit::{
    App, Asset, Image, ImageCacheError, ImageFormat, ImageSource, RenderImage, Resource,
};

/// The most a picture's file or download may weigh.
const MOST: u64 = 64 * 1024 * 1024;

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
                    if links_files(&String::from_utf8_lossy(&bytes)) {
                        return Err(failed("the drawing links to files"));
                    }
                    ImageFormat::Svg
                }
            };
            Image::from_bytes(format, bytes)
                .to_image_data(svg_renderer)
                .map_err(|err| failed(format!("{err:#}")))
        }
    }
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
}
