# Changelog

All notable changes to `ourdao-contracts` are documented here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project aims to follow [Semantic Versioning](https://semver.org/). The workspace version in `Cargo.toml` has been `0.1.0` since the first commit; entries below are grouped under **Unreleased** until a version is cut, at which point the version in `Cargo.toml` should be bumped alongside a dated heading here.

## [Unreleased]

### Added
- `repay_loan_partial` for partial loan repayment.
- Loan and proposal lifecycle expiry, a `has_voted` view, name validation and an O(1) pull-based yield accumulator.
- `expire_treasury_proposal`, exposed in `lib.rs`; treasury proposals now expire after `VOTING_PERIOD`.
- Commit-reveal phase guards for private proposals (#57).
- `NotEditingPhase` and `DocumentTooLarge` errors; document hashes are limited to 64 bytes (#58, #59).
- `YieldRemainder` storage: `distribute_interest` carries rounding remainders and emits an event (#60).
- Loan terms are re-quoted at disbursement (#61).
- `bench-cost.sh` and a budget scaling benchmark; resource costs and the membership ceiling documented (#138).
- `generate-abi.sh` for a wasm-derived interface (#135).
- Property tests for exit-share, interest and staking-weight math.
- `util::isqrt`: an overflow-free integer square root over the whole `i128` range, backing the quadratic staking curve (#182).
- CI: `cargo clippy`, `cargo audit` and a `cargo-llvm-cov` coverage floor of 95% (#62).

### Changed
- `Member.join_ledger` renamed to `join_time` (#55).
- Staking boost is quadratic instead of linear: `voting_weight` is now `1 + min(isqrt(stake / 100), 5)`, so the *k*-th bonus vote costs `k² × 100` staked tokens and a whale can no longer buy proportional control (#182).
- Admin policy bounds tightened (#53, #54); loan eligibility checks and the `is_eligible_for_loan` signature corrected (#52, #56).
- Proposal document attachment is restricted to the proposal's own owner, and rejected after the editing phase (#59).

### Fixed
- `total_members` is now decremented on exit, so exit shares no longer underpay (#11).
- Stake locking and `due_time` on loan approval (#46, #51); reveal period end (#45).
- CI workflow and formatting violations.

### Documentation
- `SECURITY.md` with a coordinated disclosure policy.
- `docs/MIGRATION.md`, `docs/INVARIANTS.md`, `docs/LOAN_POLICY_GUIDE.md` and the pause/unpause incident runbook.
- `docs/THREAT_MODEL.md`: actors, trust assumptions and known gaps (#133).
