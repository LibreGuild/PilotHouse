//! Where the system has no web view for the app's windows (Linux): none is
//! made, and callers show the page another way (`supported` says so first).

use anyhow::{Result, bail};
use gpui_kit::{Bounds, Pixels, Window};

pub struct Native;

impl Native {
    pub fn new(_: &str, _: &mut Window, _: smol::channel::Sender<String>) -> Result<Self> {
        bail!("this system has no web view for the app's windows")
    }

    pub fn load(&self, _: &str) {}

    pub fn evaluate(&self, _: &str) {}

    pub fn set_visible(&self, _: bool) {}

    pub fn focus(&self) {}

    pub fn release_keys(&self) {}

    pub fn has_keys(&self) -> Option<bool> {
        None
    }

    pub fn press(&self, _: &crate::Key<'_>, _: bool) -> Result<bool> {
        Err(anyhow::anyhow!("this system has no web view in the window"))
    }

    pub fn place(&self, _: Bounds<Pixels>) {}

    pub fn snapshot(&self, _: Option<Bounds<Pixels>>) -> smol::channel::Receiver<Result<Vec<u8>>> {
        failed()
    }

    pub fn pdf(&self, _: Option<Bounds<Pixels>>) -> smol::channel::Receiver<Result<Vec<u8>>> {
        failed()
    }

    pub fn print(&self, _: gpui_kit::Size<Pixels>) -> smol::channel::Receiver<Result<Vec<u8>>> {
        failed()
    }
}

fn failed() -> smol::channel::Receiver<Result<Vec<u8>>> {
    let (done, answer) = smol::channel::bounded(1);
    let _ = done.try_send(Err(anyhow::anyhow!(
        "this system has no web view for the app's windows"
    )));
    answer
}

pub fn join(_: &[Vec<u8>]) -> Result<Vec<u8>> {
    anyhow::bail!("this system has no web view for the app's windows")
}
