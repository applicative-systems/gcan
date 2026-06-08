//! gcan — analyze, filter, and prune Nix GC roots.

mod cache;
mod format;
mod nix;
mod output;
mod size;
mod tui;
mod walk;

use cache::Cache;
use clap::{Args, Parser, Subcommand, ValueEnum};
use output::{Row, SortKey};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

/// Analyze Nix GC roots: transitive closure size, age, and location, with direnv
/// roots grouped per project. `list` and `tui` default to roots you can actually
/// delete (never the protected `current-*`/`booted-*` ones); `--all` widens the
/// view.
#[derive(Parser)]
#[command(name = "gcan", version, about, long_about = None)]
struct Cli {
    #[command(flatten)]
    global: Global,

    #[command(subcommand)]
    command: Command,
}

/// Options shared by every subcommand (usable before or after it).
#[derive(Args)]
struct Global {
    /// Directory of GC roots to analyze.
    #[arg(
        long,
        value_name = "DIR",
        default_value = "/nix/var/nix/gcroots",
        global = true
    )]
    gcroots: PathBuf,

    /// Bypass the cache (no read, no write).
    #[arg(long = "no-cache", global = true)]
    no_cache: bool,

    /// Pin "now" to a fixed epoch (testing; makes ages reproducible).
    #[arg(long, hide = true, global = true)]
    now: Option<u64>,
}

#[derive(Subcommand)]
enum Command {
    /// List GC roots as a table, JSON, or raw symlink paths.
    List(ListArgs),

    /// Delete GC roots (asks for confirmation unless --yes).
    Delete(DeleteArgs),

    /// Browse and prune GC roots interactively.
    Tui(TuiArgs),
}

/// Predicate filters, shared by list/delete/tui.
#[derive(Args)]
struct FilterArgs {
    /// Only roots whose closure is at least SIZE (e.g. 500M, 2G).
    #[arg(short = 's', long = "min-size", value_name = "SIZE")]
    min_size: Option<String>,

    /// Only roots at least AGE old (e.g. 30d, 12h, 2w).
    #[arg(short = 'a', long = "min-age", value_name = "AGE")]
    min_age: Option<String>,
}

#[derive(Args)]
struct ListArgs {
    #[command(flatten)]
    filter: FilterArgs,

    /// Sort by this column.
    #[arg(long, value_enum, default_value_t = SortKey::Size)]
    sort: SortKey,

    /// Reverse the sort order.
    #[arg(short = 'r', long)]
    reverse: bool,

    /// Include protected/undeletable roots in the listing.
    #[arg(long)]
    all: bool,

    /// Output format.
    #[arg(long, value_enum, default_value_t = Format::Table)]
    format: Format,
}

#[derive(Args)]
struct DeleteArgs {
    #[command(flatten)]
    filter: FilterArgs,

    /// Skip the confirmation prompt.
    #[arg(short = 'y', long)]
    yes: bool,
}

#[derive(Args)]
struct TuiArgs {
    #[command(flatten)]
    filter: FilterArgs,

    /// Initial sort column (change it interactively too).
    #[arg(long, value_enum, default_value_t = SortKey::Size)]
    sort: SortKey,

    /// Start showing all roots, not just the deletable ones.
    #[arg(long)]
    all: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Format {
    Table,
    Json,
    Paths,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let g = cli.global;

    if !g.gcroots.is_dir() {
        return fail(&format!("{} is not a directory", g.gcroots.display()));
    }
    let now = g.now.unwrap_or_else(real_now);
    let groups = walk::scan(&g.gcroots);
    let mut cache = if g.no_cache {
        Cache::empty()
    } else {
        Cache::load(&cache::default_path())
    };

    match cli.command {
        Command::Tui(a) => {
            let (min_size, min_age) = match filters(&a.filter) {
                Ok(v) => v,
                Err(e) => return fail(&e),
            };
            // The TUI owns the cache and persists it on exit.
            match tui::run(
                groups,
                cache,
                now,
                &g.gcroots,
                !g.no_cache,
                a.all,
                min_size,
                min_age,
                a.sort,
            ) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => fail(&format!("tui: {e}")),
            }
        }

        Command::List(a) => {
            let (min_size, min_age) = match filters(&a.filter) {
                Ok(v) => v,
                Err(e) => return fail(&e),
            };
            let mut rows = build_rows(&groups, &mut cache, now, a.all, min_size, min_age);
            save_cache(&g, &cache);
            output::sort_rows(&mut rows, a.sort, a.sort.default_desc() ^ a.reverse);
            match a.format {
                Format::Table => print!("{}", output::render_table(&rows, a.all)),
                Format::Json => println!(
                    "{}",
                    output::render_json(&rows, &g.gcroots.to_string_lossy(), now)
                ),
                Format::Paths => output::print_links(&rows),
            }
            ExitCode::SUCCESS
        }

        Command::Delete(a) => {
            let (min_size, min_age) = match filters(&a.filter) {
                Ok(v) => v,
                Err(e) => return fail(&e),
            };
            // Delete only ever touches the deletable shortlist.
            let mut rows = build_rows(&groups, &mut cache, now, false, min_size, min_age);
            save_cache(&g, &cache);
            output::sort_rows(&mut rows, SortKey::Size, true);
            ExitCode::from(output::delete(&rows, a.yes) as u8)
        }
    }
}

/// Parse the `(min_size, min_age)` predicate floor from filter args.
fn filters(f: &FilterArgs) -> Result<(u64, u64), String> {
    let size = f
        .min_size
        .as_deref()
        .map(format::parse_size)
        .transpose()?
        .unwrap_or(0);
    let age = f
        .min_age
        .as_deref()
        .map(format::parse_age)
        .transpose()?
        .unwrap_or(0);
    Ok((size, age))
}

/// Resolve closure sizes and build the displayed rows. The candidate gate is
/// applied *before* sizing so closures we won't show are never computed.
fn build_rows(
    groups: &[walk::Group],
    cache: &mut Cache,
    now: u64,
    all: bool,
    min_size: u64,
    min_age: u64,
) -> Vec<Row> {
    let age_of = |g: &walk::Group| now.saturating_sub(g.newest_mtime);
    let candidates: Vec<&walk::Group> = groups
        .iter()
        .filter(|g| (all || g.deletable()) && age_of(g) >= min_age)
        .collect();
    let sizes = size::group_sizes(&candidates, cache);
    candidates
        .iter()
        .zip(&sizes)
        .filter(|(_, &sz)| sz >= min_size)
        .map(|(g, &sz)| Row::from_group(g, sz, now))
        .collect()
}

fn save_cache(g: &Global, cache: &Cache) {
    if !g.no_cache {
        if let Err(e) = cache.save(&cache::default_path()) {
            eprintln!("warning: could not write cache: {e}");
        }
    }
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
