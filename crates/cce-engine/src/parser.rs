use cce_core::EntityKind;
use serde::{Deserialize, Serialize};
use tree_sitter::{Language, Node, Parser};

use crate::ScannedFile;

/// One extracted source unit (symbol) from a parsed file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParsedUnit {
    /// Entity kind of the unit.
    pub kind: EntityKind,
    /// Symbol name.
    pub name: String,
    /// Declaration signature when extractable.
    pub signature: Option<String>,
    /// Inclusive start byte.
    pub start_byte: usize,
    /// Exclusive end byte.
    pub end_byte: usize,
    /// 1-based start line.
    pub start_line: u32,
    /// 1-based end line.
    pub end_line: u32,
    /// Index of the enclosing unit, if nested.
    pub parent_unit: Option<usize>,
    /// Tree-sitter node kind for diagnostics.
    pub syntax_kind: String,
    /// Identifier spellings appearing in type positions inside this unit's
    /// range (sorted, deduplicated) — `fn f(x: &Token)` contributes `Token`.
    /// These are spellings for `References` edges, not resolved types.
    pub type_references: Vec<String>,
}

/// A call expression attributed to the innermost symbol that contains it.
/// `caller` indexes into `ParsedFile::units`; `None` means the call sits at
/// file scope (outside any extracted symbol).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParsedCall {
    /// Index of the calling unit; `None` = file scope.
    pub caller: Option<usize>,
    /// Callee name as written.
    pub name: String,
    /// Inclusive start byte of the call expression.
    pub start_byte: usize,
    /// Exclusive end byte of the call expression.
    pub end_byte: usize,
}

/// Everything extracted from one file: units, calls, and parse status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParsedFile {
    /// Extracted source units.
    pub units: Vec<ParsedUnit>,
    /// Call expressions with caller attribution.
    pub calls: Vec<ParsedCall>,
    /// Whether a parser handled this file.
    pub parsed: bool,
    /// Whether the parse reported syntax errors.
    pub has_syntax_errors: bool,
    /// Parser/language identifier used.
    pub parser: Option<String>,
}

/// Tree-sitter source parser for supported languages.
#[derive(Debug, Clone, Default)]
pub struct SourceParser;

impl SourceParser {
    /// Create a parser.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Whether a tree-sitter grammar exists for `language_name`.
    #[must_use]
    pub fn supports(language_name: &str) -> bool {
        language(language_name).is_some()
    }

    /// `source` must be the file's current bytes (verified against
    /// `file.content_hash` by the caller).
    pub fn parse(&self, file: &ScannedFile, source: &[u8]) -> ParsedFile {
        let Some(language_name) = file.language.as_deref() else {
            return unparsed();
        };
        let Some(language) = language(language_name) else {
            return unparsed();
        };
        let mut parser = Parser::new();
        if parser.set_language(&language).is_err() {
            return unparsed();
        }
        let Some(tree) = parser.parse(source, None) else {
            return unparsed();
        };
        let root = tree.root_node();
        let mut candidates = Vec::new();
        collect_candidates(root, language_name, source, &mut candidates);
        candidates.sort_by(|left, right| {
            left.start_byte
                .cmp(&right.start_byte)
                .then_with(|| right.end_byte.cmp(&left.end_byte))
        });
        assign_parents(&mut candidates);
        let mut calls = Vec::new();
        collect_calls(root, language_name, source, &mut calls);
        // Units are sorted by start byte, so the innermost enclosing unit is
        // the last one whose range covers the call site.
        for call in &mut calls {
            call.caller = candidates.iter().rposition(|unit| {
                unit.start_byte <= call.start_byte && unit.end_byte >= call.end_byte
            });
        }
        ParsedFile {
            units: candidates,
            calls,
            parsed: true,
            has_syntax_errors: root.has_error(),
            parser: Some(format!("tree-sitter-{language_name}")),
        }
    }
}

const fn unparsed() -> ParsedFile {
    ParsedFile {
        units: Vec::new(),
        calls: Vec::new(),
        parsed: false,
        has_syntax_errors: false,
        parser: None,
    }
}

fn is_call_node(language: &str, kind: &str) -> bool {
    match language {
        "rust" | "typescript" | "tsx" | "javascript" | "go" => kind == "call_expression",
        "python" => kind == "call",
        "java" => kind == "method_invocation" || kind == "object_creation_expression",
        "csharp" => kind == "invocation_expression" || kind == "object_creation_expression",
        _ => false,
    }
}

fn collect_calls(node: Node<'_>, language: &str, source: &[u8], output: &mut Vec<ParsedCall>) {
    let mut stack = vec![node];
    while let Some(current) = stack.pop() {
        if is_call_node(language, current.kind()) {
            let target = current
                .child_by_field_name("function")
                .or_else(|| current.child_by_field_name("name"))
                .or_else(|| current.child_by_field_name("type"))
                .or_else(|| current.child_by_field_name("constructor"));
            if let Some(name) = target.and_then(|node| call_target_name(node, source)) {
                if !name.is_empty() && name.chars().all(|c| c != ' ' && c != '(') {
                    output.push(ParsedCall {
                        caller: None,
                        name,
                        start_byte: current.start_byte(),
                        end_byte: current.end_byte(),
                    });
                }
            }
        }
        for index in (0..current.child_count())
            .rev()
            .filter_map(|value| u32::try_from(value).ok())
        {
            if let Some(child) = current.child(index) {
                if child.is_named() {
                    stack.push(child);
                }
            }
        }
    }
}

/// Resolve a callee expression to its terminal identifier: `foo()` → `foo`,
/// `a.b.c()` → `c`, `Type::method()` → `method`, `new Foo()` → `Foo`.
fn call_target_name(node: Node<'_>, source: &[u8]) -> Option<String> {
    match node.kind() {
        "identifier"
        | "field_identifier"
        | "property_identifier"
        | "type_identifier"
        | "attribute"
        | "shorthand_property_identifier" => node.utf8_text(source).ok().map(str::to_owned),
        "type_arguments" | "arguments" | "argument_list" => None,
        _ => (0..node.named_child_count())
            .rev()
            .filter_map(|index| u32::try_from(index).ok())
            .filter_map(|index| node.named_child(index))
            .find_map(|child| call_target_name(child, source)),
    }
}

/// Identifier spellings found in type positions under `node`: leaf
/// `type_identifier`s across grammars, the terminal name of scoped paths
/// (`a::b::Foo`, `a.b.Foo`), all identifiers inside Python `type`
/// annotations and class base lists, and C# `generic_name` members.
fn collect_type_references(
    node: Node<'_>,
    language: &str,
    source: &[u8],
    output: &mut Vec<String>,
) {
    let mut stack = vec![node];
    while let Some(current) = stack.pop() {
        let kind = current.kind();
        if kind == "type_identifier" {
            push_node_text(current, source, output);
            continue;
        }
        if matches!(
            kind,
            "scoped_type_identifier"
                | "nested_type_identifier"
                | "qualified_type"
                | "qualified_name"
        ) {
            if let Some(name) = (0..current.named_child_count())
                .rev()
                .filter_map(|index| u32::try_from(index).ok())
                .filter_map(|index| current.named_child(index))
                .find_map(|child| child.utf8_text(source).ok())
            {
                output.push(name.to_owned());
            }
            continue;
        }
        if kind == "generic_name" || (language == "python" && kind == "type") {
            collect_identifiers(current, source, output);
            continue;
        }
        if language == "python" && kind == "class_definition" {
            // Base classes sit in `superclasses`, outside `type` nodes.
            if let Some(bases) = current.child_by_field_name("superclasses") {
                collect_identifiers(bases, source, output);
            }
        }
        for index in (0..current.child_count())
            .rev()
            .filter_map(|value| u32::try_from(value).ok())
        {
            if let Some(child) = current.child(index) {
                if child.is_named() {
                    stack.push(child);
                }
            }
        }
    }
}

/// Every `identifier` descendant of `node` — used where a grammar does not
/// mark type names distinctly (Python annotations, C# generics).
fn collect_identifiers(node: Node<'_>, source: &[u8], output: &mut Vec<String>) {
    let mut stack = vec![node];
    while let Some(current) = stack.pop() {
        if current.kind() == "identifier" {
            push_node_text(current, source, output);
            continue;
        }
        for index in (0..current.child_count())
            .rev()
            .filter_map(|value| u32::try_from(value).ok())
        {
            if let Some(child) = current.child(index) {
                if child.is_named() {
                    stack.push(child);
                }
            }
        }
    }
}

fn push_node_text(node: Node<'_>, source: &[u8], output: &mut Vec<String>) {
    if let Ok(text) = node.utf8_text(source) {
        output.push(text.to_owned());
    }
}

fn language(name: &str) -> Option<Language> {
    match name {
        "rust" => Some(tree_sitter_rust::LANGUAGE.into()),
        "typescript" => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        "tsx" => Some(tree_sitter_typescript::LANGUAGE_TSX.into()),
        "javascript" => Some(tree_sitter_javascript::LANGUAGE.into()),
        "python" => Some(tree_sitter_python::LANGUAGE.into()),
        "go" => Some(tree_sitter_go::LANGUAGE.into()),
        "java" => Some(tree_sitter_java::LANGUAGE.into()),
        "csharp" => Some(tree_sitter_c_sharp::LANGUAGE.into()),
        _ => None,
    }
}

fn collect_candidates(node: Node<'_>, language: &str, source: &[u8], output: &mut Vec<ParsedUnit>) {
    let mut stack = vec![node];
    while let Some(current) = stack.pop() {
        if let Some(kind) = entity_kind(language, current.kind()) {
            if let Some(name) = node_name(current, source) {
                let signature = signature(current, source);
                let mut type_references = Vec::new();
                collect_type_references(current, language, source, &mut type_references);
                type_references.sort_unstable();
                type_references.dedup();
                output.push(ParsedUnit {
                    kind,
                    name,
                    signature,
                    start_byte: current.start_byte(),
                    end_byte: current.end_byte(),
                    start_line: current.start_position().row as u32 + 1,
                    end_line: current.end_position().row as u32 + 1,
                    parent_unit: None,
                    syntax_kind: current.kind().to_owned(),
                    type_references,
                });
            }
        }
        for index in (0..current.child_count())
            .rev()
            .filter_map(|value| u32::try_from(value).ok())
        {
            if let Some(child) = current.child(index) {
                if child.is_named() {
                    stack.push(child);
                }
            }
        }
    }
}

fn assign_parents(units: &mut [ParsedUnit]) {
    let mut ancestors: Vec<usize> = Vec::new();
    for index in 0..units.len() {
        while let Some(parent) = ancestors.last().copied() {
            let (Some(parent_end), Some(index_end)) = (
                units.get(parent).map(|unit| unit.end_byte),
                units.get(index).map(|unit| unit.end_byte),
            ) else {
                break;
            };
            if parent_end >= index_end {
                break;
            }
            ancestors.pop();
        }
        if let Some(unit) = units.get_mut(index) {
            unit.parent_unit = ancestors.last().copied();
        }
        ancestors.push(index);
    }
}

fn node_name(node: Node<'_>, source: &[u8]) -> Option<String> {
    let candidate = node.child_by_field_name("name").or_else(|| {
        node.child_by_field_name("declarator")
            .and_then(|value| value.child_by_field_name("name"))
    })?;
    candidate
        .utf8_text(source)
        .ok()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn signature(node: Node<'_>, source: &[u8]) -> Option<String> {
    let body_start = node.child_by_field_name("body").map_or_else(
        || node.end_byte().min(node.start_byte() + 512),
        |body| body.start_byte(),
    );
    let end = body_start
        .min(node.end_byte())
        .min(node.start_byte() + 2_048);
    let value = source
        .get(node.start_byte()..end)
        .and_then(|slice| std::str::from_utf8(slice).ok())?
        .trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn entity_kind(language: &str, kind: &str) -> Option<EntityKind> {
    let value = match (language, kind) {
        ("rust", "function_item") => EntityKind::Function,
        ("rust", "struct_item") => EntityKind::Struct,
        ("rust", "enum_item") => EntityKind::Enum,
        ("rust", "trait_item") => EntityKind::Trait,
        ("rust", "impl_item") => EntityKind::Module,
        ("rust", "mod_item") => EntityKind::Module,
        ("rust", "const_item" | "static_item") => EntityKind::Constant,
        (
            "typescript" | "tsx" | "javascript",
            "function_declaration" | "generator_function_declaration",
        ) => EntityKind::Function,
        ("typescript" | "tsx" | "javascript", "method_definition" | "method_signature") => {
            EntityKind::Method
        }
        ("typescript" | "tsx" | "javascript", "class_declaration") => EntityKind::Class,
        ("typescript" | "tsx", "interface_declaration") => EntityKind::Interface,
        ("typescript" | "tsx", "type_alias_declaration") => EntityKind::Schema,
        ("python", "function_definition") => EntityKind::Function,
        ("python", "class_definition") => EntityKind::Class,
        ("go", "function_declaration") => EntityKind::Function,
        ("go", "method_declaration") => EntityKind::Method,
        ("go", "type_spec") => EntityKind::Schema,
        ("java" | "csharp", "method_declaration" | "constructor_declaration") => EntityKind::Method,
        ("java" | "csharp", "class_declaration") => EntityKind::Class,
        ("java" | "csharp", "interface_declaration") => EntityKind::Interface,
        ("java" | "csharp", "enum_declaration") => EntityKind::Enum,
        ("csharp", "struct_declaration") => EntityKind::Struct,
        _ => return None,
    };
    Some(value)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn extracts_rust_units_and_parents() {
        let source = b"struct Store {}\nimpl Store { fn open() {} }\n".to_vec();
        let file = ScannedFile {
            relative_path: "src/store.rs".to_owned(),
            absolute_path: PathBuf::from("src/store.rs"),
            language: Some("rust".to_owned()),
            content_hash: blake3::hash(&source).to_hex().to_string(),
            size_bytes: source.len() as u64,
            mtime_ms: 0,
            line_count: 2,
            cached_bytes: Some(source.clone()),
        };
        let parsed = SourceParser::new().parse(&file, &source);
        assert!(parsed.parsed);
        assert!(parsed.units.iter().any(|unit| unit.name == "Store"));
        assert!(parsed.units.iter().any(|unit| unit.name == "open"));
    }
}
