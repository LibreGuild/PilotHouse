//! The long-lived `adb shell` a phone's input goes through, so each tap or
//! key costs no process start.
//!
//! The shell runs one command at a time, and `input` is slow on a phone
//! (each starts a small Java program: a few hundred milliseconds on a slow
//! one). A trackpad sends dozens of scroll events a second, so sending
//! each as a swipe would queue up seconds of swipes that keep playing out
//! after the hand stopped, the phone's screen jumping on its own. So every
//! command ends with a marker the shell echoes back, the shell knows what
//! is still running, and scrolling is gathered into one swipe sent only
//! when the shell is free: whatever scrolled meanwhile goes as the next
//! one, the moment the last ends.

use std::io::{BufRead as _, BufReader, Write as _};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};

use crate::adb::Adb;

/// Echoed after each command, so its end can be told.
const DONE: &str = "ph-input-done";
/// Scroll events further apart than this are a new gesture.
const SCROLL_GAP: Duration = Duration::from_millis(300);
/// The shortest swipe sent (device pixels): shorter ones would land as a
/// tap, on whatever is under the pointer.
const MIN_SWIPE: f32 = 48.0;

pub(crate) struct InputShell {
    child: Child,
    shared: Arc<Mutex<Shared>>,
}

struct Shared {
    stdin: ChildStdin,
    /// Commands written whose end has not come back.
    running: usize,
    /// Scrolling not sent yet.
    scroll: Option<Scroll>,
}

#[derive(Debug, Clone, Copy)]
struct Scroll {
    /// Where the gesture started (device pixels).
    from: (i32, i32),
    /// How far the finger is to move up (device pixels; negative: down).
    dy: f32,
    display: (u32, u32),
    last: Instant,
}

impl InputShell {
    pub(crate) fn start(adb: &Adb, serial: &str) -> Result<Self> {
        let mut child = adb
            .command(Some(serial))
            .arg("shell")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("adb shell")?;
        let stdin = child.stdin.take().context("no stdin")?;
        let stdout = child.stdout.take().context("no stdout")?;
        let shared = Arc::new(Mutex::new(Shared {
            stdin,
            running: 0,
            scroll: None,
        }));
        let reader = shared.clone();
        std::thread::Builder::new()
            .name("adb-input".into())
            .spawn(move || {
                // Ends when the shell does.
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    if line.trim() != DONE {
                        continue;
                    }
                    let Ok(mut shared) = reader.lock() else { break };
                    shared.running = shared.running.saturating_sub(1);
                    if shared.running == 0 {
                        let _ = shared.send_scroll();
                    }
                }
            })
            .context("input thread")?;
        Ok(Self { child, shared })
    }

    /// Runs `command` after those before it.
    pub(crate) fn send(&self, command: &str) -> Result<()> {
        let mut shared = self
            .shared
            .lock()
            .map_err(|_| anyhow::anyhow!("poisoned"))?;
        shared.write(command)
    }

    /// Scrolls by `dy` device pixels (the finger moving up when positive)
    /// from `at`, gathered with the rest of the gesture.
    pub(crate) fn scroll(&self, at: (i32, i32), dy: f32, display: (u32, u32)) -> Result<()> {
        let mut shared = self
            .shared
            .lock()
            .map_err(|_| anyhow::anyhow!("poisoned"))?;
        let now = Instant::now();
        let scroll = match shared.scroll {
            Some(scroll) if now.duration_since(scroll.last) < SCROLL_GAP => Scroll {
                dy: scroll.dy + dy,
                last: now,
                ..scroll
            },
            // A new gesture; what was left of the last one is dropped.
            _ => Scroll {
                from: at,
                dy,
                display,
                last: now,
            },
        };
        shared.scroll = Some(scroll);
        if shared.running == 0 {
            shared.send_scroll()?;
        }
        Ok(())
    }
}

impl Shared {
    fn write(&mut self, command: &str) -> Result<()> {
        writeln!(self.stdin, "{command}; echo {DONE}")?;
        self.stdin.flush()?;
        self.running += 1;
        Ok(())
    }

    /// Sends the scrolling gathered, once it is a swipe's worth.
    fn send_scroll(&mut self) -> Result<()> {
        let Some(scroll) = self.scroll else {
            return Ok(());
        };
        let Some(command) = swipe_command(scroll.from, scroll.dy, scroll.display) else {
            return Ok(());
        };
        self.scroll = Some(Scroll { dy: 0.0, ..scroll });
        self.write(&command)
    }
}

impl Drop for InputShell {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// One swipe for a scroll of `dy` from `from`, or none when too short. It
/// starts away from the screen's edges, where a swipe is a system gesture
/// (home, back, the notification shade) rather than a scroll, and stays
/// on the screen.
fn swipe_command(from: (i32, i32), dy: f32, (width, height): (u32, u32)) -> Option<String> {
    if dy.abs() < MIN_SWIPE || width == 0 || height == 0 {
        return None;
    }
    let (w, h) = (width as f32, height as f32);
    let x = (from.0 as f32).clamp(w * 0.1, w * 0.9);
    let y = (from.1 as f32).clamp(h * 0.2, h * 0.8);
    let to = (y - dy).clamp(h * 0.05, h * 0.95);
    let distance = (y - to).abs();
    if distance < MIN_SWIPE {
        return None;
    }
    // Slow enough to scroll as far as the fingers went, not fling past.
    let ms = (distance * 0.6).clamp(120.0, 450.0) as u32;
    Some(format!(
        "input swipe {} {} {} {} {ms}",
        x as i32, y as i32, x as i32, to as i32
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A trackpad scroll: 60 small events over a second, on a real phone or
    /// emulator (`PH_TEST_SERIAL`, `PH_TEST_ADB`). The shell must be free
    /// soon after the hand stops, not seconds later.
    #[test]
    #[ignore = "needs a device: PH_TEST_SERIAL and PH_TEST_ADB"]
    fn a_trackpad_scroll_does_not_pile_up() {
        let serial = std::env::var("PH_TEST_SERIAL").unwrap();
        let adb = Adb::new(std::env::var("PH_TEST_ADB").unwrap().into());
        let shell = InputShell::start(&adb, &serial).unwrap();
        for _ in 0..60 {
            shell.scroll((540, 1400), 12.0, (1080, 2400)).unwrap();
            std::thread::sleep(Duration::from_millis(16));
        }
        let stopped = Instant::now();
        while shell.shared.lock().unwrap().running > 0 {
            assert!(
                stopped.elapsed() < Duration::from_secs(2),
                "swipes piled up"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        eprintln!("free {:?} after the last event", stopped.elapsed());
    }

    const DISPLAY: (u32, u32) = (1000, 2000);

    #[test]
    fn small_scrolls_are_not_taps() {
        assert_eq!(swipe_command((500, 1000), 10.0, DISPLAY), None);
        assert_eq!(swipe_command((500, 1000), -47.0, DISPLAY), None);
    }

    #[test]
    fn swipes_keep_clear_of_the_edges() {
        // From the very bottom, scrolling up: not the home gesture.
        let up = swipe_command((500, 1990), 600.0, DISPLAY).unwrap();
        assert_eq!(up, "input swipe 500 1600 500 1000 360");
        // From the top, scrolling down: not the notification shade, and
        // it ends on the screen.
        let down = swipe_command((5, 0), -5000.0, DISPLAY).unwrap();
        assert_eq!(down, "input swipe 100 400 100 1900 450");
    }
}
