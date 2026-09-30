WASM_TARGET := wasm32v1-none
WASM := target/$(WASM_TARGET)/release/ourdao_dao.wasm

.PHONY: all build test fmt clippy audit ci clean optimize

all: fmt clippy test build

build:
	cargo build --locked --target $(WASM_TARGET) --release

# Small, deterministic release wasm ready for deployment.
optimize: build
	stellar contract build --optimize

test:
	cargo test --locked

fmt:
	cargo fmt --all -- --check

clippy:
	cargo clippy --all-targets --locked -- -D warnings

# Reproduces the full CI sequence locally (minus coverage and supply-chain jobs).
# Use this before pushing to confirm CI will pass.
ci: fmt clippy test build

# Requires `cargo install cargo-audit --locked` once.
audit:
	cargo audit

clean:
	cargo clean
