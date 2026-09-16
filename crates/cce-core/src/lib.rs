#![forbid(unsafe_code)]

mod address;
mod context;
mod entity;
mod error;
mod manifest;
mod region;
mod relation;
mod retrieval;
mod snapshot;

pub use address::SourceAddress;
pub use context::{ContextItem, ContextItemKind, ContextPack, ContextProvenance, Uncertainty};
pub use entity::{CodeEntity, EntityKind};
pub use error::{CceError, Result};
pub use manifest::{Capability, ViewKind, ViewManifest, ViewState, ViewStatus};
pub use region::{CodeRegion, RegionKind};
pub use relation::{Relation, RelationKind, RelationOrigin};
pub use retrieval::{
    GraphPolicy, QueryIntent, RetrievalDocument, RetrievalRepresentation, SearchHit, SearchRequest,
    SearchRoute,
};
pub use snapshot::{IndexProfile, RepositoryIdentity, SnapshotIdentity};

pub const DATA_FORMAT_VERSION: u32 = 1;
