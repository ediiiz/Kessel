#!/usr/bin/env bash
# Keeps the build cache from filling the disk. Run by `just` before every cargo recipe.
#
#   cache-guard.sh [--force] [--report]
#
# The cargo target dir is pruned when it grows past KESSEL_CACHE_MAX_GB (default 15) or when the
# disk holding it has less than KESSEL_MIN_FREE_GB (default 5) left. Pruning drops incremental
# state, docs and stale scratch first, and clears the whole target dir only if that is not enough.
# The bun package cache is cleared when it grows past KESSEL_BUN_CACHE_MAX_GB (default 4).
set -euo pipefail

target_dir="${CARGO_TARGET_DIR:?CARGO_TARGET_DIR must be set (just sets it)}"
max_gb="${KESSEL_CACHE_MAX_GB:-15}"
min_free_gb="${KESSEL_MIN_FREE_GB:-5}"
bun_max_gb="${KESSEL_BUN_CACHE_MAX_GB:-4}"
bun_cache="${BUN_INSTALL_CACHE_DIR:-$HOME/.bun/install/cache}"

force=0
report=0
for arg in "$@"; do
    case "$arg" in
        --force) force=1 ;;
        --report) report=1 ;;
        *) echo "cache-guard: unknown argument $arg" >&2; exit 2 ;;
    esac
done

gib() { echo $(( $1 / 1024 / 1024 )); }
size_kib() { if [[ -e "$1" ]]; then du -sk "$1" 2>/dev/null | cut -f1; else echo 0; fi; }
free_kib() {
    local dir="$1"
    while [[ ! -e "$dir" ]]; do dir="$(dirname "$dir")"; done
    df -Pk "$dir" | awk 'NR == 2 { print $4 }'
}
over_budget() {
    local size free
    size="$(size_kib "$target_dir")"
    free="$(free_kib "$target_dir")"
    (( size > max_gb * 1024 * 1024 || free < min_free_gb * 1024 * 1024 ))
}

if (( report )); then
    echo "target dir  $target_dir: $(gib "$(size_kib "$target_dir")") GiB (limit $max_gb GiB)"
    echo "bun cache   $bun_cache: $(gib "$(size_kib "$bun_cache")") GiB (limit $bun_max_gb GiB)"
    echo "disk free   $(gib "$(free_kib "$target_dir")") GiB (minimum $min_free_gb GiB)"
    exit 0
fi

if (( force )) || over_budget; then
    echo "cache-guard: pruning incremental state and docs in $target_dir" >&2
    rm -rf "$target_dir"/*/incremental "$target_dir"/doc "$target_dir"/tmp "$target_dir"/flycheck*
    if (( force )) || over_budget; then
        echo "cache-guard: still over budget, clearing $target_dir" >&2
        rm -rf "${target_dir:?}"/debug "${target_dir:?}"/release "${target_dir:?}"/*-unknown-*
    fi
fi

if [[ -d "$bun_cache" ]] && (( $(size_kib "$bun_cache") > bun_max_gb * 1024 * 1024 )); then
    echo "cache-guard: clearing the bun package cache ($bun_cache)" >&2
    rm -rf "${bun_cache:?}"
fi
