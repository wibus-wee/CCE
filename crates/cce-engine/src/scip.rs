//! SCIP index ingest. Parses a `index.scip` protobuf artifact (however it
//! was produced — a provider subprocess or a committed file) into
//! `References`/`Implements` relations at `RelationOrigin::Scip`, trust
//! level 1. SCIP carries definition/reference truth, not dataflow.
//!
//! Symbol identity stays opaque: occurrences and symbol info are joined on
//! the raw symbol string, then resolved to CCE entities by byte-range
//! containment — the smallest region enclosing an occurrence is its
//! enclosing symbol.

use std::collections::{HashMap, HashSet};

use cce_core::{CceError, Relation, RelationKind, RelationOrigin, Result, SourceAddress};
use protobuf::Message;
use scip::types::occurrence::Typed_range;
use scip::types::{Index, Occurrence, TextEncoding};

use crate::engine::relation_id;

/// Index-side inputs the ingest needs: byte ranges of known entities per
/// path (ascending by span, smallest last = file entity) and the file text
/// for line/column → byte conversion.
pub(crate) struct ScipContext<'a> {
    pub repository_id: &'a str,
    pub snapshot_id: &'a str,
    pub ranges: &'a HashMap<String, Vec<(usize, usize, String)>>,
    pub texts: &'a HashMap<String, String>,
}

#[derive(Debug, Default)]
pub(crate) struct ScipIngest {
    /// Deduped relations, ready to merge into `SnapshotRecords`.
    pub relations: Vec<Relation>,
    /// Documents whose `relative_path` matched an indexed file.
    pub documents: usize,
    /// Documents in the artifact that did not match this snapshot.
    pub foreign_documents: usize,
    /// Resolved definition occurrences.
    pub definitions: usize,
    /// Symbols referenced but never defined in this index (external deps).
    pub external_symbols: usize,
    /// Producing tool, e.g. `rust-analyzer 0.3.2591`.
    pub tool: String,
}

const DEFINITION_ROLE: i32 = 1; // scip.SymbolRole.Definition bit
const MAX_EVIDENCE_PER_EDGE: usize = 8;

/// SCIP symbol identity. `local <id>` symbols are unique only within
/// their own Document — the same spelling in another file is a different
/// symbol, so keying them globally would fabricate cross-file edges at
/// confidence 1.0. Everything else is a global descriptor and resolves
/// across documents.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum SymbolKey {
    Global(String),
    Local { path: String, symbol: String },
}

impl SymbolKey {
    /// The only constructor — every lookup in definitions, references
    /// and relationships goes through it, so the three resolution paths
    /// can never disagree about scope. Only the exact SCIP spelling
    /// `local <id>` scopes to a document; no substring guessing.
    fn of(symbol: &str, document_path: &str) -> Self {
        symbol.strip_prefix("local ").map_or_else(
            || Self::Global(symbol.to_owned()),
            |rest| Self::Local {
                path: document_path.to_owned(),
                symbol: rest.trim().to_owned(),
            },
        )
    }
}

/// One non-definition occurrence awaiting symbol resolution.
struct ReferenceSite {
    path: String,
    range: (u32, u32, u32, u32),
    bytes: (usize, usize),
    /// Entity enclosing the reference site — the referrer.
    referencer: String,
}

/// Parse and map a SCIP artifact. Malformed input returns `Err`, which the
/// caller records as a `Failed` provider report — it never aborts indexing.
pub(crate) fn ingest(bytes: &[u8], context: &ScipContext<'_>) -> Result<ScipIngest> {
    let index = Index::parse_from_bytes(bytes)
        .map_err(|error| CceError::Configuration(format!("invalid SCIP index: {error}")))?;
    let encoding = index
        .metadata
        .text_document_encoding
        .enum_value()
        .unwrap_or(TextEncoding::UTF8);
    let tool = index.metadata.tool_info.as_ref().map_or_else(
        || "unknown".to_owned(),
        |info| {
            if info.version.is_empty() {
                info.name.clone()
            } else {
                format!("{} {}", info.name, info.version)
            }
        },
    );
    let mut outcome = ScipIngest {
        tool,
        ..ScipIngest::default()
    };

    // Pass 1: definitions (symbol → defining entity + site) and raw
    // reference sites. Resolution needs the complete definition map, so
    // references and relationships emit afterwards.
    let mut definitions: HashMap<SymbolKey, (String, SourceAddress)> = HashMap::new();
    let mut sites: Vec<(String, ReferenceSite)> = Vec::new();
    for document in &index.documents {
        let path = normalize_path(&document.relative_path);
        let (Some(ranges), Some(text)) = (context.ranges.get(&path), context.texts.get(&path))
        else {
            outcome.foreign_documents += 1;
            continue;
        };
        outcome.documents += 1;
        let offsets = line_offsets(text);
        for occurrence in &document.occurrences {
            if occurrence.symbol.is_empty() {
                continue;
            }
            let Some(range) = occurrence_range(occurrence) else {
                continue;
            };
            let Some(bytes) = byte_range(text, &offsets, range, encoding) else {
                continue;
            };
            let Some(entity) = enclosing_entity(ranges, bytes.0) else {
                continue;
            };
            if occurrence.symbol_roles & DEFINITION_ROLE != 0 {
                if let Some(address) = site_address(context, &path, range, bytes, &entity) {
                    definitions
                        .entry(SymbolKey::of(&occurrence.symbol, &path))
                        .or_insert((entity, address));
                    outcome.definitions += 1;
                }
            } else {
                sites.push((
                    occurrence.symbol.clone(),
                    ReferenceSite {
                        path: path.clone(),
                        range,
                        bytes,
                        referencer: entity,
                    },
                ));
            }
        }
    }

    // Pass 2: references resolve through the definition map. Unresolved
    // symbols are external — counted, never fabricated into entities.
    let mut external: HashSet<SymbolKey> = HashSet::new();
    let mut evidence: HashMap<(String, String), Vec<SourceAddress>> = HashMap::new();
    let mut order: Vec<(String, String)> = Vec::new();
    for (symbol, site) in sites {
        let Some(target) = definitions.get(&SymbolKey::of(&symbol, &site.path)) else {
            external.insert(SymbolKey::of(&symbol, &site.path));
            continue;
        };
        let target_entity = &target.0;
        if *target_entity == site.referencer {
            continue; // self-edge from a definition site that lacked the bit
        }
        let key = (site.referencer.clone(), target_entity.clone());
        let bucket = evidence.entry(key.clone()).or_insert_with(|| {
            order.push(key.clone());
            Vec::new()
        });
        if bucket.len() < MAX_EVIDENCE_PER_EDGE {
            if let Some(address) =
                site_address(context, &site.path, site.range, site.bytes, target_entity)
            {
                bucket.push(address);
            }
        }
    }
    outcome.external_symbols = external.len();

    // Pass 3: SymbolInformation.relationships — def-to-def facts such as
    // `S implements T`. Both sides must resolve to ingested definitions.
    let extractor = format!("scip:{}", outcome.tool);
    for document in &index.documents {
        // SymbolInformation lives on the document that owns the symbol —
        // locals on either side of a relationship resolve inside it.
        let document_path = normalize_path(&document.relative_path);
        for info in &document.symbols {
            let Some((source_entity, source_site)) =
                definitions.get(&SymbolKey::of(&info.symbol, &document_path))
            else {
                continue;
            };
            for relationship in &info.relationships {
                let Some((target_entity, _)) =
                    definitions.get(&SymbolKey::of(&relationship.symbol, &document_path))
                else {
                    continue;
                };
                let kind = if relationship.is_implementation {
                    RelationKind::Implements
                } else if relationship.is_type_definition {
                    RelationKind::TypeUses
                } else if relationship.is_reference {
                    RelationKind::References
                } else {
                    continue;
                };
                outcome.relations.push(Relation {
                    id: relation_id(source_entity, target_entity, &kind_name(&kind)),
                    source_entity_id: source_entity.clone(),
                    target_entity_id: target_entity.clone(),
                    kind,
                    origin: RelationOrigin::Scip,
                    confidence: 1.0,
                    snapshot_id: context.snapshot_id.to_owned(),
                    extractor: extractor.clone(),
                    evidence: vec![source_site.clone()],
                    attributes: serde_json::Map::new(),
                });
            }
        }
    }
    for key in order {
        let (source, target) = key.clone();
        outcome.relations.push(Relation {
            id: relation_id(&source, &target, "references"),
            source_entity_id: source,
            target_entity_id: target,
            kind: RelationKind::References,
            origin: RelationOrigin::Scip,
            confidence: 1.0,
            snapshot_id: context.snapshot_id.to_owned(),
            extractor: extractor.clone(),
            evidence: evidence.remove(&key).unwrap_or_default(),
            attributes: serde_json::Map::new(),
        });
    }
    Ok(outcome)
}

/// The discriminator embedded in `relation_id`: the kind's `snake_case` wire
/// name, matching how existing edges are keyed (`"references"`, …).
fn kind_name(kind: &RelationKind) -> String {
    serde_json::to_value(kind)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "other".to_owned())
}

fn normalize_path(path: &str) -> String {
    path.trim_start_matches("./").replace('\\', "/")
}

/// `(start_line, start_col, end_line, end_col)` — 0-based. `typed_range`
/// takes precedence over the deprecated packed `range`; in the packed form,
/// 3 elements mean `[line, start_col, end_col]` (single line), 4 elements
/// mean `[start_line, start_col, end_line, end_col]`.
fn occurrence_range(occurrence: &Occurrence) -> Option<(u32, u32, u32, u32)> {
    let positive = |value: i32| u32::try_from(value).ok();
    match occurrence.typed_range.as_ref() {
        Some(Typed_range::SingleLineRange(range)) => {
            return Some((
                positive(range.line)?,
                positive(range.start_character)?,
                positive(range.line)?,
                positive(range.end_character)?,
            ));
        }
        Some(Typed_range::MultiLineRange(range)) => {
            return Some((
                positive(range.start_line)?,
                positive(range.start_character)?,
                positive(range.end_line)?,
                positive(range.end_character)?,
            ));
        }
        _ => {}
    }
    match occurrence.range.as_slice() {
        [line, start, end] => Some((
            positive(*line)?,
            positive(*start)?,
            positive(*line)?,
            positive(*end)?,
        )),
        [sl, sc, el, ec] => Some((
            positive(*sl)?,
            positive(*sc)?,
            positive(*el)?,
            positive(*ec)?,
        )),
        _ => None,
    }
}

/// Byte offset of each line's first byte.
fn line_offsets(text: &str) -> Vec<usize> {
    let mut offsets = vec![0];
    for (index, byte) in text.bytes().enumerate() {
        if byte == b'\n' {
            offsets.push(index + 1);
        }
    }
    offsets
}

/// Convert a 0-based line/column range to byte offsets. UTF-8 columns are
/// bytes within the line; UTF-16 columns are counted in code units.
fn byte_range(
    text: &str,
    line_offsets: &[usize],
    range: (u32, u32, u32, u32),
    encoding: TextEncoding,
) -> Option<(usize, usize)> {
    let (sl, sc, el, ec) = (range.0 as usize, range.1, range.2 as usize, range.3);
    let start = column_to_byte(text, *line_offsets.get(sl)?, sc, encoding)?;
    let end = column_to_byte(text, *line_offsets.get(el)?, ec, encoding)?;
    (end >= start).then_some((start, end))
}

fn column_to_byte(
    text: &str,
    line_start: usize,
    column: u32,
    encoding: TextEncoding,
) -> Option<usize> {
    let line_end = text[line_start..]
        .find('\n')
        .map_or(text.len(), |offset| line_start + offset);
    if encoding == TextEncoding::UTF16 {
        let mut units = 0_u32;
        for (offset, ch) in text[line_start..line_end].char_indices() {
            if units >= column {
                return Some(line_start + offset);
            }
            units += ch.len_utf16() as u32;
        }
        (units >= column).then_some(line_end)
    } else {
        // UTF-8 (and unspecified): columns are byte offsets in the line.
        let byte = line_start.checked_add(column as usize)?;
        (byte <= line_end).then_some(byte)
    }
}

/// Smallest region containing `offset` — ranges are ascending by span, so
/// the first containing hit is the innermost enclosing symbol; the final
/// entry is the whole-file entity.
fn enclosing_entity(ranges: &[(usize, usize, String)], offset: usize) -> Option<String> {
    ranges
        .iter()
        .find(|(start, end, _)| offset >= *start && offset < *end)
        .map(|(_, _, entity)| entity.clone())
}

fn site_address(
    context: &ScipContext<'_>,
    path: &str,
    range: (u32, u32, u32, u32),
    bytes: (usize, usize),
    symbol_entity: &str,
) -> Option<SourceAddress> {
    SourceAddress::new(
        context.repository_id,
        context.snapshot_id,
        path,
        bytes.0 as u64..bytes.1 as u64,
        range.0 + 1..=range.2 + 1,
    )
    .ok()
    .map(|address| address.with_symbol(symbol_entity))
}

#[cfg(test)]
mod tests {
    use super::*;
    use protobuf::{EnumOrUnknown, MessageField};
    use scip::types::symbol_information::Kind as SymbolKind;
    use scip::types::{Document, Metadata, SingleLineRange, SymbolInformation, ToolInfo};

    fn fixture_index(encoding: TextEncoding) -> Vec<u8> {
        let mut tool = ToolInfo::new();
        tool.name = "fixture-analyzer".to_owned();
        tool.version = "1.0.0".to_owned();
        let mut metadata = Metadata::new();
        metadata.tool_info = MessageField::some(tool);
        metadata.text_document_encoding = EnumOrUnknown::new(encoding);
        let mut index = Index::new();
        index.metadata = MessageField::some(metadata);

        // "pub fn helper() -> bool {\n    true\n}\npub fn caller() -> bool {\n    helper()\n}\n"
        // helper name at 0:7..13; helper() call at 3:4..10 inside caller.
        let mut definition = Occurrence::new();
        definition.range = vec![0, 7, 0, 13];
        definition.symbol = "fixture . helper().".to_owned();
        definition.symbol_roles = DEFINITION_ROLE;
        let mut reference = Occurrence::new();
        reference.range = vec![3, 4, 3, 10];
        reference.symbol = "fixture . helper().".to_owned();
        reference.symbol_roles = 8; // ReadAccess
        let mut external = Occurrence::new();
        external.range = vec![3, 12, 3, 16];
        external.symbol = "external . nowhere().".to_owned();
        external.symbol_roles = 8;
        let mut document = Document::new();
        document.relative_path = "src/lib.rs".to_owned();
        document.language = "rust".to_owned();
        document.occurrences = vec![definition, reference, external];

        let mut info = SymbolInformation::new();
        info.symbol = "fixture . helper().".to_owned();
        info.kind = EnumOrUnknown::new(SymbolKind::Function);
        document.symbols = vec![info];

        index.documents = vec![document];
        index.write_to_bytes().expect("serialize fixture index")
    }

    fn fixture_context<'a>(
        ranges: &'a HashMap<String, Vec<(usize, usize, String)>>,
        texts: &'a HashMap<String, String>,
    ) -> ScipContext<'a> {
        ScipContext {
            repository_id: "repo",
            snapshot_id: "snap",
            ranges,
            texts,
        }
    }

    #[test]
    fn ingest_maps_references_to_enclosing_entities() {
        let text =
            "pub fn helper() -> bool {\n    true\n}\npub fn caller() -> bool {\n    helper()\n}\n";
        let texts = HashMap::from([("src/lib.rs".to_owned(), text.to_owned())]);
        // helper unit 0..37, caller unit 38..76, file entity last.
        let ranges = HashMap::from([(
            "src/lib.rs".to_owned(),
            vec![
                (0, 37, "e-helper".to_owned()),
                (38, 76, "e-caller".to_owned()),
                (0, text.len(), "e-file".to_owned()),
            ],
        )]);
        let context = fixture_context(&ranges, &texts);
        let outcome = ingest(&fixture_index(TextEncoding::UTF8), &context).expect("ingest");
        assert_eq!(outcome.documents, 1);
        assert_eq!(outcome.definitions, 1);
        assert_eq!(outcome.external_symbols, 1);
        assert_eq!(outcome.relations.len(), 1);
        let edge = &outcome.relations[0];
        assert_eq!(edge.source_entity_id, "e-caller");
        assert_eq!(edge.target_entity_id, "e-helper");
        assert_eq!(edge.origin, RelationOrigin::Scip);
        assert!((edge.confidence - 1.0).abs() < f32::EPSILON);
        assert_eq!(edge.kind, RelationKind::References);
        assert!(!edge.evidence.is_empty());
        assert_eq!(outcome.tool, "fixture-analyzer 1.0.0");
    }

    /// Two documents each define `local 0` on a different entity. Local
    /// identity is document-scoped: refs must land on their own file's
    /// entity, never borrow the other file's definition. A shared global
    /// symbol still resolves across files, and `SymbolInformation`
    /// relationships honor the same scope on both sides.
    #[test]
    fn local_symbols_resolve_within_their_document() {
        use scip::types::Relationship;

        let mut index = Index::new();
        let mut metadata = Metadata::new();
        let mut tool = ToolInfo::new();
        tool.name = "fixture-analyzer".to_owned();
        tool.version = "1.0.0".to_owned();
        metadata.tool_info = MessageField::some(tool);
        metadata.text_document_encoding = EnumOrUnknown::new(TextEncoding::UTF8);
        index.metadata = MessageField::some(metadata);

        let occurrence = |range: &[i32], symbol: &str, roles: i32| {
            let mut occurrence = Occurrence::new();
            occurrence.range = range.to_vec();
            occurrence.symbol = symbol.to_owned();
            occurrence.symbol_roles = roles;
            occurrence
        };

        // src/a.rs: "fn alpha() {}\nfn use_a() {\n    alpha()\n}\nfn shared() {}\n"
        let mut doc_a = Document::new();
        doc_a.relative_path = "src/a.rs".to_owned();
        doc_a.occurrences = vec![
            occurrence(&[0, 3, 0, 8], "local 0", DEFINITION_ROLE), // alpha
            occurrence(&[2, 4, 2, 9], "local 0", 8),               // alpha() in use_a
            occurrence(&[4, 3, 4, 9], "shared . shared().", DEFINITION_ROLE),
        ];
        let mut info_a = SymbolInformation::new();
        info_a.symbol = "local 0".to_owned();
        let mut rel_a = Relationship::new();
        rel_a.symbol = "shared . shared().".to_owned();
        rel_a.is_reference = true;
        info_a.relationships = vec![rel_a];
        doc_a.symbols = vec![info_a];

        // src/b.rs: "fn beta() {}\nfn use_b() {\n    beta()\n    shared()\n}\n"
        let mut doc_b = Document::new();
        doc_b.relative_path = "src/b.rs".to_owned();
        doc_b.occurrences = vec![
            occurrence(&[0, 3, 0, 7], "local 0", DEFINITION_ROLE), // beta
            occurrence(&[2, 4, 2, 8], "local 0", 8),               // beta() in use_b
            occurrence(&[3, 4, 3, 10], "shared . shared().", 8),   // shared() in use_b
        ];
        let mut info_b = SymbolInformation::new();
        info_b.symbol = "local 0".to_owned();
        let mut rel_b = Relationship::new();
        rel_b.symbol = "shared . shared().".to_owned();
        rel_b.is_reference = true;
        info_b.relationships = vec![rel_b];
        doc_b.symbols = vec![info_b];

        index.documents = vec![doc_a, doc_b];
        let bytes = index.write_to_bytes().expect("serialize fixture index");

        let text_a = "fn alpha() {}\nfn use_a() {\n    alpha()\n}\nfn shared() {}\n";
        let text_b = "fn beta() {}\nfn use_b() {\n    beta()\n    shared()\n}\n";
        let texts = HashMap::from([
            ("src/a.rs".to_owned(), text_a.to_owned()),
            ("src/b.rs".to_owned(), text_b.to_owned()),
        ]);
        let ranges = HashMap::from([
            (
                "src/a.rs".to_owned(),
                vec![
                    (0, 13, "e-alpha".to_owned()),
                    (14, 38, "e-use-a".to_owned()),
                    (39, 52, "e-shared".to_owned()),
                    (0, text_a.len(), "e-file-a".to_owned()),
                ],
            ),
            (
                "src/b.rs".to_owned(),
                vec![
                    (0, 12, "e-beta".to_owned()),
                    (13, 47, "e-use-b".to_owned()),
                    (0, text_b.len(), "e-file-b".to_owned()),
                ],
            ),
        ]);
        let context = fixture_context(&ranges, &texts);
        let outcome = ingest(&bytes, &context).expect("ingest");

        let edges: Vec<(String, String)> = outcome
            .relations
            .iter()
            .map(|r| (r.source_entity_id.clone(), r.target_entity_id.clone()))
            .collect();
        // Local refs land on the same document's entity — never phantom
        // cross-file edges at scip confidence 1.0.
        assert!(edges.contains(&("e-use-a".to_owned(), "e-alpha".to_owned())));
        assert!(edges.contains(&("e-use-b".to_owned(), "e-beta".to_owned())));
        assert!(!edges.contains(&("e-use-b".to_owned(), "e-alpha".to_owned())));
        assert!(!edges.contains(&("e-use-a".to_owned(), "e-beta".to_owned())));
        // Globals still resolve across documents.
        assert!(edges.contains(&("e-use-b".to_owned(), "e-shared".to_owned())));
        // Relationships scope both sides to their owning document: doc a's
        // local 0 is alpha, doc b's is beta.
        assert!(edges.contains(&("e-alpha".to_owned(), "e-shared".to_owned())));
        assert!(edges.contains(&("e-beta".to_owned(), "e-shared".to_owned())));
        assert_eq!(outcome.definitions, 3);
        assert_eq!(outcome.external_symbols, 0);
    }

    /// A local symbol with no definition in its own document is external —
    /// it must not borrow another document's `local 0` just because the
    /// spelling matches.
    #[test]
    fn undefined_local_stays_external() {
        let mut index = Index::new();
        let mut defined = Document::new();
        defined.relative_path = "src/has_def.rs".to_owned();
        let mut definition = Occurrence::new();
        definition.range = vec![0, 3, 0, 8];
        definition.symbol = "local 0".to_owned();
        definition.symbol_roles = DEFINITION_ROLE;
        defined.occurrences = vec![definition];
        let mut bare = Document::new();
        bare.relative_path = "src/only_ref.rs".to_owned();
        let mut reference = Occurrence::new();
        reference.range = vec![1, 4, 1, 8];
        reference.symbol = "local 0".to_owned();
        reference.symbol_roles = 8;
        bare.occurrences = vec![reference];
        index.documents = vec![defined, bare];
        let bytes = index.write_to_bytes().expect("serialize");

        let text_def = "fn alpha() {}\n";
        let text_ref = "fn f() {\n    beta()\n}\n";
        let texts = HashMap::from([
            ("src/has_def.rs".to_owned(), text_def.to_owned()),
            ("src/only_ref.rs".to_owned(), text_ref.to_owned()),
        ]);
        let ranges = HashMap::from([
            (
                "src/has_def.rs".to_owned(),
                vec![
                    (0, 13, "e-alpha".to_owned()),
                    (0, text_def.len(), "e-file-def".to_owned()),
                ],
            ),
            (
                "src/only_ref.rs".to_owned(),
                vec![
                    (0, 9, "e-f".to_owned()),
                    (0, text_ref.len(), "e-file-ref".to_owned()),
                ],
            ),
        ]);
        let context = fixture_context(&ranges, &texts);
        let outcome = ingest(&bytes, &context).expect("ingest");
        assert_eq!(outcome.definitions, 1);
        assert_eq!(outcome.external_symbols, 1);
        assert!(outcome.relations.is_empty());
    }

    #[test]
    fn utf16_columns_convert_through_multibyte_chars() {
        // "hé" = 2 utf16 units but 3 utf8 bytes.
        let text = "héllo wörld\n";
        let offsets = line_offsets(text);
        // utf16 col 6 = the 'w'; its utf8 byte offset is 7.
        let bytes = byte_range(text, &offsets, (0, 6, 0, 7), TextEncoding::UTF16).unwrap();
        assert_eq!(bytes, (7, 8));
        let utf8 = byte_range(text, &offsets, (0, 7, 0, 8), TextEncoding::UTF8).unwrap();
        assert_eq!(utf8, (7, 8));
    }

    #[test]
    fn foreign_documents_are_counted_not_fatal() {
        let mut index = Index::new();
        let mut document = Document::new();
        document.relative_path = "elsewhere.rs".to_owned();
        index.documents = vec![document];
        let bytes = index.write_to_bytes().expect("serialize");
        let ranges = HashMap::new();
        let texts = HashMap::new();
        let context = fixture_context(&ranges, &texts);
        let outcome = ingest(&bytes, &context).expect("ingest");
        assert_eq!(outcome.documents, 0);
        assert_eq!(outcome.foreign_documents, 1);
        assert!(outcome.relations.is_empty());
    }

    /// Regression: the packed 3-element range is `[line, start_col,
    /// end_col]` — not `[line, col, len]`. Treating the third element as a
    /// length overflows short lines (e.g. a multi-line signature's first
    /// line ending in `(`), which silently drops the definition and orphans
    /// every reference to it.
    #[test]
    fn packed_three_element_range_uses_end_column() {
        // line 0 "fn target(" — `target` at cols 3..9; len-misread gives 3..12.
        let text = "fn target(\n    x: i32,\n) {}\nfn caller() { target() }\n";
        let texts = HashMap::from([("src/lib.rs".to_owned(), text.to_owned())]);
        let ranges = HashMap::from([(
            "src/lib.rs".to_owned(),
            vec![
                (0, 24, "e-target".to_owned()),
                (24, 47, "e-caller".to_owned()),
                (0, text.len(), "e-file".to_owned()),
            ],
        )]);

        let mut definition = Occurrence::new();
        definition.range = vec![0, 3, 9];
        definition.symbol = "fixture . target().".to_owned();
        definition.symbol_roles = DEFINITION_ROLE;
        let mut reference = Occurrence::new();
        reference.range = vec![3, 14, 20];
        reference.symbol = "fixture . target().".to_owned();
        let mut document = Document::new();
        document.relative_path = "src/lib.rs".to_owned();
        document.occurrences = vec![definition, reference];
        let mut index = Index::new();
        index.documents = vec![document];
        let bytes = index.write_to_bytes().expect("serialize");

        let context = fixture_context(&ranges, &texts);
        let outcome = ingest(&bytes, &context).expect("ingest");
        assert_eq!(outcome.definitions, 1);
        assert_eq!(outcome.relations.len(), 1);
        assert_eq!(outcome.relations[0].target_entity_id, "e-target");
        assert_eq!(outcome.relations[0].source_entity_id, "e-caller");
        let evidence = &outcome.relations[0].evidence[0];
        assert_eq!(evidence.start_line, 4);
    }

    /// `typed_range` takes precedence over the packed `range` field.
    #[test]
    fn typed_range_wins_over_packed_range() {
        let text = "fn target() {}\nfn caller() { target() }\n";
        let texts = HashMap::from([("src/lib.rs".to_owned(), text.to_owned())]);
        let ranges = HashMap::from([(
            "src/lib.rs".to_owned(),
            vec![
                (0, 14, "e-target".to_owned()),
                (14, 39, "e-caller".to_owned()),
                (0, text.len(), "e-file".to_owned()),
            ],
        )]);

        let mut definition = Occurrence::new();
        // Bogus packed range; the typed range (0:3..9) is authoritative.
        definition.range = vec![9, 9, 9, 9];
        definition.typed_range = Some(Typed_range::SingleLineRange(SingleLineRange {
            line: 0,
            start_character: 3,
            end_character: 9,
            ..Default::default()
        }));
        definition.symbol = "fixture . target().".to_owned();
        definition.symbol_roles = DEFINITION_ROLE;
        let mut document = Document::new();
        document.relative_path = "src/lib.rs".to_owned();
        document.occurrences = vec![definition];
        let mut index = Index::new();
        index.documents = vec![document];
        let bytes = index.write_to_bytes().expect("serialize");

        let context = fixture_context(&ranges, &texts);
        let outcome = ingest(&bytes, &context).expect("ingest");
        assert_eq!(outcome.definitions, 1);
    }

    #[tokio::test]
    async fn repo_local_index_scip_drives_graph_ready() {
        let dir = tempfile::tempdir().expect("tempdir");
        let source =
            "pub fn helper() -> bool {\n    true\n}\npub fn caller() -> bool {\n    helper()\n}\n";
        std::fs::create_dir_all(dir.path().join("src")).expect("mkdir");
        std::fs::write(dir.path().join("src/lib.rs"), source).expect("write lib");
        std::fs::write(
            dir.path().join("index.scip"),
            fixture_index(TextEncoding::UTF8),
        )
        .expect("write index.scip");

        let engine = crate::CceEngine::open(crate::EngineConfig::for_repository(dir.path()))
            .expect("engine");
        let report = engine.index().await.expect("index");
        let provider = report
            .providers
            .iter()
            .find(|report| report.provider_id == "scip:file")
            .expect("scip:file report");
        assert_eq!(provider.state, crate::providers::ProviderState::Ready);
        eprintln!("PROVIDER {provider:?}");
        assert_eq!(provider.scip_reference_edges, 1);
        assert_eq!(provider.scip_definitions, 1);
        assert!(provider.artifact_digest.is_some());

        let manifest = report.manifest;
        assert_eq!(
            manifest.views[&cce_core::ViewKind::Graph].state,
            cce_core::ViewState::Ready
        );
        assert_eq!(
            manifest.views[&cce_core::ViewKind::Dataflow].state,
            cce_core::ViewState::Partial
        );

        let references = engine.references("helper").expect("references");
        // tree-sitter `Calls` (0.6, guessed by name) and the SCIP
        // `References` (1.0, compiler-resolved) edge coexist — origin tags
        // keep them distinguishable.
        let scip_hit = references
            .references
            .iter()
            .find(|hit| hit.origin == "scip")
            .expect("scip reference edge");
        assert_eq!(scip_hit.from_name, "caller");
        assert_eq!(scip_hit.via, "references");
    }
}
