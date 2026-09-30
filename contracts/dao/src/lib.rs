#![no_std]
//! OurDAO — a member-owned lending DAO for Stellar Soroban.
//!
//! All value moves through a single configurable token set at initialization
//! (USDC, XLM via the Stellar Asset Contract, or any Stellar asset). Four
//! additional Soroban-native modules extend the core lending/treasury flow:
//!
//! * [`registry`] name registry
//! * [`docs`] content-hash proposal metadata
//! * [`privacy`] commit-reveal voting
//! * [`staking`] voting-weight staking

mod admin;
mod docs;
mod error;
mod loans;
mod membership;
mod privacy;
mod registry;
mod staking;
mod storage;
mod treasury;
mod types;
mod util;

#[cfg(test)]
mod tests;

use soroban_sdk::{contract, contractimpl, Address, Bytes, BytesN, Env, String, Vec};

pub use error::Error;
pub use storage::ProposalKind;
pub use types::{
    Loan, LoanPolicy, LoanProposal, LoanTerms, Member, MemberLoanStats, PendingPolicyUpdate,
    StakingRewardClaimed, TreasuryProposal,
};

#[contract]
pub struct OurDao;

#[contractimpl]
impl OurDao {
    // ==================== lifecycle / governance ====================

    /// One-time setup. `admins` bootstrap governance, `consensus_threshold` is
    /// in basis points (e.g. 5100 = 51%), and `token` is the asset all DAO
    /// value flows through.
    pub fn initialize(
        env: Env,
        admins: Vec<Address>,
        consensus_threshold: u32,
        membership_fee: i128,
        token: Address,
        policy: LoanPolicy,
    ) -> Result<(), Error> {
        admin::initialize(
            &env,
            admins,
            consensus_threshold,
            membership_fee,
            token,
            policy,
        )
    }

    pub fn add_admin(env: Env, caller: Address, admin: Address) -> Result<(), Error> {
        admin::add_admin(&env, caller, admin)
    }

    pub fn remove_admin(env: Env, caller: Address, admin: Address) -> Result<(), Error> {
        admin::remove_admin(&env, caller, admin)
    }

    pub fn set_consensus_threshold(env: Env, caller: Address, threshold: u32) -> Result<(), Error> {
        admin::set_consensus_threshold(&env, caller, threshold)
    }

    pub fn propose_policy_update(
        env: Env,
        caller: Address,
        policy: LoanPolicy,
    ) -> Result<(), Error> {
        admin::propose_policy_update(&env, caller, policy)
    }

    pub fn execute_policy_update(env: Env, caller: Address) -> Result<(), Error> {
        admin::execute_policy_update(&env, caller)
    }

    pub fn cancel_policy_update(env: Env, caller: Address) -> Result<(), Error> {
        admin::cancel_policy_update(&env, caller)
    }

    pub fn set_loan_policy(env: Env, caller: Address, policy: LoanPolicy) -> Result<(), Error> {
        admin::set_policy(&env, caller, policy)
    }

    pub fn pause(env: Env, caller: Address) -> Result<(), Error> {
        admin::pause(&env, caller)
    }

    pub fn unpause(env: Env, caller: Address) -> Result<(), Error> {
        admin::unpause(&env, caller)
    }

    // ==================== membership ====================

    pub fn register_member(env: Env, member: Address) -> Result<(), Error> {
        membership::register_member(&env, member)
    }

    pub fn exit_dao(env: Env, member: Address) -> Result<(), Error> {
        membership::exit_dao(&env, member)
    }

    /// Withdraw accrued loan-interest yield for the caller.
    pub fn claim_rewards(env: Env, member: Address) -> Result<i128, Error> {
        membership::claim_rewards(&env, member)
    }

    pub fn delegate_vote(env: Env, delegator: Address, delegatee: Address) -> Result<(), Error> {
        membership::delegate_vote(&env, delegator, delegatee)
    }

    // ==================== loans ====================

    pub fn request_loan(
        env: Env,
        borrower: Address,
        amount: i128,
        metadata_cid: Option<String>,
    ) -> Result<u32, Error> {
        loans::request_loan(&env, borrower, amount, metadata_cid)
    }

    pub fn edit_loan_proposal(
        env: Env,
        borrower: Address,
        proposal_id: u32,
        new_amount: i128,
    ) -> Result<(), Error> {
        loans::edit_loan_proposal(&env, borrower, proposal_id, new_amount)
    }

    pub fn vote_on_loan_proposal(
        env: Env,
        voter: Address,
        proposal_id: u32,
        support: bool,
    ) -> Result<(), Error> {
        loans::vote_on_loan_proposal(&env, voter, proposal_id, support)
    }

    pub fn vote_batch(
        env: Env,
        voter: Address,
        votes: Vec<crate::types::ProposalVote>,
    ) -> Result<(), Error> {
        for v in votes.iter() {
            loans::vote_on_loan_proposal(&env, voter.clone(), v.proposal_id, v.vote)?;
        }
        Ok(())
    }

    pub fn disburse_approved_loan(env: Env, proposal_id: u32) -> Result<(), Error> {
        loans::disburse_approved_loan(&env, proposal_id)
    }

    pub fn repay_loan(env: Env, borrower: Address, loan_id: u32) -> Result<(), Error> {
        loans::repay_loan(&env, borrower, loan_id)
    }

    /// Repays up to `amount` of a loan's outstanding balance. See
    /// `loans::repay_loan_partial` for the interest/principal split and why
    /// this is a separate entrypoint from `repay_loan`.
    pub fn repay_loan_partial(
        env: Env,
        borrower: Address,
        loan_id: u32,
        amount: i128,
    ) -> Result<(), Error> {
        loans::repay_loan_partial(&env, borrower, loan_id, amount)
    }

    /// Marks an overdue loan as defaulted. Permissionless — see `loans::mark_loan_defaulted`.
    pub fn mark_loan_defaulted(env: Env, loan_id: u32) -> Result<(), Error> {
        loans::mark_loan_defaulted(&env, loan_id)
    }

    /// Permissionless keeper call: persists the expired/rejected transition for
    /// a loan proposal whose voting window has passed without reaching quorum.
    pub fn expire_loan_proposal(env: Env, proposal_id: u32) -> Result<(), Error> {
        loans::expire_loan_proposal(&env, proposal_id)
    }

    // ==================== treasury ====================

    pub fn propose_treasury_withdrawal(
        env: Env,
        proposer: Address,
        amount: i128,
        destination: Address,
        reason: String,
        private: bool,
    ) -> Result<u32, Error> {
        treasury::propose_withdrawal(&env, proposer, amount, destination, reason, private)
    }

    pub fn vote_on_treasury_proposal(
        env: Env,
        voter: Address,
        proposal_id: u32,
        support: bool,
    ) -> Result<(), Error> {
        treasury::vote(&env, voter, proposal_id, support)
    }

    pub fn expire_treasury_proposal(env: Env, proposal_id: u32) -> Result<(), Error> {
        treasury::expire_treasury_proposal(&env, proposal_id)
    }

    pub fn execute_treasury_proposal(env: Env, proposal_id: u32) -> Result<(), Error> {
        treasury::execute_approved(&env, proposal_id)
    }

    // ==================== maintenance ====================

    #[allow(deprecated)]
    pub fn bump_dao_ttl(env: Env) {
        storage::extend_instance(&env);
        env.events().publish((soroban_sdk::symbol_short!("TtlBumped"),), ());
    }

    // ==================== native swap: staking ====================

    pub fn stake(env: Env, member: Address, amount: i128) -> Result<(), Error> {
        staking::stake(&env, member, amount)
    }

    pub fn unstake(env: Env, member: Address, amount: i128) -> Result<(), Error> {
        staking::unstake(&env, member, amount)
    }

    // ==================== native swap: name registry ====================

    pub fn register_name(env: Env, owner: Address, name: String) -> Result<(), Error> {
        registry::register_name(&env, owner, name)
    }

    pub fn resolve_name(env: Env, name: String) -> Option<Address> {
        registry::resolve_name(&env, name)
    }

    pub fn name_of(env: Env, owner: Address) -> Option<String> {
        registry::name_of(&env, owner)
    }

    // ==================== native swap: commit-reveal voting ====================

    pub fn commit_treasury_vote(
        env: Env,
        voter: Address,
        proposal_id: u32,
        commitment: BytesN<32>,
    ) -> Result<(), Error> {
        privacy::commit_vote(&env, voter, proposal_id, commitment)
    }

    pub fn reveal_treasury_vote(
        env: Env,
        voter: Address,
        proposal_id: u32,
        support: bool,
        salt: BytesN<32>,
    ) -> Result<(), Error> {
        privacy::reveal_vote(&env, voter, proposal_id, support, salt)
    }

    // ==================== native swap: content-hash docs ====================

    pub fn attach_document(
        env: Env,
        caller: Address,
        kind: ProposalKind,
        proposal_id: u32,
        content_hash: Bytes,
    ) -> Result<(), Error> {
        docs::attach_document(&env, caller, kind, proposal_id, content_hash)
    }

    pub fn get_document(env: Env, kind: ProposalKind, proposal_id: u32) -> Option<Bytes> {
        docs::get_document(&env, kind, proposal_id)
    }

    // ==================== views ====================

    pub fn get_member(env: Env, address: Address) -> Option<Member> {
        storage::get_member(&env, &address)
    }

    /// Returns `member`'s lifetime loan track record: total loans taken, loans
    /// fully repaid, and loans currently outstanding. Single O(1) storage read;
    /// returns `NotMember` if the address has never registered.
    pub fn get_member_loan_stats(env: Env, member: Address) -> Result<MemberLoanStats, Error> {
        storage::get_member(&env, &member)
            .map(|m| MemberLoanStats {
                total_loans: m.total_loans,
                repaid_loans: m.repaid_loans,
                active_loans: m.active_loans,
            })
            .ok_or(Error::NotMember)
    }

    pub fn get_loan(env: Env, loan_id: u32) -> Option<Loan> {
        storage::get_loan(&env, loan_id)
    }

    pub fn get_loan_proposal(env: Env, proposal_id: u32) -> Option<LoanProposal> {
        storage::get_loan_proposal(&env, proposal_id).map(|p| loans::refresh_phase(&env, p))
    }

    pub fn get_treasury_proposal(env: Env, proposal_id: u32) -> Option<TreasuryProposal> {
        storage::get_treasury_proposal(&env, proposal_id)
    }

    pub fn get_loan_proposal_count(env: Env) -> u32 {
        storage::get_proposal_count(&env, storage::DataKey::NextProposalId)
    }

    pub fn get_treasury_proposal_count(env: Env) -> u32 {
        storage::get_proposal_count(&env, storage::DataKey::NextTreasuryId)
    }

    pub fn get_loan_policy(env: Env) -> LoanPolicy {
        storage::get_policy(&env)
    }

    pub fn get_pending_policy_update(env: Env) -> Option<PendingPolicyUpdate> {
        admin::get_pending_policy_update(&env)
    }

    pub fn get_admins(env: Env) -> Vec<Address> {
        storage::get_admins(&env)
    }

    pub fn is_admin(env: Env, address: Address) -> bool {
        util::is_admin(&env, &address)
    }

    pub fn is_member(env: Env, address: Address) -> bool {
        matches!(
            storage::get_member(&env, &address),
            Some(m) if m.status == types::MemberStatus::ActiveMember
        )
    }

    pub fn is_eligible_for_loan(env: Env, member: Address) -> Result<(), Error> {
        loans::is_eligible_for_loan(&env, &member)
    }

    pub fn get_treasury_balance(env: Env) -> i128 {
        util::treasury_balance(&env)
    }

    pub fn get_total_members(env: Env) -> u32 {
        storage::get_total_members(&env)
    }

    pub fn get_active_members(env: Env) -> u32 {
        storage::get_active_members(&env)
    }

    pub fn get_consensus_threshold(env: Env) -> u32 {
        storage::get_threshold(&env)
    }

    pub fn get_token(env: Env) -> Address {
        storage::get_token(&env)
    }

    pub fn is_paused(env: Env) -> bool {
        storage::is_paused(&env)
    }

    /// Returns this contract's semver, read from CARGO_PKG_VERSION at build
    /// time (#197) — lets off-chain tooling / indexers detect which
    /// contract build a given deployment is running without relying on the
    /// WASM hash alone.
    pub fn get_version(env: Env) -> String {
        String::from_str(&env, env!("CARGO_PKG_VERSION"))
    }

    pub fn get_stake(env: Env, member: Address) -> i128 {
        storage::get_stake(&env, &member)
    }

    /// Returns `member`'s current voting weight (base vote + staking bonus).
    /// Equivalent to the weight applied when they cast a loan or treasury vote.
    pub fn get_voting_weight(env: Env, member: Address) -> i128 {
        util::get_voting_weight(&env, &member)
    }

    /// Stake tokens required per additional unit of voting bonus.
    /// Every `stake_weight_unit` tokens staked grants +1 vote, up to the cap.
    pub fn get_stake_weight_unit(_env: Env) -> i128 {
        util::get_stake_weight_unit()
    }

    /// Maximum additional votes a member can earn through staking.
    pub fn get_max_stake_bonus(_env: Env) -> i128 {
        util::get_max_stake_bonus()
    }

    pub fn get_pending_yield(env: Env, member: Address) -> i128 {
        let acc = storage::get_yield_accumulator(&env);
        let snap = storage::get_yield_snapshot(&env, &member);
        (acc - snap).max(0)
    }

    /// Returns whether `voter` has cast a (possibly not-yet-revealed) vote
    /// on the given proposal. For public (non-private) proposals this is a
    /// simple check against the vote record. For commit-reveal treasury
    /// proposals it checks whether the voter has committed (which is the
    /// earliest point at which a double-vote is prevented).
    pub fn has_voted(env: Env, kind: ProposalKind, proposal_id: u32, voter: Address) -> bool {
        match kind {
            ProposalKind::Loan => storage::has_loan_voted(&env, proposal_id, &voter),
            ProposalKind::Treasury => {
                // For private (commit-reveal) proposals, a commitment is
                // sufficient to report "voted" — the double-vote guard is
                // enforced at commit time, and the client needs to know the
                // member can't vote again regardless of reveal status.
                storage::has_treasury_voted(&env, proposal_id, &voter)
                    || storage::get_commit(&env, proposal_id, &voter).is_some()
            }
        }
    }

    pub fn calculate_loan_terms(env: Env, amount: i128) -> LoanTerms {
        loans::calculate_loan_terms(&env, amount)
    }

    pub fn calculate_exit_share(env: Env, member: Address) -> i128 {
        membership::calculate_exit_share(&env, &member)
    }
}
