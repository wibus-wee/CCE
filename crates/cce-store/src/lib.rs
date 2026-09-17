#![forbid(unsafe_code)]
//! Canonical persistence for CCE: the `SQLite` metadata/FTS store plus the
//! content-addressed artifact store for large payloads.

mod artifact;
mod metadata;

pub use artifact::{ArtifactKind, ArtifactRecord, ArtifactStore};
pub use metadata::{
    DocumentContent, GcReport, IndexedDocument, LexicalHit, MetadataStore, RelationDirection,
    ScanCacheEntry, SnapshotRecords, SourceFileRecord, SourceFileRow, StoreHealth,
};
