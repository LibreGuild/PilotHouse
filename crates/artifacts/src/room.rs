//! Who has an artifact's page open right now, and what reaches them live:
//! each open page holds one stream (`/_artifact/events`), and everything
//! the app tells it goes through that page's [`Outbox`]: which publish is
//! live, who else is here, what the others say, what changed in the shared
//! storage.
//!
//! The app only relays. A message is passed on to the pages open at that
//! moment and kept nowhere; what must outlast the moment goes into the
//! shared storage. So a page's game works between two tabs on this computer
//! as it does between colleagues, and there is no server code to write.
//!
//! A person is told apart by an id their browser keeps (`who`). It names,
//! it does not prove: anyone could send another's id, which is fine for a
//! game among colleagues and nothing to hang a secret on.

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use serde_json::{Value, json};

/// Pages open at once on one artifact. Beyond a team at a table, a page
/// wants a server of its own.
pub const MAX_OPEN: usize = 100;
/// What may wait for one page. A page that reads slower than the others
/// write is let go (its browser connects again) rather than fill memory.
const MAX_WAITING: usize = 512;
/// One message's data, as JSON text.
pub const MAX_MESSAGE: usize = 64 * 1024;
const MAX_NAME: usize = 40;
const MAX_WHO: usize = 40;

/// What waits to be written to one open page.
#[derive(Default)]
pub struct Outbox {
    waiting: Mutex<VecDeque<String>>,
    ready: Condvar,
    closed: AtomicBool,
}

/// What a page's stream does next.
pub enum Next {
    Send(String),
    /// Nothing came in the time waited.
    Quiet,
    Closed,
}

impl Outbox {
    /// Waits up to `wait` for something to send.
    pub fn next(&self, wait: Duration) -> Next {
        let waiting = self.waiting.lock().unwrap_or_else(PoisonError::into_inner);
        let (mut waiting, _) = self
            .ready
            .wait_timeout_while(waiting, wait, |waiting| {
                waiting.is_empty() && !self.closed.load(Ordering::SeqCst)
            })
            .unwrap_or_else(PoisonError::into_inner);
        match waiting.pop_front() {
            Some(event) => Next::Send(event),
            None if self.closed.load(Ordering::SeqCst) => Next::Closed,
            None => Next::Quiet,
        }
    }

    fn push(&self, event: &str) {
        let mut waiting = self.waiting.lock().unwrap_or_else(PoisonError::into_inner);
        if waiting.len() >= MAX_WAITING {
            drop(waiting);
            return self.close();
        }
        waiting.push_back(event.to_string());
        self.ready.notify_all();
    }

    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.ready.notify_all();
    }
}

/// One line of the stream: an event's name and its data (JSON, on one line).
fn event(name: &str, data: &Value) -> String {
    format!("event: {name}\ndata: {data}\n\n")
}

/// Someone with the page open.
#[derive(Debug, Clone, PartialEq)]
pub struct Person {
    /// The id their browser keeps for this artifact.
    pub who: String,
    /// What they call themselves; empty until they say.
    pub name: String,
    /// On the computer that keeps the artifact.
    pub host: bool,
}

impl Person {
    /// `who` and `name` as a page gave them, cut to what a list shows.
    pub fn new(who: &str, name: &str, host: bool) -> Option<Self> {
        let known = !who.is_empty()
            && who.len() <= MAX_WHO
            && who
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_');
        known.then(|| Self {
            who: who.to_string(),
            name: clean_name(name),
            host,
        })
    }

    fn json(&self) -> Value {
        json!({ "id": self.who, "name": self.name, "host": self.host })
    }
}

fn clean_name(name: &str) -> String {
    name.chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .chars()
        .take(MAX_NAME)
        .collect()
}

struct Open {
    person: Person,
    /// Through the shared link, not on this computer.
    guest: bool,
    outbox: Arc<Outbox>,
}

#[derive(Default)]
struct Pages {
    last: u64,
    /// By the number each got when it connected: in the order they came.
    open: BTreeMap<u64, Open>,
}

impl Pages {
    /// Everyone here, once each (a person with two tabs is one person), in
    /// the order they came.
    fn people(&self) -> Vec<Person> {
        let mut people: Vec<Person> = Vec::new();
        for page in self.open.values() {
            match people.iter_mut().find(|known| known.who == page.person.who) {
                // Named in one tab is named.
                Some(known) => {
                    if known.name.is_empty() {
                        known.name = page.person.name.clone();
                    }
                }
                None => people.push(page.person.clone()),
            }
        }
        people
    }

    fn tell_people(&self) {
        let people: Vec<Value> = self.people().iter().map(Person::json).collect();
        let text = event("people", &json!(people));
        for page in self.open.values() {
            page.outbox.push(&text);
        }
    }
}

/// The pages open on one artifact.
#[derive(Default)]
pub struct Room {
    pages: Mutex<Pages>,
}

impl Room {
    fn pages(&self) -> std::sync::MutexGuard<'_, Pages> {
        self.pages.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A page connected. It is told who it is and who is here (`hello`),
    /// the others that it came. `None` when the room is full. `change`:
    /// whether it may send and write what is shared.
    pub fn join(&self, person: Person, guest: bool, change: bool) -> Option<(u64, Arc<Outbox>)> {
        let mut pages = self.pages();
        if pages.open.len() >= MAX_OPEN {
            return None;
        }
        pages.last += 1;
        let number = pages.last;
        let outbox = Arc::new(Outbox::default());
        pages.open.insert(
            number,
            Open {
                person: person.clone(),
                guest,
                outbox: outbox.clone(),
            },
        );
        let people: Vec<Value> = pages.people().iter().map(Person::json).collect();
        outbox.push(&event(
            "hello",
            &json!({ "page": number, "you": person.json(), "people": people, "change": change }),
        ));
        let others = event("people", &json!(people));
        for (other, page) in &pages.open {
            if *other != number {
                page.outbox.push(&others);
            }
        }
        Some((number, outbox))
    }

    /// A page went away.
    pub fn leave(&self, number: u64) {
        let mut pages = self.pages();
        if let Some(page) = pages.open.remove(&number) {
            page.outbox.close();
            pages.tell_people();
        }
    }

    /// The person of page `number` calls themselves `name` from now on.
    pub fn rename(&self, number: u64, name: &str) -> bool {
        let mut pages = self.pages();
        let Some(who) = pages.open.get(&number).map(|page| page.person.who.clone()) else {
            return false;
        };
        let name = clean_name(name);
        // In every tab of theirs.
        for page in pages.open.values_mut() {
            if page.person.who == who {
                page.person.name = name.clone();
            }
        }
        pages.tell_people();
        true
    }

    /// Passes `data` from page `number` on: to everyone else here, or to
    /// the person `to` alone. Says whether `number` is a page that is here.
    pub fn say(&self, number: u64, to: Option<&str>, data: &Value) -> bool {
        let pages = self.pages();
        let Some(from) = pages.open.get(&number) else {
            return false;
        };
        let text = event("message", &json!({ "from": from.person.who, "data": data }));
        for (other, page) in &pages.open {
            let meant = match to {
                Some(to) => page.person.who == to,
                None => true,
            };
            if *other != number && meant {
                page.outbox.push(&text);
            }
        }
        true
    }

    /// Tells every open page (but `except`, the one that caused it).
    pub fn announce(&self, name: &str, data: &Value, except: Option<u64>) {
        let text = event(name, data);
        for (number, page) in &self.pages().open {
            if Some(*number) != except {
                page.outbox.push(&text);
            }
        }
    }

    /// Whether the pages that came through the shared link may send and
    /// write from now on.
    pub fn guests_may_change(&self, change: bool) {
        let text = event("access", &json!({ "change": change }));
        for page in self.pages().open.values().filter(|page| page.guest) {
            page.outbox.push(&text);
        }
    }

    /// Sharing ended: the pages that came through the link are let go.
    pub fn close_guests(&self) {
        let mut pages = self.pages();
        let before = pages.open.len();
        pages.open.retain(|_, page| {
            if page.guest {
                page.outbox.close();
            }
            !page.guest
        });
        if pages.open.len() != before {
            pages.tell_people();
        }
    }

    /// Everyone is let go (the artifact is no longer served).
    pub fn close(&self) {
        let mut pages = self.pages();
        for page in pages.open.values() {
            page.outbox.close();
        }
        pages.open.clear();
    }

    /// How many people are here, and how many of them through the link.
    pub fn here(&self) -> (usize, usize) {
        let pages = self.pages();
        let people = pages.people();
        let guests = people.iter().filter(|person| !person.host).count();
        (people.len(), guests)
    }

    /// Whether page `number` is here, and came through the link.
    pub fn is_guest(&self, number: u64) -> Option<bool> {
        self.pages().open.get(&number).map(|page| page.guest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn person(who: &str, host: bool) -> Person {
        Person::new(who, "", host).unwrap()
    }

    /// Everything waiting for a page, without waiting for more.
    fn read(outbox: &Outbox) -> Vec<String> {
        let mut events = Vec::new();
        while let Next::Send(event) = outbox.next(Duration::ZERO) {
            events.push(event);
        }
        events
    }

    #[test]
    fn people_come_talk_and_go() {
        let room = Room::default();
        let (maya, to_maya) = room.join(person("maya", true), false, true).unwrap();
        let hello = read(&to_maya);
        assert_eq!(hello.len(), 1);
        assert!(hello[0].starts_with("event: hello\ndata: "), "{hello:?}");
        assert!(hello[0].contains(r#""id":"maya""#) && hello[0].contains(r#""change":true"#));

        let (chen, to_chen) = room.join(person("chen", false), true, false).unwrap();
        // A second tab of Maya's: still two people.
        let (tab, to_tab) = room.join(person("maya", true), false, true).unwrap();
        assert_eq!(read(&to_tab).len(), 1, "its own hello");
        assert_eq!(room.here(), (2, 1));
        assert!(read(&to_chen)[0].contains(r#""change":false"#));
        let news = read(&to_maya);
        assert!(
            news.iter().all(|event| event.starts_with("event: people")),
            "{news:?}"
        );

        // To everyone else: Chen and Maya's other tab, not the sender.
        assert!(room.say(maya, None, &json!({ "move": 3 })));
        assert!(read(&to_maya).is_empty());
        let heard = read(&to_chen);
        let said = heard
            .last()
            .and_then(|event| event.strip_prefix("event: message\ndata: "))
            .expect("a message");
        assert_eq!(
            serde_json::from_str::<Value>(said).unwrap(),
            json!({ "from": "maya", "data": { "move": 3 } })
        );
        assert_eq!(read(&to_tab).len(), 1);
        // To one person: every tab of theirs.
        assert!(room.say(chen, Some("maya"), &json!("hi")));
        assert_eq!(read(&to_maya).len(), 1);
        assert_eq!(read(&to_tab).len(), 1);
        assert!(
            !room.say(99, None, &json!(1)),
            "a page that is not here says nothing"
        );

        assert!(room.rename(tab, "  Maya\n R. "));
        assert!(read(&to_chen)[0].contains(r#""name":"Maya R.""#));

        // Sharing ends: the guest is let go, and the others hear of it.
        room.close_guests();
        assert!(matches!(to_chen.next(Duration::ZERO), Next::Closed));
        assert_eq!(room.is_guest(chen), None);
        assert_eq!(room.here(), (1, 0));
        room.leave(tab);
        room.leave(maya);
        assert_eq!(room.here(), (0, 0));
    }

    #[test]
    fn a_page_that_does_not_read_is_let_go() {
        let room = Room::default();
        let (fast, _) = room.join(person("fast", true), false, true).unwrap();
        let (_, slow) = room.join(person("slow", false), true, true).unwrap();
        for n in 0..MAX_WAITING + 2 {
            room.say(fast, None, &json!(n));
        }
        // What waited is still read; then the stream ends.
        while let Next::Send(_) = slow.next(Duration::ZERO) {}
        assert!(matches!(slow.next(Duration::ZERO), Next::Closed));
    }

    #[test]
    fn ids_and_names_are_cut_to_what_is_safe_to_show() {
        assert!(Person::new("p-1_a", "x", false).is_some());
        assert!(Person::new("", "x", false).is_none());
        assert!(Person::new("a b", "x", false).is_none());
        assert!(Person::new(&"a".repeat(41), "x", false).is_none());
        let long = Person::new("p", &"n".repeat(90), false).unwrap();
        assert_eq!(long.name.chars().count(), MAX_NAME);
        let room = Room::default();
        for n in 0..MAX_OPEN {
            assert!(
                room.join(person(&format!("p{n}"), false), true, true)
                    .is_some()
            );
        }
        assert!(
            room.join(person("late", false), true, true).is_none(),
            "full"
        );
    }
}
