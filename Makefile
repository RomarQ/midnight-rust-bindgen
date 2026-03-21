# midnight-rust-bindgen Makefile
#
# Compiles Compact example contracts using the fork compiler (with ledger fields
# in contract-info.json) and runs codegen tests against them.
#
# Prerequisites:
#   - Nix with flakes enabled (for building the Compact compiler from the submodule)
#   - Rust toolchain
#
# Usage:
#   make build-compiler   # Build the Compact compiler from the submodule
#   make compile          # Compile all example contracts
#   make test             # Run all tests (compiles contracts first)
#   make clean            # Remove compiled outputs
#
# The Compact compiler submodule (compact/) points to the fork at
# https://github.com/RomarQ/compact on the feat/ledger-in-contract-info branch,
# which extends contract-info.json with ledger field metadata.

# Compiler built from the submodule via nix
COMPACTC := $(shell nix --extra-experimental-features "nix-command flakes" build ./compact\#compactc --no-link --print-out-paths 2>/dev/null)/bin/compactc

# Example contracts from the compact submodule
EXAMPLES_DIR := compact/examples
COMPILED_DIR := tests/fixtures/compiled

# Contracts to compile (source -> output dir mapping)
# proposal.compact excluded — uses syntax not supported by compiler v0.29.107
CONTRACTS := counter election tiny zerocash

# Derive paths
CONTRACT_CHECKSUMS := $(foreach c,$(CONTRACTS),$(COMPILED_DIR)/$(c)/.checksum)

.PHONY: all build-compiler compile test test-codegen test-rust clean check clippy help

all: compile test

help:
	@echo "Targets:"
	@echo "  build-compiler — Build the Compact compiler from the submodule (nix)"
	@echo "  compile        — Compile all example contracts (skip ZK proofs)"
	@echo "  test           — Run all tests (Rust + codegen)"
	@echo "  test-codegen   — Run codegen tests only"
	@echo "  test-rust      — Run Rust workspace tests"
	@echo "  check          — cargo check --workspace"
	@echo "  clippy         — cargo clippy --workspace"
	@echo "  clean          — Remove compiled contract outputs"

# --- Compact compiler ---

build-compiler:
	@echo "Building Compact compiler from submodule..."
	@nix --extra-experimental-features "nix-command flakes" build ./compact#compactc --no-link --print-out-paths
	@echo "Compact compiler ready:"
	@echo "  Path: $(COMPACTC)"
	@$(COMPACTC) --version

# --- Contract compilation ---

compile: $(CONTRACT_CHECKSUMS)

# Pattern rule: compile a contract only if source changed (checksum-based)
$(COMPILED_DIR)/%/.checksum: $(EXAMPLES_DIR)/%.compact
	@mkdir -p $(COMPILED_DIR)/$*
	@current_hash=$$(shasum -a 256 $< | cut -d' ' -f1); \
	stored_hash=""; \
	if [ -f $@ ]; then stored_hash=$$(cat $@); fi; \
	if [ "$$current_hash" != "$$stored_hash" ]; then \
		echo "Compiling $*..."; \
		$(COMPACTC) --skip-zk $< $(COMPILED_DIR)/$*; \
		echo "$$current_hash" > $@; \
		echo "  done"; \
	else \
		echo "  $* is up to date"; \
	fi

# --- Testing ---

test: compile test-rust

test-rust:
	cargo test --workspace

test-codegen: compile
	cargo test -p compact-codegen

check:
	cargo check --workspace

clippy:
	cargo clippy --workspace -- -D warnings

# --- Cleanup ---

clean:
	rm -rf $(COMPILED_DIR)
