#![forbid(unsafe_code)]

mod artifact;
mod metadata;

pub use artifact::{ArtifactKind, ArtifactRecord, ArtifactStore};
pub use metadata::{
    DocumentContent, IndexedDocument, LexicalHit, MetadataStore, RelationDirection,
    SnapshotRecords, SourceFileRecord, StoreHealth,
};
