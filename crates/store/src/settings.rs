//! App-wide preferences as typed JSON values.

use anyhow::Result;
use rusqlite::{OptionalExtension as _, params};
use serde::{Serialize, de::DeserializeOwned};

use crate::Store;

impl Store {
    /// Reads a setting, falling back to `T::default()` when it is missing or
    /// no longer parses (for example after a type changed between versions).
    pub fn setting<T: DeserializeOwned + Default>(&self, key: &str) -> T {
        let raw: Option<String> = self
            .conn()
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
            .ok()
            .flatten();
        raw.and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    pub fn set_setting<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        let raw = serde_json::to_string(value)?;
        self.conn().execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, raw],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_and_default() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.setting::<String>("appearance"), "");
        store.set_setting("appearance", &"dark").unwrap();
        assert_eq!(store.setting::<String>("appearance"), "dark");
        // Wrong type falls back to the default instead of failing.
        assert_eq!(store.setting::<u32>("appearance"), 0);
    }
}
