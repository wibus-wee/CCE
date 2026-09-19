use std::borrow::Cow;

use ast_grep_core::language::Language as AstLanguage;
use ast_grep_core::matcher::{Pattern, PatternBuilder, PatternError};
use ast_grep_core::tree_sitter::{LanguageExt, StrDoc, TSLanguage};
use regex::Regex;

/// Thin adapter from a vendored tree-sitter grammar to ast-grep's
/// `Language`/`LanguageExt` surface. One struct serves every entry in
/// the parser's `language()` table: `kind_to_id`/`field_to_id` delegate
/// to the underlying grammar. `expando` is the runtime metavariable
/// sigil for grammars that cannot lex `$IDENT` — same mechanism as
/// ast-grep-language's `impl_lang_expando!` (`µ` for rust/go/python/
/// csharp; `$` passthrough for java and the JS/TS family).
#[derive(Clone)]
pub(crate) struct CceLang {
    inner: TSLanguage,
    name: String,
    expando: char,
}

impl CceLang {
    pub(crate) fn for_name(name: &str) -> Option<Self> {
        let canonical = name.to_ascii_lowercase();
        let inner = crate::parser::language(&canonical)?;
        let expando = match canonical.as_str() {
            "rust" | "python" | "go" | "csharp" => 'µ',
            _ => '$',
        };
        Some(Self {
            inner,
            name: canonical,
            expando,
        })
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }
}

impl AstLanguage for CceLang {
    fn kind_to_id(&self, kind: &str) -> u16 {
        self.inner.id_for_node_kind(kind, true)
    }

    fn field_to_id(&self, field: &str) -> Option<u16> {
        self.inner
            .field_id_for_name(field)
            .map(std::num::NonZeroU16::get)
    }

    fn expando_char(&self) -> char {
        self.expando
    }

    fn pre_process_pattern<'q>(&self, query: &'q str) -> Cow<'q, str> {
        if self.expando == '$' {
            return Cow::Borrowed(query);
        }
        // Rewrite `$META`/`$$$` sigils to the expando char so the grammar
        // can lex them as identifiers; lone `$`/`$$` stay literal.
        let mut ret = String::with_capacity(query.len());
        let mut dollar_count = 0usize;
        for c in query.chars() {
            if c == '$' {
                dollar_count += 1;
                continue;
            }
            let need_replace = matches!(c, 'A'..='Z' | '_') || dollar_count == 3;
            let sigil = if need_replace { self.expando } else { '$' };
            ret.extend(std::iter::repeat_n(sigil, dollar_count));
            dollar_count = 0;
            ret.push(c);
        }
        let sigil = if dollar_count == 3 { self.expando } else { '$' };
        ret.extend(std::iter::repeat_n(sigil, dollar_count));
        Cow::Owned(ret)
    }

    fn build_pattern(&self, builder: &PatternBuilder<'_>) -> Result<Pattern, PatternError> {
        builder.build(|src| StrDoc::try_new(src, self.clone()))
    }
}

impl LanguageExt for CceLang {
    fn get_ts_language(&self) -> TSLanguage {
        self.inner.clone()
    }
}

/// Translates comby-style template syntax into ast-grep metavariable
/// spelling: `:[name]` → `$NAME`, `...` → `$$$`, existing `$X`/`$$$X`
/// pass through. Metavar names are normalized to the `[A-Z_][A-Z0-9_]*`
/// alphabet ast-grep requires.
fn translate_template(template: &str) -> String {
    let chars: Vec<char> = template.chars().collect();
    let mut out = String::with_capacity(template.len());
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        if c == ':' && chars.get(i + 1) == Some(&'[') {
            if let Some(close) = chars
                .get(i + 2..)
                .and_then(|rest| rest.iter().position(|&c| c == ']'))
            {
                let name: String = chars
                    .get(i + 2..i + 2 + close)
                    .unwrap_or_default()
                    .iter()
                    .map(|c| {
                        if c.is_ascii_alphanumeric() || *c == '_' {
                            c.to_ascii_uppercase()
                        } else {
                            '_'
                        }
                    })
                    .collect();
                out.push('$');
                if name.is_empty() || name.starts_with('_') {
                    out.push('_');
                }
                out.push_str(&name);
                i += 2 + close + 1;
                continue;
            }
        }
        if c == '.' && chars.get(i + 1) == Some(&'.') && chars.get(i + 2) == Some(&'.') {
            out.push_str("$$$");
            i += 3;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Compiles a `pat:` template into an ast-grep `Pattern` for `lang`.
///
/// Some grammars parse a bare expression ambiguously — Go's
/// `fmt.Println($A)` reads as `type_conversion_expression` outside a
/// statement context, while real call sites parse as `call_expression`.
/// When a bare compile lands on a language's documented ambiguous kind,
/// the template is re-compiled inside a statement context with a kind
/// selector (the upstream `pattern.context`/`pattern.selector` shape).
/// A template that fails to parse under both paths is an explicit
/// error — never a silent regex fallback.
pub(crate) fn compile_template(template: &str, lang: &CceLang) -> Result<Pattern, PatternError> {
    let translated = translate_template(template);
    let pattern = Pattern::try_new(&translated, lang.clone())?;
    // ast-grep tolerates ERROR-rooted parses, but a template that did
    // not parse cannot structurally match what the user wrote — report
    // it instead of letting fuzzy error-node matching degrade silently.
    if pattern
        .internal_kind()
        .is_some_and(ast_grep_core::matcher::kind_utils::is_error_kind)
    {
        return Err(PatternError::Parse(translated));
    }
    let ambiguous = match lang.name() {
        "go" => pattern
            .internal_kind()
            .is_some_and(|kind| kind == lang.kind_to_id("type_conversion_expression")),
        _ => false,
    };
    if !ambiguous {
        return Ok(pattern);
    }
    let context = format!("func _cce_pattern_ctx() {{\n{translated}\n}}");
    Ok(Pattern::contextual(&context, "call_expression", lang.clone()).unwrap_or(pattern))
}

trait PatternKind {
    fn internal_kind(&self) -> Option<u16>;
}

impl PatternKind for Pattern {
    fn internal_kind(&self) -> Option<u16> {
        match &self.node {
            ast_grep_core::matcher::PatternNode::Internal { kind_id, .. } => Some(*kind_id),
            _ => None,
        }
    }
}

/// Regex prefilter for candidate files: the template split on holes and
/// metavars, literal fragments regex-escaped and joined in order with
/// lazy any-spans, whitespace made elastic both ways. The superset
/// contract — every structural match's file passes the regex — is what
/// makes the funnel exact, so fragments emit `\s*` at every non-word
/// boundary rather than assuming source spacing matches the template.
pub(crate) struct RegexPrefilter {
    regex: Option<Regex>,
    /// Literal anchors extracted from the template, in order. Empty for
    /// hole-only templates — the degenerate full-enumeration path the
    /// caller reports instead of silently accepting.
    pub(crate) anchors: Vec<String>,
}

impl RegexPrefilter {
    pub(crate) fn is_file_candidate(&self, text: &str) -> bool {
        self.regex.as_ref().is_none_or(|regex| regex.is_match(text))
    }
}

pub(crate) fn literal_anchors(template: &str) -> RegexPrefilter {
    let chars: Vec<char> = template.chars().collect();
    let mut fragments: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        let hole_len = if c == ':' && chars.get(i + 1) == Some(&'[') {
            chars
                .get(i + 2..)
                .and_then(|rest| rest.iter().position(|&c| c == ']'))
                .map(|close| 2 + close + 1)
        } else if c == '.' && chars.get(i + 1) == Some(&'.') && chars.get(i + 2) == Some(&'.') {
            Some(3)
        } else if c == '$' {
            // `$$$` / `$$$NAME` / `$NAME` metavars.
            let run = chars
                .get(i..)
                .unwrap_or_default()
                .iter()
                .take_while(|&&c| c == '$')
                .count();
            let named = chars
                .get(i + run)
                .is_some_and(|c| c.is_ascii_alphabetic() || *c == '_');
            if run >= 3 || named {
                let name_len = chars
                    .get(i + run..)
                    .unwrap_or_default()
                    .iter()
                    .take_while(|&&c| c.is_ascii_alphanumeric() || c == '_')
                    .count();
                Some(run + name_len)
            } else {
                None
            }
        } else {
            None
        };
        if let Some(len) = hole_len {
            if !current.is_empty() {
                fragments.push(std::mem::take(&mut current));
            }
            i += len;
        } else {
            current.push(c);
            i += 1;
        }
    }
    if !current.is_empty() {
        fragments.push(current);
    }
    let anchored: Vec<String> = fragments
        .into_iter()
        .filter(|fragment| fragment.chars().any(char::is_alphanumeric))
        .collect();
    if anchored.is_empty() {
        return RegexPrefilter {
            regex: None,
            anchors: Vec::new(),
        };
    }
    let mut pattern = String::new();
    for (index, fragment) in anchored.iter().enumerate() {
        if index > 0 {
            pattern.push_str("[\\s\\S]*?");
        }
        for c in fragment.chars() {
            if !(c.is_alphanumeric() || c == '_') {
                pattern.push_str("\\s*");
            }
            pattern.push_str(&regex::escape(&c.to_string()));
        }
    }
    RegexPrefilter {
        regex: Regex::new(&pattern).ok(),
        anchors: anchored,
    }
}

/// Byte ranges of every structural match of `pattern` in `source`,
/// in source order.
pub(crate) fn match_ranges(source: &str, lang: &CceLang, pattern: &Pattern) -> Vec<(usize, usize)> {
    let grep = lang.ast_grep(source);
    grep.root()
        .find_all(pattern.clone())
        .map(|matched| {
            let range = matched.range();
            (range.start, range.end)
        })
        .collect()
}

/// 1-based line of a byte offset — the region-mapping input.
pub(crate) fn line_of_byte(source: &str, byte: usize) -> u32 {
    source[..byte.min(source.len())]
        .bytes()
        .filter(|b| *b == b'\n')
        .count() as u32
        + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hits(template: &str, lang_name: &str, source: &str) -> Vec<String> {
        let lang = CceLang::for_name(lang_name).expect("language");
        let pattern = compile_template(template, &lang).expect("pattern compiles");
        let grep = lang.ast_grep(source);
        grep.root()
            .find_all(pattern)
            .map(|m| m.text().to_string())
            .collect()
    }

    #[test]
    fn spike_rust_fn_pattern() {
        let source = "fn alpha(x: i32) {}\nfn beta() {}\nfn gamma(a: i32, b: i32) {}\n";
        let found = hits("fn $F($$$ARGS) { $$$BODY }", "rust", source);
        assert_eq!(
            found,
            vec![
                "fn alpha(x: i32) {}",
                "fn beta() {}",
                "fn gamma(a: i32, b: i32) {}"
            ]
        );
    }

    #[test]
    fn spike_go_call_pattern() {
        let source = "package main\n\nfunc main() {\n\tfmt.Println(\"hi\")\n\tfmt.Println(a, b)\n\tlog.Println(\"no\")\n}\n";
        let found = hits("fmt.Println($$$X)", "go", source);
        assert_eq!(found, vec!["fmt.Println(\"hi\")", "fmt.Println(a, b)"]);
    }

    #[test]
    fn translate_comby_spellings() {
        assert_eq!(translate_template("foo(:[x])"), "foo($X)");
        assert_eq!(translate_template("foo(...)"), "foo($$$)");
        assert_eq!(translate_template("if :[cond] { ... }"), "if $COND { $$$ }");
        assert_eq!(translate_template("$A.lock()"), "$A.lock()");
    }

    #[test]
    fn prefilter_matches_all_hit_files() {
        let prefilter = literal_anchors("fmt.Println($$$X)");
        assert_eq!(prefilter.anchors, vec!["fmt.Println("]);
        let source = "package main\nfunc main() {\n\tfmt.Println( \"hi\" )\n}\n";
        assert!(prefilter.is_file_candidate(source));
        assert!(!prefilter.is_file_candidate("package main\nfunc main() {}\n"));
    }

    #[test]
    fn prefilter_whitespace_is_elastic() {
        let prefilter = literal_anchors("foo($A)");
        assert!(prefilter.is_file_candidate("foo ( bar )"));
        assert!(prefilter.is_file_candidate("foo(bar)"));
    }

    #[test]
    fn prefilter_without_anchors_degenerates() {
        let prefilter = literal_anchors(":[x]");
        assert!(prefilter.anchors.is_empty());
        assert!(prefilter.is_file_candidate("anything at all"));
    }

    #[test]
    fn prefilter_preserves_anchor_order() {
        let prefilter = literal_anchors("if :[c] { ... } else { ... }");
        assert!(prefilter.is_file_candidate("if ready { go() } else { stop() }"));
        assert!(!prefilter.is_file_candidate("if ready { go() }"));
    }

    #[test]
    fn illegal_template_errors() {
        let lang = CceLang::for_name("rust").unwrap();
        assert!(compile_template("fn $F(", &lang).is_err());
    }
}
