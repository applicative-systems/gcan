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

/// Run `nix-collect-garbage`, inheriting stdio so its progress is visible.
pub fn collect_garbage() -> io::Result<()> {
    eprintln!("Running nix-collect-garbage…");
    let status = Command::new("nix-collect-garbage").status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "nix-collect-garbage exited with {status}"
        )))
    }
}

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

/// Of `paths`, the ones Nix no longer considers valid. Uses
/// `nix-store --check-validity --print-invalid`, which *prints* the invalid
/// paths instead of failing — so a stray invalid entry (e.g. one
/// `--print-dead` reports for a half-collected path) never aborts a sizing pass.
pub fn invalid_paths(paths: &[String]) -> io::Result<Vec<String>> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let mut invalid = Vec::new();
    for chunk in paths.chunks(CHUNK) {
        let out = Command::new("nix-store")
            .args(["--check-validity", "--print-invalid"])
            .args(chunk)
            .output()?;
        if !out.status.success() {
            let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
            return Err(io::Error::other(if msg.is_empty() {
                "nix-store --check-validity failed".to_string()
            } else {
                msg
            }));
        }
        invalid.extend(
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(str::to_string),
        );
    }
    Ok(invalid)
}

/// The store paths that are dead right now — exactly what a plain
/// `nix-collect-garbage` (without `-d`) would delete. Stable interface:
/// `nix-store --gc --print-dead`.
pub fn dead_paths() -> io::Result<Vec<String>> {
    let out = Command::new("nix-store")
        .args(["--gc", "--print-dead"])
        .output()?;
    if !out.status.success() {
        let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(io::Error::other(if msg.is_empty() {
            "nix-store --gc --print-dead failed".to_string()
        } else {
            msg
        }));
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("/nix/store/"))
        .map(str::to_string)
        .collect())
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
