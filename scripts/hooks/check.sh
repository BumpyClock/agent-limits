#!/usr/bin/env bash
set -euo pipefail
command -v cargo >/dev/null || { echo 'Prepare the project Rust toolchain and locked dependencies first.' >&2; exit 1; }
export RUSTUP_AUTO_INSTALL=0 CARGO_NET_OFFLINE=true
[[ -f Cargo.lock ]] || { echo 'Missing Cargo.lock; restore the tracked lockfile.' >&2; exit 1; }
if grep -q 'https://github.com/BumpyClock/zed' Cargo.toml; then
  echo 'Pinned GPUI points at a deleted repository. Owner must choose a replacement; hook never fetches it.' >&2; exit 1
fi
[[ -f vendor/gpui-component/crates/ui/Cargo.toml ]] || { echo 'Prepare the pinned gpui-component submodule explicitly before pushing.' >&2; exit 1; }
cargo fmt --all -- --check
cargo check --workspace --locked --offline
