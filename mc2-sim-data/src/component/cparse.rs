//! C-library text helpers used by the component loaders.
//!
//! `atoi` and `atof` read the longest numeric prefix and return zero when no
//! number starts the text, as the C library does. `strtok` drops empty cells,
//! which the master loader depends on (see [`strtok_fields`]).

/// Characters `isspace` accepts in the C locale.
fn is_c_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\x0b' | b'\x0c' | b'\r')
}

fn skip_space(text: &str) -> &[u8] {
    let bytes = text.as_bytes();
    let start = bytes
        .iter()
        .position(|&b| !is_c_space(b))
        .unwrap_or(bytes.len());
    &bytes[start..]
}

/// `atoi`: optional sign, then decimal digits. Saturates instead of overflowing.
pub(crate) fn atoi(text: &str) -> i64 {
    let bytes = skip_space(text);
    let (negative, digits) = match bytes.first() {
        Some(b'-') => (true, &bytes[1..]),
        Some(b'+') => (false, &bytes[1..]),
        _ => (false, bytes),
    };
    let mut value: i64 = 0;
    for &b in digits.iter().take_while(|b| b.is_ascii_digit()) {
        value = value.saturating_mul(10).saturating_add(i64::from(b - b'0'));
    }
    if negative {
        -value
    } else {
        value
    }
}

/// `atof`: optional sign, digits with an optional fraction, and an optional
/// exponent. Text without a number gives `0.0`.
pub(crate) fn atof(text: &str) -> f64 {
    let bytes = skip_space(text);
    let mut i = 0usize;
    if matches!(bytes.first(), Some(b'+' | b'-')) {
        i += 1;
    }
    let int_start = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    let mut digits = i - int_start;
    if i < bytes.len() && bytes[i] == b'.' {
        let frac_start = i + 1;
        let mut j = frac_start;
        while j < bytes.len() && bytes[j].is_ascii_digit() {
            j += 1;
        }
        digits += j - frac_start;
        if digits > 0 {
            i = j;
        }
    }
    if digits == 0 {
        return 0.0;
    }
    if i < bytes.len() && matches!(bytes[i], b'e' | b'E') {
        let mut j = i + 1;
        if j < bytes.len() && matches!(bytes[j], b'+' | b'-') {
            j += 1;
        }
        let exp_start = j;
        while j < bytes.len() && bytes[j].is_ascii_digit() {
            j += 1;
        }
        if j > exp_start {
            i = j;
        }
    }
    std::str::from_utf8(&bytes[..i])
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0)
}

/// The cells `strtok(line, ",")` yields: runs of commas count as one separator,
/// so empty cells disappear and later cells move left.
pub(crate) fn strtok_fields(line: &str) -> Vec<&str> {
    line.split(',').filter(|cell| !cell.is_empty()).collect()
}

/// Lines as `File::readLine` yields them: a line ends at CR or LF, and a CR LF
/// pair counts as one terminator. A trailing terminator adds no empty line.
pub(crate) fn split_lines(data: &[u8]) -> Vec<&[u8]> {
    let mut lines = Vec::new();
    let mut pos = 0usize;
    while pos < data.len() {
        let rest = &data[pos..];
        let end = rest
            .iter()
            .position(|&b| b == b'\r' || b == b'\n')
            .unwrap_or(rest.len());
        lines.push(&rest[..end]);
        pos += end;
        if pos < data.len() {
            let terminator = data[pos];
            pos += 1;
            if terminator == b'\r' && pos < data.len() && data[pos] == b'\n' {
                pos += 1;
            }
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atoi_reads_prefix_and_defaults_to_zero() {
        assert_eq!(atoi("42"), 42);
        assert_eq!(atoi("  -7"), -7);
        assert_eq!(atoi("+3x"), 3);
        assert_eq!(atoi("NA"), 0);
        assert_eq!(atoi(""), 0);
    }

    #[test]
    fn atof_reads_prefix_and_defaults_to_zero() {
        assert_eq!(atof("7.00"), 7.0);
        assert_eq!(atof("1.25"), 1.25);
        assert_eq!(atof(".5"), 0.5);
        assert_eq!(atof("3."), 3.0);
        assert_eq!(atof("-2e2"), -200.0);
        assert_eq!(atof("N/A"), 0.0);
        assert_eq!(atof("."), 0.0);
        assert_eq!(atof("-"), 0.0);
    }

    #[test]
    fn strtok_drops_empty_cells() {
        assert_eq!(strtok_fields("a,,b,,,c,"), vec!["a", "b", "c"]);
    }

    #[test]
    fn split_lines_handles_crlf_and_lf() {
        let lines = split_lines(b"one\r\ntwo\nthree\r");
        assert_eq!(lines, vec![&b"one"[..], &b"two"[..], &b"three"[..]]);
        assert_eq!(split_lines(b"a\r\n\r\nb\r\n").len(), 3);
    }
}
