// src/data/economic_calendar.rs
// Data types for the economic calendar widget.
//
// EconomicEvent stores per-event data from Finnhub's economic calendar API.
// EconomicCalendar accumulates events with full replacement on each fetch.
// Impact enum handles both text ("high") and numeric ("3") format strings.

use chrono::{Datelike, NaiveDateTime};
use serde::Deserialize;

/// Impact level for an economic event.
///
/// Handles both text ("high", "medium", "low") and numeric ("3", "2", "1")
/// formats from Finnhub API responses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Impact {
    Unknown,
    Low,
    Medium,
    High,
}

impl Impact {
    /// Parse impact from a string, handling both text and numeric formats.
    pub fn from_str(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "high" | "3" => Impact::High,
            "medium" | "2" => Impact::Medium,
            "low" | "1" => Impact::Low,
            _ => Impact::Unknown,
        }
    }

    /// Display label for the impact level.
    pub fn label(self) -> &'static str {
        match self {
            Impact::High => "HIGH",
            Impact::Medium => "MED",
            Impact::Low => "LOW",
            Impact::Unknown => "---",
        }
    }
}

/// Raw Finnhub API response wrapper.
#[derive(Debug, Deserialize)]
pub struct EconomicCalendarResponse {
    #[serde(rename = "economicCalendar")]
    pub economic_calendar: Option<Vec<RawEconomicEvent>>,
}

/// Raw event from the Finnhub API — all fields optional.
#[derive(Debug, Deserialize)]
pub struct RawEconomicEvent {
    pub actual: Option<f64>,
    pub country: Option<String>,
    pub estimate: Option<f64>,
    pub event: Option<String>,
    pub impact: Option<String>,
    pub prev: Option<f64>,
    pub time: Option<String>,
    pub unit: Option<String>,
}

/// Parsed economic event for display.
#[derive(Debug, Clone)]
pub struct EconomicEvent {
    pub time_raw: String,
    pub time_parsed: Option<NaiveDateTime>,
    pub country: String,
    pub event: String,
    pub impact: Impact,
    pub actual: Option<f64>,
    pub estimate: Option<f64>,
    pub prev: Option<f64>,
    pub unit: String,
}

impl EconomicEvent {
    /// Convert a raw Finnhub API event into a parsed EconomicEvent.
    pub fn from_raw(raw: RawEconomicEvent) -> Self {
        let time_raw = raw.time.clone().unwrap_or_default();
        let time_parsed = parse_time(&time_raw);

        let impact = raw
            .impact
            .as_deref()
            .map(Impact::from_str)
            .unwrap_or(Impact::Unknown);

        Self {
            time_raw,
            time_parsed,
            country: raw.country.unwrap_or_default(),
            event: raw.event.unwrap_or_default(),
            impact,
            actual: raw.actual,
            estimate: raw.estimate,
            prev: raw.prev,
            unit: raw.unit.unwrap_or_default(),
        }
    }
}

/// Try multiple chrono format strings to parse a time string.
fn parse_time(s: &str) -> Option<NaiveDateTime> {
    if s.is_empty() {
        return None;
    }

    // Try full datetime formats
    if let Ok(dt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
        return Some(dt);
    }
    if let Ok(dt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%SZ") {
        return Some(dt);
    }
    // Date-only as midnight
    if let Ok(date) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Some(date.and_hms_opt(0, 0, 0).unwrap());
    }

    tracing::warn!(time = %s, "Failed to parse economic event time");
    None
}

/// Date range for calendar queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateRange {
    Today,
    ThisWeek,
    NextWeek,
}

impl DateRange {
    /// Compute (from, to) date strings in YYYY-MM-DD format.
    pub fn to_dates(self) -> (String, String) {
        let today = chrono::Utc::now().date_naive();
        match self {
            DateRange::Today => {
                let s = today.format("%Y-%m-%d").to_string();
                (s.clone(), s)
            }
            DateRange::ThisWeek => {
                let weekday = today.weekday().num_days_from_monday();
                let monday = today - chrono::Duration::days(weekday as i64);
                let friday = monday + chrono::Duration::days(4);
                (
                    monday.format("%Y-%m-%d").to_string(),
                    friday.format("%Y-%m-%d").to_string(),
                )
            }
            DateRange::NextWeek => {
                let weekday = today.weekday().num_days_from_monday();
                let next_monday = today + chrono::Duration::days((7 - weekday) as i64);
                let next_friday = next_monday + chrono::Duration::days(4);
                (
                    next_monday.format("%Y-%m-%d").to_string(),
                    next_friday.format("%Y-%m-%d").to_string(),
                )
            }
        }
    }

    /// Cycle forward through date ranges.
    pub fn next(self) -> Self {
        match self {
            DateRange::Today => DateRange::ThisWeek,
            DateRange::ThisWeek => DateRange::NextWeek,
            DateRange::NextWeek => DateRange::Today,
        }
    }

    /// Cycle backward through date ranges.
    pub fn prev(self) -> Self {
        match self {
            DateRange::Today => DateRange::NextWeek,
            DateRange::ThisWeek => DateRange::Today,
            DateRange::NextWeek => DateRange::ThisWeek,
        }
    }

    /// Display label for the date range.
    pub fn label(self) -> &'static str {
        match self {
            DateRange::Today => "Today",
            DateRange::ThisWeek => "This Week",
            DateRange::NextWeek => "Next Week",
        }
    }
}

/// Sort column for the economic calendar table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortColumn {
    Time,
    Country,
    Event,
    Impact,
}

impl SortColumn {
    /// Cycle to the next sort column.
    pub fn cycle(self) -> Self {
        match self {
            SortColumn::Time => SortColumn::Country,
            SortColumn::Country => SortColumn::Event,
            SortColumn::Event => SortColumn::Impact,
            SortColumn::Impact => SortColumn::Time,
        }
    }

}

/// Container for economic calendar events.
pub struct EconomicCalendar {
    events: Vec<EconomicEvent>,
}

impl EconomicCalendar {
    /// Create a new empty calendar.
    pub fn new() -> Self {
        Self { events: Vec::new() }
    }

    /// Replace all events with a new set.
    pub fn replace(&mut self, events: Vec<EconomicEvent>) {
        self.events = events;
    }

    /// Return events sorted by the given column and direction.
    pub fn sorted_events(&self, sort: SortColumn, ascending: bool) -> Vec<&EconomicEvent> {
        let mut entries: Vec<&EconomicEvent> = self.events.iter().collect();

        entries.sort_by(|a, b| {
            let cmp = match sort {
                SortColumn::Time => {
                    // None (unparsed) sorts last
                    match (&a.time_parsed, &b.time_parsed) {
                        (Some(ta), Some(tb)) => ta.cmp(tb),
                        (Some(_), None) => std::cmp::Ordering::Less,
                        (None, Some(_)) => std::cmp::Ordering::Greater,
                        (None, None) => std::cmp::Ordering::Equal,
                    }
                }
                SortColumn::Country => a.country.cmp(&b.country),
                SortColumn::Event => a.event.cmp(&b.event),
                SortColumn::Impact => a.impact.cmp(&b.impact),
            };

            if ascending {
                cmp
            } else {
                cmp.reverse()
            }
        });

        entries
    }

    /// Number of events in the calendar.
    pub fn len(&self) -> usize {
        self.events.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_impact_from_str_text_formats() {
        assert_eq!(Impact::from_str("high"), Impact::High);
        assert_eq!(Impact::from_str("HIGH"), Impact::High);
        assert_eq!(Impact::from_str("medium"), Impact::Medium);
        assert_eq!(Impact::from_str("Medium"), Impact::Medium);
        assert_eq!(Impact::from_str("low"), Impact::Low);
        assert_eq!(Impact::from_str("LOW"), Impact::Low);
        assert_eq!(Impact::from_str(""), Impact::Unknown);
        assert_eq!(Impact::from_str("unknown"), Impact::Unknown);
    }

    #[test]
    fn test_impact_from_str_numeric_formats() {
        assert_eq!(Impact::from_str("3"), Impact::High);
        assert_eq!(Impact::from_str("2"), Impact::Medium);
        assert_eq!(Impact::from_str("1"), Impact::Low);
        assert_eq!(Impact::from_str("0"), Impact::Unknown);
    }

    #[test]
    fn test_impact_labels() {
        assert_eq!(Impact::High.label(), "HIGH");
        assert_eq!(Impact::Medium.label(), "MED");
        assert_eq!(Impact::Low.label(), "LOW");
        assert_eq!(Impact::Unknown.label(), "---");
    }

    #[test]
    fn test_impact_ordering() {
        assert!(Impact::High > Impact::Medium);
        assert!(Impact::Medium > Impact::Low);
        assert!(Impact::Low > Impact::Unknown);
    }

    #[test]
    fn test_date_range_labels() {
        assert_eq!(DateRange::Today.label(), "Today");
        assert_eq!(DateRange::ThisWeek.label(), "This Week");
        assert_eq!(DateRange::NextWeek.label(), "Next Week");
    }

    #[test]
    fn test_date_range_cycle() {
        assert_eq!(DateRange::Today.next(), DateRange::ThisWeek);
        assert_eq!(DateRange::ThisWeek.next(), DateRange::NextWeek);
        assert_eq!(DateRange::NextWeek.next(), DateRange::Today);

        assert_eq!(DateRange::Today.prev(), DateRange::NextWeek);
        assert_eq!(DateRange::ThisWeek.prev(), DateRange::Today);
        assert_eq!(DateRange::NextWeek.prev(), DateRange::ThisWeek);
    }

    #[test]
    fn test_date_range_to_dates_format() {
        // Just verify format is YYYY-MM-DD
        let (from, to) = DateRange::Today.to_dates();
        assert_eq!(from.len(), 10);
        assert_eq!(to.len(), 10);
        assert_eq!(from, to); // Today: same date

        let (from, to) = DateRange::ThisWeek.to_dates();
        assert_eq!(from.len(), 10);
        assert_eq!(to.len(), 10);
        assert!(from <= to); // Monday <= Friday
    }

    #[test]
    fn test_sort_column_cycle() {
        assert_eq!(SortColumn::Time.cycle(), SortColumn::Country);
        assert_eq!(SortColumn::Country.cycle(), SortColumn::Event);
        assert_eq!(SortColumn::Event.cycle(), SortColumn::Impact);
        assert_eq!(SortColumn::Impact.cycle(), SortColumn::Time);
    }

    #[test]
    fn test_economic_event_from_raw() {
        let raw = RawEconomicEvent {
            actual: Some(3.5),
            country: Some("US".to_string()),
            estimate: Some(3.4),
            event: Some("CPI".to_string()),
            impact: Some("high".to_string()),
            prev: Some(3.3),
            time: Some("2026-02-21 14:30:00".to_string()),
            unit: Some("%".to_string()),
        };

        let event = EconomicEvent::from_raw(raw);
        assert_eq!(event.country, "US");
        assert_eq!(event.event, "CPI");
        assert_eq!(event.impact, Impact::High);
        assert_eq!(event.actual, Some(3.5));
        assert_eq!(event.estimate, Some(3.4));
        assert_eq!(event.prev, Some(3.3));
        assert_eq!(event.unit, "%");
        assert!(event.time_parsed.is_some());
    }

    #[test]
    fn test_economic_event_from_raw_all_none() {
        let raw = RawEconomicEvent {
            actual: None,
            country: None,
            estimate: None,
            event: None,
            impact: None,
            prev: None,
            time: None,
            unit: None,
        };

        let event = EconomicEvent::from_raw(raw);
        assert_eq!(event.country, "");
        assert_eq!(event.event, "");
        assert_eq!(event.impact, Impact::Unknown);
        assert_eq!(event.actual, None);
        assert!(event.time_parsed.is_none());
    }

    #[test]
    fn test_economic_calendar_replace_and_sort() {
        let mut cal = EconomicCalendar::new();
        assert_eq!(cal.len(), 0);

        let events = vec![
            EconomicEvent {
                time_raw: "2026-02-21 14:30:00".to_string(),
                time_parsed: NaiveDateTime::parse_from_str("2026-02-21 14:30:00", "%Y-%m-%d %H:%M:%S").ok(),
                country: "US".to_string(),
                event: "CPI".to_string(),
                impact: Impact::High,
                actual: Some(3.5),
                estimate: Some(3.4),
                prev: Some(3.3),
                unit: "%".to_string(),
            },
            EconomicEvent {
                time_raw: "2026-02-21 10:00:00".to_string(),
                time_parsed: NaiveDateTime::parse_from_str("2026-02-21 10:00:00", "%Y-%m-%d %H:%M:%S").ok(),
                country: "GB".to_string(),
                event: "GDP".to_string(),
                impact: Impact::Medium,
                actual: None,
                estimate: Some(0.3),
                prev: Some(0.2),
                unit: "%".to_string(),
            },
        ];

        cal.replace(events);
        assert_eq!(cal.len(), 2);

        // Sort by time ascending: GB event (10:00) before US event (14:30)
        let sorted = cal.sorted_events(SortColumn::Time, true);
        assert_eq!(sorted[0].country, "GB");
        assert_eq!(sorted[1].country, "US");

        // Sort by impact descending: High before Medium
        let sorted = cal.sorted_events(SortColumn::Impact, false);
        assert_eq!(sorted[0].impact, Impact::High);
        assert_eq!(sorted[1].impact, Impact::Medium);
    }
}
