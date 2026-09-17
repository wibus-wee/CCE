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
