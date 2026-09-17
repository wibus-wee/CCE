use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::SourceAddress;

/// The kind of edge between two entities.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RelationKind {
    /// Containment (file contains symbol, directory contains file, …).
    Contains,
    /// The source defines the target.
    Defines,
    /// The source imports the target.
    Imports,
    /// The source re-exports the target.
    Exports,
    /// The source references the target (uses, reads, or names it).
    References,
    /// The source implements the target trait/interface.
    Implements,
    /// The source extends/inherits the target.
    Extends,
    /// The source uses the target as a type.
    TypeUses,
    /// The source instantiates the target generic/type.
    Instantiates,
    /// The source calls the target.
    Calls,
    /// The source tests the target.
    Tests,
    /// Build-level dependency between packages/targets.
    BuildDependsOn,
    /// An HTTP/IPC route handled by the target handler.
    RouteHandledBy,
    /// The source publishes the target event.
    PublishesEvent,
    /// The source subscribes to the target event.
    SubscribesEvent,
    /// The source reads from the target store/state.
    ReadsStore,
    /// The source writes to the target store/state.
    WritesStore,
    /// The source uses the target hook.
    UsesHook,
    /// The source serializes as the target schema.
    SerializesAs,
    /// The source persists to the target storage.
    PersistsTo,
    /// The source and target were changed together (co-change).
    ChangedWith,
    /// Semantic similarity association.
    Semantic,
    /// A kind not in the built-in set.
    Other(String),
}

/// How an edge was derived — deterministic fact vs derived vs inferred must
/// remain distinguishable for provenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RelationOrigin {
    /// Emitted by a language compiler/frontend.
    Compiler,
    /// Ingested from a SCIP index artifact.
    Scip,
    /// Reported by an LSP server.
    Lsp,
    /// Extracted by tree-sitter syntax analysis.
    TreeSitter,
    /// Derived from build system metadata (Cargo.toml, package.json, …).
    BuildSystem,
    /// Derived by a framework-specific rule (routes, events, stores, …).
    FrameworkRule,
    /// Inferred by a model; never to be presented as source truth.
    ModelInference,
}

/// A directed edge between two entities within one snapshot, carrying
/// provenance, confidence, and source evidence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Relation {
    /// Deterministic identifier (function of source, target, kind).
    pub id: String,
    /// Source (from) entity id.
    pub source_entity_id: String,
    /// Target (to) entity id.
    pub target_entity_id: String,
    /// Edge kind.
    pub kind: RelationKind,
    /// How the edge was derived.
    pub origin: RelationOrigin,
    /// Confidence in [0, 1]; 1.0 for compiler/SCIP facts.
    pub confidence: f32,
    /// Snapshot the edge was extracted from.
    pub snapshot_id: String,
    /// Name of the extractor/provider that produced the edge.
    pub extractor: String,
    /// Source addresses where the relation is witnessed.
    #[serde(default)]
    pub evidence: Vec<SourceAddress>,
    /// Free-form extractor-specific attributes.
    #[serde(default)]
    pub attributes: serde_json::Map<String, serde_json::Value>,
}
