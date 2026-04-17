##@var CARGO Cargo executable to use
CARGO ?= cargo
##@var ARGS Extra args passed to `trench` via `make run`
ARGS ?=
##@var COMPLETIONS_DIR Output dir for generated shell completions
COMPLETIONS_DIR ?= target/completions
##@var CLIPPY_COMPAT_ALLOW Temporary allowlist for baseline-compatible clippy runs
CLIPPY_COMPAT_ALLOW ?= -A clippy::approx_constant

.DEFAULT_GOAL := help

.PHONY: \
	help \
	build \
	check \
	test \
	run \
	fmt \
	fmt-check \
	lint \
	lint-strict \
	install \
	completion-bash \
	completion-zsh \
	completion-fish \
	completions \
	clean

help: ## Show available targets and configurable vars
	@printf "Usage: make <target> [VAR=value]\n\n"
	@printf "Targets:\n"
	@awk 'BEGIN {FS = ":.*## "}; /^[a-zA-Z0-9_.-]+:.*## / {printf "  %-18s %s\n", $$1, $$2}' $(MAKEFILE_LIST)
	@printf "\nVariables:\n"
	@awk '/^##@var / { name = $$2; desc = $$0; sub(/^##@var [^ ]+ /, "", desc); printf "  %-18s %s\n", name, desc; }' $(MAKEFILE_LIST)

build: ## Build the project
	$(CARGO) build

check: ## Run baseline-safe compile checks across all targets
	$(CARGO) check --all-targets

test: ## Run the test suite
	$(CARGO) test

run: ## Run `trench` with optional `ARGS="..."`
	$(CARGO) run -- $(ARGS)

fmt: ## Format the codebase
	$(CARGO) fmt --all

fmt-check: ## Check formatting without rewriting files
	$(CARGO) fmt --all -- --check

lint: ## Run compatibility clippy checks against current repo baseline
	$(CARGO) clippy --all-targets --all-features -- $(CLIPPY_COMPAT_ALLOW)

lint-strict: ## Run strict clippy with warnings denied
	$(CARGO) clippy --all-targets --all-features -- -D warnings

install: ## Install `trench` from the current checkout
	$(CARGO) install --path . --locked --force

completion-bash: ## Generate bash completions into `$(COMPLETIONS_DIR)`
	@mkdir -p $(COMPLETIONS_DIR)
	$(CARGO) run -- completions bash > $(COMPLETIONS_DIR)/trench.bash

completion-zsh: ## Generate zsh completions into `$(COMPLETIONS_DIR)`
	@mkdir -p $(COMPLETIONS_DIR)
	$(CARGO) run -- completions zsh > $(COMPLETIONS_DIR)/_trench

completion-fish: ## Generate fish completions into `$(COMPLETIONS_DIR)`
	@mkdir -p $(COMPLETIONS_DIR)
	$(CARGO) run -- completions fish > $(COMPLETIONS_DIR)/trench.fish

completions: completion-bash completion-zsh completion-fish ## Generate all shell completions

clean: ## Remove build artifacts
	$(CARGO) clean
