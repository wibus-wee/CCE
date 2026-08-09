use cce_core::EntityKind;
use serde::{Deserialize, Serialize};
use tree_sitter::{Language, Node, Parser};

use crate::ScannedFile;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParsedUnit {
    pub kind: EntityKind,
    pub name: String,
    pub signature: Option<String>,
    pub start_byte: usize,
    pub end_byte: usize,
    pub start_line: u32,
    pub end_line: u32,
    pub parent_unit: Option<usize>,
    pub syntax_kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParsedFile {
    pub units: Vec<ParsedUnit>,
    pub parsed: bool,
    pub has_syntax_errors: bool,
    pub parser: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct SourceParser;

impl SourceParser {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    #[must_use]
    pub fn supports(language_name: &str) -> bool {
        language(language_name).is_some()
    }

    pub fn parse(&self, file: &ScannedFile) -> ParsedFile {
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
        let Some(tree) = parser.parse(&file.bytes, None) else {
            return unparsed();
        };
        let root = tree.root_node();
        let mut candidates = Vec::new();
        collect_candidates(root, language_name, &file.bytes, &mut candidates);
        candidates.sort_by(|left, right| {
            left.start_byte
                .cmp(&right.start_byte)
                .then_with(|| right.end_byte.cmp(&left.end_byte))
        });
        assign_parents(&mut candidates);
        ParsedFile {
            units: candidates,
            parsed: true,
            has_syntax_errors: root.has_error(),
            parser: Some(format!("tree-sitter-{language_name}")),
        }
    }
}

fn unparsed() -> ParsedFile {
    ParsedFile {
        units: Vec::new(),
        parsed: false,
        has_syntax_errors: false,
        parser: None,
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
            if units[parent].end_byte >= units[index].end_byte {
                break;
            }
            ancestors.pop();
        }
        units[index].parent_unit = ancestors.last().copied();
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
    let value = std::str::from_utf8(&source[node.start_byte()..end])
        .ok()?
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
            line_count: 2,
            bytes: source,
        };
        let parsed = SourceParser::new().parse(&file);
        assert!(parsed.parsed);
        assert!(parsed.units.iter().any(|unit| unit.name == "Store"));
        assert!(parsed.units.iter().any(|unit| unit.name == "open"));
    }
}
