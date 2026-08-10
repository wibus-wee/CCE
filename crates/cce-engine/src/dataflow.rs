use std::{
    collections::{HashMap, VecDeque},
    path::Path,
    process::Stdio,
    time::Duration,
};

use cce_core::{
    CceError, CodeEntity, EntityKind, Relation, RelationKind, RelationOrigin, Result, SourceAddress,
};
use serde::{Deserialize, Serialize};
use tokio::{process::Command, time::timeout};
use walkdir::WalkDir;

use crate::{DataflowBackendConfig, ScannedFile, ScannedRepository};

const MAX_DATAFLOW_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_DATAFLOW_NODES: usize = 5_000_000;
const MAX_DATAFLOW_EDGES: usize = 20_000_000;

#[derive(Debug)]
pub(crate) struct DataflowImport {
    pub bytes: Vec<u8>,
    pub entities: Vec<CodeEntity>,
    pub relations: Vec<Relation>,
    pub nodes: usize,
    pub edges: usize,
    pub skipped_edges: usize,
    pub generator: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DataflowArtifact {
    schema_version: u32,
    repository_id: String,
    workspace_overlay_hash: String,
    base_revision: Option<String>,
    generator: Generator,
    files: Vec<ArtifactFile>,
    nodes: Vec<ArtifactNode>,
    edges: Vec<ArtifactEdge>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Generator {
    name: String,
    version: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ArtifactFile {
    path: String,
    content_hash: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ArtifactNode {
    id: String,
    kind: String,
    name: String,
    address: ArtifactAddress,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ArtifactAddress {
    path: String,
    start_byte: u64,
    end_byte: u64,
    start_line: u32,
    end_line: u32,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
enum ArtifactEdgeKind {
    #[serde(rename = "data_flow")]
    Data,
    #[serde(rename = "taint_flow")]
    Taint,
    #[serde(rename = "control_flow")]
    Control,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ArtifactEdge {
    id: String,
    source_node_id: String,
    target_node_id: String,
    kind: ArtifactEdgeKind,
    #[serde(default = "default_confidence")]
    confidence: f32,
    #[serde(default)]
    evidence: Vec<ArtifactAddress>,
}

const fn default_confidence() -> f32 {
    1.0
}

pub(crate) async fn build(
    config: &DataflowBackendConfig,
    repository_root: &Path,
    data_root: &Path,
    scanned: &ScannedRepository,
    existing_entities: &[CodeEntity],
) -> Result<Option<DataflowImport>> {
    let (bytes, skipped_edges) = match config {
        DataflowBackendConfig::Disabled => return Ok(None),
        DataflowBackendConfig::Supplied { path } => {
            let metadata = std::fs::metadata(path).map_err(|error| CceError::io(path, error))?;
            if metadata.len() > MAX_DATAFLOW_BYTES {
                return Err(CceError::Configuration(format!(
                    "dataflow artifact {} exceeds the 2 GiB safety limit",
                    path.display()
                )));
            }
            (
                std::fs::read(path).map_err(|error| CceError::io(path, error))?,
                0,
            )
        }
        DataflowBackendConfig::Joern {
            parse_executable,
            export_executable,
            version,
            timeout_seconds,
            language,
        } => {
            generate_with_joern(
                JoernExecutables {
                    parse: parse_executable,
                    export: export_executable,
                },
                version,
                *timeout_seconds,
                repository_root,
                data_root,
                scanned,
                language.as_deref(),
            )
            .await?
        }
    };
    let artifact = serde_json::from_slice::<DataflowArtifact>(&bytes).map_err(|error| {
        CceError::ArtifactCorrupt(format!("invalid CCE dataflow JSON: {error}"))
    })?;
    validate_header(&artifact, scanned)?;
    if artifact.nodes.len() > MAX_DATAFLOW_NODES || artifact.edges.len() > MAX_DATAFLOW_EDGES {
        return Err(CceError::Configuration(format!(
            "dataflow artifact exceeds node/edge limits ({}/{})",
            artifact.nodes.len(),
            artifact.edges.len()
        )));
    }
    let files = scanned
        .files
        .iter()
        .map(|file| (file.relative_path.as_str(), file))
        .collect::<HashMap<_, _>>();
    let declared_files = validate_files(&artifact.files, &files)?;
    let artifact_digest = blake3::hash(&bytes).to_hex().to_string();
    let entities_by_path = existing_entities_by_path(existing_entities);
    let mut node_ids = HashMap::new();
    let mut entities = Vec::with_capacity(artifact.nodes.len());
    let mut relations = Vec::with_capacity(artifact.edges.len() + artifact.nodes.len());
    for node in &artifact.nodes {
        if node.id.is_empty() || node_ids.contains_key(&node.id) {
            return Err(CceError::ArtifactCorrupt(format!(
                "duplicate or empty dataflow node id: {}",
                node.id
            )));
        }
        let address = validate_address(&node.address, scanned, &files, &declared_files)?;
        let entity_id = digest_id(
            "entity",
            &[&scanned.identity.id, "dataflow", &artifact_digest, &node.id],
        );
        node_ids.insert(node.id.clone(), entity_id.clone());
        entities.push(CodeEntity {
            id: entity_id.clone(),
            kind: node_entity_kind(&node.kind),
            name: node.name.clone(),
            qualified_name: Some(format!("{}::{}", address.path, node.id)),
            signature: None,
            language: files
                .get(address.path.as_str())
                .and_then(|file| file.language.clone()),
            address: Some(address.clone().with_symbol(entity_id.clone())),
            capabilities: vec!["precise_dataflow_node".to_owned()],
            attributes: serde_json::Map::from_iter([
                ("analysisNodeId".to_owned(), node.id.clone().into()),
                ("analysisNodeKind".to_owned(), node.kind.clone().into()),
                ("artifactDigest".to_owned(), artifact_digest.clone().into()),
            ]),
        });
        if let Some(parent) = enclosing_entity(&entities_by_path, &address) {
            relations.push(Relation {
                id: digest_id("rel", &[&parent.id, &entity_id, "contains", "dataflow"]),
                source_entity_id: parent.id.clone(),
                target_entity_id: entity_id,
                kind: RelationKind::Contains,
                origin: RelationOrigin::StaticAnalysis,
                confidence: 1.0,
                snapshot_id: scanned.snapshot.id.clone(),
                extractor: format!("{}@{}", artifact.generator.name, artifact.generator.version),
                evidence: vec![address],
                attributes: serde_json::Map::from_iter([(
                    "artifactDigest".to_owned(),
                    artifact_digest.clone().into(),
                )]),
            });
        }
    }
    let mut edge_ids = std::collections::HashSet::new();
    for edge in &artifact.edges {
        if edge.id.is_empty() || !edge_ids.insert(&edge.id) {
            return Err(CceError::ArtifactCorrupt(format!(
                "duplicate or empty dataflow edge id: {}",
                edge.id
            )));
        }
        if !edge.confidence.is_finite() || !(0.0..=1.0).contains(&edge.confidence) {
            return Err(CceError::ArtifactCorrupt(format!(
                "invalid dataflow confidence for edge {}",
                edge.id
            )));
        }
        let source = node_ids.get(&edge.source_node_id).ok_or_else(|| {
            CceError::ArtifactCorrupt(format!(
                "dataflow edge {} has unknown source {}",
                edge.id, edge.source_node_id
            ))
        })?;
        let target = node_ids.get(&edge.target_node_id).ok_or_else(|| {
            CceError::ArtifactCorrupt(format!(
                "dataflow edge {} has unknown target {}",
                edge.id, edge.target_node_id
            ))
        })?;
        let evidence = edge
            .evidence
            .iter()
            .map(|address| validate_address(address, scanned, &files, &declared_files))
            .collect::<Result<Vec<_>>>()?;
        relations.push(Relation {
            id: digest_id("rel", &[&artifact_digest, &edge.id]),
            source_entity_id: source.clone(),
            target_entity_id: target.clone(),
            kind: match edge.kind {
                ArtifactEdgeKind::Data => RelationKind::DataFlowsTo,
                ArtifactEdgeKind::Taint => RelationKind::TaintFlowsTo,
                ArtifactEdgeKind::Control => RelationKind::ControlFlowsTo,
            },
            origin: RelationOrigin::StaticAnalysis,
            confidence: edge.confidence,
            snapshot_id: scanned.snapshot.id.clone(),
            extractor: format!("{}@{}", artifact.generator.name, artifact.generator.version),
            evidence,
            attributes: serde_json::Map::from_iter([
                ("analysisEdgeId".to_owned(), edge.id.clone().into()),
                ("artifactDigest".to_owned(), artifact_digest.clone().into()),
            ]),
        });
    }
    Ok(Some(DataflowImport {
        bytes,
        entities,
        relations,
        nodes: artifact.nodes.len(),
        edges: artifact.edges.len(),
        skipped_edges,
        generator: format!("{}@{}", artifact.generator.name, artifact.generator.version),
    }))
}

#[derive(Clone, Copy)]
struct JoernExecutables<'a> {
    parse: &'a Path,
    export: &'a Path,
}

#[derive(Debug, Default)]
struct GraphNode {
    id: String,
    properties: HashMap<String, String>,
}

#[derive(Debug, Default)]
struct GraphEdge {
    id: String,
    source: String,
    target: String,
    properties: HashMap<String, String>,
}

async fn generate_with_joern(
    executables: JoernExecutables<'_>,
    version: &str,
    timeout_seconds: u64,
    repository_root: &Path,
    data_root: &Path,
    scanned: &ScannedRepository,
    configured_language: Option<&str>,
) -> Result<(Vec<u8>, usize)> {
    let temporary_root = data_root.join("tmp");
    std::fs::create_dir_all(&temporary_root)
        .map_err(|error| CceError::io(&temporary_root, error))?;
    let temporary = tempfile::Builder::new()
        .prefix("joern-dataflow-")
        .tempdir_in(&temporary_root)
        .map_err(|error| CceError::io(&temporary_root, error))?;
    let inferred_javascript = configured_language.is_none()
        && scanned.files.iter().any(|file| {
            matches!(
                file.language.as_deref(),
                Some("javascript" | "typescript" | "tsx")
            )
        })
        && !scanned.files.iter().any(|file| {
            matches!(
                file.language.as_deref(),
                Some("rust" | "c" | "cpp" | "python" | "go" | "java" | "csharp")
            )
        });
    let language = configured_language.or(inferred_javascript.then_some("JAVASCRIPT"));
    let mut parse_arguments = vec![repository_root.as_os_str()];
    if let Some(language) = language {
        parse_arguments.push(std::ffi::OsStr::new("--language"));
        parse_arguments.push(std::ffi::OsStr::new(language));
    }
    run_local_analysis_command(
        executables.parse,
        &parse_arguments,
        temporary.path(),
        timeout_seconds,
        "joern-parse",
    )
    .await?;
    let cpg = WalkDir::new(temporary.path())
        .max_depth(3)
        .into_iter()
        .filter_map(std::result::Result::ok)
        .find(|entry| entry.file_type().is_file() && entry.file_name() == "cpg.bin")
        .map(walkdir::DirEntry::into_path)
        .ok_or_else(|| {
            CceError::ArtifactCorrupt(
                "joern-parse completed without producing a local cpg.bin".to_owned(),
            )
        })?;
    let export_directory = temporary.path().join("graphml");
    let arguments = [
        cpg.as_os_str(),
        std::ffi::OsStr::new("--repr"),
        std::ffi::OsStr::new("all"),
        std::ffi::OsStr::new("--format"),
        std::ffi::OsStr::new("graphml"),
        std::ffi::OsStr::new("--out"),
        export_directory.as_os_str(),
    ];
    run_local_analysis_command(
        executables.export,
        &arguments,
        temporary.path(),
        timeout_seconds,
        "joern-export",
    )
    .await?;
    let graph_files = WalkDir::new(&export_directory)
        .into_iter()
        .filter_map(std::result::Result::ok)
        .filter(|entry| {
            entry.file_type().is_file()
                && entry.path().extension().is_some_and(|extension| {
                    extension.eq_ignore_ascii_case("graphml")
                        || extension.eq_ignore_ascii_case("xml")
                })
        })
        .map(walkdir::DirEntry::into_path)
        .collect::<Vec<_>>();
    if graph_files.is_empty() {
        return Err(CceError::ArtifactCorrupt(
            "joern-export completed without producing GraphML".to_owned(),
        ));
    }
    let mut graph_nodes = Vec::new();
    let mut graph_edges = Vec::new();
    let mut total_bytes = 0_u64;
    for path in graph_files {
        let metadata = std::fs::metadata(&path).map_err(|error| CceError::io(&path, error))?;
        total_bytes = total_bytes.saturating_add(metadata.len());
        if total_bytes > MAX_DATAFLOW_BYTES {
            return Err(CceError::Configuration(
                "Joern GraphML output exceeds the 2 GiB safety limit".to_owned(),
            ));
        }
        let bytes = std::fs::read(&path).map_err(|error| CceError::io(&path, error))?;
        let (mut nodes, mut edges) = parse_graphml(&bytes)?;
        graph_nodes.append(&mut nodes);
        graph_edges.append(&mut edges);
    }
    joern_artifact(version, repository_root, scanned, graph_nodes, graph_edges)
}

async fn run_local_analysis_command(
    executable: &Path,
    arguments: &[&std::ffi::OsStr],
    current_directory: &Path,
    timeout_seconds: u64,
    name: &str,
) -> Result<()> {
    let diagnostic_path = current_directory.join(format!("{name}.stderr.log"));
    let diagnostic = std::fs::File::create(&diagnostic_path)
        .map_err(|error| CceError::io(&diagnostic_path, error))?;
    let mut command = Command::new(executable);
    command
        .args(arguments)
        .current_dir(current_directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(diagnostic))
        .kill_on_drop(true);
    let status = timeout(Duration::from_secs(timeout_seconds), command.status())
        .await
        .map_err(|_| {
            CceError::Configuration(format!(
                "{name} exceeded the configured {timeout_seconds}s timeout"
            ))
        })?
        .map_err(|error| {
            CceError::Configuration(format!(
                "failed to run local analyzer {}: {error}",
                executable.display()
            ))
        })?;
    if !status.success() {
        let diagnostic = std::fs::read_to_string(&diagnostic_path)
            .unwrap_or_else(|error| format!("failed to read bounded analyzer diagnostic: {error}"));
        return Err(CceError::Configuration(format!(
            "{name} exited with {}: {}",
            status,
            truncate_diagnostic(&diagnostic, 4_000)
        )));
    }
    Ok(())
}

fn truncate_diagnostic(value: &str, maximum: usize) -> String {
    value.chars().take(maximum).collect()
}

fn parse_graphml(bytes: &[u8]) -> Result<(Vec<GraphNode>, Vec<GraphEdge>)> {
    let mut keys = HashMap::<String, String>::new();
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut node = None::<GraphNode>;
    let mut edge = None::<GraphEdge>;
    let mut data_key = None::<String>;
    let mut data_value = String::new();
    let mut position = 0_usize;
    while position < bytes.len() {
        let Some(open_offset) = bytes[position..].iter().position(|byte| *byte == b'<') else {
            break;
        };
        let open = position + open_offset;
        if data_key.is_some() && open > position {
            data_value.push_str(
                std::str::from_utf8(&bytes[position..open]).map_err(|error| {
                    CceError::ArtifactCorrupt(format!("GraphML text is not UTF-8: {error}"))
                })?,
            );
        }
        if bytes[open..].starts_with(b"<!--") {
            let end = find_bytes(bytes, open + 4, b"-->").ok_or_else(|| {
                CceError::ArtifactCorrupt("unterminated GraphML comment".to_owned())
            })?;
            position = end + 3;
            continue;
        }
        if bytes[open..].starts_with(b"<?") {
            let end = find_bytes(bytes, open + 2, b"?>").ok_or_else(|| {
                CceError::ArtifactCorrupt("unterminated GraphML processing instruction".to_owned())
            })?;
            position = end + 2;
            continue;
        }
        if bytes[open..].starts_with(b"<![CDATA[") {
            let start = open + 9;
            let end = find_bytes(bytes, start, b"]]>").ok_or_else(|| {
                CceError::ArtifactCorrupt("unterminated GraphML CDATA".to_owned())
            })?;
            if data_key.is_some() {
                data_value.push_str(std::str::from_utf8(&bytes[start..end]).map_err(|error| {
                    CceError::ArtifactCorrupt(format!("GraphML CDATA is not UTF-8: {error}"))
                })?);
            }
            position = end + 3;
            continue;
        }
        if bytes[open..].starts_with(b"<!") {
            let end = find_tag_end(bytes, open + 2).ok_or_else(|| {
                CceError::ArtifactCorrupt("unterminated GraphML declaration".to_owned())
            })?;
            position = end + 1;
            continue;
        }
        let close = find_tag_end(bytes, open + 1)
            .ok_or_else(|| CceError::ArtifactCorrupt("unterminated GraphML tag".to_owned()))?;
        let raw = std::str::from_utf8(&bytes[open + 1..close]).map_err(|error| {
            CceError::ArtifactCorrupt(format!("GraphML tag is not UTF-8: {error}"))
        })?;
        let tag = parse_xml_tag(raw)?;
        match (tag.closing, tag.name.as_str()) {
            (false, "key") => {
                if let Some(id) = tag.attributes.get("id") {
                    keys.insert(
                        id.clone(),
                        tag.attributes
                            .get("attr.name")
                            .cloned()
                            .unwrap_or_else(|| id.clone()),
                    );
                }
            }
            (false, "node") => {
                let mut value = GraphNode {
                    properties: tag.attributes,
                    ..GraphNode::default()
                };
                value.id = value.properties.remove("id").unwrap_or_default();
                node = Some(value);
            }
            (false, "edge") => {
                let mut value = GraphEdge {
                    properties: tag.attributes,
                    ..GraphEdge::default()
                };
                value.id = value.properties.remove("id").unwrap_or_default();
                value.source = value.properties.remove("source").unwrap_or_default();
                value.target = value.properties.remove("target").unwrap_or_default();
                edge = Some(value);
            }
            (false, "data") => {
                data_key = tag.attributes.get("key").cloned();
                data_value.clear();
            }
            (true, "data") => {
                if let Some(key) = data_key.take() {
                    let name = keys.get(&key).cloned().unwrap_or(key);
                    let value = unescape_xml(data_value.trim())?;
                    if let Some(node) = node.as_mut() {
                        node.properties.insert(name, value);
                    } else if let Some(edge) = edge.as_mut() {
                        edge.properties.insert(name, value);
                    }
                }
                data_value.clear();
            }
            (true, "node") => {
                if let Some(node) = node.take()
                    && !node.id.is_empty()
                {
                    nodes.push(node);
                }
            }
            (true, "edge") => {
                if let Some(edge) = edge.take()
                    && !edge.source.is_empty()
                    && !edge.target.is_empty()
                {
                    edges.push(edge);
                }
            }
            _ => {}
        }
        position = close + 1;
    }
    Ok((nodes, edges))
}

#[derive(Debug)]
struct XmlTag {
    name: String,
    closing: bool,
    attributes: HashMap<String, String>,
}

fn parse_xml_tag(raw: &str) -> Result<XmlTag> {
    let raw = raw.trim();
    let closing = raw.starts_with('/');
    let raw = raw.trim_start_matches('/').trim_end_matches('/').trim();
    let name_end = raw.find(char::is_whitespace).unwrap_or(raw.len());
    let qualified_name = &raw[..name_end];
    let name = qualified_name
        .rsplit(':')
        .next()
        .unwrap_or(qualified_name)
        .to_ascii_lowercase();
    Ok(XmlTag {
        name,
        closing,
        attributes: parse_xml_attributes(&raw[name_end..])?,
    })
}

fn parse_xml_attributes(raw: &str) -> Result<HashMap<String, String>> {
    let bytes = raw.as_bytes();
    let mut attributes = HashMap::new();
    let mut position = 0_usize;
    while position < bytes.len() {
        while position < bytes.len() && bytes[position].is_ascii_whitespace() {
            position += 1;
        }
        if position == bytes.len() {
            break;
        }
        let name_start = position;
        while position < bytes.len()
            && !bytes[position].is_ascii_whitespace()
            && bytes[position] != b'='
        {
            position += 1;
        }
        let name = &raw[name_start..position];
        while position < bytes.len() && bytes[position].is_ascii_whitespace() {
            position += 1;
        }
        if position == bytes.len() || bytes[position] != b'=' {
            return Err(CceError::ArtifactCorrupt(format!(
                "GraphML attribute {name} lacks '='"
            )));
        }
        position += 1;
        while position < bytes.len() && bytes[position].is_ascii_whitespace() {
            position += 1;
        }
        if position == bytes.len() || !matches!(bytes[position], b'\'' | b'"') {
            return Err(CceError::ArtifactCorrupt(format!(
                "GraphML attribute {name} is not quoted"
            )));
        }
        let quote = bytes[position];
        position += 1;
        let value_start = position;
        while position < bytes.len() && bytes[position] != quote {
            position += 1;
        }
        if position == bytes.len() {
            return Err(CceError::ArtifactCorrupt(format!(
                "GraphML attribute {name} is unterminated"
            )));
        }
        attributes.insert(name.to_owned(), unescape_xml(&raw[value_start..position])?);
        position += 1;
    }
    Ok(attributes)
}

fn unescape_xml(value: &str) -> Result<String> {
    let mut output = String::with_capacity(value.len());
    let mut remainder = value;
    while let Some(offset) = remainder.find('&') {
        output.push_str(&remainder[..offset]);
        remainder = &remainder[offset + 1..];
        let end = remainder
            .find(';')
            .ok_or_else(|| CceError::ArtifactCorrupt("unterminated XML entity".to_owned()))?;
        let entity = &remainder[..end];
        match entity {
            "amp" => output.push('&'),
            "lt" => output.push('<'),
            "gt" => output.push('>'),
            "quot" => output.push('"'),
            "apos" => output.push('\''),
            value if value.starts_with("#x") => output.push(
                char::from_u32(u32::from_str_radix(&value[2..], 16).map_err(|_| {
                    CceError::ArtifactCorrupt(format!("invalid XML entity &{entity};"))
                })?)
                .ok_or_else(|| {
                    CceError::ArtifactCorrupt(format!("invalid XML scalar &{entity};"))
                })?,
            ),
            value if value.starts_with('#') => output.push(
                char::from_u32(value[1..].parse::<u32>().map_err(|_| {
                    CceError::ArtifactCorrupt(format!("invalid XML entity &{entity};"))
                })?)
                .ok_or_else(|| {
                    CceError::ArtifactCorrupt(format!("invalid XML scalar &{entity};"))
                })?,
            ),
            _ => {
                return Err(CceError::ArtifactCorrupt(format!(
                    "unsupported XML entity &{entity};"
                )));
            }
        }
        remainder = &remainder[end + 1..];
    }
    output.push_str(remainder);
    Ok(output)
}

fn find_tag_end(bytes: &[u8], mut position: usize) -> Option<usize> {
    let mut quote = None::<u8>;
    while position < bytes.len() {
        match (quote, bytes[position]) {
            (None, b'\'' | b'"') => quote = Some(bytes[position]),
            (Some(active), value) if active == value => quote = None,
            (None, b'>') => return Some(position),
            _ => {}
        }
        position += 1;
    }
    None
}

fn find_bytes(haystack: &[u8], start: usize, needle: &[u8]) -> Option<usize> {
    haystack
        .get(start..)?
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|offset| start + offset)
}

fn joern_artifact(
    version: &str,
    repository_root: &Path,
    scanned: &ScannedRepository,
    graph_nodes: Vec<GraphNode>,
    graph_edges: Vec<GraphEdge>,
) -> Result<(Vec<u8>, usize)> {
    let current_files = scanned
        .files
        .iter()
        .map(|file| (file.relative_path.as_str(), file))
        .collect::<HashMap<_, _>>();
    let resolved_paths =
        resolve_joern_paths(&graph_nodes, &graph_edges, repository_root, &current_files);
    let mut nodes = HashMap::<String, ArtifactNode>::new();
    for node in graph_nodes {
        if let Some(aligned) = align_joern_node(
            &node,
            resolved_paths.get(&node.id).map(String::as_str),
            &current_files,
        )? {
            nodes.entry(node.id).or_insert(aligned);
        }
    }
    let mut referenced_nodes = std::collections::HashSet::new();
    let mut edges = Vec::new();
    let mut skipped_edges = 0_usize;
    let mut edge_ids = std::collections::HashSet::new();
    for (offset, edge) in graph_edges.into_iter().enumerate() {
        let Some(kind) = joern_edge_kind(&edge) else {
            continue;
        };
        let (Some(source), Some(target)) = (nodes.get(&edge.source), nodes.get(&edge.target))
        else {
            skipped_edges += 1;
            continue;
        };
        referenced_nodes.insert(edge.source.clone());
        referenced_nodes.insert(edge.target.clone());
        let mut id = if edge.id.is_empty() {
            digest_id(
                "joern-edge",
                &[&edge.source, &edge.target, &offset.to_string()],
            )
        } else {
            edge.id
        };
        if !edge_ids.insert(id.clone()) {
            id = digest_id("joern-edge", &[&id, &offset.to_string()]);
            edge_ids.insert(id.clone());
        }
        edges.push(ArtifactEdge {
            id,
            source_node_id: edge.source,
            target_node_id: edge.target,
            kind,
            confidence: 1.0,
            evidence: vec![source.address.clone(), target.address.clone()],
        });
    }
    let mut artifact_nodes = referenced_nodes
        .into_iter()
        .filter_map(|id| nodes.remove(&id))
        .collect::<Vec<_>>();
    artifact_nodes.sort_by(|left, right| left.id.cmp(&right.id));
    edges.sort_by(|left, right| left.id.cmp(&right.id));
    let mut used_paths = artifact_nodes
        .iter()
        .map(|node| node.address.path.as_str())
        .collect::<Vec<_>>();
    used_paths.sort_unstable();
    used_paths.dedup();
    let files = used_paths
        .into_iter()
        .filter_map(|path| {
            current_files.get(path).map(|file| ArtifactFile {
                path: path.to_owned(),
                content_hash: file.content_hash.clone(),
            })
        })
        .collect();
    let artifact = DataflowArtifact {
        schema_version: 1,
        repository_id: scanned.identity.id.clone(),
        workspace_overlay_hash: scanned.snapshot.workspace_overlay_hash.clone(),
        base_revision: scanned.snapshot.base_revision.clone(),
        generator: Generator {
            name: "joern-pdg-graphml".to_owned(),
            version: version.to_owned(),
        },
        files,
        nodes: artifact_nodes,
        edges,
    };
    Ok((serde_json::to_vec(&artifact)?, skipped_edges))
}

fn joern_edge_kind(edge: &GraphEdge) -> Option<ArtifactEdgeKind> {
    let label = graph_property(&edge.properties, &["labelE", "label", "TYPE", ":TYPE"])?;
    match label.trim().to_ascii_uppercase().as_str() {
        "REACHING_DEF" | "DDG" => Some(ArtifactEdgeKind::Data),
        "CDG" => Some(ArtifactEdgeKind::Control),
        _ => None,
    }
}

fn align_joern_node(
    node: &GraphNode,
    resolved_path: Option<&str>,
    files: &HashMap<&str, &ScannedFile>,
) -> Result<Option<ArtifactNode>> {
    let Some(path) = resolved_path.map(str::to_owned) else {
        return Ok(None);
    };
    let Some(file) = files.get(path.as_str()).copied() else {
        return Ok(None);
    };
    let Some(line) = graph_property(&node.properties, &["LINE_NUMBER", "lineNumber"])
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|value| *value > 0)
    else {
        return Ok(None);
    };
    let code = graph_property(&node.properties, &["CODE", "code"])
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let column = graph_property(&node.properties, &["COLUMN_NUMBER", "columnNumber"])
        .and_then(|value| value.parse::<usize>().ok());
    let text = file.text()?;
    let Some((start, end)) = align_source_range(text, line, column, code) else {
        return Ok(None);
    };
    let address = ArtifactAddress {
        path,
        start_byte: start as u64,
        end_byte: end as u64,
        start_line: line_for_offset(text, start),
        end_line: line_for_offset(text, end),
    };
    let label =
        graph_property(&node.properties, &["labelV", "label", ":LABEL"]).unwrap_or("expression");
    let name = graph_property(&node.properties, &["NAME", "name"])
        .or(code)
        .unwrap_or(label)
        .to_owned();
    Ok(Some(ArtifactNode {
        id: node.id.clone(),
        kind: joern_node_kind(label).to_owned(),
        name,
        address,
    }))
}

fn resolve_joern_paths(
    nodes: &[GraphNode],
    edges: &[GraphEdge],
    repository_root: &Path,
    files: &HashMap<&str, &ScannedFile>,
) -> HashMap<String, String> {
    let mut resolved = HashMap::<String, String>::new();
    let mut queue = VecDeque::new();
    for node in nodes {
        let label = graph_property(&node.properties, &["labelV", "label", ":LABEL"]);
        let raw_path = graph_property(&node.properties, &["FILENAME", "filename"]).or_else(|| {
            label
                .is_some_and(|value| value.eq_ignore_ascii_case("FILE"))
                .then(|| graph_property(&node.properties, &["NAME", "name"]))
                .flatten()
        });
        if let Some(path) =
            raw_path.and_then(|value| normalize_joern_path(value, repository_root, files))
        {
            resolved.insert(node.id.clone(), path);
            queue.push_back(node.id.clone());
        }
    }

    let mut outgoing = HashMap::<&str, Vec<&str>>::new();
    for edge in edges {
        let Some(label) = graph_property(&edge.properties, &["labelE", "label", "TYPE", ":TYPE"])
        else {
            continue;
        };
        match label.trim().to_ascii_uppercase().as_str() {
            "AST" | "CONTAINS" => outgoing
                .entry(edge.source.as_str())
                .or_default()
                .push(edge.target.as_str()),
            "SOURCE_FILE" => {
                outgoing
                    .entry(edge.source.as_str())
                    .or_default()
                    .push(edge.target.as_str());
                outgoing
                    .entry(edge.target.as_str())
                    .or_default()
                    .push(edge.source.as_str());
            }
            _ => {}
        }
    }
    while let Some(source) = queue.pop_front() {
        let Some(path) = resolved.get(&source).cloned() else {
            continue;
        };
        for target in outgoing.get(source.as_str()).into_iter().flatten() {
            if !resolved.contains_key(*target) {
                resolved.insert((*target).to_owned(), path.clone());
                queue.push_back((*target).to_owned());
            }
        }
    }
    resolved
}

fn graph_property<'a>(properties: &'a HashMap<String, String>, names: &[&str]) -> Option<&'a str> {
    properties.iter().find_map(|(key, value)| {
        names
            .iter()
            .any(|name| key.eq_ignore_ascii_case(name))
            .then_some(value.as_str())
    })
}

fn normalize_joern_path(
    raw: &str,
    repository_root: &Path,
    files: &HashMap<&str, &ScannedFile>,
) -> Option<String> {
    let raw = raw
        .strip_prefix("file://")
        .unwrap_or(raw)
        .replace('\\', "/");
    let root = repository_root.to_string_lossy().replace('\\', "/");
    let relative = raw
        .strip_prefix(&root)
        .map(|value| value.trim_start_matches('/'))
        .unwrap_or(raw.trim_start_matches("./"));
    if files.contains_key(relative) {
        return Some(relative.to_owned());
    }
    let matches = files
        .keys()
        .filter(|path| raw.ends_with(path.trim_start_matches('/')))
        .copied()
        .collect::<Vec<_>>();
    (matches.len() == 1).then(|| matches[0].to_owned())
}

fn align_source_range(
    text: &str,
    line: u32,
    column: Option<usize>,
    code: Option<&str>,
) -> Option<(usize, usize)> {
    let line_index = usize::try_from(line.checked_sub(1)?).ok()?;
    let start = if line_index == 0 {
        0
    } else {
        text.match_indices('\n').nth(line_index - 1)?.0 + 1
    };
    let end = text[start..]
        .find('\n')
        .map_or(text.len(), |offset| start + offset);
    if let Some(code) = code {
        for candidate_column in column
            .into_iter()
            .flat_map(|value| [value, value.saturating_sub(1)])
        {
            let candidate = start
                + text[start..end]
                    .char_indices()
                    .nth(candidate_column)
                    .map_or(end - start, |(offset, _)| offset);
            if text
                .get(candidate..)
                .is_some_and(|tail| tail.starts_with(code))
            {
                return Some((candidate, candidate + code.len()));
            }
        }
        if let Some(offset) = text[start..end].find(code) {
            return Some((start + offset, start + offset + code.len()));
        }
    }
    let line_text = &text[start..end];
    let leading = line_text.len() - line_text.trim_start().len();
    let trailing = line_text.trim_end().len();
    (leading < trailing).then_some((start + leading, start + trailing))
}

fn joern_node_kind(label: &str) -> &'static str {
    match label.trim().to_ascii_uppercase().as_str() {
        "METHOD" => "function",
        "METHOD_PARAMETER_IN" | "METHOD_PARAMETER_OUT" => "parameter",
        "MEMBER" => "field",
        "LOCAL" => "local",
        _ => "expression",
    }
}

fn validate_header(artifact: &DataflowArtifact, scanned: &ScannedRepository) -> Result<()> {
    if artifact.schema_version != 1 {
        return Err(CceError::UnsupportedFormat {
            found: artifact.schema_version,
            supported: 1,
        });
    }
    if artifact.repository_id != scanned.identity.id
        || artifact.workspace_overlay_hash != scanned.snapshot.workspace_overlay_hash
        || artifact.base_revision != scanned.snapshot.base_revision
    {
        return Err(CceError::ArtifactCorrupt(format!(
            "dataflow artifact targets repository {} overlay {} revision {:?}; current source is {} overlay {} revision {:?}",
            artifact.repository_id,
            artifact.workspace_overlay_hash,
            artifact.base_revision,
            scanned.identity.id,
            scanned.snapshot.workspace_overlay_hash,
            scanned.snapshot.base_revision
        )));
    }
    if artifact.generator.name.trim().is_empty() || artifact.generator.version.trim().is_empty() {
        return Err(CceError::ArtifactCorrupt(
            "dataflow artifact generator name/version is required".to_owned(),
        ));
    }
    Ok(())
}

fn validate_files<'a>(
    declared: &'a [ArtifactFile],
    current: &HashMap<&str, &ScannedFile>,
) -> Result<HashMap<&'a str, &'a str>> {
    let mut files = HashMap::new();
    for file in declared {
        if files
            .insert(file.path.as_str(), file.content_hash.as_str())
            .is_some()
        {
            return Err(CceError::ArtifactCorrupt(format!(
                "duplicate dataflow file fingerprint: {}",
                file.path
            )));
        }
        let current = current.get(file.path.as_str()).ok_or_else(|| {
            CceError::ArtifactCorrupt(format!(
                "dataflow artifact references missing file {}",
                file.path
            ))
        })?;
        if current.content_hash != file.content_hash {
            return Err(CceError::ArtifactCorrupt(format!(
                "dataflow file hash mismatch for {}",
                file.path
            )));
        }
    }
    Ok(files)
}

fn validate_address(
    address: &ArtifactAddress,
    scanned: &ScannedRepository,
    files: &HashMap<&str, &ScannedFile>,
    declared_files: &HashMap<&str, &str>,
) -> Result<SourceAddress> {
    if !declared_files.contains_key(address.path.as_str()) {
        return Err(CceError::ArtifactCorrupt(format!(
            "dataflow address lacks a source fingerprint: {}",
            address.path
        )));
    }
    let file = files.get(address.path.as_str()).ok_or_else(|| {
        CceError::ArtifactCorrupt(format!(
            "dataflow address file is missing: {}",
            address.path
        ))
    })?;
    let text = file.text()?;
    let start = usize::try_from(address.start_byte).map_err(|_| {
        CceError::ArtifactCorrupt(format!("dataflow byte range overflows: {}", address.path))
    })?;
    let end = usize::try_from(address.end_byte).map_err(|_| {
        CceError::ArtifactCorrupt(format!("dataflow byte range overflows: {}", address.path))
    })?;
    if start > end
        || end > text.len()
        || !text.is_char_boundary(start)
        || !text.is_char_boundary(end)
        || address.start_line != line_for_offset(text, start)
        || address.end_line != line_for_offset(text, end)
    {
        return Err(CceError::InvalidSourceRange {
            path: address.path.clone(),
            start_byte: address.start_byte,
            end_byte: address.end_byte,
        });
    }
    SourceAddress::new(
        &scanned.identity.id,
        &scanned.snapshot.id,
        &address.path,
        address.start_byte..address.end_byte,
        address.start_line..=address.end_line,
    )
}

fn line_for_offset(text: &str, offset: usize) -> u32 {
    text.as_bytes()[..offset.min(text.len())]
        .iter()
        .filter(|byte| **byte == b'\n')
        .count() as u32
        + 1
}

fn existing_entities_by_path(entities: &[CodeEntity]) -> HashMap<&str, Vec<&CodeEntity>> {
    let mut by_path = HashMap::<&str, Vec<&CodeEntity>>::new();
    for entity in entities {
        if let Some(address) = &entity.address {
            by_path.entry(&address.path).or_default().push(entity);
        }
    }
    for values in by_path.values_mut() {
        values.sort_by_key(|entity| {
            entity
                .address
                .as_ref()
                .map_or(u64::MAX, SourceAddress::byte_len)
        });
    }
    by_path
}

fn enclosing_entity<'a>(
    entities: &'a HashMap<&str, Vec<&'a CodeEntity>>,
    address: &SourceAddress,
) -> Option<&'a CodeEntity> {
    entities
        .get(address.path.as_str())?
        .iter()
        .copied()
        .find(|entity| {
            entity.address.as_ref().is_some_and(|candidate| {
                candidate.start_byte <= address.start_byte && candidate.end_byte >= address.end_byte
            })
        })
}

fn node_entity_kind(kind: &str) -> EntityKind {
    match kind {
        "function" => EntityKind::Function,
        "method" => EntityKind::Method,
        "field" => EntityKind::Field,
        "constant" => EntityKind::Constant,
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

#[cfg(test)]
mod tests {
    use super::*;
    use cce_core::{RepositoryIdentity, SnapshotIdentity};
    use chrono::Utc;
    use std::path::PathBuf;

    #[test]
    fn rejects_non_finite_confidence() {
        assert!(!f32::NAN.is_finite());
        assert!(!(0.0..=1.0).contains(&1.1));
    }

    #[test]
    fn maps_precise_edge_kinds_without_inference() {
        assert!(matches!(ArtifactEdgeKind::Taint, ArtifactEdgeKind::Taint));
        assert_eq!(node_entity_kind("expression"), EntityKind::Unknown);
    }

    #[test]
    fn converts_joern_graphml_to_source_aligned_artifact() {
        let source = b"int main() {\n  int value = 1;\n  return value;\n}\n".to_vec();
        let content_hash = blake3::hash(&source).to_hex().to_string();
        let scanned = ScannedRepository {
            identity: RepositoryIdentity {
                id: "repo_test".to_owned(),
                canonical_root: "/repo".to_owned(),
                remote: None,
            },
            snapshot: SnapshotIdentity {
                id: "snapshot".to_owned(),
                repository_id: "repo_test".to_owned(),
                base_revision: Some("revision".to_owned()),
                workspace_overlay_hash: "overlay".to_owned(),
                index_profile_hash: "profile".to_owned(),
                created_at: Utc::now(),
                file_count: 1,
                source_bytes: source.len() as u64,
            },
            files: vec![ScannedFile {
                relative_path: "main.c".to_owned(),
                absolute_path: PathBuf::from("/repo/main.c"),
                language: Some("c".to_owned()),
                bytes: source,
                content_hash: content_hash.clone(),
                line_count: 4,
            }],
            skipped_large_files: Vec::new(),
            skipped_binary_files: Vec::new(),
        };
        let graphml = br#"<?xml version="1.0"?>
<graphml>
  <key id="labelV" for="node" attr.name="labelV" attr.type="string"/>
  <key id="filename" for="node" attr.name="FILENAME" attr.type="string"/>
  <key id="line" for="node" attr.name="LINE_NUMBER" attr.type="int"/>
  <key id="column" for="node" attr.name="COLUMN_NUMBER" attr.type="int"/>
  <key id="code" for="node" attr.name="CODE" attr.type="string"/>
  <key id="labelE" for="edge" attr.name="labelE" attr.type="string"/>
  <graph edgedefault="directed">
    <node id="n1"><data key="labelV">CALL</data><data key="filename">/repo/main.c</data><data key="line">2</data><data key="column">2</data><data key="code">int value = 1</data></node>
    <node id="n2"><data key="labelV">RETURN</data><data key="filename">/repo/main.c</data><data key="line">3</data><data key="column">2</data><data key="code">return value</data></node>
    <edge id="e1" source="n1" target="n2"><data key="labelE">REACHING_DEF</data></edge>
    <edge id="e2" source="n1" target="missing"><data key="labelE">CDG</data></edge>
    <edge id="ignored" source="n1" target="n2"><data key="labelE">CFG</data></edge>
  </graph>
</graphml>"#;
        let (nodes, edges) = parse_graphml(graphml).expect("parse GraphML");
        assert_eq!(nodes.len(), 2);
        assert_eq!(edges.len(), 3);
        let (bytes, skipped) =
            joern_artifact("joern-test", Path::new("/repo"), &scanned, nodes, edges)
                .expect("convert Joern");
        assert_eq!(skipped, 1);
        let artifact: DataflowArtifact = serde_json::from_slice(&bytes).expect("artifact JSON");
        validate_header(&artifact, &scanned).expect("header");
        assert_eq!(artifact.files.len(), 1);
        assert_eq!(artifact.files[0].content_hash, content_hash);
        assert_eq!(artifact.nodes.len(), 2);
        assert_eq!(artifact.edges.len(), 1);
        assert!(matches!(artifact.edges[0].kind, ArtifactEdgeKind::Data));
        assert_eq!(artifact.edges[0].evidence.len(), 2);
    }

    #[test]
    fn graphml_parser_unescapes_code_properties() {
        let graphml = br#"<graphml><key id="code" attr.name="CODE"/><graph><node id="n"><data key="code">a &lt; b &amp;&amp; b &gt; 0</data></node></graph></graphml>"#;
        let (nodes, _) = parse_graphml(graphml).expect("parse GraphML");
        assert_eq!(
            nodes[0].properties.get("CODE").map(String::as_str),
            Some("a < b && b > 0")
        );
    }

    #[test]
    fn propagates_joern_file_identity_through_ast_edges() {
        let file = ScannedFile {
            relative_path: "src/main.ts".to_owned(),
            absolute_path: PathBuf::from("/repo/src/main.ts"),
            language: Some("typescript".to_owned()),
            bytes: b"const value = input;\n".to_vec(),
            content_hash: "hash".to_owned(),
            line_count: 1,
        };
        let files = HashMap::from([("src/main.ts", &file)]);
        let nodes = vec![
            GraphNode {
                id: "method".to_owned(),
                properties: HashMap::from([
                    ("labelV".to_owned(), "METHOD".to_owned()),
                    ("FILENAME".to_owned(), "/repo/src/main.ts".to_owned()),
                ]),
            },
            GraphNode {
                id: "identifier".to_owned(),
                properties: HashMap::from([
                    ("labelV".to_owned(), "IDENTIFIER".to_owned()),
                    ("LINE_NUMBER".to_owned(), "1".to_owned()),
                    ("COLUMN_NUMBER".to_owned(), "6".to_owned()),
                    ("CODE".to_owned(), "value".to_owned()),
                ]),
            },
        ];
        let edges = vec![GraphEdge {
            id: "ast".to_owned(),
            source: "method".to_owned(),
            target: "identifier".to_owned(),
            properties: HashMap::from([("labelE".to_owned(), "AST".to_owned())]),
        }];

        let paths = resolve_joern_paths(&nodes, &edges, Path::new("/repo"), &files);

        assert_eq!(
            paths.get("identifier").map(String::as_str),
            Some("src/main.ts")
        );
    }
}
