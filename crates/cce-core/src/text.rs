/// True for characters in the CJK ideograph, kana, hangul, and fullwidth
/// blocks.
///
/// These are the scripts whose words unicode61 indexes as single monolithic
/// tokens, which makes mixed-script queries behave differently from plain
/// Latin ones.
pub const fn has_cjk(character: char) -> bool {
    matches!(character,
        // Radicals, CJK punctuation, hiragana, katakana, bopomofo, strokes
        '\u{2E80}'..='\u{31EF}'
            // Unified ideographs plus extension A
            | '\u{3400}'..='\u{9FFF}'
            // Hangul syllables
            | '\u{AC00}'..='\u{D7AF}'
            // Compatibility ideographs, fullwidth forms
            | '\u{F900}'..='\u{FAFF}'
            | '\u{FF00}'..='\u{FFEF}'
            // Ideograph extensions B and beyond
            | '\u{20000}'..='\u{2EBEF}')
}

/// Splits an identifier into lowercase word terms.
///
/// camelCase humps, `snake_case` separators, and digit runs all produce
/// boundaries, so `setViewStatus`, `set_view_status`, and `HTTPServer`
/// yield searchable words. Sorted and deduplicated for deterministic
/// output.
#[must_use]
pub fn split_identifier_terms(name: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let mut current = String::new();
    let mut previous_lower = false;
    for character in name.chars() {
        if character.is_alphanumeric() {
            if previous_lower && character.is_uppercase() && !current.is_empty() {
                terms.push(current.to_ascii_lowercase());
                current.clear();
            }
            previous_lower = character.is_lowercase();
            current.push(character);
        } else if !current.is_empty() {
            terms.push(current.to_ascii_lowercase());
            current.clear();
            previous_lower = false;
        }
    }
    if !current.is_empty() {
        terms.push(current.to_ascii_lowercase());
    }
    terms.sort();
    terms.dedup();
    terms
}

/// The folded spelling of an identifier: alphanumeric characters
/// lowercased and concatenated (`set_view_status` → `setviewstatus`),
/// matching how a unicode61 tokenizer folds `setViewStatus`.
#[must_use]
pub fn folded_identifier(name: &str) -> String {
    name.chars()
        .filter(|character| character.is_alphanumeric())
        .collect::<String>()
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_query_filters;

    #[test]
    fn identifier_terms_split_camel_snake_and_acronyms() {
        assert_eq!(
            split_identifier_terms("setViewStatus"),
            ["set", "status", "view"]
        );
        assert_eq!(
            split_identifier_terms("set_view_status"),
            ["set", "status", "view"]
        );
        assert_eq!(split_identifier_terms("HTTPServer"), ["httpserver"]);
        assert_eq!(split_identifier_terms("name"), ["name"]);
    }

    #[test]
    fn folded_identifier_matches_unicode61_folding() {
        assert_eq!(folded_identifier("setViewStatus"), "setviewstatus");
        assert_eq!(folded_identifier("set_view_status"), "setviewstatus");
        assert_eq!(folded_identifier("plain"), "plain");
    }

    #[test]
    fn query_filters_extract_known_keys_only() {
        let (query, filters) =
            parse_query_filters("stale views lang:rust path:crates/cce-store http://x");
        assert_eq!(query, "stale views http://x");
        assert_eq!(filters.language.as_deref(), Some("rust"));
        assert_eq!(filters.path_prefix.as_deref(), Some("crates/cce-store"));
    }

    #[test]
    fn query_filters_ignore_bare_or_empty_colons() {
        let (query, filters) = parse_query_filters("a:b lang: path:");
        assert_eq!(query, "a:b lang: path:");
        assert!(filters.is_empty());
    }

    #[test]
    fn pat_filter_accepts_quoted_templates() {
        let (query, filters) =
            parse_query_filters("pat:'fn $F($$$A) { $$$B }' lang:rust auth flow");
        assert_eq!(query, "auth flow");
        assert_eq!(filters.pattern.as_deref(), Some("fn $F($$$A) { $$$B }"));
        assert_eq!(filters.language.as_deref(), Some("rust"));
    }

    #[test]
    fn pat_filter_supports_double_quotes_and_inner_colons() {
        let (query, filters) = parse_query_filters("pat:\"foo(:[x:1])\" bar");
        assert_eq!(query, "bar");
        assert_eq!(filters.pattern.as_deref(), Some("foo(:[x:1])"));
    }

    #[test]
    fn pat_filter_bare_value_runs_to_whitespace() {
        let (query, filters) = parse_query_filters("pat:unwrap() rest");
        assert_eq!(query, "rest");
        assert_eq!(filters.pattern.as_deref(), Some("unwrap()"));
    }

    #[test]
    fn unclosed_pat_quote_stays_verbatim() {
        let (query, filters) = parse_query_filters("pat:'fn $F( trailing");
        assert_eq!(query, "pat:'fn $F( trailing");
        assert!(filters.pattern.is_none());
    }

    #[test]
    fn quoted_unknown_key_stays_verbatim_and_whole() {
        let (query, filters) = parse_query_filters("repo:'my org/repo' fix");
        assert_eq!(query, "repo:'my org/repo' fix");
        assert!(filters.is_empty());
    }

    #[test]
    fn patterntype_structural_promotes_bare_text() {
        let (query, filters) =
            parse_query_filters("patterntype:structural fmt.Println($$$X) lang:go");
        assert_eq!(query, "");
        assert_eq!(filters.pattern.as_deref(), Some("fmt.Println($$$X)"));
        assert_eq!(filters.language.as_deref(), Some("go"));
    }

    #[test]
    fn patterntype_structural_loses_to_explicit_pat() {
        let (query, filters) =
            parse_query_filters("pat:'$A.lock()' patterntype:structural loose words");
        assert_eq!(query, "loose words");
        assert_eq!(filters.pattern.as_deref(), Some("$A.lock()"));
    }

    #[test]
    fn mid_token_quotes_are_inert() {
        let (query, filters) = parse_query_filters("don't stop lang:rust");
        assert_eq!(query, "don't stop");
        assert_eq!(filters.language.as_deref(), Some("rust"));
    }
}
