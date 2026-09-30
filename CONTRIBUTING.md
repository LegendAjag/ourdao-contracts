# Contributing to `ourdao-contracts`

Thanks for your interest in contributing. This repo holds the Soroban smart contract that is the single source of truth for all OurDAO state — so the bar for changes here is deliberately higher than for the other two repos. Contract bugs move real money.

Please read this in full before opening a pull request.

## Table of contents

- [Before you write code](#before-you-write-code)
- [Local setup](#local-setup)
- [Running the checks CI runs](#running-the-checks-ci-runs)
- [What a good pull request looks like](#what-a-good-pull-request-looks-like)
- [Contract-specific rules](#contract-specific-rules)
- [What gets closed without review](#what-gets-closed-without-review)
- [Reporting a security issue](#reporting-a-security-issue)
- [License](#license)

## Before you write code

**Claim the issue first.** Comment on the issue you want to work on and wait to be assigned before opening a pull request. This is not bureaucracy — it prevents two people doing the same work, and it gives us a chance to flag context that isn't in the issue text.

Pull requests that arrive without an assigned issue will be closed with a pointer back here. The one exception is a genuine security fix, which should follow [Reporting a security issue](#reporting-a-security-issue) instead.

If you think something should change but there's no issue for it, open one and describe the problem before writing the fix. A PR is a proposed solution; the issue is where we agree on the problem.

## Local setup

The repository pins its compiler via `rust-toolchain.toml` (currently **1.94.0**). `rustup` picks this up automatically — just run the commands below and you'll get the right toolchain, formatter, clippy, and the `wasm32v1-none` target without installing anything extra.

```bash
# rustup reads rust-toolchain.toml and installs the pinned toolchain.
rustup show
```

The minimum supported version is **1.84** (required by `soroban-sdk` for the `wasm32v1-none` target). The pinned version in `rust-toolchain.toml` is always ≥ that floor.

To bump the toolchain, update the `channel` in `rust-toolchain.toml`, verify all CI checks pass, and open a dedicated PR. See the comments in that file for the full procedure.

Then:

```bash
git clone https://github.com/ourdao/ourdao-contracts
cd ourdao-contracts
cargo test
```

If `cargo test` passes on a clean checkout, you're set up correctly. If it doesn't, that's a bug — please open an issue rather than working around it.

## Running the checks CI runs

The `build-and-test` job runs exactly these four, and a pull request that fails any of them will not be merged. Run them locally first:

```bash
cargo fmt --all -- --check                                  # formatting
cargo clippy --all-targets --locked -- -D warnings           # lint
cargo test --locked                                          # full test suite
cargo build --locked --target wasm32v1-none --release        # wasm build
```

`make ci` runs all four in sequence — use it as your one-command pre-push check. The individual shorthands `make fmt`, `make clippy`, `make test`, and `make build` map flag-for-flag to their CI equivalents.

A separate `audit` job runs `cargo audit` against `Cargo.lock` on every push and PR (`cargo install cargo-audit --locked` once locally, then `cargo audit` or `make audit`). It's a distinct job on purpose: an advisory landing against a dependency we already know about and can't move on (see the `ed25519-dalek` note below) shouldn't block merging unrelated PRs, but it still needs to stay visible.

If Clippy flags something you believe is wrong for this codebase, `#[allow(...)]` it at the narrowest possible scope — a single item or expression, never crate- or module-level — with a one-line comment explaining why.

Note the `--locked` flag: the committed `Cargo.lock` is authoritative. Don't update dependencies as a side effect of an unrelated change — if a dependency bump is genuinely needed, it belongs in its own pull request with its own justification.

## What a good pull request looks like

- **It's scoped to one issue.** If you find a second problem while working, open a second issue. Don't bundle.
- **It includes a test that would fail without your change.** For a bug fix, that means a regression test that reproduces the bug. For a new behavior, that means a test exercising it. "The existing tests still pass" is not sufficient — that's the floor, not the bar.
- **It doesn't reformat code you didn't change.** Whitespace-only churn makes review harder and hides the real diff.
- **Its description explains why, not just what.** The diff already shows what changed. Review needs to know why this is the right change.
- **CI is green.** Check before requesting review, not after.

## Contract-specific rules

These apply on top of the general rules above, because this is a smart contract:

- **Error codes are append-only.** `error.rs`'s numeric codes are part of the contract's public ABI. A new failure mode gets a new number. Never renumber, reuse, or reorder existing codes — client code matches on those numbers.
- **New state-changing entrypoints must emit an event.** The off-chain indexer ([`ourdao-backend`](https://github.com/ourdao/ourdao-backend)) reconstructs all queryable history from emitted events. A state change with no event is invisible to every client. If you add an event, say so in the PR description so the indexer can be updated in lockstep.
- **Respect the storage TTL discipline.** Persistent storage entries extend their time-to-live on read and write. New storage reads/writes must follow the same pattern, or that data will silently expire under Soroban's state archival.
- **Don't add a second id counter.** A loan reuses its originating proposal's id (`loan.id == proposal.id`) on purpose — independent counters silently diverge as soon as any proposal is rejected. This invariant is covered by a regression test; if your change touches it, that test should tell you.
- **Authorization changes need explicit justification.** If your PR adds, removes, or relaxes a `require_auth` call, the description must say exactly who can now do what they couldn't before, and why that's correct.

## Releases and Verification

Releases are published automatically when a tag is pushed (e.g., `v1.0.0`). Each release includes:

- A built, optimized wasm artifact (`ourdao_dao.optimized.wasm`)
- A SHA-256 checksum in the release notes
- Build metadata (commit, target, command)

**Member verification:**

To verify a deployed contract matches a GitHub release:

```bash
# 1. Download the wasm from the release or build it:
git checkout <TAG>
stellar contract build --optimize

# 2. Compute its SHA-256:
sha256sum target/wasm32v1-none/release/ourdao_dao.optimized.wasm

# 3. Compare against the checksum in the release notes
```

Deployed contract IDs for each network are documented in [DEPLOYMENTS.md](./DEPLOYMENTS.md).

## What gets closed without review

To keep review time going to real contributions, the following are closed on sight:

- Pull requests against an unassigned or unclaimed issue.
- Formatting-only, whitespace-only, or comment-typo-only changes.
- Unrelated dependency bumps bundled into a feature or fix.
- Generated or AI-authored changes whose author can't explain the diff when asked in review. The policy is outcome-based, not tool-based — use whatever tools you like, but you're accountable for understanding and defending what you submit.
- Changes with no accompanying test, where the change affects contract behavior.

## Reporting a security issue

**Do not open a public issue for a security vulnerability.** See [SECURITY.md](./SECURITY.md) for scope, how to report privately, and our response commitment.

## License

By contributing, you agree that your contributions will be licensed under the [MIT License](./LICENSE) that covers this project.
