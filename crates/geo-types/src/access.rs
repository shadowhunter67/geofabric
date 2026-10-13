//! Source authorization boundary.
//!
//! Flow (CONSTITUTION / hardening §8): `authorize → metadata read → plan → execute`.
//! Every component that opens a source (planner scan analysis, engines, source
//! listing) must hold an [`AuthorizedSource`], which can only be produced by a
//! [`SourceAccessGuard`]. The type is the proof that authorization happened
//! *before* any I/O on the source.
//!
//! The guard reveals nothing about unauthorized sources: [`AccessDenied`]
//! carries no path, no reason and no existence information, and the path is
//! rejected lexically *before* the file system is touched.

use std::path::{Component, Path, PathBuf};

/// Intentionally opaque: no existence/path oracle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("source not permitted")]
pub struct AccessDenied;

/// Proof that a source passed authorization. Cannot be constructed outside
/// this crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizedSource {
    uri: String,
    path: PathBuf,
}

impl AuthorizedSource {
    /// The URI as written in the plan.
    pub fn uri(&self) -> &str {
        &self.uri
    }
    /// Canonical absolute path (verified to be inside an allowed root).
    pub fn path(&self) -> &Path {
        &self.path
    }
}

pub trait SourceAccessGuard: Send + Sync {
    /// Decide whether `uri` may be read. Must not perform I/O on `uri` before
    /// the decision is made.
    fn authorize(&self, uri: &str) -> Result<AuthorizedSource, AccessDenied>;
}

/// Allows local files under a fixed set of root directories.
#[derive(Debug, Clone)]
pub struct RootedAccessGuard {
    roots: Vec<(PathBuf, Vec<String>)>,
}

fn key(p: &Path) -> Vec<String> {
    p.components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            Component::Prefix(pr) => {
                // strip the verbatim prefix: \\?\E: and E: compare equal
                let s = pr.as_os_str().to_string_lossy().into_owned();
                Some(s.trim_start_matches(r"\\?\").to_string())
            }
            _ => None,
        })
        .map(|s| if cfg!(windows) { s.to_lowercase() } else { s })
        .collect()
}

fn lexical_absolute(p: &Path) -> Option<PathBuf> {
    let base = if p.is_absolute() { PathBuf::new() } else { std::env::current_dir().ok()? };
    let mut out = base;
    for c in p.components() {
        match c {
            Component::Prefix(_) | Component::RootDir => out.push(c.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    return None;
                }
            }
            Component::Normal(s) => out.push(s),
        }
    }
    Some(out)
}

impl RootedAccessGuard {
    /// Roots are trusted configuration: they are canonicalized here (roots
    /// that do not exist are dropped).
    pub fn new(roots: impl IntoIterator<Item = PathBuf>) -> Self {
        let roots = roots
            .into_iter()
            .filter_map(|r| std::fs::canonicalize(r).ok())
            .map(|r| {
                let k = key(&r);
                (r, k)
            })
            .collect();
        RootedAccessGuard { roots }
    }

    fn under_root(&self, k: &[String]) -> bool {
        self.roots.iter().any(|(_, rk)| !rk.is_empty() && k.len() > rk.len() && k[..rk.len()] == rk[..])
    }

    /// Canonical root directories (for sandboxing the engine).
    pub fn roots(&self) -> Vec<PathBuf> {
        self.roots.iter().map(|(p, _)| p.clone()).collect()
    }
}

impl SourceAccessGuard for RootedAccessGuard {
    fn authorize(&self, uri: &str) -> Result<AuthorizedSource, AccessDenied> {
        if uri.is_empty() || uri.contains("://") || uri.contains('\0') {
            return Err(AccessDenied);
        }
        // 1. lexical check — no file-system access to the requested path yet
        let abs = lexical_absolute(Path::new(uri)).ok_or(AccessDenied)?;
        if !self.under_root(&key(&abs)) {
            return Err(AccessDenied);
        }
        // 2. only now touch the FS; re-check after symlink resolution
        let canon = std::fs::canonicalize(&abs).map_err(|_| AccessDenied)?;
        if !self.under_root(&key(&canon)) {
            return Err(AccessDenied);
        }
        Ok(AuthorizedSource { uri: uri.to_string(), path: canon })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (PathBuf, PathBuf) {
        let base = std::env::temp_dir().join(format!("geo-types-access-{}", std::process::id()));
        let root = base.join("root");
        let outside = base.join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(root.join("a.parquet"), b"x").unwrap();
        std::fs::write(outside.join("secret.parquet"), b"x").unwrap();
        (root, outside)
    }

    #[test]
    fn allows_inside_denies_outside_with_identical_errors() {
        let (root, outside) = setup();
        let g = RootedAccessGuard::new([root.clone()]);
        assert!(g.authorize(root.join("a.parquet").to_str().unwrap()).is_ok());
        // existing outside file and non-existent outside file are indistinguishable
        let e1 = g.authorize(outside.join("secret.parquet").to_str().unwrap()).unwrap_err();
        let e2 = g.authorize(outside.join("nope.parquet").to_str().unwrap()).unwrap_err();
        assert_eq!(e1, e2);
        assert_eq!(e1.to_string(), "source not permitted");
    }

    #[test]
    fn dotdot_escape_and_urls_are_denied() {
        let (root, _outside) = setup();
        let g = RootedAccessGuard::new([root.clone()]);
        let sneaky = format!("{}/../outside/secret.parquet", root.display());
        assert!(g.authorize(&sneaky).is_err());
        assert!(g.authorize("http://example.com/a.parquet").is_err());
        assert!(g.authorize("s3://bucket/a.parquet").is_err());
        assert!(g.authorize("").is_err());
        assert!(g.authorize(root.to_str().unwrap()).is_err(), "the root itself is not a source");
    }

    #[test]
    fn missing_file_inside_root_is_denied() {
        let (root, _o) = setup();
        let g = RootedAccessGuard::new([root.clone()]);
        assert!(g.authorize(root.join("missing.parquet").to_str().unwrap()).is_err());
    }
}
