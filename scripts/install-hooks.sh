#!/usr/bin/env bash
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
current=$(git config --get core.hooksPath || true)
if [[ -n $current && $current != .githooks ]]; then
  echo 'Refusing to replace existing core.hooksPath. Reconcile existing hooks explicitly.' >&2; exit 1
fi
if [[ -z $current ]]; then
  hooks=$(git rev-parse --git-path hooks)
  for hook in "$hooks"/*; do
    [[ $hook == *.sample || ! -x $hook || -d $hook ]] && continue
    echo 'Refusing to hide existing active hooks. Reconcile them explicitly.' >&2; exit 1
  done
fi
[[ -x .githooks/pre-push ]] || { echo 'Missing executable .githooks/pre-push.' >&2; exit 1; }
# Relative hooksPath resolves within each worktree, not the installer checkout.
git config --local core.hooksPath .githooks
echo 'Opt-in hooks enabled for this repository. No tools installed.'
