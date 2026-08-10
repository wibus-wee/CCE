use std::{
    collections::{HashMap, HashSet},
    io::Write,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use cce_core::{
    CceError, CodeEntity, EntityKind, Relation, RelationKind, RelationOrigin, Result, SourceAddress,
};
use protobuf::{Enum, EnumOrUnknown, Message, MessageField};
use scip::types::{
    Document, Index, Metadata, Occurrence, PositionEncoding, SymbolInformation, SymbolRole,
    TextEncoding, ToolInfo, occurrence, symbol_information,
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
    pub failures: Vec<String>,
}

struct LoadedScipIndex {
    bytes: Vec<u8>,
    trusted_snapshot: bool,
    failures: Vec<String>,
}

pub(crate) async fn build(
    config: &ScipBackendConfig,
    repository_root: &Path,
    data_root: &Path,
    scanned: &ScannedRepository,
    existing_entities: &[CodeEntity],
) -> Result<Option<ScipImport>> {
    let Some(loaded) = load_index(config, repository_root, data_root, scanned).await? else {
        return Ok(None);
    };
    let bytes = loaded.bytes;
    let trusted_snapshot = loaded.trusted_snapshot;
    let index = scip::types::Index::parse_from_bytes(&bytes)
        .map_err(|error| CceError::ArtifactCorrupt(format!("invalid SCIP protobuf: {error}")))?;
    let metadata = index.metadata.as_ref();
    let legacy_position_encoding = metadata
        .and_then(|value| value.text_document_encoding.enum_value().ok())
        .and_then(legacy_position_encoding);
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
            let Some(address) = occurrence_address(
                scanned,
                file,
                document,
                occurrence,
                legacy_position_encoding,
            )?
            else {
                skipped_occurrences += 1;
                continue;
            };
            definitions += 1;
            let key = symbol_key(&document.relative_path, &occurrence.symbol);
            if symbol_entities.contains_key(&key) {
                continue;
            }
            let information = symbol_information.get(&key);
            let definition_name = information
                .filter(|value| !value.display_name.is_empty())
                .map_or_else(
                    || symbol_display_name(&occurrence.symbol),
                    |value| value.display_name.clone(),
                );
            let entity_id = enclosing_entity(&entities_by_path, &address)
                .filter(|entity| entity.name == definition_name)
                .map(|entity| entity.id.clone())
                .unwrap_or_else(|| {
                    let id = digest_id(
                        "entity",
                        &[&scanned.identity.id, "scip", &key, &artifact_digest],
                    );
                    new_entities.push(CodeEntity {
                        id: id.clone(),
                        kind: information.map_or(EntityKind::Unknown, |value| {
                            entity_kind(value.kind.enum_value().ok())
                        }),
                        name: definition_name.clone(),
                        qualified_name: Some(occurrence.symbol.clone()),
                        signature: information
                            .and_then(|value| value.signature_documentation.as_ref())
                            .map(|value| value.text.clone())
                            .filter(|value| !value.is_empty()),
                        language: document_language(document, file.language.as_deref()),
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
            let Some(evidence) = occurrence_address(
                scanned,
                file,
                document,
                occurrence,
                legacy_position_encoding,
            )?
            else {
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
                file.language.as_deref(),
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
        let fallback_language = files
            .get(document.relative_path.as_str())
            .and_then(|file| file.language.as_deref());
        for information in &document.symbols {
            add_symbol_relationships(
                scanned,
                document,
                information,
                fallback_language,
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
        failures: loaded.failures,
    }))
}

async fn load_index(
    config: &ScipBackendConfig,
    repository_root: &Path,
    data_root: &Path,
    scanned: &ScannedRepository,
) -> Result<Option<LoadedScipIndex>> {
    match config {
        ScipBackendConfig::Disabled => Ok(None),
        ScipBackendConfig::Supplied { path } => Ok(Some(LoadedScipIndex {
            bytes: read_bounded(path)?,
            trusted_snapshot: false,
            failures: Vec::new(),
        })),
        ScipBackendConfig::RustAnalyzer {
            executable,
            threads,
            timeout_seconds,
            ..
        } => Ok(Some(LoadedScipIndex {
            bytes: generate_rust_index(
                executable,
                *threads,
                *timeout_seconds,
                repository_root,
                data_root,
            )
            .await?,
            trusted_snapshot: true,
            failures: Vec::new(),
        })),
        ScipBackendConfig::Auto {
            rust_analyzer,
            typescript_indexer,
            threads,
            timeout_seconds,
        } => {
            let has_rust = scanned
                .files
                .iter()
                .any(|file| file.language.as_deref() == Some("rust"));
            let has_typescript = scanned.files.iter().any(|file| {
                matches!(
                    file.language.as_deref(),
                    Some("typescript" | "tsx" | "javascript")
                )
            });
            if !has_rust && !has_typescript {
                return Ok(None);
            }
            let mut indexes = Vec::new();
            let mut failures = Vec::new();
            if has_rust {
                match resolve_runtime_executable(rust_analyzer)
                    .map(|executable| (executable, *threads, *timeout_seconds))
                {
                    Ok((executable, threads, timeout_seconds)) => {
                        match generate_rust_index(
                            &executable,
                            threads,
                            timeout_seconds,
                            repository_root,
                            data_root,
                        )
                        .await
                        {
                            Ok(bytes) => indexes.push(bytes),
                            Err(error) => failures.push(format!("rust-analyzer: {error}")),
                        }
                    }
                    Err(error) => failures.push(format!("rust-analyzer: {error}")),
                }
            }
            if has_typescript {
                match resolve_runtime_executable(typescript_indexer) {
                    Ok(executable) => {
                        match generate_typescript_indexes(
                            &executable,
                            *timeout_seconds,
                            repository_root,
                            data_root,
                            scanned,
                        )
                        .await
                        {
                            Ok((generated, provider_failures)) => {
                                indexes.extend(generated);
                                failures.extend(
                                    provider_failures
                                        .into_iter()
                                        .map(|failure| format!("scip-typescript: {failure}")),
                                );
                            }
                            Err(error) => failures.push(format!("scip-typescript: {error}")),
                        }
                    }
                    Err(error) => failures.push(format!("scip-typescript: {error}")),
                }
            }
            if indexes.is_empty() {
                return Err(CceError::Provider(failures.join("; ")));
            }
            Ok(Some(LoadedScipIndex {
                bytes: merge_scip_indexes(indexes)?,
                trusted_snapshot: true,
                failures,
            }))
        }
    }
}

async fn generate_rust_index(
    executable: &Path,
    threads: Option<usize>,
    timeout_seconds: u64,
    repository_root: &Path,
    data_root: &Path,
) -> Result<Vec<u8>> {
    let output = temporary_scip_file(data_root, "cce-rust-analyzer-")?;
    let mut command = tokio::process::Command::new(executable);
    command
        .arg("scip")
        .arg(repository_root)
        .arg("--output")
        .arg(output.path())
        .arg("--exclude-vendored-libraries");
    if let Some(threads) = threads {
        command.arg("--num-threads").arg(threads.to_string());
    }
    run_scip_command(command, repository_root, timeout_seconds, "rust-analyzer").await?;
    read_bounded(output.path())
}

async fn generate_typescript_indexes(
    executable: &Path,
    timeout_seconds: u64,
    repository_root: &Path,
    data_root: &Path,
    scanned: &ScannedRepository,
) -> Result<(Vec<Vec<u8>>, Vec<String>)> {
    let configured_projects = scanned
        .files
        .iter()
        .filter(|file| {
            Path::new(&file.relative_path)
                .file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.starts_with("tsconfig") && value.ends_with(".json"))
        })
        .map(|file| repository_root.join(&file.relative_path))
        .collect::<Vec<_>>();
    let mut indexes = Vec::new();
    let mut failures = Vec::new();

    if !configured_projects.is_empty() {
        match generate_typescript_project_index(
            executable,
            timeout_seconds,
            repository_root,
            data_root,
            &configured_projects,
            None,
            "configured projects",
        )
        .await
        {
            Ok(bytes) => indexes.push(bytes),
            Err(error) => failures.push(format!("configured projects: {error}")),
        }
    }

    let workspace_flag = if scanned
        .files
        .iter()
        .any(|file| file.relative_path == "pnpm-workspace.yaml")
    {
        Some("--pnpm-workspaces")
    } else if has_yarn_workspaces(scanned) {
        Some("--yarn-workspaces")
    } else {
        None
    };
    if let Some(workspace_flag) = workspace_flag {
        match generate_typescript_project_index(
            executable,
            timeout_seconds,
            repository_root,
            data_root,
            &[],
            Some(workspace_flag),
            "workspace projects",
        )
        .await
        {
            Ok(bytes) => indexes.push(bytes),
            Err(error) => failures.push(format!("workspace projects: {error}")),
        }
    }

    let covered_paths = scip_document_paths(&indexes)?;
    let supplemental_files = scanned
        .files
        .iter()
        .filter(|file| {
            matches!(
                file.language.as_deref(),
                Some("javascript" | "typescript" | "tsx")
            ) && !covered_paths.contains(&file.relative_path)
        })
        .map(|file| repository_root.join(&file.relative_path))
        .collect::<Vec<_>>();
    if !supplemental_files.is_empty() {
        let temporary_dir = data_root.join("tmp");
        std::fs::create_dir_all(&temporary_dir)
            .map_err(|error| CceError::io(&temporary_dir, error))?;
        let mut config = tempfile::Builder::new()
            .prefix("cce-scip-unconfigured-")
            .suffix(".json")
            .tempfile_in(&temporary_dir)
            .map_err(|error| CceError::io(&temporary_dir, error))?;
        let files = supplemental_files
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        let config_json = serde_json::json!({
            "compilerOptions": {
                "allowJs": true,
                "checkJs": true,
                "noEmit": true,
                "target": "ES2022",
                "module": "NodeNext",
                "moduleResolution": "NodeNext",
                "jsx": "react-jsx",
                "allowSyntheticDefaultImports": true,
                "skipLibCheck": true
            },
            "files": files
        });
        serde_json::to_writer(config.as_file_mut(), &config_json)
            .map_err(|error| CceError::ArtifactCorrupt(error.to_string()))?;
        config
            .as_file_mut()
            .flush()
            .map_err(|error| CceError::io(config.path(), error))?;
        let supplemental_project = config.path().to_path_buf();
        match generate_typescript_project_index(
            executable,
            timeout_seconds,
            repository_root,
            data_root,
            std::slice::from_ref(&supplemental_project),
            None,
            "unconfigured JS/TS supplement",
        )
        .await
        {
            Ok(bytes) => indexes.push(bytes),
            Err(error) => failures.push(format!("unconfigured JS/TS supplement: {error}")),
        }
    }

    if indexes.is_empty() {
        return Err(CceError::Provider(failures.join("; ")));
    }
    Ok((indexes, failures))
}

fn scip_document_paths(indexes: &[Vec<u8>]) -> Result<HashSet<String>> {
    let mut paths = HashSet::new();
    for bytes in indexes {
        let index = Index::parse_from_bytes(bytes).map_err(|error| {
            CceError::ArtifactCorrupt(format!("invalid generated SCIP protobuf: {error}"))
        })?;
        paths.extend(
            index
                .documents
                .into_iter()
                .map(|document| document.relative_path.replace('\\', "/")),
        );
    }
    Ok(paths)
}

#[allow(clippy::too_many_arguments)]
async fn generate_typescript_project_index(
    executable: &Path,
    timeout_seconds: u64,
    repository_root: &Path,
    data_root: &Path,
    projects: &[PathBuf],
    workspace_flag: Option<&str>,
    label: &str,
) -> Result<Vec<u8>> {
    let output = temporary_scip_file(data_root, "cce-scip-typescript-")?;
    let mut command = tokio::process::Command::new(executable);
    command
        .arg("index")
        .arg("--cwd")
        .arg(repository_root)
        .arg("--output")
        .arg(output.path())
        .arg("--no-progress-bar");
    if let Some(workspace_flag) = workspace_flag {
        command.arg(workspace_flag);
    }
    for project in projects {
        command.arg(project);
    }
    run_scip_command(command, repository_root, timeout_seconds, label).await?;
    read_bounded(output.path())
}

fn has_yarn_workspaces(scanned: &ScannedRepository) -> bool {
    let has_yarn_lock = scanned
        .files
        .iter()
        .any(|file| file.relative_path == "yarn.lock");
    let has_workspaces = scanned
        .files
        .iter()
        .find(|file| file.relative_path == "package.json")
        .and_then(|file| serde_json::from_slice::<serde_json::Value>(&file.bytes).ok())
        .is_some_and(|value| value.get("workspaces").is_some());
    has_yarn_lock && has_workspaces
}

fn temporary_scip_file(data_root: &Path, prefix: &str) -> Result<tempfile::NamedTempFile> {
    let temporary_dir = data_root.join("tmp");
    std::fs::create_dir_all(&temporary_dir).map_err(|error| CceError::io(&temporary_dir, error))?;
    tempfile::Builder::new()
        .prefix(prefix)
        .suffix(".scip")
        .tempfile_in(&temporary_dir)
        .map_err(|error| CceError::io(&temporary_dir, error))
}

async fn run_scip_command(
    mut command: tokio::process::Command,
    repository_root: &Path,
    timeout_seconds: u64,
    name: &str,
) -> Result<()> {
    command
        .current_dir(repository_root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let result = tokio::time::timeout(Duration::from_secs(timeout_seconds), command.output())
        .await
        .map_err(|_| {
            CceError::Provider(format!(
                "{name} SCIP generation exceeded {timeout_seconds} seconds"
            ))
        })?
        .map_err(|error| CceError::Provider(format!("failed to run {name}: {error}")))?;
    if !result.status.success() {
        return Err(CceError::Provider(format!(
            "{name} exited with {}: stdout={} stderr={}",
            result.status,
            truncate_utf8(&result.stdout, 8_192),
            truncate_utf8(&result.stderr, 8_192)
        )));
    }
    Ok(())
}

fn resolve_runtime_executable(path: &Path) -> Result<PathBuf> {
    if path.components().count() == 1 {
        if let Some(resolved) = std::env::var_os("PATH")
            .into_iter()
            .flat_map(|value| std::env::split_paths(&value).collect::<Vec<_>>())
            .map(|directory| directory.join(path))
            .find(|candidate| candidate.is_file())
        {
            return Ok(resolved);
        }
    } else if path.is_file() {
        return std::path::absolute(path).map_err(|error| CceError::io(path, error));
    }
    Err(CceError::Configuration(format!(
        "SCIP indexer {} was not found",
        path.display()
    )))
}

fn merge_scip_indexes(indexes: Vec<Vec<u8>>) -> Result<Vec<u8>> {
    let mut parsed = indexes
        .into_iter()
        .map(|bytes| {
            Index::parse_from_bytes(&bytes).map_err(|error| {
                CceError::ArtifactCorrupt(format!("invalid generated SCIP protobuf: {error}"))
            })
        })
        .collect::<Result<Vec<_>>>()?;
    if parsed.len() == 1 {
        return parsed
            .pop()
            .expect("one parsed index")
            .write_to_bytes()
            .map_err(|error| CceError::ArtifactCorrupt(error.to_string()));
    }
    let tools = parsed
        .iter()
        .filter_map(|index| index.metadata.as_ref())
        .filter_map(|metadata| metadata.tool_info.as_ref())
        .map(|tool| format!("{}@{}", tool.name, tool.version))
        .collect::<Vec<_>>();
    for index in &mut parsed {
        apply_legacy_encoding(index)?;
    }
    let mut merged = parsed.remove(0);
    let mut documents = HashMap::<String, Document>::new();
    for document in std::mem::take(&mut merged.documents) {
        retain_richer_document(&mut documents, document);
    }
    for mut index in parsed {
        for document in std::mem::take(&mut index.documents) {
            retain_richer_document(&mut documents, document);
        }
        merged.external_symbols.append(&mut index.external_symbols);
    }
    merged.documents = documents.into_values().collect();
    merged
        .documents
        .sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    let mut seen_symbols = HashSet::new();
    merged
        .external_symbols
        .retain(|symbol| seen_symbols.insert(symbol.symbol.clone()));
    let mut tool = ToolInfo::new();
    tool.name = "cce-scip-auto".to_owned();
    tool.version = tools.join("+");
    let metadata = merged
        .metadata
        .0
        .get_or_insert_with(|| Box::new(Metadata::new()));
    metadata.tool_info = MessageField::some(tool);
    metadata.text_document_encoding = EnumOrUnknown::new(TextEncoding::UnspecifiedTextEncoding);
    let bytes = merged
        .write_to_bytes()
        .map_err(|error| CceError::ArtifactCorrupt(error.to_string()))?;
    if bytes.len() as u64 > MAX_SCIP_BYTES {
        return Err(CceError::Configuration(
            "merged SCIP artifact exceeds the 2 GiB safety limit".to_owned(),
        ));
    }
    Ok(bytes)
}

fn retain_richer_document(documents: &mut HashMap<String, Document>, candidate: Document) {
    let candidate_quality = candidate.occurrences.len().saturating_mul(2) + candidate.symbols.len();
    match documents.entry(candidate.relative_path.clone()) {
        std::collections::hash_map::Entry::Vacant(entry) => {
            entry.insert(candidate);
        }
        std::collections::hash_map::Entry::Occupied(mut entry) => {
            let existing = entry.get();
            let existing_quality =
                existing.occurrences.len().saturating_mul(2) + existing.symbols.len();
            if candidate_quality > existing_quality {
                entry.insert(candidate);
            }
        }
    }
}

fn apply_legacy_encoding(index: &mut Index) -> Result<()> {
    let fallback = index
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.text_document_encoding.enum_value().ok())
        .and_then(legacy_position_encoding);
    for document in &mut index.documents {
        let encoding = document.position_encoding.enum_value().map_err(|value| {
            CceError::ArtifactCorrupt(format!("unknown SCIP position encoding {value}"))
        })?;
        if encoding == PositionEncoding::UnspecifiedPositionEncoding
            && let Some(fallback) = fallback
        {
            document.position_encoding = EnumOrUnknown::new(fallback);
        }
    }
    Ok(())
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
    fallback_language: Option<&str>,
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
            language: document_language(document, fallback_language),
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
    fallback_language: Option<&str>,
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
            fallback_language,
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
    legacy_encoding: Option<PositionEncoding>,
) -> Result<Option<SourceAddress>> {
    let Some((start_line, start_character, end_line, end_character)) = occurrence_range(occurrence)
    else {
        return Ok(None);
    };
    let document_encoding = document.position_encoding.enum_value().map_err(|value| {
        CceError::ArtifactCorrupt(format!(
            "unknown SCIP position encoding {value} in {}",
            document.relative_path
        ))
    })?;
    let encoding = if document_encoding == PositionEncoding::UnspecifiedPositionEncoding {
        legacy_encoding
    } else {
        Some(document_encoding)
    };
    let Some(encoding) = encoding else {
        return Ok(None);
    };
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

fn legacy_position_encoding(value: TextEncoding) -> Option<PositionEncoding> {
    match value {
        TextEncoding::UTF8 => Some(PositionEncoding::UTF8CodeUnitOffsetFromLineStart),
        TextEncoding::UTF16 => Some(PositionEncoding::UTF16CodeUnitOffsetFromLineStart),
        TextEncoding::UnspecifiedTextEncoding => None,
    }
}

fn document_language(document: &Document, fallback: Option<&str>) -> Option<String> {
    (!document.language.is_empty())
        .then(|| document.language.clone())
        .or_else(|| fallback.map(str::to_owned))
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
        Some(
            symbol_information::Kind::Method
            | symbol_information::Kind::Constructor
            | symbol_information::Kind::AbstractMethod
            | symbol_information::Kind::StaticMethod
            | symbol_information::Kind::Getter
            | symbol_information::Kind::Setter,
        ) => EntityKind::Method,
        Some(
            symbol_information::Kind::Field
            | symbol_information::Kind::Property
            | symbol_information::Kind::StaticField
            | symbol_information::Kind::StaticProperty
            | symbol_information::Kind::Parameter,
        ) => EntityKind::Field,
        Some(
            symbol_information::Kind::Constant
            | symbol_information::Kind::Variable
            | symbol_information::Kind::StaticVariable
            | symbol_information::Kind::Value,
        ) => EntityKind::Constant,
        Some(symbol_information::Kind::Module) => EntityKind::Module,
        Some(symbol_information::Kind::Namespace) => EntityKind::Namespace,
        Some(symbol_information::Kind::Type | symbol_information::Kind::TypeAlias) => {
            EntityKind::Schema
        }
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

    #[test]
    fn upgrades_legacy_metadata_encoding_per_document() {
        let mut metadata = Metadata::new();
        metadata.text_document_encoding = EnumOrUnknown::new(TextEncoding::UTF8);
        let mut index = Index::new();
        index.metadata = MessageField::some(metadata);
        index.documents.push(Document::new());

        apply_legacy_encoding(&mut index).expect("upgrade encoding");

        assert_eq!(
            index.documents[0]
                .position_encoding
                .enum_value()
                .expect("known encoding"),
            PositionEncoding::UTF8CodeUnitOffsetFromLineStart
        );
    }

    #[test]
    fn merged_scip_indexes_retain_one_richest_document_per_path() {
        let mut sparse_document = Document::new();
        sparse_document.relative_path = "src/index.js".to_owned();
        sparse_document.position_encoding =
            EnumOrUnknown::new(PositionEncoding::UTF8CodeUnitOffsetFromLineStart);
        sparse_document.occurrences.push(Occurrence::new());
        let mut sparse = Index::new();
        sparse.documents.push(sparse_document);

        let mut rich_document = Document::new();
        rich_document.relative_path = "src/index.js".to_owned();
        rich_document.position_encoding =
            EnumOrUnknown::new(PositionEncoding::UTF8CodeUnitOffsetFromLineStart);
        rich_document.occurrences.push(Occurrence::new());
        rich_document.occurrences.push(Occurrence::new());
        let mut rich = Index::new();
        rich.documents.push(rich_document);

        let bytes = merge_scip_indexes(vec![
            sparse.write_to_bytes().expect("serialize sparse"),
            rich.write_to_bytes().expect("serialize rich"),
        ])
        .expect("merge indexes");
        let merged = Index::parse_from_bytes(&bytes).expect("parse merged");

        assert_eq!(merged.documents.len(), 1);
        assert_eq!(merged.documents[0].occurrences.len(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn runtime_executable_preserves_multicall_symlink_name() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let target = directory.path().join("rustup");
        std::fs::write(&target, b"launcher").expect("launcher");
        let symlink = directory.path().join("rust-analyzer");
        std::os::unix::fs::symlink(&target, &symlink).expect("symlink");

        let resolved = resolve_runtime_executable(&symlink).expect("resolved executable");

        assert_eq!(
            resolved.file_name().and_then(|name| name.to_str()),
            Some("rust-analyzer")
        );
    }
}
