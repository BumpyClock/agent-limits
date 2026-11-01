# Opt-in local checks

Owner must first choose a replacement for the GPUI dependency that points at the deleted BumpyClock/zed repository. Never fetch or recreate that repository. Then explicitly prepare the approved pinned gpui-component submodule and installed Rust toolchain/dependencies.

## Installation and push contract

Enable explicitly with `bash scripts/install-hooks.sh` using Bash/Git Bash. Windows-target repositories also provide `pwsh -NoProfile -File scripts/install-hooks.ps1`. Installer is idempotent and refuses to hide existing active hooks or replace custom core.hooksPath. Relative repository-local .githooks resolves within each linked worktree. It is never installed automatically.

Every nondeleted ref must resolve to checked-out HEAD. Checks require a clean checkout including nonignored untracked files. No stash, rewriting, fetch or actual push occurs. New refs, unavailable bases, empty input/ranges, deletions, renames and shared changes broaden to full project checks. Multiple refs are consumed before one validation. Deleted refs alone skip. Other commits need their own clean checkout.

## Commands and acceptance

Prepared hook refuses the deleted dependency before Cargo can fetch it. After dependency ownership is resolved, it runs full `cargo fmt --all -- --check` and `cargo check --workspace --locked --offline`. Whole workspace is conservative until a passing baseline exists. No credentials or live provider accounts are touched.

Application ref is main, not data-only tsq-sync. Deleted pinned GPUI origin and absent vendor submodule prevent real workspace acceptance. No dependency substitution is guessed.

`python3 scripts/hooks/test-hooks.py` exercises executable hooks and installers in disposable Git repositories with stub project commands, including linked worktrees. No actual push. Harness evidence is separate from actual project/native acceptance. No under-nine-second promise, CI activation or affected-only optimization is claimed. No user hooks were installed during preparation.
