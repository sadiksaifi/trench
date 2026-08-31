> [!WARNING]
> This project is under heavy development. Nothing is stable or usable yet.

# trench

A fast, ergonomic, headless-first Git worktree manager built in Rust.

**trench** removes all friction from Git worktree management — from creation to teardown — while providing structured, machine-readable output so AI agents and automation scripts can use it as reliably as a human using the TUI.

## CLI

Trench exposes nine commands: `create`, `remove`, `switch`, `open`, `list`, `sync`, `init`, `shell-init`, and `completions`. Run `trench <command> --help` for command-specific options.

Structured and preview flags are local to the commands that support them:

- `create`, `remove`, and `sync`: `--json`, `--dry-run`
- `list`: `--json`, `--porcelain`
- `switch`, `open`, `init`, `shell-init`, and `completions`: no structured-output or preview flags

## Filesystem paths

Trench follows the XDG Base Directory specification on macOS and Linux. Explicit
XDG values must be absolute; unset, empty, or relative values use the standard
`HOME`-based default.

| Purpose | Environment variable | Default application directory |
| --- | --- | --- |
| Configuration | `XDG_CONFIG_HOME` | `$HOME/.config/trench` |
| Data | `XDG_DATA_HOME` | `$HOME/.local/share/trench` |
| State and logs | `XDG_STATE_HOME` | `$HOME/.local/state/trench` |
| Cache | `XDG_CACHE_HOME` | `$HOME/.cache/trench` |

The global configuration file is `config.toml` in the configuration directory.
Diagnostic logs are written to `trench.log` in the state directory. Trench does
not currently persist application data or cache files.

Trench is stateless. Live Git worktrees and refs are the source of truth; it does not create or maintain a product database.
