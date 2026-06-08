# gcan

Analyze, filter, and prune **Nix GC roots**. For every root under
`/nix/var/nix/gcroots` it reports the transitive closure size, where the
indirect "result" symlink lives, and how old it is. `direnv` roots are grouped
per project, and the listing is gated to roots the **current user can actually
delete** (never the protected `current-*` / `booted-*` roots), so it doubles as
a safe deletion preview.

It is a Rust port of the reference `gcroot-sizes.sh` in this repo, adding JSON
output and a cache.

## Usage

```
gcan [OPTIONS] [GCROOTS_DIR]            # default GCROOTS_DIR: /nix/var/nix/gcroots

  -s, --min-size <SIZE>   only roots whose closure is >= SIZE  (500M, 2G, bytes)
  -a, --min-age  <AGE>    only roots at least AGE old           (30d, 12h, 2w)
      --all               also list protected/undeletable roots (table/JSON only)
      --tui               interactive terminal UI (browse, sort, toggle, delete)
      --json              structured JSON output
  -p, --print-links       print indirect symlink paths (pipe into `xargs rm`)
  -d, --delete            delete matching roots after confirmation
  -y, --yes               skip the confirmation (with --delete)
      --no-cache          bypass the cache (no read, no write)
  -h, --help
```

### Interactive TUI (`--tui`)

`gcan --tui` opens a full-screen browser of every root:

```
↑/↓ (or j/k)  move        s  sort by size      r  reverse sort order
Home/End      jump        n  sort by name      t  toggle all / deletable-only
D             delete      a  sort by age        q / Esc  quit
```

`D` asks for confirmation before unlinking, then rescans live. Protected
(`current-*`/`booted-*`) and root-owned roots are shown greyed out with a marker
and cannot be deleted. As with the other modes, run `nix-collect-garbage`
afterwards to reclaim the space.

Examples:

```sh
gcan -s 1G -a 30d                 # preview: groups >= 1G, older than 30 days
gcan -s 1G -a 30d -p | xargs rm   # release them, then:
nix-collect-garbage               # actually reclaim the store space
gcan -s 2G -a 30d -d              # interactive delete with a confirmation
gcan --all --json                 # full inventory as JSON
```

Deleting a root only unlinks its indirect symlink; run `nix-collect-garbage`
afterwards to reclaim the disk space and clear the stale `auto/<hash>` entries.

## Caching

Nix store paths are immutable, so closure sizes never change. `gcan` caches them
in `${XDG_CACHE_HOME:-~/.cache}/gcan/cache.json`:

- `sizes`: each store path's own NAR size
- `groups`: each member-set's union closure size

A warm, unchanged run reads every group size from the cache and issues **zero**
`nix-store` calls. `--no-cache` bypasses the cache entirely; it produces
byte-identical output to a cached run (only slower), which the test suite asserts.

## Requirements

`gcan` shells out to `nix-store`, so **`nix` must be on `PATH`** at runtime. This
is always the case on a host that has `/nix/var/nix/gcroots`. The Nix package
intentionally does not bundle `nix` (keeps the closure tiny).

## Build

```sh
nix build              # -> ./result/bin/gcan
nix develop            # dev shell with cargo/rustc/clippy
cargo build --release
cargo test
```
