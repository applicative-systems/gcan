//! Human-readable formatting and CLI value parsing.
//!
//! The IEC size formatter reproduces `numfmt --to=iec --suffix=B`: base 1024,
//! up to 3 significant digits, one decimal below 10, and rounding *away from
//! zero* (numfmt's default `--round=from-zero`, i.e. ceiling for positive
//! values). All math is integer-only so a given byte count always formats to
//! the exact same string.

const UNITS: [&str; 9] = ["", "K", "M", "G", "T", "P", "E", "Z", "Y"];

/// Format a byte count like `numfmt --to=iec --suffix=B` (e.g. `3.7GB`, `197KB`,
/// `96B`, `0B`).
pub fn iec_size(bytes: u64) -> String {
    // Pick the largest unit whose scaled value is >= 1 (and < 1024).
    let mut unit = 0usize;
    let mut divisor: u128 = 1;
    while unit + 1 < UNITS.len() && (bytes as u128) >= divisor * 1024 {
        divisor *= 1024;
        unit += 1;
    }

    // Bytes: print the exact integer, no rounding.
    if unit == 0 {
        return format!("{bytes}B");
    }

    let v = bytes as u128;
    // One decimal place while the scaled value is < 10, else a whole number.
    if v < 10 * divisor {
        // tenths = ceil(v * 10 / divisor), rounding away from zero.
        let mut tenths = (v * 10).div_ceil(divisor);
        // Rounding can bump us to 10.0 — drop the decimal to match numfmt.
        if tenths >= 100 {
            return format!("{}{}B", tenths / 10, UNITS[unit]);
        }
        // And to exactly the unit ceiling (rare) — renormalize one unit up.
        if tenths >= 10 * 1024 {
            tenths = 10;
            return format!("{}{}B", tenths / 10, UNITS[unit + 1]);
        }
        format!("{}.{}{}B", tenths / 10, tenths % 10, UNITS[unit])
    } else {
        // whole = ceil(v / divisor), away from zero.
        let whole = v.div_ceil(divisor);
        if whole >= 1024 && unit + 1 < UNITS.len() {
            // Renormalize, e.g. 1024K -> 1.0M.
            return iec_size_renorm(whole, unit + 1);
        }
        format!("{}{}B", whole, UNITS[unit])
    }
}

/// Helper for the rare carry where a whole-number ceiling reaches 1024 in its
/// unit and must move up one unit (e.g. `1024K` -> `1.0M`).
fn iec_size_renorm(whole_in_prev_unit: u128, unit: usize) -> String {
    // whole_in_prev_unit is ~1024; in the new unit that's ~1.0.
    let tenths = (whole_in_prev_unit * 10).div_ceil(1024);
    format!("{}.{}{}B", tenths / 10, tenths % 10, UNITS[unit])
}

/// Format an age (seconds) like the bash `human_age`: days, else hours, else
/// minutes — including `0m` for anything under a minute.
pub fn human_age(secs: u64) -> String {
    let d = secs / 86_400;
    let h = (secs % 86_400) / 3_600;
    if d > 0 {
        format!("{d}d")
    } else if h > 0 {
        format!("{h}h")
    } else {
        format!("{}m", (secs % 3_600) / 60)
    }
}

/// Parse a `--min-size` value: base-1024 IEC like `500M`, `2G`, `1.5G`, or a
/// bare byte count. Suffixes K/M/G/T/P/E (case-insensitive).
pub fn parse_size(s: &str) -> Result<u64, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err(format!("invalid size: {s:?}"));
    }
    let (num, suffix) = split_numeric(s);
    let mult: u128 = match suffix.to_ascii_uppercase().as_str() {
        "" | "B" => 1,
        "K" | "KB" | "KIB" => 1024,
        "M" | "MB" | "MIB" => 1024u128.pow(2),
        "G" | "GB" | "GIB" => 1024u128.pow(3),
        "T" | "TB" | "TIB" => 1024u128.pow(4),
        "P" | "PB" | "PIB" => 1024u128.pow(5),
        "E" | "EB" | "EIB" => 1024u128.pow(6),
        _ => return Err(format!("invalid size unit in: {s:?}")),
    };
    let value: f64 = num.parse().map_err(|_| format!("invalid size: {s:?}"))?;
    if value < 0.0 || !value.is_finite() {
        return Err(format!("invalid size: {s:?}"));
    }
    Ok((value * mult as f64).round() as u64)
}

/// Parse a `--min-age` value: `30d`, `12h`, `2w`, `45m`, `90s`, or a bare
/// seconds count. Units s/m/h/d/w.
pub fn parse_age(s: &str) -> Result<u64, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err(format!("invalid age: {s:?}"));
    }
    let (num, suffix) = split_numeric(s);
    let factor: u64 = match suffix.to_ascii_lowercase().as_str() {
        "" | "s" => 1,
        "m" => 60,
        "h" => 3_600,
        "d" => 86_400,
        "w" => 604_800,
        _ => return Err(format!("invalid age unit in: {s:?}")),
    };
    let n: u64 = num.parse().map_err(|_| format!("invalid age: {s:?}"))?;
    n.checked_mul(factor)
        .ok_or_else(|| format!("age out of range: {s:?}"))
}

/// Split a string into its leading numeric part (digits and at most one dot)
/// and the trailing unit suffix.
fn split_numeric(s: &str) -> (&str, &str) {
    let end = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(s.len());
    (&s[..end], &s[end..])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn age_format() {
        assert_eq!(human_age(0), "0m");
        assert_eq!(human_age(59), "0m");
        assert_eq!(human_age(60), "1m");
        assert_eq!(human_age(3_599), "59m");
        assert_eq!(human_age(3_600), "1h");
        assert_eq!(human_age(86_399), "23h");
        assert_eq!(human_age(86_400), "1d");
        assert_eq!(human_age(505 * 86_400), "505d");
    }

    #[test]
    fn size_parse() {
        assert_eq!(parse_size("1024").unwrap(), 1024);
        assert_eq!(parse_size("500M").unwrap(), 500 * 1024 * 1024);
        assert_eq!(parse_size("2G").unwrap(), 2 * 1024 * 1024 * 1024);
        assert_eq!(
            parse_size("1.5G").unwrap(),
            (1.5 * 1024.0 * 1024.0 * 1024.0) as u64
        );
        assert_eq!(parse_size("0").unwrap(), 0);
        assert!(parse_size("abc").is_err());
        assert!(parse_size("1Q").is_err());
    }

    #[test]
    fn age_parse() {
        assert_eq!(parse_age("90").unwrap(), 90);
        assert_eq!(parse_age("30d").unwrap(), 30 * 86_400);
        assert_eq!(parse_age("12h").unwrap(), 12 * 3_600);
        assert_eq!(parse_age("2w").unwrap(), 2 * 604_800);
        assert_eq!(parse_age("45m").unwrap(), 45 * 60);
        assert!(parse_age("3x").is_err());
    }

    /// The cached/uncached byte-identity assertion shares this formatter, so it
    /// cannot catch rounding bugs — pin it against real `numfmt` instead.
    #[test]
    fn iec_matches_numfmt() {
        let probe = Command::new("numfmt")
            .args(["--to=iec", "--suffix=B", "0"])
            .output();
        if probe.is_err() {
            eprintln!("skipping: numfmt not on PATH");
            return;
        }

        let values: [u64; 24] = [
            0,
            1,
            96,
            1000,
            1023,
            1024,
            1025,
            1536,
            7987,
            9999,
            10_239,
            10_240,
            10_241,
            201_424,
            1_048_575,
            1_048_576,
            737_120_568,
            1_073_741_824,
            3_700_000_000,
            8_100_000_000,
            13_000_000_000,
            33_000_000_000,
            201_424_000_000,
            1_099_511_627_776,
        ];
        for v in values {
            let out = Command::new("numfmt")
                .args(["--to=iec", "--suffix=B", &v.to_string()])
                .output()
                .expect("run numfmt");
            let expected = String::from_utf8(out.stdout).unwrap().trim().to_string();
            assert_eq!(iec_size(v), expected, "mismatch for {v} bytes");
        }
    }
}
