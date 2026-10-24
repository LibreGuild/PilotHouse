//! Kanban cards and their activity log.
//!
//! Cards are ordered inside a column by `position`, a float with gaps, so a
//! card can move between two neighbours without renumbering the column.

use std::collections::HashMap;

use anyhow::{Result, bail};
use rusqlite::{OptionalExtension as _, Row, params};
use serde::{Deserialize, Serialize};

use crate::projects::ProjectId;
use crate::{Store, now_millis};

pub type CardId = i64;

/// The board's columns, left to right.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CardStatus {
    Todo,
    InProgress,
    Review,
    Done,
}

impl CardStatus {
    pub const ALL: [CardStatus; 4] = [
        CardStatus::Todo,
        CardStatus::InProgress,
        CardStatus::Review,
        CardStatus::Done,
    ];

    /// Stable identifier, used in the database and in MCP tool arguments.
    pub fn key(self) -> &'static str {
        match self {
            CardStatus::Todo => "todo",
            CardStatus::InProgress => "in_progress",
            CardStatus::Review => "review",
            CardStatus::Done => "done",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            CardStatus::Todo => "To do",
            CardStatus::InProgress => "In progress",
            CardStatus::Review => "Review",
            CardStatus::Done => "Done",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|status| status.key() == key)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Card {
    pub id: CardId,
    pub project_id: ProjectId,
    pub title: String,
    pub description: String,
    /// What "done" means for this card, one criterion per line.
    pub acceptance: String,
    pub status: CardStatus,
    pub position: f64,
    /// The agent (or `you`) currently working on the card.
    pub assignee: Option<String>,
    pub updated_at: i64,
}

/// Fields for a new card. Empty strings are fine for the optional text.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct NewCard {
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub acceptance: String,
}

/// A partial edit: only the fields that are `Some` change.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct CardPatch {
    pub title: Option<String>,
    pub description: Option<String>,
    pub acceptance: Option<String>,
    pub status: Option<CardStatus>,
    /// `Some(None)` clears the assignee.
    pub assignee: Option<Option<String>>,
}

/// One line in a card's activity log, written by the user or an agent.
#[derive(Debug, Clone, PartialEq)]
pub struct CardEvent {
    pub id: i64,
    pub card_id: CardId,
    pub author: String,
    pub text: String,
    pub created_at: i64,
}

impl Card {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        let status: String = row.get("status")?;
        Ok(Self {
            id: row.get("id")?,
            project_id: row.get("project_id")?,
            title: row.get("title")?,
            description: row.get("description")?,
            acceptance: row.get("acceptance")?,
            // Unknown statuses (from a newer build) land in To do rather than vanish.
            status: CardStatus::from_key(&status).unwrap_or(CardStatus::Todo),
            position: row.get("position")?,
            assignee: row.get("assignee")?,
            updated_at: row.get("updated_at")?,
        })
    }
}

impl Store {
    /// All cards of a project, ordered by column then position.
    pub fn cards(&self, project_id: ProjectId) -> Result<Vec<Card>> {
        let conn = self.conn();
        let mut statement =
            conn.prepare("SELECT * FROM cards WHERE project_id = ?1 ORDER BY position, id")?;
        let mut cards = statement
            .query_map(params![project_id], Card::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        cards.sort_by_key(|card| CardStatus::ALL.iter().position(|s| *s == card.status));
        Ok(cards)
    }

    pub fn card(&self, id: CardId) -> Result<Option<Card>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT * FROM cards WHERE id = ?1",
                params![id],
                Card::from_row,
            )
            .optional()?)
    }

    /// Appends cards to the bottom of the To do column, in the given order.
    pub fn add_cards(&self, project_id: ProjectId, cards: &[NewCard]) -> Result<Vec<Card>> {
        if cards.iter().any(|card| card.title.trim().is_empty()) {
            bail!("every card needs a title");
        }
        let now = now_millis();
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let mut position: f64 = tx.query_row(
            "SELECT COALESCE(MAX(position), 0) FROM cards WHERE project_id = ?1 AND status = 'todo'",
            params![project_id],
            |row| row.get(0),
        )?;
        let mut ids = Vec::with_capacity(cards.len());
        for card in cards {
            position += 1.0;
            tx.execute(
                "INSERT INTO cards (project_id, title, description, acceptance, status, position, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, 'todo', ?5, ?6, ?6)",
                params![project_id, card.title.trim(), card.description, card.acceptance, position, now],
            )?;
            ids.push(tx.last_insert_rowid());
        }
        tx.commit()?;
        drop(conn);
        ids.into_iter()
            .map(|id| {
                self.card(id)?
                    .ok_or_else(|| anyhow::anyhow!("card {id} vanished"))
            })
            .collect()
    }

    /// Applies a partial edit. Changing the status moves the card to the
    /// bottom of its new column.
    pub fn update_card(&self, id: CardId, patch: &CardPatch) -> Result<Card> {
        let Some(card) = self.card(id)? else {
            bail!("no card with id {id}");
        };
        let status = patch.status.unwrap_or(card.status);
        let position = if status != card.status {
            self.column_end(card.project_id, status)?
        } else {
            card.position
        };
        let assignee = match &patch.assignee {
            Some(assignee) => assignee.clone(),
            None => card.assignee.clone(),
        };
        self.conn().execute(
            "UPDATE cards SET title = ?2, description = ?3, acceptance = ?4, status = ?5,
                              position = ?6, assignee = ?7, updated_at = ?8
             WHERE id = ?1",
            params![
                id,
                patch.title.as_deref().unwrap_or(&card.title),
                patch.description.as_deref().unwrap_or(&card.description),
                patch.acceptance.as_deref().unwrap_or(&card.acceptance),
                status.key(),
                position,
                assignee,
                now_millis()
            ],
        )?;
        self.card(id)?
            .ok_or_else(|| anyhow::anyhow!("card {id} vanished"))
    }

    /// The UI tests `card` must pass to be Done (test file names), in order.
    pub fn card_tests(&self, card: CardId) -> Result<Vec<String>> {
        let conn = self.conn();
        let mut statement =
            conn.prepare("SELECT test FROM card_tests WHERE card_id = ?1 ORDER BY test")?;
        let tests = statement
            .query_map(params![card], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        Ok(tests)
    }

    /// How many tests each card of a project needs, for the board's tiles.
    pub fn card_test_counts(&self, project_id: ProjectId) -> Result<HashMap<CardId, usize>> {
        let conn = self.conn();
        let mut statement = conn.prepare(
            "SELECT card_tests.card_id, COUNT(*) FROM card_tests
             JOIN cards ON cards.id = card_tests.card_id
             WHERE cards.project_id = ?1 GROUP BY card_tests.card_id",
        )?;
        let counts = statement
            .query_map(params![project_id], |row| {
                Ok((row.get::<_, CardId>(0)?, row.get::<_, i64>(1)? as usize))
            })?
            .collect::<rusqlite::Result<HashMap<_, _>>>()?;
        Ok(counts)
    }

    /// Links (or with `linked: false`, unlinks) a test to a card.
    pub fn link_card_test(&self, card: CardId, test: &str, linked: bool) -> Result<()> {
        if linked {
            self.conn().execute(
                "INSERT OR IGNORE INTO card_tests (card_id, test) VALUES (?1, ?2)",
                params![card, test],
            )?;
        } else {
            self.conn().execute(
                "DELETE FROM card_tests WHERE card_id = ?1 AND test = ?2",
                params![card, test],
            )?;
        }
        Ok(())
    }

    /// Moves a card into `status`, placed before `before` (or at the end when
    /// `before` is `None`). Used by drag and drop.
    pub fn move_card(
        &self,
        id: CardId,
        status: CardStatus,
        before: Option<CardId>,
    ) -> Result<Card> {
        let Some(card) = self.card(id)? else {
            bail!("no card with id {id}");
        };
        let position = match before.and_then(|before| self.card(before).ok().flatten()) {
            Some(anchor) if anchor.status == status && anchor.id != id => {
                let previous: Option<f64> = self
                    .conn()
                    .query_row(
                        "SELECT MAX(position) FROM cards
                         WHERE project_id = ?1 AND status = ?2 AND position < ?3 AND id != ?4",
                        params![card.project_id, status.key(), anchor.position, id],
                        |row| row.get(0),
                    )
                    .optional()?
                    .flatten();
                match previous {
                    Some(previous) => (previous + anchor.position) / 2.0,
                    None => anchor.position - 1.0,
                }
            }
            _ => self.column_end(card.project_id, status)?,
        };
        self.conn().execute(
            "UPDATE cards SET status = ?2, position = ?3, updated_at = ?4 WHERE id = ?1",
            params![id, status.key(), position, now_millis()],
        )?;
        self.card(id)?
            .ok_or_else(|| anyhow::anyhow!("card {id} vanished"))
    }

    pub fn delete_card(&self, id: CardId) -> Result<()> {
        self.conn()
            .execute("DELETE FROM cards WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Claims the top To do card for `assignee`: moves it to In progress and
    /// returns it. `None` when the column is empty.
    pub fn claim_next_card(&self, project_id: ProjectId, assignee: &str) -> Result<Option<Card>> {
        let next: Option<CardId> = self
            .conn()
            .query_row(
                "SELECT id FROM cards WHERE project_id = ?1 AND status = 'todo'
                 ORDER BY position, id LIMIT 1",
                params![project_id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(id) = next else {
            return Ok(None);
        };
        let card = self.update_card(
            id,
            &CardPatch {
                status: Some(CardStatus::InProgress),
                assignee: Some(Some(assignee.to_string())),
                ..Default::default()
            },
        )?;
        self.add_card_event(id, assignee, "Started working on this card")?;
        Ok(Some(card))
    }

    pub fn add_card_event(&self, card_id: CardId, author: &str, text: &str) -> Result<CardEvent> {
        let now = now_millis();
        let conn = self.conn();
        conn.execute(
            "INSERT INTO card_events (card_id, author, text, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![card_id, author, text, now],
        )?;
        Ok(CardEvent {
            id: conn.last_insert_rowid(),
            card_id,
            author: author.to_string(),
            text: text.to_string(),
            created_at: now,
        })
    }

    pub fn card_events(&self, card_id: CardId) -> Result<Vec<CardEvent>> {
        let conn = self.conn();
        let mut statement = conn.prepare(
            "SELECT id, card_id, author, text, created_at FROM card_events
             WHERE card_id = ?1 ORDER BY id",
        )?;
        let events = statement
            .query_map(params![card_id], |row| {
                Ok(CardEvent {
                    id: row.get(0)?,
                    card_id: row.get(1)?,
                    author: row.get(2)?,
                    text: row.get(3)?,
                    created_at: row.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(events)
    }

    fn column_end(&self, project_id: ProjectId, status: CardStatus) -> Result<f64> {
        let max: f64 = self.conn().query_row(
            "SELECT COALESCE(MAX(position), 0) FROM cards WHERE project_id = ?1 AND status = ?2",
            params![project_id, status.key()],
            |row| row.get(0),
        )?;
        Ok(max + 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (Store, ProjectId) {
        let store = Store::open_in_memory().unwrap();
        let dir = std::env::temp_dir().join(format!("ph-board-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let project = store.add_project(&dir).unwrap();
        (store, project.id)
    }

    fn new(title: &str) -> NewCard {
        NewCard {
            title: title.into(),
            ..Default::default()
        }
    }

    fn titles(store: &Store, project: ProjectId, status: CardStatus) -> Vec<String> {
        store
            .cards(project)
            .unwrap()
            .into_iter()
            .filter(|card| card.status == status)
            .map(|card| card.title)
            .collect()
    }

    #[test]
    fn cards_link_the_tests_they_need() {
        let (store, project) = setup();
        let card = store
            .add_cards(project, &[new("Sign in")])
            .unwrap()
            .remove(0);
        store
            .link_card_test(card.id, "sign-in-works", true)
            .unwrap();
        store
            .link_card_test(card.id, "sign-in-works", true)
            .unwrap();
        store.link_card_test(card.id, "a-smoke-test", true).unwrap();
        assert_eq!(
            store.card_tests(card.id).unwrap(),
            ["a-smoke-test", "sign-in-works"]
        );
        assert_eq!(store.card_test_counts(project).unwrap()[&card.id], 2);
        store
            .link_card_test(card.id, "a-smoke-test", false)
            .unwrap();
        assert_eq!(store.card_tests(card.id).unwrap(), ["sign-in-works"]);
    }

    #[test]
    fn agents_claim_cards_top_down() {
        let (store, project) = setup();
        store.add_cards(project, &[new("A"), new("B")]).unwrap();

        let first = store.claim_next_card(project, "claude").unwrap().unwrap();
        assert_eq!(first.title, "A");
        assert_eq!(first.status, CardStatus::InProgress);
        assert_eq!(first.assignee.as_deref(), Some("claude"));
        assert_eq!(store.card_events(first.id).unwrap().len(), 1);

        assert_eq!(
            store
                .claim_next_card(project, "claude")
                .unwrap()
                .unwrap()
                .title,
            "B"
        );
        assert!(store.claim_next_card(project, "claude").unwrap().is_none());
    }

    #[test]
    fn moving_before_a_card_inserts_between_neighbours() {
        let (store, project) = setup();
        let cards = store
            .add_cards(project, &[new("A"), new("B"), new("C")])
            .unwrap();
        store
            .move_card(cards[2].id, CardStatus::Todo, Some(cards[1].id))
            .unwrap();
        assert_eq!(titles(&store, project, CardStatus::Todo), ["A", "C", "B"]);

        store
            .move_card(cards[0].id, CardStatus::Done, None)
            .unwrap();
        assert_eq!(titles(&store, project, CardStatus::Done), ["A"]);
        assert_eq!(titles(&store, project, CardStatus::Todo), ["C", "B"]);
    }

    #[test]
    fn cards_without_titles_are_rejected() {
        let (store, project) = setup();
        assert!(store.add_cards(project, &[new("  ")]).is_err());
        assert!(store.cards(project).unwrap().is_empty());
    }
}
