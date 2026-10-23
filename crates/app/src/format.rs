//! Small text formatters for the table and status bar.

/// Track length as `m:ss`, or `h:mm:ss` from an hour up. Empty when unknown.
pub fn clock(ms: u32) -> String {
    if ms == 0 {
        return String::new();
    }
    let secs = (ms + 500) / 1000;
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

/// Total playing time in the largest sensible unit, like "3.4 days".
pub fn span(ms: u64) -> String {
    let minutes = ms as f64 / 60_000.0;
    if minutes < 90.0 {
        format!("{minutes:.0} min")
    } else if minutes < 48.0 * 60.0 {
        format!("{:.1} hours", minutes / 60.0)
    } else {
        format!("{:.1} days", minutes / 1440.0)
    }
}

pub fn bytes(n: u64) -> String {
    let gb = n as f64 / 1e9;
    if gb >= 1.0 { format!("{gb:.2} GB") } else { format!("{:.0} MB", n as f64 / 1e6) }
}

/// A file's format and bitrate as the song table shows them: "FLAC", "MP3 320".
pub fn quality(ext: &str, lossless: bool, kbps: u32) -> String {
    let format = ext.to_ascii_uppercase();
    if lossless || kbps == 0 { format } else { format!("{format} {kbps}") }
}

/// A Unix time as "13 Oct 2026, 14:36 UTC".
pub fn stamp(secs: i64) -> String {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let (days, rest) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Days since 1970 to a calendar date, after Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let (era, doe) = (z.div_euclid(146_097), z.rem_euclid(146_097));
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{day} {} {year}, {:02}:{:02} UTC", MONTHS[month as usize - 1], rest / 3600, rest / 60 % 60)
}

/// Integer with thousands separators.
pub fn count(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(clock(0), "");
        assert_eq!(clock(247_400), "4:07");
        assert_eq!(clock(3_723_000), "1:02:03");
        assert_eq!(span(45 * 60_000), "45 min");
        assert_eq!(span(5 * 3_600_000), "5.0 hours");
        assert_eq!(span(72 * 3_600_000), "3.0 days");
        assert_eq!(bytes(9_810_000_000), "9.81 GB");
        assert_eq!(bytes(512_000_000), "512 MB");
        assert_eq!(count(7), "7");
        assert_eq!(count(29_502), "29,502");
        assert_eq!(count(1_000_000), "1,000,000");
        assert_eq!(quality("flac", true, 912), "FLAC");
        assert_eq!(quality("mp3", false, 320), "MP3 320");
        assert_eq!(quality("wma", false, 0), "WMA");
        assert_eq!(stamp(0), "1 Jan 1970, 00:00 UTC");
        assert_eq!(stamp(951_782_400), "29 Feb 2000, 00:00 UTC");
        assert_eq!(stamp(1_791_902_160), "13 Oct 2026, 14:36 UTC");
    }
}
