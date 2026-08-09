use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{CceError, Result};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SourceAddress {
    pub repository_id: String,
    pub snapshot_id: String,
    pub path: String,
    pub start_byte: u64,
    pub end_byte: u64,
    pub start_line: u32,
    pub end_line: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol_id: Option<String>,
}

impl SourceAddress {
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

    #[must_use]
    pub fn with_symbol(mut self, symbol_id: impl Into<String>) -> Self {
        self.symbol_id = Some(symbol_id.into());
        self
    }

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
