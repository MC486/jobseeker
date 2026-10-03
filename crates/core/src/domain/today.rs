//! What is worth doing today: overdue and upcoming next actions.
//!
//! Dates are `YYYY-MM-DD`. Comparison is lexical, which matches that spelling.

use chrono::{Duration, NaiveDate};

use crate::time::Timestamp;

/// FR-T-02: a next action due inside this window, including today, is "due soon".
pub const DUE_SOON_DAYS: i64 = 7;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DueWhen {
    /// No due date, or the date does not parse.
    None,
    Overdue,
    /// Due today through today + [`DUE_SOON_DAYS`].
    Soon,
    Later,
}

pub fn utc_day(now: Timestamp) -> String {
    now.format("%Y-%m-%d").to_string()
}

pub fn classify_due(due: Option<&str>, today: &str) -> DueWhen {
    let Some(due) = due.map(date_prefix).filter(|d| d.len() == 10) else {
        return DueWhen::None;
    };
    if due.as_str() < today {
        return DueWhen::Overdue;
    }
    let Ok(start) = NaiveDate::parse_from_str(today, "%Y-%m-%d") else {
        return DueWhen::None;
    };
    let end = (start + Duration::days(DUE_SOON_DAYS))
        .format("%Y-%m-%d")
        .to_string();
    if due.as_str() <= end.as_str() {
        DueWhen::Soon
    } else {
        DueWhen::Later
    }
}

fn date_prefix(raw: &str) -> String {
    let t = raw.trim();
    if t.len() >= 10 {
        t[..10].to_string()
    } else {
        t.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_overdue_soon_and_later() {
        let today = "2026-10-03";
        assert_eq!(classify_due(None, today), DueWhen::None);
        assert_eq!(classify_due(Some(""), today), DueWhen::None);
        assert_eq!(classify_due(Some("soon"), today), DueWhen::None);
        assert_eq!(classify_due(Some("2026-10-02"), today), DueWhen::Overdue);
        assert_eq!(classify_due(Some("2026-10-03"), today), DueWhen::Soon);
        assert_eq!(
            classify_due(Some("2026-10-10"), today),
            DueWhen::Soon,
            "the seventh day is still soon"
        );
        assert_eq!(classify_due(Some("2026-10-11"), today), DueWhen::Later);
        assert_eq!(
            classify_due(Some("2026-09-01T00:00:00Z"), today),
            DueWhen::Overdue
        );
    }
}
