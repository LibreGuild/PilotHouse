use std::time::{SystemTime, UNIX_EPOCH};

/// Wall-clock time as Unix milliseconds, the timestamp format of every table.
pub fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or_default()
}
