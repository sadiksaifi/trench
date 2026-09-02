> [!NOTE]
> Trench currently publishes unsigned, unnotarized macOS binaries. The installer
> verifies GitHub release checksums; GitHub also publishes build-provenance
> attestations for every release asset.

# trench

A fast, ergonomic, headless-first Git worktree manager built in Rust.

**trench** removes all friction from Git worktree management — from creation to teardown — while providing structured, machine-readable output so AI agents and automation scripts can use it as reliably as a human using the TUI.

## Install on macOS

Apple Silicon and Intel Macs running macOS 11 or newer are supported. Install
without `sudo`, configure `trench` and the `tn` shell function, then reload the
login shell:

```sh
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/sadiksaifi/trench/releases/latest/download/trench-installer.sh |
  sh && exec "${SHELL:-/bin/zsh}" -l
```

The default executable is `$HOME/.local/bin/trench`. Set
`TRENCH_INSTALL_DIR` to choose another directory, or install without changing
shell configuration:

```sh
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/sadiksaifi/trench/releases/latest/download/trench-installer.sh |
  sh -s -- --no-modify-shell
```

The installer verifies the release manifest and archive checksum before
installing, backs up any shell file it changes, and reports only actual effects.
Standalone installs can later run `trench upgrade`. Future Homebrew installs
delegate that command to `brew upgrade trench`; source and manual installs are
never overwritten automatically.

## CLI

Trench exposes ten commands: `create`, `remove`, `switch`, `open`, `list`, `sync`, `init`, `shell-init`, `completions`, and `upgrade`. Run `trench <command> --help` for command-specific options.

Structured and preview flags are local to the commands that support them:

- `create`, `remove`, and `sync`: `--json`, `--dry-run`
- `list`: `--json`, `--porcelain`
- `switch`, `open`, `init`, `shell-init`, `completions`, and `upgrade`: no structured-output or preview flags

## Filesystem paths

Trench follows the XDG Base Directory specification on macOS and Linux. Explicit
XDG values must be absolute; unset, empty, or relative values use the standard
`HOME`-based default.

| Purpose | Environment variable | Default application directory |
| --- | --- | --- |
| Configuration | `XDG_CONFIG_HOME` | `$HOME/.config/trench` |
| Data and standalone install receipt | `XDG_DATA_HOME` | `$HOME/.local/share/trench` |
| State and logs | `XDG_STATE_HOME` | `$HOME/.local/state/trench` |
| Cache | `XDG_CACHE_HOME` | `$HOME/.cache/trench` |

The global configuration file is `config.toml` in the configuration directory.
Diagnostic logs are written to `trench.log` in the state directory. Trench does
not currently persist application data or cache files.

Trench keeps no product database or installed-version state. Live Git worktrees
and refs remain the source of truth; a standalone installer writes only
`install-receipt.json` so `trench upgrade` can prove which executable it owns.

## Maintainer releases

Git tags are the only release-version source of truth. Package metadata remains
at `0.0.0`; builds derive their displayed version from the exact Git tag, commit,
and dirty state. To preview or publish a canonical annotated release tag, use
TagSmith latest exclusively:

```sh
pnpm dlx tagsmith@latest tag --target trench --channel stable --version 0.1.0 --dry-run --json
pnpm dlx tagsmith@latest tag --target trench --channel stable --version 0.1.0 --push
```

Never create or push release tags manually. A valid pushed `vX.Y.Z` tag triggers
shared CI gates, native builds for both macOS architectures, portability checks,
checksums, a schema-versioned manifest, attestations, transient git-cliff notes,
and all-or-nothing draft publication.
