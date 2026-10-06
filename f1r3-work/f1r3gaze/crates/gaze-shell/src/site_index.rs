//! `site-data/origins.json`: the sites that keep a store, as a JSON array of
//! strings. The Site data panel lists them. The index heals itself: a site
//! whose store opens is added again.

/// The index's bytes for `sites`: compact JSON, as the engine writes it.
pub fn index_bytes(sites: &[String]) -> Vec<u8> {
    serde_json::to_vec(sites).expect("an array of strings always serializes")
}

/// The sites an index names, if it is a JSON array of strings.
pub fn read_index(bytes: &[u8]) -> Result<Vec<String>, String> {
    serde_json::from_slice::<Vec<String>>(bytes).map_err(|e| e.to_string())
}

/// What can be kept of a damaged index: its string elements, in order, and
/// how many elements (or unreadable rests) were dropped.
///
/// A JSON array keeps its strings and drops every other element. Otherwise
/// the bytes are read as an array cut short or broken off: a byte-order
/// mark and white space are skipped, `[` must come first, and then each
/// string is read in turn until something that is not one; an unreadable
/// rest counts as one dropped element. Anything that is not an array keeps
/// nothing.
pub fn salvage_index(bytes: &[u8]) -> (Vec<String>, usize) {
    if let Ok(serde_json::Value::Array(items)) = serde_json::from_slice::<serde_json::Value>(bytes) {
        let total = items.len();
        let kept: Vec<String> = items
            .into_iter()
            .filter_map(|item| match item {
                serde_json::Value::String(site) => Some(site),
                _ => None,
            })
            .collect();
        let dropped = total - kept.len();
        return (kept, dropped);
    }
    let bytes = bytes.strip_prefix("\u{feff}".as_bytes()).unwrap_or(bytes);
    let skip = |at: &mut usize| {
        while bytes.get(*at).is_some_and(u8::is_ascii_whitespace) {
            *at += 1;
        }
    };
    let mut at = 0;
    skip(&mut at);
    if bytes.get(at) != Some(&b'[') {
        return (Vec::new(), usize::from(!bytes.iter().all(u8::is_ascii_whitespace)));
    }
    at += 1;
    let mut kept = Vec::new();
    loop {
        skip(&mut at);
        match bytes.get(at) {
            Some(b']') => return (kept, 0),
            Some(b'"') => {
                let mut strings = serde_json::Deserializer::from_slice(&bytes[at..]).into_iter::<String>();
                match strings.next() {
                    Some(Ok(site)) => {
                        at += strings.byte_offset();
                        kept.push(site);
                    }
                    _ => return (kept, 1),
                }
                skip(&mut at);
                match bytes.get(at) {
                    Some(b',') => at += 1,
                    // Cut short after a whole name: nothing unreadable is left.
                    Some(b']') | None => return (kept, 0),
                    Some(_) => return (kept, 1),
                }
            }
            None => return (kept, 0),
            Some(_) => return (kept, 1),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sites(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn an_index_round_trips() {
        let index = sites(&["https://a.example:443", "f1r3://ab/cd"]);
        assert_eq!(read_index(&index_bytes(&index)), Ok(index.clone()));
        assert_eq!(index_bytes(&index), br#"["https://a.example:443","f1r3://ab/cd"]"#);
        assert!(read_index(b"[1]").is_err());
        assert!(read_index(b"{}").is_err());
    }

    #[test]
    fn a_damaged_index_keeps_its_names() {
        let cases: [(&[u8], &[&str], usize); 9] = [
            (br#"["a", 7, "b", null]"#, &["a", "b"], 2),
            (br#"["a", "b""#, &["a", "b"], 0),
            (br#"["a", "b", "#, &["a", "b"], 0),
            (br#"["a", "b"#, &["a"], 1),
            (br#"["a" "b"]"#, &["a"], 1),
            (b"\xef\xbb\xbf [ \"a\" , \"b\" ] trailing", &["a", "b"], 0),
            (br#"{"a": "b"}"#, &[], 1),
            (b"garbage", &[], 1),
            (b"", &[], 0),
        ];
        for (bytes, kept, dropped) in cases {
            assert_eq!(salvage_index(bytes), (sites(kept), dropped), "{}", String::from_utf8_lossy(bytes));
        }
    }
}
