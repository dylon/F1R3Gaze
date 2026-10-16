//! Validation shared by OS URL delivery paths.

/// Only addresses the browser can resolve may enter an existing window.
/// The URL parser normalizes the scheme; the fetch pipeline validates the
/// address itself. Custom scheme host case is preserved until site parsing.
pub(crate) fn accepted_url(raw: &str) -> Option<String> {
    let parsed = url::Url::parse(raw).ok()?;
    let canonical = parsed.as_str();
    match parsed.scheme() {
        "f1r3" if gaze_shard::SiteAddr::parse(canonical).is_some() => Some(canonical.to_string()),
        "f1r3h" if gaze_net::content_hash(canonical).is_some() => Some(canonical.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_valid_gaze_addresses() {
        let hash = format!("f1r3h://blake2b-256/{}", "a".repeat(64));
        assert_eq!(
            accepted_url("F1R3://ABCD/project"),
            Some("f1r3://ABCD/project".into())
        );
        assert_eq!(accepted_url(&hash), Some(hash));
        for raw in [
            "https://example.org",
            "f1r3://not-hex/project",
            "f1r3h://blake2b-256/bad",
        ] {
            assert_eq!(accepted_url(raw), None, "{raw}");
        }
    }
}
