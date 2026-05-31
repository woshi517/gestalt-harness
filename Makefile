# Makefile for automating gestalt-harness development workflow

.PHONY: all check fmt fmt-check clippy test audit clean

all: fmt-check check clippy test audit

check:
	cargo check --workspace

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all --check

clippy:
	cargo clippy --workspace --all-targets -- -D warnings

test:
	cargo test --workspace

audit:
	bash scripts/check-deps.sh

clean:
	cargo clean
