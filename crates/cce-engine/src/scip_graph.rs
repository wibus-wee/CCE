use std::{
    collections::{HashMap, HashSet},
    path::Path,
    process::Stdio,
    time::Duration,
};

use cce_core::{
    CceError, CodeEntity, EntityKind, Relation, RelationKind, RelationOrigin, Result, SourceAddress,
};
use protobuf::{Enum, Message};
use scip::types::{
    Document, Occurrence, PositionEncoding, SymbolInformation, SymbolRole, occurrence,
    symbol_information,
};

use crate::{ScannedFile, ScannedRepository, ScipBackendConfig};

const MAX_SCIP_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_EVIDENCE_PER_EDGE: usize = 32;

#[derive(Debug)]
pub(crate) struct ScipImport {
    pub bytes: Vec<u8>,
    pub trusted_snapshot: bool,
    pub entities: Vec<CodeEntity>,
    pub relations: Vec<Relation>,
    pub documents: usize,
    pub matched_documents: usize,
    pub definitions: usize,
    pub references: usize,
    pub skipped_occurrences: usize,
    pub tool: String,
}

pub(crate) async fn build(
    config: &ScipBackendConfig,
    repository_root: &Path,
    data_root: &Path,
    scanned: &ScannedRepository,
    existing_entities: &[CodeEntity],
) -> Result<Option<ScipImport>> {
    let Some((bytes, trusted_snapshot)) = load_index(config, repository_root, data_root).await?
    else {
        return Ok(None);
    };
    let index = scip::types::Index::parse_from_bytes(&bytes)
        .map_err(|error| CceError::ArtifactCorrupt(format!("invalid SCIP protobuf: {error}")))?;
    let metadata = index.metadata.as_ref();
    let tool = metadata
        .and_then(|value| value.tool_info.as_ref())
        .map_or_else(
            || "unknown-scip-indexer".to_owned(),
            |value| format!("{}@{}", value.name, value.version),
        );
    let artifact_digest = blake3::hash(&bytes).to_hex().to_string();
    let files = scanned
        .files
        .iter()
        .map(|file| (file.relative_path.as_str(), file))
        .collect::<HashMap<_, _>>();
    let entities_by_path = entities_by_path(existing_entities);
    let symbol_information = symbol_information(&index.documents, &index.external_symbols);
    let mut symbol_entities = HashMap::<String, String>::new();
    let mut new_entities = Vec::new();
    let mut matched_documents = 0_usize;
    let mut definitions = 0_usize;
    let mut skipped_occurrences = 0_usize;

    for document in &index.documents {
        let Some(file) = files.get(document.relative_path.as_str()).copied() else {
            skipped_occurrences = skipped_occurrences.saturating_add(document.occurrences.len());
            continue;
        };
        matched_documents += 1;
        for occurrence in &document.occurrences {
            if occurrence.symbol.is_empty() || !has_role(occurrence, SymbolRole::Definition) {
                continue;
            }
            let Some(address) = occurrence_address(scanned, file, document, occurrence)? else {
                skipped_occurrences += 1;
                continue;
            };
            definitions += 1;
            let key = symbol_key(&document.relative_path, &occurrence.symbol);
            let entity_id = enclosing_entity(&entities_by_path, &address)
                .map(|entity| entity.id.clone())
                .unwrap_or_else(|| {
                    let information = symbol_information.get(&key);
                    let id = digest_id(
                        "entity",
                        &[&scanned.identity.id, "scip", &key, &artifact_digest],
                    );
                    new_entities.push(CodeEntity {
                        id: id.clone(),
                        kind: information.map_or(EntityKind::Unknown, |value| {
                            entity_kind(value.kind.enum_value().ok())
                        }),
                        name: information
                            .and_then(|value| (!value.display_name.is_empty()).then_some(value))
                            .map_or_else(
                                || symbol_display_name(&occurrence.symbol),
                                |value| value.display_name.clone(),
                            ),
                        qualified_name: Some(occurrence.symbol.clone()),
                        signature: information
                            .and_then(|value| value.signature_documentation.as_ref())
                            .map(|value| value.text.clone())
                            .filter(|value| !value.is_empty()),
                        language: Some(document.language.clone()),
                        address: Some(address.clone().with_symbol(id.clone())),
                        capabilities: vec!["scip_definition".to_owned()],
                        attributes: serde_json::Map::from_iter([
                            ("scipSymbol".to_owned(), occurrence.symbol.clone().into()),
                            (
                                "scipArtifactDigest".to_owned(),
                                artifact_digest.clone().into(),
                            ),
                        ]),
                    });
                    id
                });
            symbol_entities.entry(key).or_insert(entity_id);
        }
    }

    let mut edges = HashMap::<(String, String, String), Relation>::new();
    let mut external_entities = HashSet::new();
    let mut references = 0_usize;
    for document in &index.documents {
        let Some(file) = files.get(document.relative_path.as_str()).copied() else {
            continue;
        };
        for occurrence in &document.occurrences {
            if occurrence.symbol.is_empty() || has_role(occurrence, SymbolRole::Definition) {
                continue;
            }
            let Some(evidence) = occurrence_address(scanned, file, document, occurrence)? else {
                skipped_occurrences += 1;
                continue;
            };
            let Some(source) = enclosing_entity(&entities_by_path, &evidence) else {
                skipped_occurrences += 1;
                continue;
            };
            let key = symbol_key(&document.relative_path, &occurrence.symbol);
            let target = ensure_symbol_entity(
                scanned,
                &key,
                &occurrence.symbol,
                document,
                &artifact_digest,
                &symbol_information,
                &mut symbol_entities,
                &mut new_entities,
                &mut external_entities,
            );
            if source.id == target {
                continue;
            }
            references += 1;
            let kind = if has_role(occurrence, SymbolRole::Import) {
                RelationKind::Imports
            } else if has_role(occurrence, SymbolRole::Test) {
                RelationKind::Tests
            } else {
                RelationKind::References
            };
            add_edge(
                &mut edges,
                &scanned.snapshot.id,
                &source.id,
                &target,
                kind,
                &tool,
                &artifact_digest,
                Some(evidence),
                serde_json::Map::from_iter([
                    ("scipSymbol".to_owned(), occurrence.symbol.clone().into()),
                    ("symbolRoles".to_owned(), occurrence.symbol_roles.into()),
                ]),
            );
        }
    }

    for document in &index.documents {
        for information in &document.symbols {
            add_symbol_relationships(
                scanned,
                document,
                information,
                &tool,
                &artifact_digest,
                &symbol_information,
                &mut symbol_entities,
                &mut new_entities,
                &mut external_entities,
                &mut edges,
            );
        }
    }

    Ok(Some(ScipImport {
        bytes,
        trusted_snapshot,
        entities: new_entities,
        relations: edges.into_values().collect(),
        documents: index.documents.len(),
        matched_documents,
        definitions,
        references,
        skipped_occurrences,
        tool,
    }))
}

async fn load_index(
    config: &ScipBackendConfig,
    repository_root: &Path,
    data_root: &Path,
) -> Result<Option<(Vec<u8>, bool)>> {
    match config {
        ScipBackendConfig::Disabled => Ok(None),
        ScipBackendConfig::Supplied { path } => Ok(Some((read_bounded(path)?, false))),
        ScipBackendConfig::RustAnalyzer {
            executable,
            threads,
            timeout_seconds,
            ..
        } => {
            let temporary_dir = data_root.join("tmp");
            std::fs::create_dir_all(&temporary_dir)
                .map_err(|error| CceError::io(&temporary_dir, error))?;
            let output = tempfile::Builder::new()
                .prefix("cce-rust-analyzer-")
                .suffix(".scip")
                .tempfile_in(&temporary_dir)
                .map_err(|error| CceError::io(&temporary_dir, error))?;
            let mut command = tokio::process::Command::new(executable);
            command
                .arg("scip")
                .arg(repository_root)
                .arg("--output")
                .arg(output.path())
                .arg("--exclude-vendored-libraries")
                .current_dir(repository_root)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            if let Some(threads) = threads {
                command.arg("--num-threads").arg(threads.to_string());
            }
            let result =
                tokio::time::timeout(Duration::from_secs(*timeout_seconds), command.output())
                    .await
                    .map_err(|_| {
                        CceError::Provider(format!(
                            "rust-analyzer SCIP generation exceeded {timeout_seconds} seconds"
                        ))
                    })?
                    .map_err(|error| {
                        CceError::Provider(format!("failed to run rust-analyzer SCIP: {error}"))
                    })?;
            if !result.status.success() {
                return Err(CceError::Provider(format!(
                    "rust-analyzer SCIP exited with {}: {}",
                    result.status,
                    truncate_utf8(&result.stderr, 8_192)
                )));
            }
            Ok(Some((read_bounded(output.path())?, true)))
        }
    }
}

fn read_bounded(path: &Path) -> Result<Vec<u8>> {
    let metadata = std::fs::metadata(path).map_err(|error| CceError::io(path, error))?;
    if metadata.len() > MAX_SCIP_BYTES {
        return Err(CceError::Configuration(format!(
            "SCIP artifact {} exceeds the 2 GiB safety limit",
            path.display()
        )));
    }
    std::fs::read(path).map_err(|error| CceError::io(path, error))
}

fn entities_by_path(entities: &[CodeEntity]) -> HashMap<&str, Vec<&CodeEntity>> {
    let mut by_path = HashMap::<&str, Vec<&CodeEntity>>::new();
    for entity in entities {
        if let Some(address) = &entity.address {
            by_path.entry(&address.path).or_default().push(entity);
        }
    }
    for entities in by_path.values_mut() {
        entities.sort_by_key(|entity| {
            entity
                .address
                .as_ref()
                .map_or(u64::MAX, SourceAddress::byte_len)
        });
    }
    by_path
}

fn enclosing_entity<'a>(
    entities_by_path: &'a HashMap<&str, Vec<&'a CodeEntity>>,
    address: &SourceAddress,
) -> Option<&'a CodeEntity> {
    entities_by_path
        .get(address.path.as_str())?
        .iter()
        .copied()
        .find(|entity| {
            entity.address.as_ref().is_some_and(|candidate| {
                candidate.start_byte <= address.start_byte && candidate.end_byte >= address.end_byte
            })
        })
}

fn symbol_information<'a>(
    documents: &'a [Document],
    external: &'a [SymbolInformation],
) -> HashMap<String, &'a SymbolInformation> {
    let mut information = HashMap::new();
    for document in documents {
        for symbol in &document.symbols {
            information.insert(symbol_key(&document.relative_path, &symbol.symbol), symbol);
        }
    }
    for symbol in external {
        information.insert(symbol.symbol.clone(), symbol);
    }
    information
}

#[allow(clippy::too_many_arguments)]
fn ensure_symbol_entity(
    scanned: &ScannedRepository,
    key: &str,
    symbol: &str,
    document: &Document,
    artifact_digest: &str,
    information: &HashMap<String, &SymbolInformation>,
    symbol_entities: &mut HashMap<String, String>,
    new_entities: &mut Vec<CodeEntity>,
    external_entities: &mut HashSet<String>,
) -> String {
    if let Some(id) = symbol_entities.get(key) {
        return id.clone();
    }
    let id = digest_id("entity", &[&scanned.identity.id, "scip-external", key]);
    if external_entities.insert(id.clone()) {
        let info = information.get(key).copied();
        new_entities.push(CodeEntity {
            id: id.clone(),
            kind: info.map_or(EntityKind::Unknown, |value| {
                entity_kind(value.kind.enum_value().ok())
            }),
            name: info
                .filter(|value| !value.display_name.is_empty())
                .map_or_else(
                    || symbol_display_name(symbol),
                    |value| value.display_name.clone(),
                ),
            qualified_name: Some(symbol.to_owned()),
            signature: info
                .and_then(|value| value.signature_documentation.as_ref())
                .map(|value| value.text.clone())
                .filter(|value| !value.is_empty()),
            language: Some(document.language.clone()),
            address: None,
            capabilities: vec!["scip_external_symbol".to_owned()],
            attributes: serde_json::Map::from_iter([
                ("scipSymbol".to_owned(), symbol.to_owned().into()),
                (
                    "scipArtifactDigest".to_owned(),
                    artifact_digest.to_owned().into(),
                ),
            ]),
        });
    }
    symbol_entities.insert(key.to_owned(), id.clone());
    id
}

#[allow(clippy::too_many_arguments)]
fn add_symbol_relationships(
    scanned: &ScannedRepository,
    document: &Document,
    information: &SymbolInformation,
    tool: &str,
    artifact_digest: &str,
    all_information: &HashMap<String, &SymbolInformation>,
    symbol_entities: &mut HashMap<String, String>,
    new_entities: &mut Vec<CodeEntity>,
    external_entities: &mut HashSet<String>,
    edges: &mut HashMap<(String, String, String), Relation>,
) {
    let source_key = symbol_key(&document.relative_path, &information.symbol);
    let Some(source) = symbol_entities.get(&source_key).cloned() else {
        return;
    };
    for relationship in &information.relationships {
        let target_key = symbol_key(&document.relative_path, &relationship.symbol);
        let target = ensure_symbol_entity(
            scanned,
            &target_key,
            &relationship.symbol,
            document,
            artifact_digest,
            all_information,
            symbol_entities,
            new_entities,
            external_entities,
        );
        for (enabled, kind) in [
            (relationship.is_implementation, RelationKind::Implements),
            (relationship.is_type_definition, RelationKind::TypeUses),
            (relationship.is_definition, RelationKind::Defines),
            (relationship.is_reference, RelationKind::References),
        ] {
            if enabled && source != target {
                add_edge(
                    edges,
                    &scanned.snapshot.id,
                    &source,
                    &target,
                    kind,
                    tool,
                    artifact_digest,
                    None,
                    serde_json::Map::from_iter([("scipRelationship".to_owned(), true.into())]),
                );
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn add_edge(
    edges: &mut HashMap<(String, String, String), Relation>,
    snapshot_id: &str,
    source: &str,
    target: &str,
    kind: RelationKind,
    tool: &str,
    artifact_digest: &str,
    evidence: Option<SourceAddress>,
    attributes: serde_json::Map<String, serde_json::Value>,
) {
    let kind_key = serde_json::to_string(&kind).unwrap_or_else(|_| "unknown".to_owned());
    let key = (source.to_owned(), target.to_owned(), kind_key.clone());
    let relation = edges.entry(key).or_insert_with(|| Relation {
        id: digest_id("rel", &[source, target, &kind_key, "scip"]),
        source_entity_id: source.to_owned(),
        target_entity_id: target.to_owned(),
        kind,
        origin: RelationOrigin::Scip,
        confidence: 1.0,
        snapshot_id: snapshot_id.to_owned(),
        extractor: tool.to_owned(),
        evidence: Vec::new(),
        attributes: serde_json::Map::from_iter([
            ("scipArtifactDigest".to_owned(), artifact_digest.into()),
            ("occurrenceCount".to_owned(), 0_u64.into()),
        ]),
    });
    if let Some(evidence) = evidence {
        if relation.evidence.len() < MAX_EVIDENCE_PER_EDGE {
            relation.evidence.push(evidence);
        }
        let count = relation
            .attributes
            .get("occurrenceCount")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or_default()
            .saturating_add(1);
        relation
            .attributes
            .insert("occurrenceCount".to_owned(), count.into());
    }
    relation.attributes.extend(attributes);
}

fn occurrence_address(
    scanned: &ScannedRepository,
    file: &ScannedFile,
    document: &Document,
    occurrence: &Occurrence,
) -> Result<Option<SourceAddress>> {
    let Some((start_line, start_character, end_line, end_character)) = occurrence_range(occurrence)
    else {
        return Ok(None);
    };
    let encoding = document.position_encoding.enum_value().map_err(|value| {
        CceError::ArtifactCorrupt(format!(
            "unknown SCIP position encoding {value} in {}",
            document.relative_path
        ))
    })?;
    if encoding == PositionEncoding::UnspecifiedPositionEncoding {
        return Ok(None);
    }
    let text = file.text()?;
    let start = position_to_byte(text, start_line, start_character, encoding)?;
    let end = position_to_byte(text, end_line, end_character, encoding)?;
    if start > end {
        return Ok(None);
    }
    Ok(Some(
        SourceAddress::new(
            &scanned.identity.id,
            &scanned.snapshot.id,
            &document.relative_path,
            start as u64..end as u64,
            (start_line as u32 + 1)..=(end_line as u32 + 1),
        )?
        .with_symbol(&occurrence.symbol),
    ))
}

fn occurrence_range(occurrence: &Occurrence) -> Option<(usize, usize, usize, usize)> {
    let values = match &occurrence.typed_range {
        Some(occurrence::Typed_range::SingleLineRange(value)) => [
            value.line,
            value.start_character,
            value.line,
            value.end_character,
        ],
        Some(occurrence::Typed_range::MultiLineRange(value)) => [
            value.start_line,
            value.start_character,
            value.end_line,
            value.end_character,
        ],
        Some(_) => return None,
        None => match occurrence.range.as_slice() {
            [line, start, end] => [*line, *start, *line, *end],
            [start_line, start, end_line, end] => [*start_line, *start, *end_line, *end],
            _ => return None,
        },
    };
    values
        .into_iter()
        .map(usize::try_from)
        .collect::<std::result::Result<Vec<_>, _>>()
        .ok()
        .map(|value| (value[0], value[1], value[2], value[3]))
}

fn position_to_byte(
    text: &str,
    line: usize,
    character: usize,
    encoding: PositionEncoding,
) -> Result<usize> {
    let line_start = if line == 0 {
        0
    } else {
        text.match_indices('\n')
            .nth(line - 1)
            .map(|(offset, _)| offset + 1)
            .ok_or_else(|| CceError::ArtifactCorrupt(format!("SCIP line {line} is out of range")))?
    };
    let line_end = text[line_start..]
        .find('\n')
        .map_or(text.len(), |offset| line_start + offset);
    let line_text = text.get(line_start..line_end).ok_or_else(|| {
        CceError::ArtifactCorrupt("SCIP line is not on a UTF-8 boundary".to_owned())
    })?;
    let relative = match encoding {
        PositionEncoding::UTF8CodeUnitOffsetFromLineStart => {
            if character > line_text.len() || !line_text.is_char_boundary(character) {
                return Err(CceError::ArtifactCorrupt(format!(
                    "SCIP UTF-8 column {character} is out of range"
                )));
            }
            character
        }
        PositionEncoding::UTF16CodeUnitOffsetFromLineStart => {
            byte_offset_for_code_units(line_text, character, char::len_utf16)?
        }
        PositionEncoding::UTF32CodeUnitOffsetFromLineStart => {
            byte_offset_for_code_units(line_text, character, |_| 1)?
        }
        PositionEncoding::UnspecifiedPositionEncoding => {
            return Err(CceError::ArtifactCorrupt(
                "SCIP position encoding is unspecified".to_owned(),
            ));
        }
    };
    Ok(line_start + relative)
}

fn byte_offset_for_code_units(
    text: &str,
    expected: usize,
    units: impl Fn(char) -> usize,
) -> Result<usize> {
    let mut consumed = 0_usize;
    for (offset, character) in text.char_indices() {
        if consumed == expected {
            return Ok(offset);
        }
        consumed += units(character);
        if consumed > expected {
            return Err(CceError::ArtifactCorrupt(format!(
                "SCIP column {expected} splits a Unicode scalar"
            )));
        }
    }
    if consumed == expected {
        Ok(text.len())
    } else {
        Err(CceError::ArtifactCorrupt(format!(
            "SCIP column {expected} is out of range"
        )))
    }
}

fn has_role(occurrence: &Occurrence, role: SymbolRole) -> bool {
    occurrence.symbol_roles & role.value() != 0
}

fn symbol_key(path: &str, symbol: &str) -> String {
    if scip::symbol::is_local_symbol(symbol) {
        format!("{path}\0{symbol}")
    } else {
        symbol.to_owned()
    }
}

fn symbol_display_name(symbol: &str) -> String {
    symbol
        .split(['/', '#', '.', '(', ')', ' '])
        .rfind(|value| !value.is_empty())
        .unwrap_or(symbol)
        .trim_matches('`')
        .to_owned()
}

fn entity_kind(kind: Option<symbol_information::Kind>) -> EntityKind {
    match kind {
        Some(symbol_information::Kind::Class) => EntityKind::Class,
        Some(symbol_information::Kind::Interface) => EntityKind::Interface,
        Some(symbol_information::Kind::Trait) => EntityKind::Trait,
        Some(symbol_information::Kind::Struct) => EntityKind::Struct,
        Some(symbol_information::Kind::Enum) => EntityKind::Enum,
        Some(symbol_information::Kind::Function) => EntityKind::Function,
        Some(symbol_information::Kind::Method) => EntityKind::Method,
        Some(symbol_information::Kind::Field) => EntityKind::Field,
        Some(symbol_information::Kind::Constant) => EntityKind::Constant,
        Some(symbol_information::Kind::Module) => EntityKind::Module,
        Some(symbol_information::Kind::Namespace) => EntityKind::Namespace,
        _ => EntityKind::Unknown,
    }
}

fn digest_id(prefix: &str, components: &[&str]) -> String {
    let mut hasher = blake3::Hasher::new();
    for component in components {
        hasher.update(component.as_bytes());
        hasher.update(&[0]);
    }
    format!("{prefix}_{}", &hasher.finalize().to_hex()[..32])
}

fn truncate_utf8(bytes: &[u8], maximum: usize) -> String {
    String::from_utf8_lossy(&bytes[..bytes.len().min(maximum)]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_all_scip_position_encodings() {
        let text = "a二😀z\nnext";
        assert_eq!(
            position_to_byte(
                text,
                0,
                8,
                PositionEncoding::UTF8CodeUnitOffsetFromLineStart
            )
            .expect("utf8"),
            8
        );
        assert_eq!(
            position_to_byte(
                text,
                0,
                4,
                PositionEncoding::UTF16CodeUnitOffsetFromLineStart
            )
            .expect("utf16"),
            8
        );
        assert_eq!(
            position_to_byte(
                text,
                0,
                3,
                PositionEncoding::UTF32CodeUnitOffsetFromLineStart
            )
            .expect("utf32"),
            8
        );
        assert_eq!(
            position_to_byte(
                text,
                1,
                2,
                PositionEncoding::UTF8CodeUnitOffsetFromLineStart
            )
            .expect("second line"),
            12
        );
    }

    #[test]
    fn local_scip_symbols_are_document_scoped() {
        assert_ne!(symbol_key("a.rs", "local 1"), symbol_key("b.rs", "local 1"));
        assert_eq!(
            symbol_key("a.rs", "rust cargo pkg 1 Foo#"),
            "rust cargo pkg 1 Foo#"
        );
    }
}
