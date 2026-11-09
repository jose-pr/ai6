//! The one thing the pinned path needs out of a certificate besides its hash: when it is
//! valid. Read by a small DER walk, and only ever run on bytes whose hash already matched a
//! pin, so a parser fault cannot be reached by an unpinned peer. Every failure refuses.

/// `(not_before, not_after)` in seconds since the Unix epoch.
pub(crate) fn validity(der: &[u8]) -> Result<(i64, i64), &'static str> {
    let (certificate, rest) = element(der, 0x30)?;
    if !rest.is_empty() {
        return Err("trailing bytes after the certificate");
    }
    let (tbs, _) = element(certificate, 0x30)?;
    let mut cursor = tbs;
    if cursor.first() == Some(&0xa0) {
        cursor = element(cursor, 0xa0)?.1;
    }
    cursor = element(cursor, 0x02)?.1; // serial number
    cursor = element(cursor, 0x30)?.1; // signature algorithm
    cursor = element(cursor, 0x30)?.1; // issuer
    let (validity, _) = element(cursor, 0x30)?;
    let (not_before, rest) = time(validity)?;
    let (not_after, _) = time(rest)?;
    Ok((not_before, not_after))
}

/// Splits one element with `tag` off the front: its content, and what follows it.
fn element(input: &[u8], tag: u8) -> Result<(&[u8], &[u8]), &'static str> {
    let (&first, rest) = input.split_first().ok_or("truncated")?;
    if first != tag {
        return Err("unexpected element");
    }
    let (&length, rest) = rest.split_first().ok_or("truncated")?;
    let (length, rest) = if length < 0x80 {
        (usize::from(length), rest)
    } else {
        let count = usize::from(length & 0x7f);
        if count == 0 || count > 4 || rest.len() < count {
            return Err("unsupported length");
        }
        let length = rest[..count]
            .iter()
            .fold(0usize, |length, byte| (length << 8) | usize::from(*byte));
        (length, &rest[count..])
    };
    if rest.len() < length {
        return Err("truncated");
    }
    Ok(rest.split_at(length))
}

fn time(input: &[u8]) -> Result<(i64, &[u8]), &'static str> {
    let (&tag, _) = input.split_first().ok_or("truncated")?;
    let (content, rest) = element(input, tag)?;
    let digits = |text: &[u8]| -> Result<i64, &'static str> {
        text.iter().try_fold(0i64, |value, byte| {
            byte.is_ascii_digit()
                .then(|| value * 10 + i64::from(byte - b'0'))
                .ok_or("malformed time")
        })
    };
    let (year, body) = match (tag, content) {
        (0x17, [y @ .., b'Z']) if y.len() == 12 => {
            let yy = digits(&y[..2])?;
            (if yy >= 50 { 1900 + yy } else { 2000 + yy }, &y[2..])
        }
        (0x18, [y @ .., b'Z']) if y.len() == 14 => (digits(&y[..4])?, &y[4..]),
        _ => return Err("malformed time"),
    };
    let (month, day) = (digits(&body[0..2])?, digits(&body[2..4])?);
    let (hour, minute, second) = (
        digits(&body[4..6])?,
        digits(&body[6..8])?,
        digits(&body[8..10])?,
    );
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return Err("malformed time");
    }
    let seconds = days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second;
    Ok((seconds, rest))
}

/// Days since 1970-01-01 of a proleptic Gregorian date.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let shifted_month = (month + 9) % 12;
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates_convert() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        assert_eq!(days_from_civil(1969, 12, 31), -1);
    }

    #[test]
    fn times_parse_in_both_forms() {
        let utc = [&[0x17, 13][..], b"250102030405Z"].concat();
        assert_eq!(
            time(&utc).unwrap().0,
            days_from_civil(2025, 1, 2) * 86_400 + 3 * 3600 + 4 * 60 + 5
        );
        let general = [&[0x18, 15][..], b"40960101000000Z"].concat();
        assert!(time(&general).unwrap().0 > time(&utc).unwrap().0);
        assert!(time(&[0x17, 3, b'1', b'2', b'Z']).is_err());
        assert!(time(&[]).is_err());
    }

    #[test]
    fn garbage_is_refused() {
        assert!(validity(&[]).is_err());
        assert!(validity(&[0x30, 0x02, 0x30, 0x00]).is_err());
        assert!(validity(&[0x30, 0x84, 0xff, 0xff, 0xff, 0xff]).is_err());
    }
}
