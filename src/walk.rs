//! Walk the gcroots tree and group the roots.

use std::collections::HashMap;
use std::ffi::CString;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Direnv,
    Single,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Direnv => "direnv",
            Kind::Single => "single",
        }
    }
}

/// One logical group of GC roots (a direnv project, or a single standalone root).
pub struct Group {
    /// Grouping identity (project dir for direnv, indirect path for single).
    pub key: String,
    /// Display location.
    pub loc: String,
    pub kind: Kind,
    /// Deduplicated, sorted `/nix/store/...` paths of the group's members.
    pub members: Vec<String>,
    /// Deduplicated, sorted indirect symlink paths (what `--delete`/`-p` act on).
    pub links: Vec<String>,
    /// Newest member mtime (epoch seconds).
    pub newest_mtime: u64,
    /// Number of raw roots collapsed into this group.
    pub count: usize,
    /// Every member sits in a user-writable directory.
    pub writable: bool,
    /// Any member is a protected `current-*` / `booted-*` root.
    pub protected: bool,
}

impl Group {
    /// Removable by the current user: writable everywhere and not protected.
    pub fn deletable(&self) -> bool {
        self.writable && !self.protected
    }

    /// Cache key: the immutable member set determines the union closure size.
    pub fn member_key(&self) -> String {
        self.members.join("\n")
    }
}

/// Accumulator used while folding raw roots into groups.
struct Acc {
    loc: String,
    kind: Kind,
    members: Vec<String>,
    links: Vec<String>,
    newest_mtime: u64,
    count: usize,
    writable: bool,
    protected: bool,
}

/// Walk `gcroots` and return the grouped roots. Already-deleted (dangling)
/// roots awaiting GC are skipped.
pub fn scan(gcroots: &Path) -> Vec<Group> {
    let mut links = Vec::new();
    collect_symlinks(gcroots, &mut links);
    links.sort();

    let mut groups: HashMap<String, Acc> = HashMap::new();

    for link in &links {
        let Some(indirect) = read_indirect(link) else {
            continue;
        };

        // Skip roots whose indirect symlink entry is already gone (deleted,
        // awaiting `nix-collect-garbage`). lstat succeeds for a dangling symlink.
        if fs::symlink_metadata(&indirect).is_err() {
            continue;
        }

        let final_store = fs::canonicalize(link).ok().and_then(|p| store_path(&p));
        let mtime = mtime_secs(&indirect)
            .or_else(|| mtime_secs(link))
            .unwrap_or(0);
        let writable = indirect.parent().map(access_wx).unwrap_or(false);
        let protected = indirect
            .file_name()
            .and_then(|n| n.to_str())
            .map(is_protected)
            .unwrap_or(false);

        let indirect_str = indirect.to_string_lossy().into_owned();
        let (key, loc, kind) = classify(&indirect_str);

        let acc = groups.entry(key).or_insert_with(|| Acc {
            loc,
            kind,
            members: Vec::new(),
            links: Vec::new(),
            newest_mtime: 0,
            count: 0,
            writable: true,
            protected: false,
        });
        if let Some(f) = final_store {
            acc.members.push(f);
        }
        acc.links.push(indirect_str);
        acc.newest_mtime = acc.newest_mtime.max(mtime);
        acc.count += 1;
        acc.writable &= writable;
        acc.protected |= protected;
    }

    let mut out: Vec<Group> = groups
        .into_iter()
        .map(|(key, mut a)| {
            a.members.sort();
            a.members.dedup();
            a.links.sort();
            a.links.dedup();
            Group {
                key,
                loc: a.loc,
                kind: a.kind,
                members: a.members,
                links: a.links,
                newest_mtime: a.newest_mtime,
                count: a.count,
                writable: a.writable,
                protected: a.protected,
            }
        })
        .collect();
    out.sort_by(|x, y| x.key.cmp(&y.key));
    out
}

/// Recursively collect every symlink under `dir`, descending only into real
/// directories (never following a symlinked directory).
fn collect_symlinks(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(md) = fs::symlink_metadata(&path) else {
            continue;
        };
        let ft = md.file_type();
        if ft.is_symlink() {
            out.push(path);
        } else if ft.is_dir() {
            collect_symlinks(&path, out);
        }
    }
}

/// Read a symlink one level, resolving a relative target against its directory.
fn read_indirect(link: &Path) -> Option<PathBuf> {
    let raw = fs::read_link(link).ok()?;
    if raw.is_absolute() {
        Some(raw)
    } else {
        Some(link.parent()?.join(raw))
    }
}

/// Reduce a fully resolved path to its `/nix/store/<name>` root, or `None` if it
/// is not under the store.
fn store_path(canonical: &Path) -> Option<String> {
    let rest = canonical.to_str()?.strip_prefix("/nix/store/")?;
    let comp = rest.split('/').next()?;
    if comp.is_empty() {
        None
    } else {
        Some(format!("/nix/store/{comp}"))
    }
}

/// mtime (epoch seconds, truncated) of a path's own lstat.
fn mtime_secs(path: &Path) -> Option<u64> {
    let md = fs::symlink_metadata(path).ok()?;
    let t = md.modified().ok()?;
    Some(t.duration_since(UNIX_EPOCH).ok()?.as_secs())
}

/// Whether the current user can unlink entries in `dir` (write + execute).
fn access_wx(dir: &Path) -> bool {
    let Ok(c) = CString::new(dir.as_os_str().as_bytes()) else {
        return false;
    };
    unsafe { libc::access(c.as_ptr(), libc::W_OK | libc::X_OK) == 0 }
}

/// A protected "current" root that must never be offered for deletion.
fn is_protected(name: &str) -> bool {
    name == "current"
        || name.starts_with("current-")
        || name.ends_with("-current")
        || name.starts_with("booted-")
}

/// Determine the group key, display location, and kind for an indirect path.
fn classify(indirect: &str) -> (String, String, Kind) {
    if let Some(idx) = indirect.find("/.direnv/") {
        let key = indirect[..idx].to_string();
        let loc = format!("{key}/.direnv/  (direnv)");
        (key, loc, Kind::Direnv)
    } else {
        (indirect.to_string(), indirect.to_string(), Kind::Single)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protected_patterns() {
        assert!(is_protected("current"));
        assert!(is_protected("current-system"));
        assert!(is_protected("current-home"));
        assert!(is_protected("booted-system"));
        assert!(is_protected("hm-current"));
        assert!(!is_protected("home-manager-17-link"));
        assert!(!is_protected("result-1"));
        assert!(!is_protected("flake-profile-abc"));
    }

    #[test]
    fn classify_direnv_vs_single() {
        let (k, l, kind) = classify("/home/u/proj/.direnv/flake-inputs/xyz-source");
        assert_eq!(k, "/home/u/proj");
        assert_eq!(l, "/home/u/proj/.direnv/  (direnv)");
        assert!(kind == Kind::Direnv);

        let (k, l, kind) = classify("/home/u/proj/result-1");
        assert_eq!(k, "/home/u/proj/result-1");
        assert_eq!(l, "/home/u/proj/result-1");
        assert!(kind == Kind::Single);
    }

    #[test]
    fn store_path_trim() {
        assert_eq!(
            store_path(Path::new("/nix/store/abc-foo")).as_deref(),
            Some("/nix/store/abc-foo")
        );
        assert_eq!(
            store_path(Path::new("/nix/store/abc-foo/bin/bar")).as_deref(),
            Some("/nix/store/abc-foo")
        );
        assert_eq!(store_path(Path::new("/run/current-system")), None);
    }
}
