<p align="center">
  <img src="assets/logo.png" alt="OurDAO logo" width="96" />
</p>

# OurDAO — Stellar Soroban Lending DAO

[![CI](https://github.com/ourdao/ourdao-contracts/actions/workflows/ci.yml/badge.svg)](https://github.com/ourdao/ourdao-contracts/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](./LICENSE)

A member-owned lending DAO implemented as a single [Soroban](https://developers.stellar.org/docs/build/smart-contracts) smart contract in Rust — 2,300+ lines across 13 focused modules, 31 unit tests, CI-gated on every push.

The execution model, storage layout, authorization, and value transfer are all Soroban-native. All DAO value flows through a single configurable token set at initialization (USDC, XLM via the Stellar Asset Contract, or any Stellar asset).

This repository is one of three that make up OurDAO:

| Repo | Role |
|---|---|
| **`ourdao-contracts`** (this repo) | The Soroban contract — the single source of truth for all DAO state |
| [`ourdao-backend`](https://github.com/ourdao/ourdao-backend) | Off-chain indexer + read API, since Soroban keeps no queryable history |
| [`ourdao-frontend`](https://github.com/ourdao/ourdao-frontend) | Next.js web app members actually use |

## Table of contents

- [What the DAO does](#what-the-dao-does)
- [Additional feature modules](#additional-feature-modules)
- [Architecture & design decisions](#architecture--design-decisions)
- [Layout](#layout)
- [Public interface (ABI)](#public-interface-abi)
- [Event catalog](#event-catalog)
- [Error codes](#error-codes)
- [Build & test](#build--test)
- [Deploy to testnet](#deploy-to-testnet)
- [Security notes](#security-notes)
- [Known limitations](#known-limitations)
- [Roadmap](#roadmap)
- [Contributing](#contributing)
- [License](#license)

## What the DAO does

- **Governance** — a set of admins, a basis-points consensus threshold (default 51%), and a tunable loan policy. Admins can be added/removed (the last admin can never be removed, preventing a bricked DAO), the threshold and policy can be updated, and the whole contract can be paused/unpaused in an emergency.
- **Membership** — anyone can join by paying a membership fee in the DAO token; the fee becomes their pro-rata share in the treasury. A member who exits (and has no active loan) withdraws their share, their staked balance, and any pending yield in a single transaction. Rejoining after exit is handled correctly — it doesn't double-count a member's original contribution.
- **Lending** — members request loans that go through a fixed editable draft phase, then member voting weighted by stake-boosted voting power. On reaching the consensus threshold the loan is automatically approved and the principal disbursed from the treasury — no separate "execute" step. Interest is computed from a loan-size-relative-to-treasury curve, clamped to the policy's min/max rate. Repaying a loan returns the principal to the treasury and splits the interest across all active members as claimable yield.
- **Loan defaults** — an overdue loan (past `due_time` plus a configurable grace period) can be marked defaulted by **anyone**, not just an admin — an on-chain keeper pattern. Defaulting slashes a policy-defined share of the borrower's treasury claim and frees their `has_active_loan` flag, so they can still exit the DAO with their reduced share or borrow again after the normal cooldown. This closes what was previously the DAO's most notable functional gap: prior to this, an overdue loan had zero on-chain consequence.
- **Treasury** — any member can propose a withdrawal to any destination; execution requires a higher (60%) consensus than loan approval, reflecting the larger blast radius of moving treasury funds arbitrarily.
- **Safety** — admin pause/unpause blocks all state-changing operations instantly (except loan repayments, which remain available to prevent forcing borrowers into default), and extensive view functions let any client inspect full DAO state without needing an indexer for real-time reads.

## Additional feature modules

Four Soroban-native features beyond the core lending/treasury flow, each fully implemented — not stubbed out:

| Feature | Description | Module |
|---|---|---|
| Name registry | On-chain **name registry** (name ⇄ address, 1:1) | `registry.rs` |
| Content-hash metadata | Anchor an IPFS CID / digest to a loan or treasury proposal | `docs.rs` |
| Commit-reveal voting | **Commit-reveal voting** for private treasury proposals (`sha256(support ++ salt)`, revealed later, tallied through the same code path as public votes) | `privacy.rs` |
| Staking | **Staking** for a capped voting-weight boost (1 base vote + up to 5 bonus, on a quadratic curve: the *k*-th bonus vote costs `k² × 100` staked tokens), tracked separately so staked funds are never lent out or counted as treasury | `staking.rs` |

## Architecture & design decisions

- **Single-contract design.** Every module (`admin`, `membership`, `loans`, `treasury`, `registry`, `docs`, `privacy`, `staking`) lives inside one `#[contract]` struct (`lib.rs`) rather than being split into separate deployed contracts. This keeps cross-module invariants (e.g. a member's `has_active_loan` flag, staked funds excluded from treasury balance) enforceable in normal Rust function calls instead of cross-contract calls.
- **`loan.id == proposal.id`, always.** A loan proposal that gets approved disburses exactly one loan, so the loan reuses the proposal's own id rather than drawing from a separate counter. This was a deliberate fix (see the [`Known limitations`](#known-limitations) history below) — two independent counters looked harmless but silently diverged the moment any proposal was rejected, breaking the off-chain indexer's ability to correlate a loan back to the request that created it.
- **Permissionless default-marking.** `mark_loan_defaulted` requires no admin authorization and no caller-side auth at all — it's a pure, objective, time-based state transition (like a keeper bot pattern), not a privileged action. Anyone can call it once the on-chain clock says a loan is overdue.
- **Commit-reveal privacy shares the public tally path.** Rather than duplicating vote-counting logic for private proposals, `privacy::reveal_vote` recomputes and checks the commitment, then hands off to the *same* `treasury::tally` function public votes use — one code path, no risk of the two diverging.
- **TTL-aware storage.** All persistent storage entries (members, loans, proposals, votes, stakes, documents) extend their time-to-live on every read/write (30-day bump for instance storage, 90-day for persistent), so active DAO data doesn't silently expire under Soroban's [state archival](https://developers.stellar.org/docs/learn/encyclopedia/storage/state-archival) model.
- **Stable, append-only error codes.** `error.rs`'s numeric codes are documented as part of the contract's public ABI — new failure modes get a new number rather than renumbering existing ones, so client code that matches on error codes doesn't silently break across contract upgrades (see [Known limitations](#known-limitations) — there is currently no upgrade path at all, which makes this moot today but is kept as forward-looking discipline).

## Layout

```
contracts/dao/
  src/
    lib.rs         # contract entrypoints (the public ABI) + views
    admin.rs       # init, admins, threshold, policy, pause
    membership.rs  # join, exit, claim yield, exit-share math
    loans.rs       # request -> edit -> vote -> disburse -> repay -> default -> interest
    treasury.rs    # propose -> vote -> execute (shared tally)
    registry.rs    # name registry
    docs.rs        # content-hash proposal metadata
    privacy.rs     # commit-reveal voting
    staking.rs     # voting-weight staking
    storage.rs     # typed storage keys + TTL management
    types.rs       # data model + constants
    error.rs       # contract error codes
    util.rs        # token client, auth guards, vote math
    test.rs        # full test suite (19 tests)
  scripts/
    deploy-testnet.sh   # builds, deploys AND initializes; --dry-run, guarded against non-testnet
```

## Public interface (ABI)

All entrypoints are on the `OurDao` contract (`lib.rs`). Errors are the numeric [`Error`](#error-codes) codes below.

**Lifecycle / governance**

| Method | Description |
|---|---|
| `initialize(admins, consensus_threshold, membership_fee, token, policy)` | One-time setup. |
| `add_admin(caller, admin)` / `remove_admin(caller, admin)` | Admin-only. The last admin can never be removed. |
| `set_consensus_threshold(caller, threshold)` | Admin-only, basis points. |
| `set_loan_policy(caller, policy)` | Admin-only. |
| `pause(caller)` / `unpause(caller)` | Admin-only emergency stop. |

**Membership**

| Method | Description |
|---|---|
| `register_member(member)` | Pays the membership fee, joins the DAO. |
| `exit_dao(member)` | Withdraws pro-rata share + stake + pending yield. Blocked while `has_active_loan`. |
| `claim_rewards(member)` | Withdraws accrued loan-interest yield. |

**Lending**

| Method | Description |
|---|---|
| `request_loan(borrower, amount) -> proposal_id` | Opens a loan proposal (3-day editing window). |
| `edit_loan_proposal(borrower, proposal_id, new_amount)` | Only during the editing window, only the borrower. |
| `vote_on_loan_proposal(voter, proposal_id, support)` | Auto-approves and disburses at the consensus threshold. |
| `repay_loan(borrower, loan_id)` | Collects the full outstanding balance in one transaction. |
| `repay_loan_partial(borrower, loan_id, amount)` | Collects up to `amount` of the outstanding balance. Applies to accrued interest first, then principal; the loan flips to repaid only once the balance reaches exactly zero. |
| `mark_loan_defaulted(loan_id)` | **Permissionless.** Callable once `due_time + grace_period` has elapsed. |
| `expire_loan_proposal(proposal_id)` | **Permissionless keeper call.** Persists the expired state for a proposal whose voting window has passed without reaching quorum. No-op on repeat calls. |

**Treasury**

| Method | Description |
|---|---|
| `propose_treasury_withdrawal(proposer, amount, destination, reason, private) -> proposal_id` | `private` routes it through commit-reveal instead of open voting. |
| `vote_on_treasury_proposal(voter, proposal_id, support)` | Auto-executes at 60% consensus. Rejected on a private proposal — use commit/reveal instead. |

**Staking**

| Method | Description |
|---|---|
| `stake(member, amount)` / `unstake(member, amount)` | Boosts voting weight quadratically (`1 + isqrt(stake / 100)`, capped at 6); kept separate from lendable treasury. |

**Name registry**

| Method | Description |
|---|---|
| `register_name(owner, name)` | 1:1 name ⇄ address; re-registering frees the owner's previous name. Names must be 3–32 chars, ASCII lowercase alphanumeric plus `-` and `_`, with no leading/trailing separator. |
| `resolve_name(name) -> Option<Address>` / `name_of(owner) -> Option<String>` | Views. |

**Commit-reveal voting**

| Method | Description |
|---|---|
| `commit_treasury_vote(voter, proposal_id, commitment)` | `commitment = sha256(support_byte ++ salt)`. |
| `reveal_treasury_vote(voter, proposal_id, support, salt)` | Verifies the commitment, then tallies through the shared path. |

**Content-hash documents**

| Method | Description |
|---|---|
| `attach_document(caller, kind, proposal_id, content_hash)` | Anchors an off-chain content hash (e.g. IPFS CID) to a real loan/treasury proposal. |
| `get_document(kind, proposal_id) -> Option<Bytes>` | View. |

**Views** — `get_member`, `get_loan`, `get_loan_proposal` (auto-refreshes expired phase), `get_treasury_proposal`, `get_loan_policy`, `get_admins`, `is_admin`, `is_member`, `is_eligible_for_loan`, `get_treasury_balance`, `get_total_members`, `get_active_members`, `get_consensus_threshold`, `get_token`, `is_paused`, `get_stake`, `get_pending_yield`, `calculate_loan_terms`, `calculate_exit_share`, `has_voted(kind, proposal_id, voter)`.

## Event catalog

Every state-changing call publishes an event, which `ourdao-backend` indexes since Soroban itself keeps no queryable history. Topic symbol → data tuple:

| Symbol | Data | Emitted by |
|---|---|---|
| `init` | `(admins, threshold, fee, token)` | `initialize` |
| `admin_add` / `admin_rem` | `admin` | `add_admin` / `remove_admin` |
| `threshold` | `threshold` | `set_consensus_threshold` |
| `policy` / `paused` / `unpaused` | — | `set_loan_policy` / `pause` / `unpause` |
| `joined` | `(member, fee)` | `register_member` |
| `exited` | `(member, share)` | `exit_dao` |
| `claimed` | `(member, pending)` | `claim_rewards` |
| `loan_req` | `(id, borrower, amount, total_repayment)` | `request_loan` |
| `loan_edit` | `(proposal_id, borrower, new_amount, total_repayment)` | `edit_loan_proposal` |
| `loan_vote` | `(proposal_id, voter, support)` | `vote_on_loan_proposal` |
| `loan_appr` | `(id, borrower, amount)` | auto-fired on approval — `id` is both the loan's and its proposal's id |
| `loan_rpy` | `(loan_id, borrower, outstanding)` | `repay_loan` / `repay_loan_partial` — `outstanding` is the balance still remaining *after* this payment (0 for a payment that fully repays the loan) |
| `loan_dflt` | `(loan_id, borrower, penalty)` | `mark_loan_defaulted` |
| `loan_exp` | `(proposal_id, borrower)` | `expire_loan_proposal` (permissionless keeper) |
| `interest` | `(interest, active_members)` | fired alongside `loan_rpy` |
| `tre_prop` | `(id, amount, destination, private)` | `propose_treasury_withdrawal` |
| `tre_vote` | `(id, voter, support)` | `vote_on_treasury_proposal` |
| `tre_exec` | `(id, amount, destination)` | auto-fired on execution |
| `staked` / `unstaked` | `(member, amount, new_stake)` | `stake` / `unstake` |
| `name_reg` | `(name, owner)` | `register_name` |
| `committed` | `(proposal_id, voter)` | `commit_treasury_vote` |
| `revealed` | `(proposal_id, voter, support)` | `reveal_treasury_vote` |
| `doc_attn` | `(kind, proposal_id, caller)` | `attach_document` |

## Error codes

Numeric codes are stable and part of the ABI — new variants get appended rather than renumbering existing ones.

| Range | Meaning |
|---|---|
| 1–7 | Lifecycle/config: `AlreadyInitialized`, `NotInitialized`, `InvalidThreshold`, `InvalidAmount`, `InvalidLoanPolicy`, `Paused`, `NotPaused` |
| 10–15 | Authorization: `NotAuthorized`, `NotAdmin`, `NotMember`, `AlreadyAdmin`, `AlreadyMember`, `CannotRemoveLastAdmin` |
| 20–21 | Membership: `MemberNotActive`, `HasActiveLoan` |
| 30–42 | Loans: `ProposalNotFound`, `NotBorrower`, `NotInEditingPhase`, `NotInVotingPhase`, `VotingEnded`, `AlreadyVoted`, `NotEligibleForLoan`, `CooldownActive`, `LoanNotFound`, `LoanNotActive`, `ExceedsTreasuryRatio`, `InsufficientTreasury`, `LoanNotOverdue` |
| 50 | Treasury: `TreasuryProposalNotFound` |
| 60–67 | Native-swap modules: `NameTaken`, `NameNotFound`, `NoStake`, `InsufficientStake`, `NoCommitment`, `CommitmentMismatch`, `AlreadyRevealed`, `NothingToClaim` |
| 70–72 | Appended: `ProposalNotExpired`, `InvalidName`, `NotYetRevealed` |

## Build & test

Requires the Rust `wasm32v1-none` target (Rust 1.84+) and the [`stellar` CLI](https://developers.stellar.org/docs/tools/developer-tools/cli/stellar-cli).

```bash
# Native unit tests — including loan defaults, commit-reveal privacy,
# staking-boosted voting, and property tests over the exit-share, interest,
# and staking-weight math
cargo test

# Formatting check (matches CI)
cargo fmt --check

# Lint (matches CI — warnings fail the build)
cargo clippy --all-targets -- -D warnings

# Dependency advisories (matches CI, informational — see the audit job note below)
cargo install cargo-audit --locked # one-time
cargo audit

# Release wasm
cargo build --target wasm32v1-none --release

# Optimized, deployment-ready wasm
stellar contract build --optimize
```

CI (`.github/workflows/ci.yml`) runs `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`, and the wasm build in one job on every push and PR. A separate `audit` job runs `cargo audit` on the same triggers — it's split out so a newly published advisory (which can land against a pin we already know about and can't move, like the one below) surfaces visibly without blocking merges on unrelated PRs.

> **Note on dependencies:** `Cargo.lock` pins `ed25519-dalek` to `2.2.0`. A newer
> transitive release (`3.0.0`) is incompatible with the pinned `rand_core` used by
> `soroban-env-host` and breaks the test build if allowed to float. The lockfile is
> committed to keep builds reproducible.

## Deploy to testnet

The quickest path is the helper script. It creates and funds a deployer
identity (via friendbot), builds the optimized wasm, prints the wasm SHA-256,
asks for confirmation, deploys, and then runs `initialize` in the same
invocation so a contract is never left deployed-but-uninitialized (which would
let anyone front-run `initialize`):

```bash
./scripts/deploy-testnet.sh --dry-run   # print every command, change nothing
./scripts/deploy-testnet.sh             # the real thing (asks for confirmation)
```

By default the DAO uses the native XLM Stellar Asset Contract, with the deployer
as the only admin. Override the defaults with environment variables:

```bash
IDENTITY=my-key ALIAS=ourdao-dao TOKEN=<token-contract-id> \
  ADMINS_JSON='["G...","G..."]' CONSENSUS_THRESHOLD=5100 MEMBERSHIP_FEE=10000000 \
  ./scripts/deploy-testnet.sh
```

`POLICY_JSON` overrides the default loan policy; see `./scripts/deploy-testnet.sh --help`
for every option. If `initialize` fails after the deploy, the script says so
loudly and exits non-zero: initialize that contract immediately or discard it.

### Deploying to mainnet

The script refuses any network other than `testnet` unless you pass
`--allow-non-testnet` (an exported `NETWORK=mainnet` alone does nothing). For
another network it is deliberately stricter:

- it never generates or funds keys: the deployer identity must already exist,
- `TOKEN` must be set explicitly (there is no default asset),
- it needs an interactive terminal and you must type the network name to
  confirm; `--yes` does not skip this.

```bash
./scripts/deploy-testnet.sh --dry-run --allow-non-testnet --network mainnet   # review first
TOKEN=<usdc-contract-id> IDENTITY=<existing-key> \
  ./scripts/deploy-testnet.sh --allow-non-testnet --network mainnet
```

The contract cannot be upgraded after deployment, so review the dry run, check
the printed wasm SHA-256 against a release (see `DEPLOYMENTS.md`), and get the
audit items in `docs/AUDIT_PREPARATION.md` closed before deploying real funds.

Or deploy manually:

```bash
stellar contract deploy \
  --wasm target/wasm32v1-none/release/ourdao_dao.optimized.wasm \
  --network testnet \
  --source <your-identity> \
  --alias ourdao-dao
```

After deploying, initialize the DAO with your admin set, consensus threshold
(bps), membership fee, DAO token contract id, and loan policy. The script
prints a filled-in example; the token id can be the testnet USDC contract or
the native XLM Stellar Asset Contract (`stellar contract id asset --asset native --network testnet`).

## Security notes

Found a vulnerability? See [SECURITY.md](./SECURITY.md) for scope and how to report it privately.

- **No reentrancy surface.** Soroban's execution model has no arbitrary external calls back into the contract mid-execution from an untrusted token, but all balance-changing operations still follow check-effects-interactions ordering (state updated before/alongside the token transfer, not after).
- **Auth is enforced per-call, not assumed.** Every state-changing entrypoint that moves a specific member's funds or represents their vote calls `require_auth()` on that member's own address — a caller cannot act on behalf of another address.
- **`mark_loan_defaulted` is intentionally unauthenticated.** This is a deliberate design choice, not an oversight: the action is purely a function of on-chain time and existing loan state, so there is nothing to authorize — restricting it to admins would just add unnecessary liveness risk (an admin going offline shouldn't block defaults from being recorded).
- **This contract has not been externally audited.** It is a testnet-stage prototype. Do not deploy to mainnet with real value without an independent security review first.

## Known limitations

- **No upgrade path.** The contract is immutable once deployed — a deliberate trust-minimization tradeoff, not an oversight. Migrating to a new version requires a fresh deployment; see [`docs/MIGRATION.md`](./docs/MIGRATION.md) for the procedure, what state can and can't be carried over, and the trust assumption a migration necessarily introduces.
- **Testnet-only deploy tooling**, consistent with this project's current testnet-stage positioning.
- ~~Loan defaults had zero on-chain consequence~~ — fixed; see [What the DAO does](#what-the-dao-does).
- ~~`loan.id` could silently diverge from its originating `proposal.id`~~ — fixed by removing the separate loan-id counter; a loan now always reuses its proposal's id, with a regression test locking in the invariant.
- ~~Interest distribution was O(n) over every member~~ — fixed with a pull-based yield accumulator: O(1) on repay, each member settles lazily on claim/exit.

## Roadmap

- External security audit before any mainnet consideration.
- Implement the `seed_state` migration entrypoint designed in [`docs/MIGRATION.md`](./docs/MIGRATION.md) (the doc itself is done; the contract-side entrypoint is a follow-up).
- Deeper integration testing against `ourdao-backend`'s indexer (event schema drift detection).

## Contributing

Contributions are welcome — see [CONTRIBUTING.md](./CONTRIBUTING.md) for local setup, the checks CI enforces, and the contract-specific rules (append-only error codes, events on every state change, TTL discipline). Please claim an issue before opening a pull request.

Found a security vulnerability? See [SECURITY.md](./SECURITY.md) — don't open a public issue.

## License

MIT

## Resource Costs and Membership Ceiling

As measured via SDK budget instrumentation (Issue #138):
- `exit_dao` has O(n) scaling due to iterating all members in `total_active_contributions`.
- `is_admin` scales with admin count.
- Other entrypoints are O(1) or scale with smaller sets (like active proposals).

**Membership Ceiling:** Due to the O(n) scan in `exit_dao`, the contract can safely support up to **~10,000 members** before risking transaction resource limit exhaustion on exit. Optimizations (e.g. tracking total contributions natively) are required to scale further.
