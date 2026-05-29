use chrono::{DateTime, FixedOffset, Utc};

/// JST = UTC+9 (no DST).
pub fn jst_offset() -> FixedOffset {
    FixedOffset::east_opt(9 * 3_600).expect("JST offset is valid")
}

/// Convert a UTC instant to JST for display.
pub fn to_jst(dt: &DateTime<Utc>) -> DateTime<FixedOffset> {
    dt.with_timezone(&jst_offset())
}

/// Convert epoch milliseconds to JST.
pub fn jst_from_millis(ms: i64) -> Option<DateTime<FixedOffset>> {
    DateTime::<Utc>::from_timestamp_millis(ms).map(|dt| dt.with_timezone(&jst_offset()))
}
