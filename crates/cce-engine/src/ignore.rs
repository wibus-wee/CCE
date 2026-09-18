//! Built-in ignore policy: names that never carry retrievable evidence.
//!
//! This layer is unconditional — it applies before `.gitignore` and
//! `.cceignore` are even consulted, because these entries cannot help a
//! query and actively pollute the index (a 500KB lockfile matches any
//! dependency term; a minified bundle is one giant unreadable line).
//!
//! Three disjoint categories:
//! - [`is_internal_or_generated`]: directories/tooling internals
//! - [`builtin_skip_reason`]: generated artifacts — lockfiles, minified
//!   assets, checksum databases. Deterministic output of a package
//!   manager or compiler; the manifest that produced them is the real
//!   source of truth and remains indexed.
//! - [`is_sensitive_name`]: credential-shaped names, separately gated by
//!   the `include_sensitive` index option and always reported.

use ignore::DirEntry;

/// Directories that are never repository content.
pub(crate) fn is_internal_or_generated(entry: &DirEntry) -> bool {
    if entry.depth() == 0 {
        return false;
    }
    let Some(name) = entry.file_name().to_str() else {
        return false;
    };
    if matches!(
        name,
        ".git" | ".cce" | "target" | "node_modules" | ".venv" | "__pycache__"
    ) {
        return true;
    }
    // CCE state under a renamed data dir (`.cce-bench-*`, `.cce_*`, …) is
    // still not source: artifact churn inside it would otherwise look like
    // fresh content on every scan. `.cceignore` is a file and stays
    // indexed.
    entry
        .file_type()
        .is_some_and(|file_type| file_type.is_dir())
        && (name.starts_with(".cce-") || name.starts_with(".cce_"))
}

/// Why a file name is skipped by the built-in policy. Returned as a
/// static label so index reports can group skips by reason.
pub(crate) fn builtin_skip_reason(name: &str) -> Option<&'static str> {
    let path = std::path::Path::new(name);
    let has_extension = |extension: &str| {
        path.extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case(extension))
    };
    if has_extension("lock") || has_extension("lockb") {
        return Some("lockfile");
    }
    let lower = name.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        // Lockfile-shaped names that do not end in `.lock`.
        "package-lock.json" | "npm-shrinkwrap.json" | "pnpm-lock.yaml" | "go.sum" | "go.work.sum"
    ) {
        return Some("lockfile");
    }
    if lower.ends_with(".min.js") || lower.ends_with(".min.css") || has_extension("map") {
        return Some("minified");
    }
    None
}

/// File names that frequently contain credentials. Skipped unless the index
/// option `include_sensitive` is enabled; always reported in the index report.
pub(crate) fn is_sensitive_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if lower == ".env" || lower.starts_with(".env.") || lower.starts_with(".env-") {
        return true;
    }
    if lower.rsplit_once('.').is_some_and(|(_, extension)| {
        matches!(
            extension,
            "env" | "pem" | "key" | "p12" | "pfx" | "keystore" | "jks"
        )
    }) {
        return true;
    }
    matches!(
        lower.as_str(),
        "id_rsa"
            | "id_dsa"
            | "id_ecdsa"
            | "id_ed25519"
            | ".netrc"
            | ".npmrc"
            | ".pypirc"
            | "credentials"
            | "credentials.json"
            | "secrets.json"
            | "secrets.yaml"
            | "secrets.yml"
    )
}

#[cfg(test)]
mod tests {
    use super::{builtin_skip_reason, is_sensitive_name};

    #[test]
    fn lockfiles_are_builtin_ignored() {
        for name in [
            "Cargo.lock",
            "uv.lock",
            "yarn.lock",
            "poetry.lock",
            "flake.lock",
            "deno.lock",
            "bun.lockb",
            "package-lock.json",
            "npm-shrinkwrap.json",
            "pnpm-lock.yaml",
            "go.sum",
        ] {
            assert_eq!(builtin_skip_reason(name), Some("lockfile"), "{name}");
        }
    }

    #[test]
    fn regular_files_are_not_ignored() {
        for name in [
            "main.rs",
            "Cargo.toml",
            "package.json",
            "pnpm-workspace.yaml",
            "go.mod",
            "unlock.rs",
            "deadlock.rs",
            "style.css",
        ] {
            assert_eq!(builtin_skip_reason(name), None, "{name}");
        }
    }

    #[test]
    fn minified_assets_are_ignored() {
        assert_eq!(builtin_skip_reason("app.min.js"), Some("minified"));
        assert_eq!(builtin_skip_reason("site.min.css"), Some("minified"));
        assert_eq!(builtin_skip_reason("bundle.js.map"), Some("minified"));
        assert_eq!(builtin_skip_reason("app.js"), None);
    }

    #[test]
    fn sensitive_names() {
        assert!(is_sensitive_name(".env"));
        assert!(is_sensitive_name(".env.production"));
        assert!(is_sensitive_name("server.pem"));
        assert!(is_sensitive_name("id_ed25519"));
        assert!(!is_sensitive_name("config.rs"));
    }
}
