//! What Windows' web view (WebView2) does beyond showing a page: a picture
//! of what it shows, and the page printed to a PDF. Both come from WebView2
//! itself, so they are the page as the user sees it.

use std::path::PathBuf;

use anyhow::{Context as _, Result, anyhow};
use gpui_kit::{Bounds, Pixels, Size};
use webview2_com::Microsoft::Web::WebView2::Win32::{
    COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG, ICoreWebView2_7, ICoreWebView2Environment6,
};
use webview2_com::{CapturePreviewCompletedHandler, PrintToPdfCompletedHandler};
use windows::Win32::System::Com::{IStream, STATFLAG_NONAME, STATSTG, STREAM_SEEK_SET};
use windows::Win32::UI::Shell::SHCreateMemStream;
use windows::core::{HSTRING, Interface as _};
use wry::WebViewExtWindows as _;

type Answer = smol::channel::Receiver<Result<Vec<u8>>>;

/// A key is pressed as the system does only on macOS (the app's tests).
pub fn press(_: &wry::WebView, _: &crate::Key<'_>, _: bool) -> Result<bool> {
    Err(anyhow!("keys are pressed this way on macOS only"))
}

/// Whether the view has the keyboard is not asked here: WebView2 keeps it
/// in a window of another process.
pub fn has_keys(_: &wry::WebView) -> Option<bool> {
    None
}

fn failed(why: anyhow::Error) -> Answer {
    let (done, answer) = smol::channel::bounded(1);
    let _ = done.try_send(Err(why));
    answer
}

/// Everything a stream holds, from its start.
fn read_all(stream: &IStream) -> Result<Vec<u8>> {
    // Safety: plain COM calls on a stream this module made; the buffer is
    // as long as the stream says it is.
    unsafe {
        let mut stat = STATSTG::default();
        stream.Stat(&mut stat, STATFLAG_NONAME)?;
        stream.Seek(0, STREAM_SEEK_SET, None)?;
        let mut bytes = vec![0u8; stat.cbSize as usize];
        let mut read = 0u32;
        stream
            .Read(
                bytes.as_mut_ptr().cast(),
                bytes.len() as u32,
                Some(&mut read),
            )
            .ok()?;
        bytes.truncate(read as usize);
        Ok(bytes)
    }
}

/// A PNG of what the view shows, at the screen's density. WebView2
/// pictures the whole view: `rect` is not looked at, and callers size the
/// view to what they want a picture of.
pub fn snapshot(view: &wry::WebView, _rect: Option<Bounds<Pixels>>) -> Answer {
    let (done, answer) = smol::channel::bounded(1);
    // Safety: plain WebView2 calls on the thread the view lives on; the
    // handler is called once, there too.
    let asked = unsafe {
        let Some(stream) = SHCreateMemStream(None) else {
            return failed(anyhow!("no room to keep the picture of the page"));
        };
        let kept = stream.clone();
        let handler = CapturePreviewCompletedHandler::create(Box::new(move |result| {
            let png = result
                .map_err(|err| anyhow!("the page could not be pictured: {err}"))
                .and_then(|()| read_all(&kept));
            let _ = done.try_send(png);
            Ok(())
        }));
        view.webview().CapturePreview(
            COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG,
            &stream,
            &handler,
        )
    };
    match asked {
        Ok(()) => answer,
        Err(err) => failed(anyhow!("the page could not be pictured: {err}")),
    }
}

/// The page as a PDF, on paper `page` large (the page's own points, 96 to
/// the inch), without margins: a page for each page-sized part of it.
pub fn print(view: &wry::WebView, page: Size<Pixels>) -> Answer {
    let (done, answer) = smol::channel::bounded(1);
    let file: PathBuf = std::env::temp_dir().join(format!(
        "{}-deck-{}-{}.pdf",
        env!("CARGO_PKG_NAME"),
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|now| now.as_nanos())
            .unwrap_or_default()
    ));
    let asked = (|| -> Result<()> {
        // Safety: as in `snapshot`.
        unsafe {
            let web: ICoreWebView2_7 = view
                .webview()
                .cast()
                .context("this WebView2 is too old to print to a PDF")?;
            let environment: ICoreWebView2Environment6 = view
                .environment()
                .cast()
                .context("this WebView2 is too old to print to a PDF")?;
            let settings = environment.CreatePrintSettings()?;
            settings.SetPageWidth(f64::from(f32::from(page.width)) / 96.)?;
            settings.SetPageHeight(f64::from(f32::from(page.height)) / 96.)?;
            settings.SetMarginTop(0.)?;
            settings.SetMarginBottom(0.)?;
            settings.SetMarginLeft(0.)?;
            settings.SetMarginRight(0.)?;
            settings.SetScaleFactor(1.)?;
            settings.SetShouldPrintBackgrounds(true)?;
            settings.SetShouldPrintHeaderAndFooter(false)?;
            let written = file.clone();
            let handler = PrintToPdfCompletedHandler::create(Box::new(move |result, printed| {
                let pdf = result
                    .map_err(|err| anyhow!("the page could not be made a PDF: {err}"))
                    .and_then(|()| {
                        if !printed {
                            return Err(anyhow!("the page could not be made a PDF"));
                        }
                        std::fs::read(&written).context("the PDF could not be read")
                    });
                let _ = std::fs::remove_file(&written);
                let _ = done.try_send(pdf);
                Ok(())
            }));
            web.PrintToPdf(&HSTRING::from(file.as_os_str()), &settings, &handler)?;
        }
        Ok(())
    })();
    match asked {
        Ok(()) => answer,
        Err(err) => failed(err),
    }
}

/// One part of the page as a PDF is not WebView2's to make: it prints the
/// whole page ([`print`]).
pub fn pdf(_: &wry::WebView, _: Option<Bounds<Pixels>>) -> Answer {
    failed(anyhow!("on Windows a page is printed whole"))
}

pub fn join(_: &[Vec<u8>]) -> Result<Vec<u8>> {
    Err(anyhow!("on Windows a page is printed whole"))
}
