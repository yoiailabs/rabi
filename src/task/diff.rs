//! Parse Git's NUL-delimited raw and numstat streams without decoding paths.
use super::classify::ChangedEntry;

pub(super) fn parse(raw: &[u8], numstat: &[u8]) -> Vec<ChangedEntry> {
    let mut entries = Vec::new();
    let mut fields = raw.split(|byte| *byte == 0).peekable();
    while let Some(header) = fields.next() {
        if header.is_empty() {
            if fields.peek().is_some() {
                entries.push(unknown_entry("unknown:raw"));
            }
            continue;
        }
        let parts: Vec<_> = header.split(|byte| *byte == b' ').collect();
        let mut entry = unknown_entry("unknown:raw");
        entry.path = fields.next().unwrap_or_default().to_vec();
        if parts.len() == 5 && parts[0].starts_with(b":") {
            let modes = mode(&parts[0][1..]).zip(mode(parts[1]));
            let status = std::str::from_utf8(parts[4]).ok();
            let valid_oid = |oid: &[u8]| !oid.is_empty() && oid.iter().all(u8::is_ascii_hexdigit);
            if let (Some((old, new)), Some(status)) = (modes, status) {
                entry.old_mode = old;
                entry.new_mode = new;
                entry.status = status.into();
                if valid_oid(parts[2]) && valid_oid(parts[3]) && !entry.path.is_empty() {
                    entry.unknown.clear();
                }
                if matches!(status.as_bytes().first(), Some(b'R' | b'C')) {
                    entry.old_path = Some(std::mem::take(&mut entry.path));
                    entry.path = fields.next().unwrap_or_default().to_vec();
                    if entry.path.is_empty() {
                        entry.unknown.push("unknown:raw".into());
                    }
                }
            }
        }
        entries.push(entry);
    }
    let mut matched = vec![false; entries.len()];
    let mut stats = numstat.split(|byte| *byte == 0).peekable();
    while let Some(header) = stats.next() {
        if header.is_empty() {
            if stats.peek().is_some() {
                entries.push(unknown_entry("unknown:numstat"));
            }
            continue;
        }
        let mut parts = header.splitn(3, |byte| *byte == b'\t');
        let added = parts.next().unwrap_or_default();
        let deleted = parts.next().unwrap_or_default();
        let path = parts.next();
        let (old, path) = match path {
            Some(b"") => (
                Some(stats.next().unwrap_or_default()),
                stats.next().unwrap_or_default(),
            ),
            Some(path) => (None, path),
            None => {
                entries.push(unknown_entry("unknown:numstat"));
                continue;
            }
        };
        let found = entries.iter().enumerate().position(|(i, entry)| {
            !matched.get(i).copied().unwrap_or(true)
                && entry.path == path
                && entry.old_path.as_deref() == old
        });
        let Some(index) = found else {
            let mut entry = unknown_entry("unknown:numstat");
            entry.path = path.to_vec();
            entry.old_path = old.map(<[u8]>::to_vec);
            entries.push(entry);
            continue;
        };
        matched[index] = true;
        let entry = &mut entries[index];
        if added == b"-" && deleted == b"-" {
            entry.binary = true;
        } else if let Some((added, deleted)) = number(added).zip(number(deleted)) {
            entry.added = added;
            entry.deleted = deleted;
        } else {
            entry.unknown.push("unknown:numstat".into());
        }
    }
    for (index, entry) in entries.iter_mut().enumerate() {
        if !matched.get(index).copied().unwrap_or(true) {
            entry.unknown.push("unknown:numstat".into());
        }
    }
    for (bytes, reason) in [(raw, "unknown:raw"), (numstat, "unknown:numstat")] {
        if !bytes.is_empty() && bytes.last() != Some(&0) {
            entries.push(unknown_entry(reason));
        }
    }
    entries
}

fn number(bytes: &[u8]) -> Option<u64> {
    let text = std::str::from_utf8(bytes).ok()?;
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

fn mode(bytes: &[u8]) -> Option<u32> {
    if bytes.len() != 6 || !bytes.iter().all(|byte| matches!(byte, b'0'..=b'7')) {
        return None;
    }
    u32::from_str_radix(std::str::from_utf8(bytes).ok()?, 8).ok()
}

fn unknown_entry(reason: &str) -> ChangedEntry {
    ChangedEntry {
        path: Vec::new(),
        old_path: None,
        status: "?".into(),
        old_mode: 0,
        new_mode: 0,
        added: 0,
        deleted: 0,
        binary: false,
        unknown: vec![reason.into()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nul_paths_and_rename_numstat_are_correlated_by_bytes() {
        let raw = b":100644 100644 aaaa bbbb R100\0old\tname\0new\nname\0:000000 100644 0000 cccc A\0binary\0";
        let stats = b"-\t-\tbinary\0"
            .iter()
            .chain(b"1\t2\t\0old\tname\0new\nname\0")
            .copied()
            .collect::<Vec<_>>();
        let entries = parse(raw, &stats);
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[0].old_path.as_deref(),
            Some(b"old\tname".as_slice())
        );
        assert_eq!(entries[0].path, b"new\nname");
        assert_eq!((entries[0].added, entries[0].deleted), (1, 2));
        assert!(entries[1].binary);
        assert!(entries.iter().all(|entry| entry.unknown.is_empty()));
    }

    #[test]
    fn malformed_or_missing_data_is_conservative() {
        for (raw, stats) in [
            (b"bad\0path\0".as_slice(), b"1\t0\tpath\0".as_slice()),
            (b":100644 100644 aaaa bbbb M\0path\0", b""),
            (b"", b"bad\0"),
            (b"\0", b""),
            (b"", b"\0"),
            (b":100644 100644 aaaa bbbb M\0path", b"1\t0\tpath"),
            (b":100644 100644 aaaa bbbb M\0path\0", b"x\t0\tpath\0"),
        ] {
            let entries = parse(raw, stats);
            assert!(!entries.is_empty());
            assert!(entries.iter().any(|entry| !entry.unknown.is_empty()));
        }
    }
}
