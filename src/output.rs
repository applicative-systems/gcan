//! Output modes: table, JSON, print-links, delete.

use crate::format::{human_age, iec_size};
use clap::ValueEnum;
use serde::Serialize;
use std::cmp::Ordering;
use std::fs;
use std::io::{self, BufRead, BufReader, Write};

/// Column to sort the listing by. Shared by `list` and the TUI.
#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SortKey {
    Size,
    Name,
    Age,
}

impl SortKey {
    /// Natural default direction: size/age descending (biggest/oldest first),
    /// name ascending (A–Z). `--reverse` (or pressing the key again) flips it.
    pub fn default_desc(self) -> bool {
        !matches!(self, SortKey::Name)
    }

    pub fn label(self) -> &'static str {
        match self {
            SortKey::Size => "size",
            SortKey::Name => "name",
            SortKey::Age => "age",
        }
    }
}

/// One displayed group, with its resolved size and age.
pub struct Row {
    pub size: u64,
    pub age: u64,
    pub newest_mtime: u64,
    pub count: usize,
    pub loc: String,
    pub kind: &'static str,
    pub links: Vec<String>,
    pub deletable: bool,
    pub protected: bool,
    pub key: String,
}

impl Row {
    /// Build a display row from a group and its resolved closure size.
    pub fn from_group(g: &crate::walk::Group, size: u64, now: u64) -> Row {
        Row {
            size,
            age: now.saturating_sub(g.newest_mtime),
            newest_mtime: g.newest_mtime,
            count: g.count,
            loc: g.loc.clone(),
            kind: g.kind.as_str(),
            links: g.links.clone(),
            deletable: g.deletable(),
            protected: g.protected,
            key: g.key.clone(),
        }
    }
}

/// Sort rows by `key`/`desc`, with a total order (location then group key as
/// tie-breakers) so cached and uncached runs are byte-identical.
pub fn sort_rows(rows: &mut [Row], key: SortKey, desc: bool) {
    rows.sort_by(|a, b| order(a, b, key, desc));
}

/// Ordering of two rows under the given sort key and direction.
pub fn order(a: &Row, b: &Row, key: SortKey, desc: bool) -> Ordering {
    let base = match key {
        SortKey::Size => a.size.cmp(&b.size),
        SortKey::Age => a.age.cmp(&b.age),
        SortKey::Name => a.loc.cmp(&b.loc),
    };
    let base = if desc { base.reverse() } else { base };
    base.then_with(|| a.loc.cmp(&b.loc))
        .then_with(|| a.key.cmp(&b.key))
}

/// Render the human table. Reclaimable total counts deletable rows only.
pub fn render_table(rows: &[Row], all: bool) -> String {
    let mut s = String::new();
    let line = |s: &mut String, a: &str, b: &str, c: &str, d: &str| {
        s.push_str(&format!("{a:>10}  {b:>6}  {c:>5}  {d}\n"));
    };
    line(&mut s, "SIZE", "AGE", "ROOTS", "LOCATION");
    line(&mut s, "----", "---", "-----", "--------");

    let mut total = 0u64;
    for r in rows {
        if r.deletable {
            total += r.size;
        }
        let mut loc = r.loc.clone();
        if all && !r.deletable {
            loc.push_str(if r.protected {
                "  [protected]"
            } else {
                "  [root-owned]"
            });
        }
        line(
            &mut s,
            &iec_size(r.size),
            &human_age(r.age),
            &r.count.to_string(),
            &loc,
        );
    }

    line(&mut s, "----", "", "", "");
    line(
        &mut s,
        &iec_size(total),
        "",
        "",
        "TOTAL reclaimable (sum of closures; paths shared across groups counted per-group)",
    );
    s
}

#[derive(Serialize)]
struct GroupJson<'a> {
    location: &'a str,
    kind: &'a str,
    roots: usize,
    size_bytes: u64,
    size_human: String,
    newest_mtime: u64,
    age_seconds: u64,
    age_human: String,
    deletable: bool,
    protected: bool,
    links: &'a [String],
}

#[derive(Serialize)]
struct TopJson<'a> {
    gcroots_dir: String,
    now: u64,
    groups: Vec<GroupJson<'a>>,
    total_reclaimable_bytes: u64,
}

/// Render the structured JSON object (deterministic field/array order).
pub fn render_json(rows: &[Row], gcroots_dir: &str, now: u64) -> String {
    let groups: Vec<GroupJson> = rows
        .iter()
        .map(|r| GroupJson {
            location: &r.loc,
            kind: r.kind,
            roots: r.count,
            size_bytes: r.size,
            size_human: iec_size(r.size),
            newest_mtime: r.newest_mtime,
            age_seconds: r.age,
            age_human: human_age(r.age),
            deletable: r.deletable,
            protected: r.protected,
            links: &r.links,
        })
        .collect();
    let total = rows.iter().filter(|r| r.deletable).map(|r| r.size).sum();
    let top = TopJson {
        gcroots_dir: gcroots_dir.to_string(),
        now,
        groups,
        total_reclaimable_bytes: total,
    };
    serde_json::to_string_pretty(&top).expect("serialize json")
}

/// Print the indirect symlink paths of every row, group by group.
pub fn print_links(rows: &[Row]) {
    let stdout = io::stdout();
    let mut w = stdout.lock();
    for r in rows {
        for l in &r.links {
            let _ = writeln!(w, "{l}");
        }
    }
}

/// Delete the indirect symlinks of `rows` (already the deletable shortlist).
/// Returns the process exit code.
pub fn delete(rows: &[Row], yes: bool) -> i32 {
    let links: Vec<&String> = rows.iter().flat_map(|r| r.links.iter()).collect();
    if links.is_empty() {
        eprintln!("Nothing to delete for the given filters.");
        return 0;
    }

    eprint!("{}", render_table(rows, false));
    eprintln!();

    if !yes && !confirm(links.len()) {
        return 1;
    }

    let mut failed = 0usize;
    let stdout = io::stdout();
    let mut out = stdout.lock();
    for l in &links {
        match fs::remove_file(l) {
            Ok(()) => {
                let _ = writeln!(out, "removed {l}");
            }
            Err(_) => {
                eprintln!("FAILED  {l}");
                failed += 1;
            }
        }
    }
    let _ = out.flush();

    eprintln!();
    eprintln!(
        "Removed {}/{} root symlink(s). Run 'nix-collect-garbage' to reclaim the store space.",
        links.len() - failed,
        links.len()
    );
    if failed > 0 { 1 } else { 0 }
}

/// Three-way confirmation: stdin if it is a tty, else /dev/tty, else refuse.
/// Returns true only on an explicit yes; prints "Aborted." on a no.
fn confirm(n: usize) -> bool {
    let prompt = format!("Delete the {n} indirect symlink(s) for the roots above? [y/N] ");

    let answer = if unsafe { libc::isatty(0) } == 1 {
        eprint!("{prompt}");
        let _ = io::stderr().flush();
        read_line(&mut io::stdin().lock())
    } else if let Ok(tty) = fs::OpenOptions::new().read(true).open("/dev/tty") {
        eprint!("{prompt}");
        let _ = io::stderr().flush();
        read_line(&mut BufReader::new(tty))
    } else {
        eprintln!("error: refusing to delete without a terminal; pass -y to confirm.");
        return false;
    };

    match answer.trim() {
        "y" | "Y" | "yes" | "YES" => true,
        _ => {
            eprintln!("Aborted.");
            false
        }
    }
}

fn read_line(r: &mut impl BufRead) -> String {
    let mut buf = String::new();
    let _ = r.read_line(&mut buf);
    buf
}
