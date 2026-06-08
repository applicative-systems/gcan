#!/usr/bin/env bash
#
# gcroot-sizes.sh — analyse Nix GC roots.
#
# For every GC root under the gcroots directory this reports:
#   * the transitive closure size (on-disk size of the whole dependency tree)
#   * where the indirect "result" symlink lives
#   * how old that symlink is
#
# direnv creates a swarm of roots under each project's `.direnv/` directory
# (one per flake input plus a flake-profile). Those are noisy and uninteresting
# individually, so all roots belonging to the same project are collapsed into a
# single grouped entry whose size is the *union* of their closures (shared store
# paths counted once).
#
# With --min-size / --min-age it doubles as a deletion preview: only roots the
# *current user* can actually delete are listed, so the output is a safe shortlist
# of what you could reclaim. A root is removed by deleting the indirect symlink it
# points at, which requires write+execute permission on that symlink's directory.
#
# Usage: gcroot-sizes.sh [-s SIZE] [-a AGE] [-p] [GCROOTS_DIR]
#
#   -s, --min-size SIZE   only show roots whose closure is >= SIZE  (e.g. 500M, 2G)
#   -a, --min-age  AGE    only show roots at least AGE old          (e.g. 30d, 12h, 2w)
#   -p, --print-links     print only the indirect symlink paths of the matching
#                         roots (one per line) instead of the table — pipe into
#                         `xargs rm` to release them, then run nix-collect-garbage
#   -d, --delete          delete the matching roots (unlink their indirect
#                         symlinks) after showing them and asking for confirmation
#   -y, --yes             skip the confirmation prompt (use with --delete)
#   -h, --help            show this help
#
# GCROOTS_DIR defaults to /nix/var/nix/gcroots.

set -euo pipefail

usage() { sed -n '3,30p' "$0" | sed 's/^# \{0,1\}//'; }

MIN_SIZE=0
MIN_AGE=0
PRINT_LINKS=0
DELETE=0
ASSUME_YES=0
GCROOTS=/nix/var/nix/gcroots

parse_size() { numfmt --from=iec "$1" 2>/dev/null || {
  echo "invalid size: $1" >&2
  exit 1
}; }

parse_age() {
  local v=$1 num unit
  num=${v%%[smhdw]*}
  unit=${v##*[0-9]}
  [[ $num =~ ^[0-9]+$ ]] || {
    echo "invalid age: $v" >&2
    exit 1
  }
  case $unit in
  s | "") echo "$num" ;;
  m) echo "$((num * 60))" ;;
  h) echo "$((num * 3600))" ;;
  d) echo "$((num * 86400))" ;;
  w) echo "$((num * 604800))" ;;
  *)
    echo "invalid age unit in: $v" >&2
    exit 1
    ;;
  esac
}

# Active "current" roots must never be offered for deletion, regardless of who
# owns them: the live system, the booted system, the current home generation, and
# any other `current-*` / `*-current` / `booted-*` pointer. Deleting these breaks
# a running configuration even though the symlink itself is user-writable.
is_protected() {
  local base
  base=$(basename "$1")
  case "$base" in
  current-* | *-current | current | booted-*) return 0 ;;
  esac
  return 1
}

while [[ $# -gt 0 ]]; do
  case $1 in
  -s | --min-size)
    MIN_SIZE=$(parse_size "$2")
    shift 2
    ;;
  -a | --min-age)
    MIN_AGE=$(parse_age "$2")
    shift 2
    ;;
  -p | --print-links)
    PRINT_LINKS=1
    shift
    ;;
  -d | --delete)
    DELETE=1
    shift
    ;;
  -y | --yes)
    ASSUME_YES=1
    shift
    ;;
  -h | --help)
    usage
    exit 0
    ;;
  -*)
    echo "unknown option: $1" >&2
    exit 1
    ;;
  *)
    GCROOTS=$1
    shift
    ;;
  esac
done

if [[ ! -d $GCROOTS ]]; then
  echo "error: $GCROOTS is not a directory" >&2
  exit 1
fi

if ((PRINT_LINKS && DELETE)); then
  echo "error: --print-links and --delete are mutually exclusive" >&2
  exit 1
fi

# group key -> data, accumulated across all roots
declare -A G_PATHS  # space-separated final /nix/store paths in the group
declare -A G_MTIME  # newest mtime (epoch seconds) seen in the group
declare -A G_COUNT  # number of roots collapsed into the group
declare -A G_LOC    # human-readable location to display
declare -A G_DELETE # 1 if every root in the group is removable by this user
declare -A G_LINKS  # newline-separated indirect symlink paths in the group

now=$(date +%s)

# Walk every symlink that is a GC root (auto/*, per-user/*, top-level ones).
while IFS= read -r link; do
  indirect=$(readlink "$link") || continue

  # Skip roots whose indirect symlink has already been removed: the auto/<hash>
  # entry lingers until `nix-collect-garbage` prunes it, but the root is gone and
  # there is nothing left for the user to delete or reclaim by hand.
  [[ -e $indirect || -L $indirect ]] || continue

  # Resolve to the actual store path; ignore roots that don't land in the store.
  final=$(readlink -f "$link" 2>/dev/null) || final=""
  case "$final" in
  /nix/store/*) ;;
  *) final="" ;;
  esac

  # Age = mtime of the indirect symlink itself (its own lstat, never followed).
  # Fall back to the gcroot symlink if the indirect target is unavailable.
  mtime=$(find "$indirect" -maxdepth 0 -printf '%T@\n' 2>/dev/null | cut -d. -f1) || true
  if [[ -z ${mtime:-} ]]; then
    mtime=$(find "$link" -maxdepth 0 -printf '%T@\n' | cut -d. -f1)
  fi

  # Removable by the current user iff we can unlink the indirect symlink (we hold
  # write+execute on its parent directory) AND it is not a protected "current" root.
  parent=$(dirname "$indirect")
  if [[ -w $parent && -x $parent ]] && ! is_protected "$indirect"; then del=1; else del=0; fi

  # Group direnv roots by their project directory; everything else stands alone.
  if [[ $indirect == */.direnv/* ]]; then
    key="${indirect%%/.direnv/*}"
    loc="$key/.direnv/  (direnv)"
  else
    key="$indirect"
    loc="$indirect"
  fi

  [[ -n $final ]] && G_PATHS["$key"]+=" $final"
  G_COUNT["$key"]=$((${G_COUNT["$key"]:-0} + 1))
  G_LOC["$key"]="$loc"
  G_DELETE["$key"]=$((${G_DELETE["$key"]:-1} * del))
  G_LINKS["$key"]+="$indirect"$'\n'
  if [[ -z ${G_MTIME["$key"]:-} || $mtime -gt ${G_MTIME["$key"]} ]]; then
    G_MTIME["$key"]=$mtime
  fi
done < <(find "$GCROOTS" -type l)

# Compute the union closure size (bytes) of a set of store paths.
closure_bytes() {
  local paths="$1"
  [[ -z ${paths// /} ]] && {
    echo 0
    return
  }
  # requisites of all members -> dedup -> sum each path's own NAR size.
  nix-store -q --requisites $paths 2>/dev/null | sort -u |
    xargs -r nix-store -q --size 2>/dev/null |
    awk '{s+=$1} END{print s+0}'
}

# Render seconds-of-age as a compact human string.
human_age() {
  local secs=$1 d h
  d=$((secs / 86400))
  h=$(((secs % 86400) / 3600))
  if ((d > 0)); then
    printf '%dd' "$d"
  elif ((h > 0)); then
    printf '%dh' "$h"
  else
    printf '%dm' "$(((secs % 3600) / 60))"
  fi
}

# Build one row per qualifying group: "bytes<TAB>age<TAB>count<TAB>location<TAB>key".
# (location/key never contain tabs.)
rows=""
for key in "${!G_LOC[@]}"; do
  # Only ever offer roots this user can actually delete.
  [[ ${G_DELETE["$key"]} -eq 1 ]] || continue

  age=$((now - ${G_MTIME["$key"]}))
  ((age >= MIN_AGE)) || continue

  bytes=$(closure_bytes "${G_PATHS["$key"]:-}") || bytes=0
  ((bytes >= MIN_SIZE)) || continue

  rows+="${bytes}	${age}	${G_COUNT["$key"]}	${G_LOC["$key"]}	${key}"$'\n'
done

sorted=$(printf '%s' "$rows" | sort -t$'\t' -k1,1 -rn)

# Pretty-print the qualifying groups as a table (largest first) on stdout.
print_table() {
  local bytes age count loc key total=0
  printf '%10s  %6s  %5s  %s\n' "SIZE" "AGE" "ROOTS" "LOCATION"
  printf '%10s  %6s  %5s  %s\n' "----" "---" "-----" "--------"
  while IFS=$'\t' read -r bytes age count loc key; do
    [[ -z $bytes ]] && continue
    total=$((total + bytes))
    printf '%10s  %6s  %5s  %s\n' \
      "$(numfmt --to=iec --suffix=B "$bytes")" \
      "$(human_age "$age")" \
      "$count" \
      "$loc"
  done < <(printf '%s\n' "$sorted")
  printf '%10s  %6s  %5s  %s\n' "----" "" "" ""
  printf '%10s  %6s  %5s  %s\n' "$(numfmt --to=iec --suffix=B "$total")" "" "" \
    "TOTAL reclaimable (sum of closures; paths shared across groups counted per-group)"
}

# Emit the indirect symlink paths of every qualifying group, largest first.
emit_links() {
  local bytes age count loc key
  while IFS=$'\t' read -r bytes age count loc key; do
    [[ -z $bytes ]] && continue
    printf '%s' "${G_LINKS["$key"]}"
  done < <(printf '%s\n' "$sorted")
}

# --print-links: just the symlinks on stdout, for piping into xargs.
if ((PRINT_LINKS)); then
  emit_links
  exit 0
fi

# --delete: show the table, confirm, then unlink the indirect symlinks.
if ((DELETE)); then
  mapfile -t links < <(emit_links)
  if ((${#links[@]} == 0)); then
    echo "Nothing to delete for the given filters." >&2
    exit 0
  fi
  print_table >&2
  echo >&2
  if ((!ASSUME_YES)); then
    prompt="Delete the ${#links[@]} indirect symlink(s) for the roots above? [y/N] "
    if [[ -t 0 ]]; then
      read -r -p "$prompt" ans
    elif { exec 3</dev/tty; } 2>/dev/null; then
      read -r -p "$prompt" ans <&3
      exec 3<&-
    else
      echo "error: refusing to delete without a terminal; pass -y to confirm." >&2
      exit 1
    fi
    case "$ans" in
    y | Y | yes | YES) ;;
    *)
      echo "Aborted." >&2
      exit 1
      ;;
    esac
  fi
  failed=0
  for l in "${links[@]}"; do
    [[ -z $l ]] && continue
    if rm -- "$l" 2>/dev/null; then
      echo "removed $l"
    else
      echo "FAILED  $l" >&2
      failed=$((failed + 1))
    fi
  done
  echo >&2
  echo "Removed $((${#links[@]} - failed))/${#links[@]} root symlink(s). Run 'nix-collect-garbage' to reclaim the store space." >&2
  ((failed == 0)) || exit 1
  exit 0
fi

# Default: print the table.
print_table
