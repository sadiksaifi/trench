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
- `State:` SQLite via `rusqlite` + embedded migrations in `src/state/sql`; DB stored under XDG data dir as `trench.db`
- `Config:` global `~/.config/trench/config.toml` plus project `.trench.toml`; resolver in `src/config/mod.rs`
- `Paths:` default worktree root `~/.worktrees`; default template `{{ repo }}/{{ branch | sanitize }}` in `src/paths.rs`
- `Core flow:` `src/main.rs` parses flags, launches TUI or dispatches commands, maps typed failures to stable exit codes
- `Layout:` `src/cli/commands/*` command handlers; `src/git/*` low-level git/worktree ops; `src/adopt.rs` DB-first lookup + unmanaged worktree adoption; `src/hooks/*` lifecycle hooks + streaming; `src/output/*` table/json/porcelain; `src/tui/*` screens/theme/watcher; `tests/` process-level integration tests

## Design Principles

- Headless-first. CLI output, exit codes, `--json`, `--porcelain`, `--dry-run` are product surface; TUI is secondary
- TDD mandatory. New behavior starts red; keep unit tests near module, add `tests/` when behavior crosses process boundary
- Keep `--dry-run` side-effect free. Use read-only path helpers and non-mutating resolution; no dir creation, DB writes, or git mutation
- Preserve config contract. Precedence `CLI > .trench.toml > ~/.config/trench/config.toml > defaults`; non-hook fields merge per-field; project hooks replace global hooks entirely
- Treat structured output as API. Changes to JSON, porcelain, exit codes, event ordering, or log payloads need tests
- Centralize worktree resolution. Raw branch names and sanitized names must keep matching through `adopt`/`paths`, not ad hoc per command

## Sharp Edges

- Bare `trench` on non-TTY errors instead of falling back; automation must call explicit subcommands
- Hook order is `copy -> run -> shell`; `pre_*` and `post_create` failures abort, `post_sync` reports after success, `post_remove` warns only
- `resolve_or_adopt` writes DB state by adopting unmanaged git worktrees; read-only flows must use `resolve_only`
- `Database::open` can rename an ahead-of-code DB to `*.backup-<ts>` and recreate fresh state
- Worktree templates must render relative paths without `..`; branch sanitization folds `/`, space, `@`, `..` into `-`
- Startup logging writes to XDG state dir immediately; `TRENCH_LOG` controls filter
