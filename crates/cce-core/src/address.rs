use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{CceError, Result};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
/// Canonical pointer to a source range: repository + snapshot + path +
/// byte/line span (+ optional symbol pin). Every result maps to one.
pub struct SourceAddress {
    /// Owning repository id.
    pub repository_id: String,
    /// Snapshot the address was captured in.
    pub snapshot_id: String,
    /// Normalized repository-relative path (`/`-separated, no `.`/`..`).
    pub path: String,
    /// Inclusive start byte offset.
    pub start_byte: u64,
    /// Exclusive end byte offset.
    pub end_byte: u64,
    /// 1-based start line.
    pub start_line: u32,
    /// 1-based end line (inclusive).
    pub end_line: u32,
    /// Entity id of the enclosing symbol, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol_id: Option<String>,
}

impl SourceAddress {
    /// Build an address; rejects parent traversal and inverted ranges.
    ///
    /// # Errors
    /// `PathEscape` for `..`/empty paths, `InvalidSourceRange` for
    /// `start > end`.
    pub fn new(
        repository_id: impl Into<String>,
        snapshot_id: impl Into<String>,
        path: impl Into<String>,
        byte_range: std::ops::Range<u64>,
        line_range: std::ops::RangeInclusive<u32>,
    ) -> Result<Self> {
        let path = normalize_relative_path(&path.into())?;
        if byte_range.start > byte_range.end {
            return Err(CceError::InvalidSourceRange {
                path,
                start_byte: byte_range.start,
                end_byte: byte_range.end,
            });
        }
        Ok(Self {
            repository_id: repository_id.into(),
            snapshot_id: snapshot_id.into(),
            path,
            start_byte: byte_range.start,
            end_byte: byte_range.end,
            start_line: *line_range.start(),
            end_line: *line_range.end(),
            symbol_id: None,
        })
    }

    /// Attach the enclosing symbol's entity id.
    #[must_use]
    pub fn with_symbol(mut self, symbol_id: impl Into<String>) -> Self {
        self.symbol_id = Some(symbol_id.into());
        self
    }

    /// Length of the byte span.
    #[must_use]
    pub const fn byte_len(&self) -> u64 {
        self.end_byte.saturating_sub(self.start_byte)
    }
}

fn normalize_relative_path(path: &str) -> Result<String> {
    let replaced = path.replace('\\', "/");
    let mut normalized = Vec::new();
    for component in replaced.split('/') {
        match component {
            "" | "." => {}
            ".." => return Err(CceError::PathEscape(path.into())),
            value => normalized.push(value),
        }
    }
    if normalized.is_empty() {
        return Err(CceError::PathEscape(path.into()));
    }
    Ok(normalized.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_windows_paths() {
        let address =
            SourceAddress::new("repo", "snap", r"src\lib.rs", 0..10, 1..=2).expect("valid address");
        assert_eq!(address.path, "src/lib.rs");
    }

    #[test]
    fn rejects_parent_traversal() {
        assert!(SourceAddress::new("repo", "snap", "../secret", 0..1, 1..=1).is_err());
    }
}
