//! A deck as the running app holds it: the text of its files, every change
//! to them as a step that can be undone, and what its open page last said
//! (what is selected, what does not fit).
//!
//! Two hands change a deck. The page changes it through [`Live::apply`]
//! (the user typed, moved, added a slide). An agent changes the files
//! themselves, with the tools it has for any file; [`Live::scan`] reads
//! them again and takes what differs as a step too. Either way the files
//! on disk are the deck, and both hands' changes are undone the same way.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use serde_json::Value;

use crate::deck::{self, DECK, SLIDES, STYLE};

/// Steps kept to undo. Text only, a few kilobytes each.
const STEPS: usize = 300;
/// The page saves while the user types: saves of the same thing this close
/// together are one step.
const TYPING: Duration = Duration::from_secs(2);
/// An agent writes a slide after a slide: files changed outside this close
/// together are one step.
const BURST: Duration = Duration::from_secs(6);
/// A slide's file larger than this is not read: it is no slide.
const MAX_FILE: u64 = 4 * 1024 * 1024;
/// What the user changed by hand, kept to tell the agent with their next
/// message.
const JOURNAL: usize = 24;

/// Who made a change.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum By {
    /// The deck's page: the user's own hand.
    Page,
    /// The files changed on disk: an agent, or the user in another program.
    Outside,
}

/// One file's change: its new text, or `None` when it is removed.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    pub path: String,
    pub text: Option<String>,
}

/// What a change came to: the deck's version after it, and the files that
/// now read differently.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Applied {
    pub version: u64,
    pub files: Vec<String>,
}

/// Why the page's change was not taken.
#[derive(Debug)]
pub enum Refused {
    /// These files changed since the page read them (an agent wrote them):
    /// the page reads them again and tries once more.
    Stale(Vec<String>),
    Failed(anyhow::Error),
}

/// One file of a step: its path, its text before, its text after.
type Changed = (String, Option<Arc<str>>, Option<Arc<str>>);

struct Step {
    changes: Vec<Changed>,
    by: By,
    label: String,
    at: Instant,
}

#[derive(Default)]
struct State {
    files: BTreeMap<String, Arc<str>>,
    version: u64,
    undo: Vec<Step>,
    redo: Vec<Step>,
    selection: Option<Value>,
    report: Option<Value>,
    journal: Vec<String>,
}

/// The deck's files as they read now.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub version: u64,
    /// Path and text, by path.
    pub files: Vec<(String, Arc<str>)>,
    pub can_undo: bool,
    pub can_redo: bool,
}

pub struct Live {
    pub id: String,
    site: PathBuf,
    state: Mutex<State>,
}

impl Live {
    /// The deck in `site`, as its files read now.
    pub fn open(id: &str, site: &Path) -> Self {
        Self {
            id: id.to_string(),
            site: site.to_path_buf(),
            state: Mutex::new(State {
                files: read_files(site),
                version: 1,
                ..State::default()
            }),
        }
    }

    pub fn site(&self) -> &Path {
        &self.site
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn snapshot(&self) -> Snapshot {
        let state = self.state();
        Snapshot {
            version: state.version,
            files: state
                .files
                .iter()
                .map(|(path, text)| (path.clone(), text.clone()))
                .collect(),
            can_undo: !state.undo.is_empty(),
            can_redo: !state.redo.is_empty(),
        }
    }

    pub fn version(&self) -> u64 {
        self.state().version
    }

    /// One file's text.
    pub fn text(&self, path: &str) -> Option<Arc<str>> {
        self.state().files.get(path).cloned()
    }

    /// The deck as `deck.json` says, or an empty one when it cannot be read.
    pub fn deck(&self) -> deck::Deck {
        self.text(DECK)
            .and_then(|text| deck::Deck::parse(&text).ok())
            .unwrap_or_default()
    }

    /// The page's change. `base` are the marks of the files as the page
    /// last read them: a file that reads differently now is not written
    /// over (`Refused::Stale`). `label` says what the user did, for the
    /// agent to be told ("Retitled slide 3"); empty for what needs no
    /// telling.
    pub fn apply(
        &self,
        changes: &[Change],
        base: &[(String, String)],
        label: &str,
    ) -> Result<Applied, Refused> {
        let mut state = self.state();
        for change in changes {
            if !deck::tracked(&change.path) {
                return Err(Refused::Failed(anyhow::anyhow!(
                    "{} is not a file of the deck's that the page writes",
                    change.path
                )));
            }
        }
        let stale: Vec<String> = base
            .iter()
            .filter(|(path, mark)| {
                let now = state.files.get(path).map(|text| deck::mark(text));
                now.as_deref().unwrap_or("") != mark
            })
            .map(|(path, _)| path.clone())
            .collect();
        if !stale.is_empty() {
            return Err(Refused::Stale(stale));
        }
        let mut done = Vec::new();
        for change in changes {
            let before = state.files.get(&change.path).cloned();
            if before.as_deref() == change.text.as_deref() {
                continue;
            }
            if let Err(err) = write(&self.site, &change.path, change.text.as_deref()) {
                // What was written before it stays written, and counted.
                state.took(done, By::Page, label);
                return Err(Refused::Failed(err));
            }
            let after: Option<Arc<str>> = change.text.as_deref().map(Arc::from);
            match &after {
                Some(text) => state.files.insert(change.path.clone(), text.clone()),
                None => state.files.remove(&change.path),
            };
            done.push((change.path.clone(), before, after));
        }
        Ok(state.took(done, By::Page, label))
    }

    /// Reads the files again: what an agent (or anyone) changed on disk
    /// becomes a step. `None` when nothing reads differently.
    pub fn scan(&self) -> Option<Applied> {
        let on_disk = read_files(&self.site);
        let mut state = self.state();
        let mut done = Vec::new();
        for (path, text) in &on_disk {
            let before = state.files.get(path).cloned();
            if before.as_deref() != Some(&**text) {
                done.push((path.clone(), before, Some(text.clone())));
            }
        }
        for (path, text) in &state.files {
            if !on_disk.contains_key(path) {
                done.push((path.clone(), Some(text.clone()), None));
            }
        }
        if done.is_empty() {
            return None;
        }
        state.files = on_disk;
        Some(state.took(done, By::Outside, ""))
    }

    /// Takes back the last step, whoever made it.
    pub fn undo(&self) -> Result<Option<Applied>> {
        self.step_back(true)
    }

    pub fn redo(&self) -> Result<Option<Applied>> {
        self.step_back(false)
    }

    fn step_back(&self, undo: bool) -> Result<Option<Applied>> {
        let mut state = self.state();
        let step = if undo {
            state.undo.pop()
        } else {
            state.redo.pop()
        };
        let Some(step) = step else {
            return Ok(None);
        };
        let mut files = Vec::new();
        for (path, before, after) in &step.changes {
            let to = if undo { before } else { after };
            write(&self.site, path, to.as_deref())?;
            match to {
                Some(text) => state.files.insert(path.clone(), text.clone()),
                None => state.files.remove(path),
            };
            files.push(path.clone());
        }
        if undo {
            state.redo.push(step);
        } else {
            state.undo.push(step);
        }
        state.version += 1;
        Ok(Some(Applied {
            version: state.version,
            files,
        }))
    }

    /// What the page says is selected, for the agent to know what "this"
    /// means.
    pub fn set_selection(&self, selection: Option<Value>) {
        self.state().selection = selection;
    }

    pub fn selection(&self) -> Option<Value> {
        self.state().selection.clone()
    }

    /// What the page measured: what does not fit, on which slide.
    pub fn set_report(&self, report: Value) {
        self.state().report = Some(report);
    }

    pub fn report(&self) -> Option<Value> {
        self.state().report.clone()
    }

    /// What the user changed by hand since this was last asked.
    pub fn take_journal(&self) -> Vec<String> {
        std::mem::take(&mut self.state().journal)
    }
}

impl State {
    /// Counts `done` as a step, joined to the one before when it is more of
    /// the same.
    fn took(&mut self, done: Vec<Changed>, by: By, label: &str) -> Applied {
        if done.is_empty() {
            return Applied {
                version: self.version,
                files: Vec::new(),
            };
        }
        self.version += 1;
        self.redo.clear();
        let files: Vec<String> = done.iter().map(|(path, ..)| path.clone()).collect();
        if by == By::Page
            && !label.is_empty()
            && self.journal.last().map(String::as_str) != Some(label)
        {
            self.journal.push(label.to_string());
            if self.journal.len() > JOURNAL {
                self.journal.remove(0);
            }
        }
        let now = Instant::now();
        let joins = self.undo.last().is_some_and(|last| {
            last.by == by
                && match by {
                    // Typing on: the same thing changed again.
                    By::Page => {
                        last.label == label
                            && now.duration_since(last.at) < TYPING
                            && last.changes.len() == done.len()
                            && last
                                .changes
                                .iter()
                                .zip(&done)
                                .all(|((old, ..), (new, ..))| old == new)
                    }
                    By::Outside => now.duration_since(last.at) < BURST,
                }
        });
        if joins && let Some(last) = self.undo.last_mut() {
            for (path, before, after) in done {
                match last.changes.iter_mut().find(|(known, ..)| *known == path) {
                    // What it was before the first of them stays.
                    Some(change) => change.2 = after,
                    None => last.changes.push((path, before, after)),
                }
            }
            last.at = now;
        } else {
            self.undo.push(Step {
                changes: done,
                by,
                label: label.to_string(),
                at: now,
            });
            if self.undo.len() > STEPS {
                self.undo.remove(0);
            }
        }
        Applied {
            version: self.version,
            files,
        }
    }
}

/// The deck's text files in `site`, by path.
fn read_files(site: &Path) -> BTreeMap<String, Arc<str>> {
    let mut files = BTreeMap::new();
    let mut take = |path: String| {
        let file = site.join(&path);
        let small = file
            .metadata()
            .is_ok_and(|meta| meta.is_file() && meta.len() <= MAX_FILE);
        if small && let Ok(text) = std::fs::read_to_string(&file) {
            files.insert(path, Arc::from(text));
        }
    };
    take(DECK.to_string());
    take(STYLE.to_string());
    for entry in std::fs::read_dir(site.join(SLIDES))
        .into_iter()
        .flatten()
        .flatten()
    {
        let path = format!("{SLIDES}/{}", entry.file_name().to_string_lossy());
        if deck::tracked(&path) {
            take(path);
        }
    }
    files
}

fn write(site: &Path, path: &str, text: Option<&str>) -> Result<()> {
    let file = site.join(deck::inside(path)?);
    match text {
        Some(text) => {
            if let Some(folder) = file.parent() {
                std::fs::create_dir_all(folder)?;
            }
            ph_artifacts::library::write_atomic(&file, text.as_bytes())
        }
        None => match std::fs::remove_file(&file) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => bail!("could not remove {}: {err}", file.display()),
        },
    }
    .with_context(|| format!("could not change {path}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ph-slides-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(SLIDES)).unwrap();
        dir
    }

    fn change(path: &str, text: &str) -> Change {
        Change {
            path: path.into(),
            text: Some(text.into()),
        }
    }

    #[test]
    fn the_pages_changes_are_written_and_undone() {
        let site = scratch("apply");
        std::fs::write(site.join(DECK), "{}").unwrap();
        let live = Live::open("d", &site);
        assert_eq!(live.snapshot().files.len(), 1);

        let applied = live
            .apply(
                &[
                    change("slides/a.html", "<section>one</section>"),
                    change(DECK, "{\"x\":1}"),
                ],
                &[],
                "Added a slide",
            )
            .unwrap();
        assert_eq!(applied.files, ["slides/a.html", "deck.json"]);
        assert_eq!(
            std::fs::read_to_string(site.join("slides/a.html")).unwrap(),
            "<section>one</section>"
        );
        // Nothing new: no step, the same version.
        let same = live.apply(&[change(DECK, "{\"x\":1}")], &[], "").unwrap();
        assert_eq!((same.version, same.files.len()), (applied.version, 0));

        // Undone: the slide is gone again, and the deck reads as before.
        let undone = live.undo().unwrap().unwrap();
        assert_eq!(undone.files.len(), 2);
        assert!(!site.join("slides/a.html").exists());
        assert_eq!(std::fs::read_to_string(site.join(DECK)).unwrap(), "{}");
        assert!(live.snapshot().can_redo && !live.snapshot().can_undo);
        live.redo().unwrap().unwrap();
        assert!(site.join("slides/a.html").exists());
        assert_eq!(live.undo().unwrap().unwrap().files.len(), 2);
        assert!(live.undo().unwrap().is_none());
        assert_eq!(live.take_journal(), ["Added a slide"]);
        assert!(live.take_journal().is_empty());
        let _ = std::fs::remove_dir_all(&site);
    }

    #[test]
    fn a_file_an_agent_changed_is_not_written_over() {
        let site = scratch("stale");
        std::fs::write(site.join("slides/a.html"), "old").unwrap();
        let live = Live::open("d", &site);
        let read = deck::mark("old");
        // The agent writes; the app notices.
        std::fs::write(site.join("slides/a.html"), "agent").unwrap();
        let seen = live.scan().unwrap();
        assert_eq!(seen.files, ["slides/a.html"]);
        assert!(live.scan().is_none(), "nothing more to notice");
        // The page still holds the old text.
        let refused = live.apply(
            &[change("slides/a.html", "mine")],
            &[("slides/a.html".into(), read)],
            "",
        );
        assert!(matches!(refused, Err(Refused::Stale(files)) if files == ["slides/a.html"]));
        assert_eq!(&*live.text("slides/a.html").unwrap(), "agent");
        // Read again, it goes through; and the agent's step is undone like any.
        live.apply(
            &[change("slides/a.html", "mine")],
            &[("slides/a.html".into(), deck::mark("agent"))],
            "",
        )
        .unwrap();
        live.undo().unwrap();
        live.undo().unwrap();
        assert_eq!(
            std::fs::read_to_string(site.join("slides/a.html")).unwrap(),
            "old"
        );
        let _ = std::fs::remove_dir_all(&site);
    }

    #[test]
    fn typing_on_is_one_step_and_so_is_an_agents_burst() {
        let site = scratch("joins");
        let live = Live::open("d", &site);
        for text in ["a", "ab", "abc"] {
            live.apply(&[change("slides/a.html", text)], &[], "Typed on slide 1")
                .unwrap();
        }
        // Another thing: a step of its own.
        live.apply(&[change("slides/a.html", "abcd")], &[], "Moved the title")
            .unwrap();
        std::fs::write(site.join("slides/b.html"), "b").unwrap();
        live.scan().unwrap();
        std::fs::write(site.join("slides/c.html"), "c").unwrap();
        live.scan().unwrap();

        live.undo().unwrap(); // both of the agent's files
        assert!(!site.join("slides/b.html").exists() && !site.join("slides/c.html").exists());
        live.undo().unwrap(); // the move
        assert_eq!(&*live.text("slides/a.html").unwrap(), "abc");
        live.undo().unwrap(); // all of the typing
        assert!(live.text("slides/a.html").is_none());
        assert!(live.undo().unwrap().is_none());
        let _ = std::fs::remove_dir_all(&site);
    }

    #[test]
    fn only_the_decks_files_are_written() {
        let site = scratch("refuse");
        let live = Live::open("d", &site);
        for path in [
            "../x.html",
            "media/a.png",
            "slides/../deck.json",
            "artifact.json",
        ] {
            let refused = live.apply(&[change(path, "x")], &[], "");
            assert!(matches!(refused, Err(Refused::Failed(_))), "{path}");
        }
        assert!(live.snapshot().files.is_empty());
        let _ = std::fs::remove_dir_all(&site);
    }
}
