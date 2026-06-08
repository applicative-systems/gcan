//! gcan — analyze, filter, and prune Nix GC roots.

mod cache;
mod format;
mod nix;
mod output;
mod size;
mod tui;
mod walk;

use cache::Cache;
use clap::Parser;
use output::Row;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

/// Analyze Nix GC roots: transitive closure size, age, and location, with
/// direnv roots grouped per project. By default only roots the current user can
/// delete (and never protected `current-*`/`booted-*` roots) are shown, so the
/// output doubles as a safe deletion preview.
#[derive(Parser)]
#[command(name = "gcan", version, about, long_about = None)]
struct Cli {
    /// Only show roots whose closure is at least SIZE (e.g. 500M, 2G).
    #[arg(short = 's', long = "min-size", value_name = "SIZE")]
    min_size: Option<String>,

    /// Only show roots at least AGE old (e.g. 30d, 12h, 2w).
    #[arg(short = 'a', long = "min-age", value_name = "AGE")]
    min_age: Option<String>,

    /// Include protected/undeletable roots in the listing (table/JSON only).
    #[arg(long)]
    all: bool,

    /// Launch the interactive terminal UI (sort, toggle, delete).
    #[arg(long, group = "mode")]
    tui: bool,

    /// Emit structured JSON instead of the table.
    #[arg(long, group = "mode")]
    json: bool,

    /// Print only the indirect symlink paths (pipe into `xargs rm`).
    #[arg(short = 'p', long = "print-links", group = "mode")]
    print_links: bool,

    /// Delete the matching roots after confirmation.
    #[arg(short = 'd', long, group = "mode")]
    delete: bool,

    /// Skip the confirmation prompt (use with --delete).
    #[arg(short = 'y', long, requires = "delete")]
    yes: bool,

    /// Bypass the cache (no read, no write).
    #[arg(long = "no-cache")]
    no_cache: bool,

    /// Pin "now" to a fixed epoch (testing; makes ages reproducible).
    #[arg(long, hide = true)]
    now: Option<u64>,

    /// Directory of GC roots to analyze.
    #[arg(value_name = "GCROOTS_DIR", default_value = "/nix/var/nix/gcroots")]
    gcroots_dir: PathBuf,
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    let min_size = match cli.min_size.as_deref().map(format::parse_size).transpose() {
        Ok(v) => v.unwrap_or(0),
        Err(e) => return fail(&e),
    };
    let min_age = match cli.min_age.as_deref().map(format::parse_age).transpose() {
        Ok(v) => v.unwrap_or(0),
        Err(e) => return fail(&e),
    };

    if !cli.gcroots_dir.is_dir() {
        return fail(&format!("{} is not a directory", cli.gcroots_dir.display()));
    }

    let now = cli.now.unwrap_or_else(real_now);
    let groups = walk::scan(&cli.gcroots_dir);

    let mut cache = if cli.no_cache {
        Cache::empty()
    } else {
        Cache::load(&cache::default_path())
    };

    if cli.tui {
        return match tui::run(groups, cache, now, &cli.gcroots_dir, !cli.no_cache, cli.all) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => fail(&format!("tui: {e}")),
        };
    }

    // Delete/print-links always act on the deletable shortlist; the table/JSON
    // listing shows the shortlist too, unless --all widens it.
    let shortlist_only = cli.delete || cli.print_links || !cli.all;

    // Candidate gate applied *before* sizing, so we never compute closures for
    // groups we won't display (e.g. the huge current-system in default mode).
    let age_of = |g: &walk::Group| now.saturating_sub(g.newest_mtime);
    let candidates: Vec<&walk::Group> = groups
        .iter()
        .filter(|g| {
            if shortlist_only && !g.deletable() {
                return false;
            }
            age_of(g) >= min_age
        })
        .collect();

    let sizes = size::group_sizes(&candidates, &mut cache);
    if !cli.no_cache {
        if let Err(e) = cache.save(&cache::default_path()) {
            eprintln!("warning: could not write cache: {e}");
        }
    }

    let mut rows: Vec<Row> = candidates
        .iter()
        .zip(&sizes)
        .filter(|(_, &sz)| sz >= min_size)
        .map(|(g, &sz)| Row::from_group(g, sz, now))
        .collect();
    output::sort_rows(&mut rows);

    if cli.delete {
        return ExitCode::from(output::delete(&rows, cli.yes) as u8);
    }
    if cli.print_links {
        output::print_links(&rows);
        return ExitCode::SUCCESS;
    }
    if cli.json {
        println!(
            "{}",
            output::render_json(&rows, &cli.gcroots_dir.to_string_lossy(), now)
        );
        return ExitCode::SUCCESS;
    }
    print!("{}", output::render_table(&rows, cli.all));
    ExitCode::SUCCESS
}

fn real_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn fail(msg: &str) -> ExitCode {
    eprintln!("error: {msg}");
    ExitCode::from(2)
}
