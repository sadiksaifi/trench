## Commands

- `make help` — show targets and configurable vars; default goal
- `make build` — build binary
- `make check` — baseline-safe compile checks across all targets
- `make test` — run test suite
- `make run ARGS="list --json"` — run `trench` with arbitrary CLI args
- `make fmt` — format codebase
- `make fmt-check` — formatting gate without rewriting
- `make lint` — baseline-compatible clippy pass
- `make lint-strict` — strict clippy with warnings denied
- `make install` — install `trench` from current checkout
- `make completion-bash` — generate bash completions into `target/completions`
- `make completion-zsh` — generate zsh completions into `target/completions`
- `make completion-fish` — generate fish completions into `target/completions`
- `make completions` — generate all shell completions
- `make clean` — remove build artifacts
- `Vars:` `CARGO`, `ARGS`, `COMPLETIONS_DIR`, `CLIPPY_COMPAT_ALLOW`

## Architecture

- `Shape:` single Rust binary; CLI first, TUI only when no subcommand and stdin/stdout are TTYs
- `Runtime:` Rust 2021; `clap` CLI, `git2` git ops, `tokio` orchestration, `ratatui`/`crossterm` TUI, `tracing` file logging
- `State:` stateless; live Git worktrees and refs are the source of truth; trench owns no product database
- `Config:` global trench config plus project `.trench.toml`; resolver in `src/config/mod.rs`
- `Paths:` configured worktree root and branch sanitization are resolved without creating product state
- `Core flow:` `src/main.rs` parses flags, launches TUI or dispatches commands, maps typed failures to stable exit codes
- `Layout:` `src/cli/commands/*` command handlers; `src/git/*` low-level git/worktree ops; `src/worktree_catalog.rs` live identity and status discovery; `src/hooks/*` lifecycle hooks + streaming; `src/output/*` table/json/porcelain; `src/tui/*` cockpit flows/theme/watcher; `tests/` process-level integration tests

## Design Principles

- Headless-first. CLI output, exit codes, `--json`, `--porcelain`, `--dry-run` are product surface; TUI is secondary
- TDD mandatory. New behavior starts red; keep unit tests near module, add `tests/` when behavior crosses process boundary
- Keep `--dry-run` side-effect free. Use read-only path helpers and non-mutating resolution; no directory creation, config writes, hooks, network access, or git mutation
- Preserve config contract. Precedence `CLI > .trench.toml > global trench config > defaults`; non-hook fields merge per-field; project hooks replace global hooks entirely
- Treat structured output as API. Changes to JSON, porcelain, exit codes, event ordering, or log payloads need tests
- Centralize worktree resolution. Raw branch names and sanitized names must keep matching through the live catalog, not ad hoc per command

## Sharp Edges

- Bare `trench` on non-TTY errors instead of falling back; automation must call explicit subcommands
- Hook order is `copy -> run -> shell`; `pre_*` and `post_create` failures abort, `post_sync` reports after success, `post_remove` warns only
- Structured and preview flags are command-local: create/remove/sync support `--json` and `--dry-run`; list supports `--json` and `--porcelain`
- Branch sanitization folds `/`, space, `@`, `..` into `-`
- Startup logging writes to XDG state dir immediately; `TRENCH_LOG` controls filter
