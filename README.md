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

## Install on Linux (from source)

Linux users can build and install from source. Prebuilt release archives and the
standalone installer currently support macOS only.

Install Rust and Cargo, Git, a C/C++ toolchain, CMake, pkg-config, and Perl. On
Debian or Ubuntu, the non-Rust prerequisites are available with:

```sh
sudo apt install build-essential git cmake pkg-config perl
```

Then, from a checkout of this repository:

```sh
make build
make install
trench --version
```

Cargo installs the executable to `${CARGO_HOME:-$HOME/.cargo}/bin/trench`; ensure
that directory is on your `PATH`. Source installs are updated by pulling the
latest source and running `make install` again, not by `trench upgrade`.

## Using trench

Run `trench` in a terminal and you get the list: every worktree of the
repository, its branch and its path. From there the keys on the last row do
the work. `c` creates a worktree, `Enter` switches to the selected one, `s`
syncs it with its base, `d` removes it, `/` searches, and `?` shows the rest.

Creating is one screen. Type a branch name. If the branch exists, locally or
on origin, trench checks it out; if it does not, trench creates it from the
base shown in `From`, which you can change with `Tab`. The lines at the bottom
of the dialog say what `Enter` is about to do, or why it cannot do it yet.

Removing is one screen too. It shows what would be lost before you decide:
uncommitted changes, or commits that exist only on a branch you chose to
delete. Only `y` removes, and `Enter` never does. When a risk has just
appeared on screen, `y` waits a moment before it works, so a fast double
press cannot accept something you have not read.

The TUI uses your terminal's own colors and background by default, and it
honors `NO_COLOR`. Set `[ui] theme` to `ops`, `catppuccin` or `gruvbox` for a
fixed palette, and add `-transparent` to keep your terminal's background.

## CLI

Everything the TUI does is also a command, for scripts and for agents:
`create`, `list`, `switch`, `open`, `sync`, `remove`, `init`, `shell-init`,
`completions` and `upgrade`. Run `trench <command> --help` for the options.

```sh
trench create feature/login            # prints the new worktree's path
trench create feature/login --from develop
trench sync feature/login --strategy rebase
trench remove feature/login --delete-branch
```

`trench list` prints each worktree's branch and path. JSON records contain
`worktree`, `branch`, `path`, `is_main`, `is_current` and `detached`;
porcelain contains name, branch and path. Listing never scans file status or
counts commits ahead and behind. Sync and remove check the worktree you name
at the moment you act.

stdout carries what a script would capture: the path from `create` and
`switch`, the output of `list`, `--dry-run` previews, and `--json` documents.
What `create`, `sync` and `remove` did, such as "Removed worktree
feature-login", goes to stderr along with hook output. Errors read
`error: ...` and, where trench knows the next step, are followed by a `hint:`
line.

If you delete a worktree's directory by hand, Git keeps listing it. trench
marks it `(missing)` in `list`, and `trench remove` cleans up what is left.

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
