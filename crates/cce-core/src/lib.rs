#![forbid(unsafe_code)]
//! Shared vocabulary types for CCE: canonical source addresses, entities,
//! relations, snapshots, view manifests, and retrieval/context contracts.
//! Every crate in the workspace speaks these types at its boundaries.

mod address;
mod context;
mod entity;
mod error;
mod manifest;
mod region;
mod relation;
mod retrieval;
mod snapshot;
mod text;

pub use address::SourceAddress;
pub use context::{ContextItem, ContextItemKind, ContextPack, ContextProvenance, Uncertainty};
pub use entity::{CodeEntity, EntityKind};
pub use error::{CceError, Result};
pub use manifest::{Capability, ViewKind, ViewManifest, ViewState, ViewStatus};
pub use region::{CodeRegion, RegionKind};
pub use relation::{Relation, RelationKind, RelationOrigin};
pub use retrieval::{
    BindingScope, BoundArtifact, ClaimFrame, ClaimPredicate, DefinedWitness, DocumentClass,
    DrillDown, EvidenceTiers, GraphPolicy, QueryFilters, QueryIntent, RelationProvenance,
    RelationWitness, RetrievalDocument, RetrievalRepresentation, SearchHit, SearchRequest,
    SearchRoute, SearchVerdict, TermWitness, VerdictState, WitnessReport, WitnessRequirement,
    document_class, parse_query_filters,
};
pub use snapshot::{IndexProfile, RepositoryIdentity, SnapshotIdentity};
pub use text::{folded_identifier, has_cjk, split_identifier_terms};

/// On-disk data format version; bumped when the metadata/artifact layout
/// changes incompatibly.
pub const DATA_FORMAT_VERSION: u32 = 1;
