//! Thin wrappers over the stable `nix-store -q` queries.
//!
//! We deliberately avoid `nix path-info --json` (nix 2.34 resolves bare store
//! paths as installables and the `--json-format` surface is in flux). The
//! `nix-store -q` interface is stable: `--size` prints one size per path in
//! input order, `--requisites` prints the deduplicated union closure.

use std::io;
use std::process::Command;

/// Largest number of store paths to pass to one `nix-store` invocation.
const CHUNK: usize = 256;

fn run(query: &str, paths: &[String]) -> io::Result<String> {
    let out = Command::new("nix-store")
        .arg("-q")
        .arg(query)
        .args(paths)
        .output()?;
    if !out.status.success() {
        let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(io::Error::other(if msg.is_empty() {
            format!("nix-store -q {query} failed")
        } else {
            msg
        }));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The transitive closure (requisites) of the union of `paths`.
pub fn requisites(paths: &[String]) -> io::Result<Vec<String>> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let mut all = Vec::new();
    // Chunk in case a group has a very large member list.
    for chunk in paths.chunks(CHUNK) {
        let out = run("--requisites", chunk)?;
        all.extend(out.lines().map(str::to_string));
    }
    Ok(all)
}

/// The own NAR size of each path, aligned to the input order.
pub fn sizes(paths: &[String]) -> io::Result<Vec<u64>> {
    let mut result = Vec::with_capacity(paths.len());
    for chunk in paths.chunks(CHUNK) {
        let out = run("--size", chunk)?;
        for line in out.lines() {
            let n: u64 = line.trim().parse().map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("bad size line: {line:?}"),
                )
            })?;
            result.push(n);
        }
    }
    if result.len() != paths.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("size count {} != path count {}", result.len(), paths.len()),
        ));
    }
    Ok(result)
}
