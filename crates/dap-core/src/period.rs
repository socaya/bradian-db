//! DHIS2 reporting periods.
//!
//! This is the subset the engine commits to: Daily, Weekly (ISO), Monthly, Quarterly, Yearly.
//! The spec lists additional weekly variants and fiscal periods as "versioned function matrix"
//! items — they live in `dap-dhis2` once the semantic compiler is up. Keeping the core set
//! small here lets planning, materialized views and the time bucketing operator reason about
//! period math without depending on DHIS2 metadata.
//!
//! All periods are UTC. `start`/`end` return inclusive-start / exclusive-end millisecond
//! boundaries in Unix time.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::{DapError, DapResult};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PeriodType {
    Daily,
    Weekly,
    Monthly,
    Quarterly,
    Yearly,
}

impl PeriodType {
    pub fn code(&self) -> &'static str {
        match self {
            PeriodType::Daily => "Daily",
            PeriodType::Weekly => "Weekly",
            PeriodType::Monthly => "Monthly",
            PeriodType::Quarterly => "Quarterly",
            PeriodType::Yearly => "Yearly",
        }
    }
}

/// An instant-in-time period. The `code` is the DHIS2 canonical ISO form.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Period {
    code: String,
    ty: PeriodType,
    year: i32,
    index: u16, // 1-based: 1-366 (day), 1-53 (week), 1-12 (month), 1-4 (quarter), 1 (year)
}

impl Period {
    pub fn code(&self) -> &str {
        &self.code
    }
    pub fn period_type(&self) -> PeriodType {
        self.ty
    }
    pub fn year(&self) -> i32 {
        self.year
    }
    pub fn index(&self) -> u16 {
        self.index
    }

    pub fn parse(raw: &str) -> DapResult<Self> {
        let err = |msg: &str| DapError::InvalidPeriod {
            raw: raw.to_string(),
            reason: msg.to_string(),
        };

        // Yearly: YYYY
        if raw.len() == 4 && raw.chars().all(|c| c.is_ascii_digit()) {
            let y: i32 = raw.parse().map_err(|_| err("year parse"))?;
            return Ok(Self {
                code: raw.to_string(),
                ty: PeriodType::Yearly,
                year: y,
                index: 1,
            });
        }
        // Monthly: YYYYMM
        if raw.len() == 6 && raw.chars().all(|c| c.is_ascii_digit()) {
            let y: i32 = raw[0..4].parse().map_err(|_| err("year parse"))?;
            let m: u16 = raw[4..6].parse().map_err(|_| err("month parse"))?;
            if !(1..=12).contains(&m) {
                return Err(err("month must be 1..12"));
            }
            return Ok(Self {
                code: raw.to_string(),
                ty: PeriodType::Monthly,
                year: y,
                index: m,
            });
        }
        // Daily: YYYYMMDD
        if raw.len() == 8 && raw.chars().all(|c| c.is_ascii_digit()) {
            let y: i32 = raw[0..4].parse().map_err(|_| err("year parse"))?;
            let m: u32 = raw[4..6].parse().map_err(|_| err("month parse"))?;
            let d: u32 = raw[6..8].parse().map_err(|_| err("day parse"))?;
            let doy = day_of_year(y, m, d).ok_or_else(|| err("day out of range"))?;
            return Ok(Self {
                code: raw.to_string(),
                ty: PeriodType::Daily,
                year: y,
                index: doy as u16,
            });
        }
        // Quarterly: YYYYQn
        if raw.len() == 6 && raw.as_bytes()[4] == b'Q' {
            let y: i32 = raw[0..4].parse().map_err(|_| err("year parse"))?;
            let q: u16 = raw[5..6].parse().map_err(|_| err("quarter parse"))?;
            if !(1..=4).contains(&q) {
                return Err(err("quarter must be 1..4"));
            }
            return Ok(Self {
                code: raw.to_string(),
                ty: PeriodType::Quarterly,
                year: y,
                index: q,
            });
        }
        // Weekly ISO: YYYYWww
        if raw.len() >= 7 && raw.as_bytes()[4] == b'W' {
            let y: i32 = raw[0..4].parse().map_err(|_| err("year parse"))?;
            let w: u16 = raw[5..].parse().map_err(|_| err("week parse"))?;
            if !(1..=53).contains(&w) {
                return Err(err("week must be 1..53"));
            }
            return Ok(Self {
                code: format!("{y:04}W{w:02}"),
                ty: PeriodType::Weekly,
                year: y,
                index: w,
            });
        }
        Err(err("unrecognised format"))
    }

    /// Inclusive start, exclusive end in Unix milliseconds (UTC).
    pub fn range_millis(&self) -> (i64, i64) {
        match self.ty {
            PeriodType::Yearly => {
                let start = days_from_civil(self.year, 1, 1) * MS_PER_DAY;
                let end = days_from_civil(self.year + 1, 1, 1) * MS_PER_DAY;
                (start, end)
            }
            PeriodType::Monthly => {
                let m = self.index as u32;
                let start = days_from_civil(self.year, m, 1) * MS_PER_DAY;
                let (ny, nm) = if m == 12 {
                    (self.year + 1, 1)
                } else {
                    (self.year, m + 1)
                };
                let end = days_from_civil(ny, nm, 1) * MS_PER_DAY;
                (start, end)
            }
            PeriodType::Quarterly => {
                let m = (self.index as u32 - 1) * 3 + 1;
                let start = days_from_civil(self.year, m, 1) * MS_PER_DAY;
                let next_month = m + 3;
                let (ny, nm) = if next_month > 12 {
                    (self.year + 1, next_month - 12)
                } else {
                    (self.year, next_month)
                };
                let end = days_from_civil(ny, nm, 1) * MS_PER_DAY;
                (start, end)
            }
            PeriodType::Daily => {
                let base = days_from_civil(self.year, 1, 1);
                let start = (base + self.index as i64 - 1) * MS_PER_DAY;
                (start, start + MS_PER_DAY)
            }
            PeriodType::Weekly => {
                // ISO week: week 1 contains the first Thursday of the year.
                let jan4 = days_from_civil(self.year, 1, 4);
                let jan4_dow = weekday_from_days(jan4); // 0=Mon..6=Sun
                let week1_start = jan4 - jan4_dow;
                let start_day = week1_start + (self.index as i64 - 1) * 7;
                let start = start_day * MS_PER_DAY;
                (start, start + 7 * MS_PER_DAY)
            }
        }
    }

    /// Return the period code containing the given UTC timestamp for a given period type.
    pub fn bucket(ty: PeriodType, ts_millis: i64) -> Period {
        let total_days = ts_millis.div_euclid(MS_PER_DAY);
        let (y, m, d) = civil_from_days(total_days);
        let code = match ty {
            PeriodType::Daily => format!("{y:04}{m:02}{d:02}"),
            PeriodType::Monthly => format!("{y:04}{m:02}"),
            PeriodType::Yearly => format!("{y:04}"),
            PeriodType::Quarterly => format!("{y:04}Q{}", (m - 1) / 3 + 1),
            PeriodType::Weekly => {
                // Canonical ISO 8601 week-of-year:
                //   1. Monday-of-week = d - weekday(d)   (Mon=0)
                //   2. Thursday-of-week = Monday + 3  -> its civil year is the ISO year.
                //   3. Jan-4 of the ISO year is always in week 1 (by definition).
                //   4. Week1-Monday = Jan4 - weekday(Jan4).
                //   5. ISO week = (Monday-of-week - Week1-Monday) / 7 + 1
                let d_days = total_days;
                let dow = weekday_from_days(d_days); // Mon=0..Sun=6
                let monday = d_days - dow;
                let thursday = monday + 3;
                let (iso_year, _, _) = civil_from_days(thursday);
                let jan4 = days_from_civil(iso_year, 1, 4);
                let jan4_dow = weekday_from_days(jan4);
                let week1_monday = jan4 - jan4_dow;
                let wn = ((monday - week1_monday) / 7 + 1) as u32;
                format!("{iso_year:04}W{wn:02}")
            }
        };
        Period::parse(&code).expect("bucket produces a valid code")
    }
}

impl fmt::Debug for Period {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Period({})", self.code)
    }
}
impl fmt::Display for Period {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.code)
    }
}
impl From<Period> for String {
    fn from(p: Period) -> Self {
        p.code
    }
}
impl TryFrom<String> for Period {
    type Error = DapError;
    fn try_from(v: String) -> DapResult<Self> {
        Period::parse(&v)
    }
}

const MS_PER_DAY: i64 = 86_400_000;

// --- Civil date arithmetic (Howard Hinnant's chrono algorithm) ---
// Returns days since 1970-01-01 (negative before epoch). Pure UTC, proleptic Gregorian.

fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u32; // 0..=399
    let doy = (153 * if m > 2 { m - 3 } else { m + 9 } + 2) / 5 + d - 1; // 0..=365
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // 0..=146096
    (era as i64) * 146097 + doe as i64 - 719468
}

fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i32 + era as i32 * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

fn day_of_year(y: i32, m: u32, d: u32) -> Option<u32> {
    if !(1..=12).contains(&m) || d == 0 || d > 31 {
        return None;
    }
    let jan1 = days_from_civil(y, 1, 1);
    let this = days_from_civil(y, m, d);
    let doy = (this - jan1 + 1) as u32;
    let next_year = days_from_civil(y + 1, 1, 1);
    if this >= next_year {
        return None;
    }
    Some(doy)
}

fn weekday_from_days(z: i64) -> i64 {
    // 1970-01-01 was a Thursday (weekday 3 with Monday=0).
    (z + 3).rem_euclid(7)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_yearly() {
        let p = Period::parse("2026").unwrap();
        assert_eq!(p.period_type(), PeriodType::Yearly);
        assert_eq!(p.year(), 2026);
    }

    #[test]
    fn parse_monthly() {
        let p = Period::parse("202601").unwrap();
        assert_eq!(p.period_type(), PeriodType::Monthly);
        assert_eq!(p.index(), 1);
    }

    #[test]
    fn parse_quarterly() {
        let p = Period::parse("2026Q3").unwrap();
        assert_eq!(p.period_type(), PeriodType::Quarterly);
        assert_eq!(p.index(), 3);
    }

    #[test]
    fn parse_weekly() {
        let p = Period::parse("2026W03").unwrap();
        assert_eq!(p.period_type(), PeriodType::Weekly);
        assert_eq!(p.index(), 3);
    }

    #[test]
    fn parse_daily() {
        let p = Period::parse("20261111").unwrap();
        assert_eq!(p.period_type(), PeriodType::Daily);
        // 2026-11-11 is day 315 of a non-leap year.
        assert_eq!(p.index(), 315);
    }

    #[test]
    fn invalid_periods_rejected() {
        for bad in ["", "abc", "20261300", "2026Q5", "2026W54"] {
            assert!(Period::parse(bad).is_err(), "should reject: {bad}");
        }
    }

    #[test]
    fn range_monthly() {
        let p = Period::parse("202602").unwrap();
        let (start, end) = p.range_millis();
        // Feb 2026 non-leap: 28 days.
        assert_eq!(end - start, 28 * MS_PER_DAY);
    }

    #[test]
    fn range_quarterly() {
        let p = Period::parse("2026Q1").unwrap();
        let (start, end) = p.range_millis();
        // Jan+Feb+Mar 2026 = 31+28+31 = 90 days.
        assert_eq!(end - start, 90 * MS_PER_DAY);
    }

    #[test]
    fn bucket_monthly_contains_timestamp() {
        let ts = days_from_civil(2026, 11, 11) * MS_PER_DAY + 1000;
        let p = Period::bucket(PeriodType::Monthly, ts);
        assert_eq!(p.code(), "202611");
        let (s, e) = p.range_millis();
        assert!(s <= ts && ts < e);
    }

    #[test]
    fn bucket_weekly_spans_7_days() {
        let ts = days_from_civil(2026, 11, 11) * MS_PER_DAY;
        let p = Period::bucket(PeriodType::Weekly, ts);
        let (s, e) = p.range_millis();
        assert_eq!(e - s, 7 * MS_PER_DAY);
        assert!(s <= ts && ts < e);
    }

    /// Canonical ISO week fixtures from the spec Appendix.
    /// Source: https://en.wikipedia.org/wiki/ISO_week_date#Examples
    #[test]
    fn iso_week_boundaries() {
        let cases = [
            // (y, m, d, expected ISO code)
            (2005, 1, 1, "2004W53"),  // Sat in last ISO week of 2004
            (2005, 1, 2, "2004W53"),
            (2005, 12, 31, "2005W52"),
            (2007, 1, 1, "2007W01"),  // Mon
            (2007, 12, 30, "2007W52"),
            (2007, 12, 31, "2008W01"), // Mon of W1-2008
            (2008, 1, 1, "2008W01"),
            (2008, 12, 29, "2009W01"), // Mon of W1-2009
            (2009, 1, 1, "2009W01"),
            (2009, 12, 31, "2009W53"),
            (2010, 1, 3, "2009W53"),
            (2026, 1, 1, "2026W01"),
            (2026, 11, 11, "2026W46"),
        ];
        for (y, m, d, expected) in cases {
            let ts = days_from_civil(y, m, d) * MS_PER_DAY;
            let p = Period::bucket(PeriodType::Weekly, ts);
            assert_eq!(p.code(), expected, "date {y}-{m:02}-{d:02}");
        }
    }

    #[test]
    fn weekly_range_contains_ts() {
        // Spot-check: each day in 2009 (an ISO 53-week year) buckets to a code whose
        // range_millis contains the timestamp.
        for days_offset in 0..365 {
            let ts = (days_from_civil(2009, 1, 1) + days_offset) * MS_PER_DAY + 1_234;
            let p = Period::bucket(PeriodType::Weekly, ts);
            let (s, e) = p.range_millis();
            assert!(s <= ts && ts < e, "{} failed on offset {}", p.code(), days_offset);
            assert_eq!(e - s, 7 * MS_PER_DAY);
        }
    }

    #[test]
    fn civil_roundtrip() {
        for &(y, m, d) in &[
            (1970, 1, 1),
            (2000, 2, 29),
            (2024, 12, 31),
            (2026, 11, 11),
            (1600, 1, 1),
        ] {
            let days = days_from_civil(y, m, d);
            let (y2, m2, d2) = civil_from_days(days);
            assert_eq!((y, m, d), (y2, m2, d2));
        }
    }
}
