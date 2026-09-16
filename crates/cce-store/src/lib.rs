#![forbid(unsafe_code)]

mod artifact;
mod metadata;

pub use artifact::{ArtifactKind, ArtifactRecord, ArtifactStore};
pub use metadata::{
    DocumentContent, GcReport, IndexedDocument, LexicalHit, MetadataStore, RelationDirection,
    ScanCacheEntry, SnapshotRecords, SourceFileRecord, StoreHealth,
};
