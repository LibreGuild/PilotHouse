//! What macOS's web view does beyond showing a page: a picture of what it
//! shows, and the page as a PDF. Both come from WebKit itself, so they are
//! the page exactly as the user sees it.

use anyhow::{Context as _, Result, anyhow};
use block2::RcBlock;
use gpui_kit::{Bounds, Pixels};
use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSApplication, NSBitmapImageFileType, NSBitmapImageRep, NSEvent, NSEventModifierFlags,
    NSEventType, NSImage, NSView,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{NSData, NSDictionary, NSError, NSString};
use objc2_web_kit::{WKPDFConfiguration, WKSnapshotConfiguration, WKWebView};
use wry::WebViewExtMacOS as _;

/// Whether the view, or something in it, has its window's keyboard.
pub fn has_keys(view: &wry::WebView) -> Option<bool> {
    let web = view.webview();
    let web: &WKWebView = &web;
    let held = web.window().and_then(|window| window.firstResponder());
    Some(held.is_some_and(|held| {
        held.downcast_ref::<NSView>()
            .is_some_and(|held| held.isDescendantOf(web))
    }))
}

/// Presses `key` in the view's window as the system does: the event goes
/// the way a real one goes (key equivalents of the views and the menus,
/// then the view that has the keyboard). For the app's own tests of who
/// gets a key; with `front`, the window is put in front first, as it is
/// when a person types.
pub fn press(view: &wry::WebView, key: &crate::Key<'_>, front: bool) -> Result<bool> {
    let main = MainThreadMarker::new().context("a key is pressed on the main thread")?;
    let web = view.webview();
    let web: &WKWebView = &web;
    let window = web.window().context("the view is in no window")?;
    let app = NSApplication::sharedApplication(main);
    if front {
        // The newer, polite call is refused while another app is in front.
        #[allow(deprecated)]
        app.activateIgnoringOtherApps(true);
        window.makeKeyAndOrderFront(None);
    }
    let mut flags = NSEventModifierFlags::empty();
    if key.command {
        flags |= NSEventModifierFlags::Command;
    }
    if key.shift {
        flags |= NSEventModifierFlags::Shift;
    }
    if key.function {
        flags |= NSEventModifierFlags::Function | NSEventModifierFlags::NumericPad;
    }
    for kind in [NSEventType::KeyDown, NSEventType::KeyUp] {
        let event = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
            kind,
            CGPoint::new(0., 0.),
            flags,
            objc2_foundation::NSProcessInfo::processInfo().systemUptime(),
            window.windowNumber(),
            None,
            &NSString::from_str(key.characters),
            &NSString::from_str(key.plain),
            false,
            key.code,
        )
        .context("the key event could not be made")?;
        // Onto the app's queue of events, not sent here: a real key comes
        // when the app is not in the middle of something.
        app.postEvent_atStart(&event, false);
    }
    // Whether the window was the one typed in: only then is the key
    // handled as a person's would be.
    Ok(window.isKeyWindow())
}

fn cg(rect: Bounds<Pixels>) -> CGRect {
    CGRect {
        origin: CGPoint::new(
            f64::from(f32::from(rect.origin.x)),
            f64::from(f32::from(rect.origin.y)),
        ),
        size: CGSize::new(
            f64::from(f32::from(rect.size.width)),
            f64::from(f32::from(rect.size.height)),
        ),
    }
}

fn failure(error: *mut NSError, what: &str) -> anyhow::Error {
    // Safety: WebKit hands the block a valid error or null, for the call.
    match unsafe { error.as_ref() } {
        Some(error) => anyhow!("{what}: {}", error.localizedDescription()),
        None => anyhow!("{what}"),
    }
}

/// A PNG of what the view shows in `rect` (in the view's own points; all of
/// it without one), at the screen's density.
pub fn snapshot(
    view: &wry::WebView,
    rect: Option<Bounds<Pixels>>,
) -> smol::channel::Receiver<Result<Vec<u8>>> {
    let (done, answer) = smol::channel::bounded(1);
    let Some(main) = MainThreadMarker::new() else {
        let _ = done.try_send(Err(anyhow!("a web view is asked on the main thread")));
        return answer;
    };
    let web = view.webview();
    let web: &WKWebView = &web;
    // Safety: plain WebKit calls on the main thread; the block is copied by
    // WebKit and called once, there too.
    unsafe {
        let configuration = WKSnapshotConfiguration::new(main);
        if let Some(rect) = rect {
            configuration.setRect(cg(rect));
        }
        let block = RcBlock::new(move |image: *mut NSImage, error: *mut NSError| {
            let png = image
                .as_ref()
                .ok_or_else(|| failure(error, "the page could not be pictured"))
                .and_then(png_of);
            let _ = done.try_send(png);
        });
        web.takeSnapshotWithConfiguration_completionHandler(Some(&configuration), &block);
    }
    answer
}

fn png_of(image: &NSImage) -> Result<Vec<u8>> {
    let tiff = image
        .TIFFRepresentation()
        .context("the picture of the page is empty")?;
    let bitmap = NSBitmapImageRep::imageRepWithData(&tiff)
        .context("the picture of the page could not be read")?;
    // Safety: an empty dictionary of options, as the method takes.
    let png = unsafe {
        bitmap.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
    }
    .context("the picture of the page could not be written as PNG")?;
    Ok(png.to_vec())
}

/// `rect` of the page (in the page's own points; all of it without one) as
/// a PDF of one page: what it draws as it draws it, text as text.
pub fn pdf(
    view: &wry::WebView,
    rect: Option<Bounds<Pixels>>,
) -> smol::channel::Receiver<Result<Vec<u8>>> {
    let (done, answer) = smol::channel::bounded(1);
    let Some(main) = MainThreadMarker::new() else {
        let _ = done.try_send(Err(anyhow!("a web view is asked on the main thread")));
        return answer;
    };
    let web = view.webview();
    let web: &WKWebView = &web;
    // Safety: as in `snapshot`.
    unsafe {
        let configuration = WKPDFConfiguration::new(main);
        if let Some(rect) = rect {
            configuration.setRect(cg(rect));
        }
        let block = RcBlock::new(move |data: *mut NSData, error: *mut NSError| {
            let pdf = data
                .as_ref()
                .map(NSData::to_vec)
                .ok_or_else(|| failure(error, "the page could not be made a PDF"));
            let _ = done.try_send(pdf);
        });
        web.createPDFWithConfiguration_completionHandler(Some(&configuration), &block);
    }
    answer
}

/// One PDF of the pages of `parts`, in order (each part a PDF of its own,
/// as [`pdf`] makes them).
pub fn join(parts: &[Vec<u8>]) -> Result<Vec<u8>> {
    use objc2::AnyThread as _;
    use objc2_pdf_kit::PDFDocument;
    let mut joined: Option<objc2::rc::Retained<PDFDocument>> = None;
    for part in parts {
        let data = NSData::with_bytes(part);
        // Safety: PDFKit reads the bytes it is given; a document that is
        // not one comes back as none.
        let document = unsafe { PDFDocument::initWithData(PDFDocument::alloc(), &data) }
            .context("a page of the PDF could not be read")?;
        match &joined {
            None => joined = Some(document),
            Some(whole) => unsafe {
                for index in 0..document.pageCount() {
                    if let Some(page) = document.pageAtIndex(index) {
                        whole.insertPage_atIndex(&page, whole.pageCount());
                    }
                }
            },
        }
    }
    let whole = joined.context("there are no pages to make a PDF of")?;
    // Safety: a plain read of the document just built.
    unsafe { whole.dataRepresentation() }
        .map(|data| data.to_vec())
        .context("the PDF could not be written")
}

/// Printing the whole page to a PDF is Windows' way; here a part of the
/// page is made a PDF ([`pdf`]) and the parts are joined ([`join`]).
pub fn print(
    _: &wry::WebView,
    _: gpui_kit::Size<Pixels>,
) -> smol::channel::Receiver<Result<Vec<u8>>> {
    let (done, answer) = smol::channel::bounded(1);
    let _ = done.try_send(Err(anyhow!("on macOS a page is made a PDF part by part")));
    answer
}
