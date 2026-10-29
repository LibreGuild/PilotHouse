//! Hands freed memory back to the system.
//!
//! The allocator keeps memory the app has freed, to reuse it, so after a
//! busy spell (a long agent run, a large file) the app kept its peak size
//! although it used far less. Every few minutes PilotHouse asks the
//! allocator to return its free pages, and once each time the app has been
//! in the background a while, Windows is also asked to trim the pages it
//! counts as in use.

use std::time::Duration;

use gpui_kit::App;

/// How often the allocator is asked to let go of free memory.
const EVERY: Duration = Duration::from_secs(120);
/// Time in the background before the deeper trim.
const AWAY: Duration = Duration::from_secs(120);
const TICK: Duration = Duration::from_secs(30);

pub fn start(cx: &mut App) {
    cx.spawn(async move |cx| {
        let (mut since_trim, mut away) = (Duration::ZERO, Duration::ZERO);
        loop {
            cx.background_executor().timer(TICK).await;
            since_trim += TICK;
            let active = cx.update(|cx| cx.active_window().is_some());
            away = if active { Duration::ZERO } else { away + TICK };
            // The deep trim runs once per spell in the background.
            let deep = away == AWAY;
            if since_trim >= EVERY || deep {
                since_trim = Duration::ZERO;
                cx.background_executor()
                    .spawn(async move { release_free_memory(deep) })
                    .await;
            }
        }
    })
    .detach();
}

/// Returns free heap pages to the system; `deep` also trims the working
/// set (Windows), which pages back in as needed.
#[cfg(target_os = "macos")]
fn release_free_memory(_deep: bool) {
    unsafe extern "C" {
        fn malloc_zone_pressure_relief(zone: *mut std::ffi::c_void, goal: usize) -> usize;
    }
    // SAFETY: a null zone means every zone, and goal 0 as much as possible;
    // the zones lock themselves.
    let released = unsafe { malloc_zone_pressure_relief(std::ptr::null_mut(), 0) };
    tracing::debug!("returned {} KB of free memory", released / 1024);
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn release_free_memory(_deep: bool) {
    unsafe extern "C" {
        fn malloc_trim(pad: usize) -> i32;
    }
    // SAFETY: glibc's own call; it locks the arenas it trims.
    unsafe { malloc_trim(0) };
}

#[cfg(windows)]
fn release_free_memory(deep: bool) {
    use windows_sys::Win32::System::Memory::{GetProcessHeap, HeapCompact};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, SetProcessWorkingSetSize};
    // SAFETY: the process heap and the current-process pseudo handle are
    // always valid; both calls only release memory.
    unsafe {
        HeapCompact(GetProcessHeap(), 0);
        if deep {
            SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
        }
    }
}

#[cfg(not(any(
    target_os = "macos",
    all(target_os = "linux", target_env = "gnu"),
    windows
)))]
fn release_free_memory(_deep: bool) {}
