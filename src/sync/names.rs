//! Save names: what RomM calls a save, what an emulator calls it, and how the two are matched.
//!
//! Ported from the Kotlin app's `canonicalSaveName`, `gameBaseName` and `localSaveName`, whose
//! comments carry the cases each rule was written for.

use std::time::{Duration, SystemTime};

/// A server save's name as an emulator would write it: RomM's history tag removed.
///
/// RomM files every upload into a history slot under a new name, adding the time it arrived —
/// `Chrono Trigger (USA) [2026-06-29_13-48-43].srm`, or with milliseconds from its web player. No
/// emulator reads or writes that name. Left on, a pulled save would land where nothing opens it,
/// and the unchanged local copy would then upload beside it as a second save forever.
pub fn canonical(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut rest = name;
    while let Some(open) = rest.find('[') {
        match tag_len(&rest[open..]) {
            Some(len) => {
                out.push_str(rest[..open].trim_end());
                rest = &rest[open + len..];
            }
            None => {
                out.push_str(&rest[..=open]);
                rest = &rest[open + 1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// The length of the history tag at the start of `s`, brackets included, if that is what it is:
/// `[YYYY-MM-DD_HH-MM-SS]`, with a space for the underscore and an optional `-mmm` or `.mmm`.
fn tag_len(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let digits = |from: usize, n: usize| b.get(from..from + n).is_some_and(|d| d.iter().all(u8::is_ascii_digit));
    let is = |at: usize, c: &[u8]| b.get(at).is_some_and(|x| c.contains(x));

    let ok = is(0, b"[")
        && digits(1, 4) && is(5, b"-") && digits(6, 2) && is(8, b"-") && digits(9, 2)
        && is(11, b"_ ")
        && digits(12, 2) && is(14, b"-") && digits(15, 2) && is(17, b"-") && digits(18, 2);
    if !ok {
        return None;
    }
    let mut end = 20;
    if is(end, b"-.") {
        let millis = b[end + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
        if !(1..=3).contains(&millis) {
            return None;
        }
        end += 1 + millis;
    }
    is(end, b"]").then_some(end + 1)
}

/// The name a server save must have on disk, given the stem of the local file it may be.
///
/// Normally the server's own name. The exception is the same name spelled differently, which is
/// what an uploader that sanitises file names produces: RomM's web player drops characters it
/// cannot put in a URL, so Pokemon Yellow's `… (CGB+SGB Enhanced)` comes back as
/// `… (CGBSGB Enhanced).srm`. A libretro core reads `<content stem>.srm` and nothing else, so such a
/// save is taken to be the local one. Deliberately narrow — punctuation only: `Driver 2 (USA)_1`
/// against `Driver 2 (USA) (Disc 1)` differs by whole words and keeps its own name.
pub fn local_name(server: &str, local_stem: &str) -> String {
    let (stem, ext) = split_ext(server);
    if stem.eq_ignore_ascii_case(local_stem) || !same_modulo_punctuation(stem, local_stem) {
        return server.to_string();
    }
    match ext {
        Some(ext) => format!("{local_stem}.{ext}"),
        None => local_stem.to_string(),
    }
}

fn split_ext(name: &str) -> (&str, Option<&str>) {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem, Some(ext)),
        _ => (name, None),
    }
}

fn same_modulo_punctuation(a: &str, b: &str) -> bool {
    let key = punctuation_free(a);
    !key.is_empty() && key == punctuation_free(b)
}

/// Letters and digits only, lowercased — the loosest form two names may be compared in.
pub fn punctuation_free(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// A name with its `(Disc N)` tag removed, so every disc of a set reduces to the set.
///
/// A multi-disc game is one rom on RomM — a folder named for the set — but RetroArch names the save
/// after whatever it loaded, which for a single disc is `Game (USA) (Disc 1)`.
pub fn without_disc(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut rest = name;
    while let Some(open) = rest.find('(') {
        let Some(close) = rest[open..].find(')') else { break };
        let inner = rest[open + 1..open + close].trim();
        // `Disc 1`, `Disk B`, `Disc1` — but not `Discography`, and not `Disc 1 of 2`, which the
        // Kotlin pattern this mirrors (`Dis[ck]\s*\w+`) does not take either.
        let is_disc = inner.get(..4).is_some_and(|w| {
            let tail = &inner[4..];
            (w.eq_ignore_ascii_case("disc") || w.eq_ignore_ascii_case("disk"))
                && tail.starts_with(|c: char| c.is_whitespace() || c.is_ascii_digit())
                && !tail.trim().is_empty()
                && tail.trim().chars().all(char::is_alphanumeric)
        });
        if is_disc {
            out.push_str(rest[..open].trim_end());
        } else {
            out.push_str(&rest[..=open + close]);
        }
        rest = &rest[open + close + 1..];
    }
    out.push_str(rest);
    let trimmed = out.trim();
    if trimmed.is_empty() { name.to_string() } else { trimmed.to_string() }
}

/// A RomM timestamp — RFC 3339, with or without fraction and offset — as a point in time.
///
/// Written out rather than pulled in from a date crate because this is all of it: RomM sends one
/// format, and what the sync needs is to order it against a file's modification time.
pub fn parse_timestamp(s: &str) -> Option<SystemTime> {
    let s = s.trim();
    let date = s.get(..10)?;
    let rest = s.get(10..)?.strip_prefix(['T', 't', ' '])?;
    let time = rest.get(..8)?;
    let mut zone = rest.get(8..)?;

    let mut date_parts = date.split('-').map(|p| p.parse::<i64>().ok());
    let (year, month, day) = (date_parts.next()??, date_parts.next()??, date_parts.next()??);
    let mut time_parts = time.split(':').map(|p| p.parse::<i64>().ok());
    let (hour, minute, second) = (time_parts.next()??, time_parts.next()??, time_parts.next()??);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 60 {
        return None;
    }

    let mut nanos = 0u32;
    if let Some(fraction) = zone.strip_prefix('.') {
        let digits = fraction.bytes().take_while(u8::is_ascii_digit).count();
        let padded = format!("{:0<9}", &fraction[..digits.min(9)]);
        nanos = padded.parse().ok()?;
        zone = &fraction[digits..];
    }
    let offset = match zone {
        "" | "Z" | "z" => 0,
        _ => {
            let sign = match zone.as_bytes()[0] {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let (hh, mm) = zone[1..].split_once(':').unwrap_or((zone.get(1..3)?, zone.get(3..).unwrap_or("0")));
            sign * (hh.parse::<i64>().ok()? * 3600 + mm.parse::<i64>().ok()? * 60)
        }
    };

    let seconds = days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second - offset;
    let seconds = u64::try_from(seconds).ok()?;
    Some(SystemTime::UNIX_EPOCH + Duration::new(seconds, nanos))
}

/// A point in time as RomM takes one: RFC 3339 in UTC, to the second — `2026-07-01T10:00:00Z`.
pub fn format_timestamp(t: SystemTime) -> String {
    format!("{}Z", format_timestamp_no_zone(t))
}

/// [`format_timestamp`] to the microsecond, as RomM keeps its own times: `2026-07-01T10:00:00.250000Z`.
/// What a sync reports a save's time as, since RomM weighs it against times of that precision.
pub fn format_timestamp_precise(t: SystemTime) -> String {
    let micros = t.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.subsec_micros());
    format!("{}.{micros:06}Z", format_timestamp_no_zone(t))
}

/// [`format_timestamp`] without the zone: `2026-07-01T10:00:00`.
pub fn format_timestamp_no_zone(t: SystemTime) -> String {
    let secs = t.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// The inverse of [`days_from_civil`] — Howard Hinnant's `civil_from_days`.
pub(crate) fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (year_of_era + era * 400 + i64::from(month <= 2), month, day)
}

/// Days since 1970-01-01 for a proleptic Gregorian date — Howard Hinnant's `days_from_civil`.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_timestamp_written_reads_back_as_the_same_instant() {
        for secs in [0u64, 951_782_400, 1_782_900_000, 4_102_444_799] {
            let t = SystemTime::UNIX_EPOCH + Duration::from_secs(secs);
            assert_eq!(parse_timestamp(&format_timestamp(t)), Some(t), "{}", format_timestamp(t));
        }
        assert_eq!(format_timestamp(SystemTime::UNIX_EPOCH + Duration::from_secs(951_782_400)), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn the_history_tag_is_stripped_in_every_spelling_romm_writes() {
        assert_eq!(canonical("Chrono Trigger (USA) [2026-06-29_13-48-43].srm"), "Chrono Trigger (USA).srm");
        // The web player's: a space for the underscore, and milliseconds.
        assert_eq!(canonical("Chrono Trigger (USA) [2026-06-29 13-48-43-123].srm"), "Chrono Trigger (USA).srm");
        assert_eq!(canonical("Chrono Trigger (USA) [2026-06-29_13-48-43.5].srm"), "Chrono Trigger (USA).srm");
    }

    #[test]
    fn brackets_that_are_not_a_history_tag_are_part_of_the_name() {
        // Dump tags use brackets too, and they are the rom's own name.
        assert_eq!(canonical("Zelda [!] (USA).srm"), "Zelda [!] (USA).srm");
        assert_eq!(canonical("Game [2026-06-29].srm"), "Game [2026-06-29].srm");
        assert_eq!(canonical("Game [2026-06-29_13-48-43-1234].srm"), "Game [2026-06-29_13-48-43-1234].srm");
    }

    #[test]
    fn a_save_spelled_without_punctuation_takes_the_local_name() {
        assert_eq!(
            local_name("Pokemon Yellow (USA) (CGBSGB Enhanced).srm", "Pokemon Yellow (USA) (CGB+SGB Enhanced)"),
            "Pokemon Yellow (USA) (CGB+SGB Enhanced).srm"
        );
    }

    #[test]
    fn a_save_that_differs_by_words_keeps_its_own_name() {
        assert_eq!(local_name("Driver 2 (USA)_1.srm", "Driver 2 (USA) (Disc 1)"), "Driver 2 (USA)_1.srm");
        assert_eq!(local_name("Other Game.srm", "Chrono Trigger (USA)"), "Other Game.srm");
    }

    #[test]
    fn a_disc_tag_reduces_to_the_set_and_nothing_else_does() {
        assert_eq!(without_disc("Final Fantasy VII (USA) (Disc 1)"), "Final Fantasy VII (USA)");
        assert_eq!(without_disc("Policenauts (Japan) (Disk 2) (v1.1)"), "Policenauts (Japan) (v1.1)");
        assert_eq!(without_disc("Discworld (USA)"), "Discworld (USA)");
        assert_eq!(without_disc("Game (Discography)"), "Game (Discography)");
    }

    #[test]
    fn timestamps_parse_with_any_offset_and_precision() {
        let utc = parse_timestamp("2026-06-29T13:48:43Z").unwrap();
        assert_eq!(utc.duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs(), 1_782_740_923);
        assert_eq!(parse_timestamp("2026-06-29T13:48:43+00:00"), Some(utc));
        assert_eq!(parse_timestamp("2026-06-29T15:48:43+02:00"), Some(utc));
        assert_eq!(parse_timestamp("2026-06-29 13:48:43"), Some(utc));
        let precise = parse_timestamp("2026-06-29T13:48:43.250000+00:00").unwrap();
        assert_eq!(precise.duration_since(utc).unwrap(), Duration::from_millis(250));
    }

    #[test]
    fn a_timestamp_that_is_not_one_is_none() {
        assert_eq!(parse_timestamp(""), None);
        assert_eq!(parse_timestamp("yesterday"), None);
        assert_eq!(parse_timestamp("2026-13-01T00:00:00Z"), None);
    }
}
