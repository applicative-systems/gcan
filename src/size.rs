//! Resolve each group's union closure size, using and filling the cache.

use crate::cache::Cache;
use crate::nix;
use crate::walk::Group;

/// Compute the union closure size (bytes) of each group in `groups`, aligned to
/// the input order. Hits the cache where possible and records new findings into
/// `cache` (the caller decides whether to persist).
pub fn group_sizes(groups: &[&Group], cache: &mut Cache) -> Vec<u64> {
    let mut out = vec![0u64; groups.len()];
    for (i, g) in groups.iter().enumerate() {
        if g.members.is_empty() {
            continue; // no store paths resolved -> size 0
        }
        let key = g.member_key();

        // Warm hit: trust the cached size only if every member still exists.
        if let Some(&sz) = cache.groups.get(&key) {
            if members_present(&g.members) {
                out[i] = sz;
                continue;
            }
        }

        match compute(&g.members, cache) {
            Ok(sz) => {
                cache.groups.insert(key, sz);
                out[i] = sz;
            }
            Err(e) => {
                eprintln!("warning: closure size for {}: {e}", g.loc);
                out[i] = 0;
            }
        }
    }
    out
}

fn members_present(members: &[String]) -> bool {
    members.iter().all(|m| std::fs::symlink_metadata(m).is_ok())
}

/// Cold path for one group: query requisites, fill missing sizes, sum.
fn compute(members: &[String], cache: &mut Cache) -> std::io::Result<u64> {
    let mut reqs = nix::requisites(members)?;
    reqs.sort();
    reqs.dedup();
    Ok(cached_sizes(&reqs, cache)?.iter().sum())
}

/// Own NAR size of each path, served from the cache where possible. Only the
/// paths not already memoized are queried (once, deduplicated) and recorded;
/// the result is aligned to `paths`. Per-path sizes are immutable, so any path
/// seen on a previous run costs nothing here. Callers decide whether to persist.
pub fn cached_sizes(paths: &[String], cache: &mut Cache) -> std::io::Result<Vec<u64>> {
    let mut missing: Vec<String> = paths
        .iter()
        .filter(|p| !cache.sizes.contains_key(*p))
        .cloned()
        .collect();
    missing.sort();
    missing.dedup();
    if !missing.is_empty() {
        for (p, s) in missing.iter().zip(nix::sizes(&missing)?) {
            cache.sizes.insert(p.clone(), s);
        }
    }
    Ok(paths
        .iter()
        .map(|p| cache.sizes.get(p).copied().unwrap_or(0))
        .collect())
}
