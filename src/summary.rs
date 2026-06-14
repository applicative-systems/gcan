//! `summary` — store-wide reclamation figures in a single call, for the daemon
//! / UI dashboards that just want the numbers, not the per-root table.
//!
//! Everything here rides the same stable interfaces the rest of gcan uses
//! (`nix-store -q --requisites` / `--size`, plus `--gc --print-dead`); no
//! `nix path-info`, whose installable-resolution and `--json` surface are still
//! in flux (see `nix.rs`).

use std::collections::BTreeSet;
use std::io;

use serde::Serialize;

use crate::cache::Cache;
use crate::format::iec_size;
use crate::nix;
use crate::size;
use crate::walk::Group;

/// Closure bytes pinned by GC roots, split by who owns them. System-first
/// dedup: `system_bytes` is the full closure of the system roots, and
/// `user_bytes` counts only the paths user roots pin that the system does not —
/// so the two never double-count the shared base (glibc, etc.) and
/// `user + system` is the real on-disk total of everything pinned.
#[derive(Serialize)]
pub struct Pinned {
    pub user_bytes: u64,
    pub system_bytes: u64,
}

/// The store-wide reclamation snapshot.
#[derive(Serialize)]
pub struct Summary {
    pub now: u64,
    /// Size of the currently-dead store paths — what a plain
    /// `nix-collect-garbage` (no `-d`) frees right now, touching no roots.
    /// `None` unless explicitly requested: it requires a full-store dead-path
    /// scan (`nix-store --gc --print-dead`) that dominates wall-clock and can't
    /// be cached, so the rest of the summary stays fast when it's off.
    pub collectable_bytes: Option<u64>,
    /// Sum of the closures of the deletable (user-removable) roots — what
    /// pruning every safe root would additionally let GC reclaim. Paths shared
    /// across groups are counted per-group, matching `list`'s reclaimable total.
    pub reclaimable_bytes: u64,
    pub pinned: Pinned,
}

/// Where a GC root is anchored, for the user/system split.
enum Scope {
    User,
    System,
}

/// Classify a group by its indirect link paths: anything under a `/per-user/`
/// tree or a user's home is user-scoped; everything else (system profile
/// generations, the running-system links) is system-scoped.
fn scope(g: &Group) -> Scope {
    if is_user_scoped(&g.links) {
        Scope::User
    } else {
        Scope::System
    }
}

/// User-scoped iff any indirect link lives under a `/per-user/` tree or a home.
fn is_user_scoped(links: &[String]) -> bool {
    links
        .iter()
        .any(|l| l.contains("/per-user/") || l.starts_with("/home/"))
}

/// Build the summary from the scanned groups. Reuses the size cache for the
/// reclaimable figure; the pinned/collectable closures are computed fresh.
pub fn compute(
    groups: &[Group],
    cache: &mut Cache,
    now: u64,
    want_collectable: bool,
) -> io::Result<Summary> {
    Ok(Summary {
        now,
        collectable_bytes: if want_collectable {
            Some(collectable(cache)?)
        } else {
            None
        },
        reclaimable_bytes: reclaimable(groups, cache),
        pinned: pinned(groups, cache)?,
    })
}

/// Sum of the deletable roots' closures — the same figure as `list`'s
/// `total_reclaimable_bytes` (paths shared across groups counted per-group).
fn reclaimable(groups: &[Group], cache: &mut Cache) -> u64 {
    let deletable: Vec<&Group> = groups.iter().filter(|g| g.deletable()).collect();
    size::group_sizes(&deletable, cache).iter().sum()
}

/// Deduplicated closure sizes pinned per scope, system-first.
fn pinned(groups: &[Group], cache: &mut Cache) -> io::Result<Pinned> {
    let mut user_members = Vec::new();
    let mut system_members = Vec::new();
    for g in groups {
        match scope(g) {
            Scope::User => user_members.extend(g.members.iter().cloned()),
            Scope::System => system_members.extend(g.members.iter().cloned()),
        }
    }

    let system: BTreeSet<String> = nix::requisites(&system_members)?.into_iter().collect();
    let user: BTreeSet<String> = nix::requisites(&user_members)?.into_iter().collect();
    let user_unique: Vec<String> = user.difference(&system).cloned().collect();
    let system_paths: Vec<String> = system.into_iter().collect();

    Ok(Pinned {
        system_bytes: size::cached_sizes(&system_paths, cache)?.iter().sum(),
        user_bytes: size::cached_sizes(&user_unique, cache)?.iter().sum(),
    })
}

/// Total size of the currently-dead store paths. `--print-dead` can include a
/// path nix already considers invalid (e.g. a half-collected VM-test output),
/// and `-q --size` aborts on those — so drop the invalid ones first. They
/// occupy no real space anyway, so excluding them is also the correct figure.
fn collectable(cache: &mut Cache) -> io::Result<u64> {
    let dead = nix::dead_paths()?;
    let invalid: BTreeSet<String> = nix::invalid_paths(&dead)?.into_iter().collect();
    let live: Vec<String> = dead.into_iter().filter(|p| !invalid.contains(p)).collect();
    Ok(size::cached_sizes(&live, cache)?.iter().sum())
}

/// Pretty multi-line table for humans (`gcan summary` without `--format json`).
pub fn render_table(s: &Summary) -> String {
    let collectable = match s.collectable_bytes {
        Some(b) => iec_size(b),
        None => "(skipped; pass --collectable)".to_string(),
    };
    format!(
        "Collectable now (dead paths):   {}\n\
         Reclaimable (prune safe roots): {}\n\
         Pinned — system:                {}\n\
         Pinned — user (beyond system):  {}\n",
        collectable,
        iec_size(s.reclaimable_bytes),
        iec_size(s.pinned.system_bytes),
        iec_size(s.pinned.user_bytes),
    )
}

/// Machine-readable JSON (`gcan summary --format json`).
pub fn render_json(s: &Summary) -> String {
    serde_json::to_string_pretty(s).expect("serialize summary json")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn links(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|p| p.to_string()).collect()
    }

    #[test]
    fn scope_split_by_link_path() {
        // Per-user profiles and anything under a home are user-scoped.
        assert!(is_user_scoped(&links(&[
            "/nix/var/nix/profiles/per-user/jacek/profile-7-link"
        ])));
        assert!(is_user_scoped(&links(&["/home/jacek/dev/proj/result"])));
        assert!(is_user_scoped(&links(&[
            "/home/jacek/dev/proj/.direnv/flake-inputs/xyz-source"
        ])));
        // System profile generations and the running-system links are system.
        assert!(!is_user_scoped(&links(&[
            "/nix/var/nix/profiles/system-181-link"
        ])));
        assert!(!is_user_scoped(&links(&[
            "/nix/var/nix/gcroots/booted-system"
        ])));
        // A group counts as user if any of its links is user-scoped.
        assert!(is_user_scoped(&links(&[
            "/nix/var/nix/profiles/system-181-link",
            "/home/jacek/dev/proj/result",
        ])));
    }
}
