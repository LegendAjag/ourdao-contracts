use soroban_sdk::{symbol_short, Address, Env, String};

use crate::error::Error;
use crate::storage;
use crate::types::{
    Loan, LoanProposal, LoanStatus, LoanTerms, LoanTermsEdited, MemberStatus, ProposalPhase,
    ProposalStatus, BASIS_POINTS,
};
use crate::util;

/// Quote the terms a loan of `amount` would carry right now. Interest scales
/// linearly with the loan's size relative to the treasury, clamped to policy.
pub fn calculate_loan_terms(env: &Env, amount: i128) -> LoanTerms {
    let policy = storage::get_policy(env);
    let treasury = util::treasury_balance(env);

    let loan_ratio = if treasury > 0 {
        (amount * BASIS_POINTS / treasury).min(BASIS_POINTS)
    } else {
        BASIS_POINTS
    };
    let spread = (policy.max_interest_rate - policy.min_interest_rate) as i128;
    let mut rate = policy.min_interest_rate as i128 + (loan_ratio * spread / BASIS_POINTS);
    if rate > policy.max_interest_rate as i128 {
        rate = policy.max_interest_rate as i128;
    }
    let total_repayment = amount + (amount * rate / BASIS_POINTS);
    LoanTerms {
        interest_rate: rate as u32,
        total_repayment,
        duration: policy.max_loan_duration,
    }
}

pub fn is_eligible_for_loan(env: &Env, member: &Address) -> Result<(), Error> {
    let record = match storage::get_member(env, member) {
        Some(m) if m.status == MemberStatus::ActiveMember => m,
        _ => return Err(Error::MemberNotActive),
    };
    if record.has_active_loan {
        return Err(Error::HasActiveLoan);
    }
    let policy = storage::get_policy(env);
    let now = env.ledger().timestamp();
    if now.saturating_sub(record.join_time) < policy.min_membership_duration {
        return Err(Error::NotEligibleForLoan);
    }
    if record.last_loan_time != 0
        && now.saturating_sub(record.last_loan_time) < policy.cooldown_period
    {
        return Err(Error::CooldownActive);
    }
    Ok(())
}

// `env.events().publish` is deprecated in soroban-sdk in favour of
// `#[contractevent]`, but migration is a coordinated, breaking wire-format
// change (#85).  Suppress per-function so unrelated deprecations still surface.
#[allow(deprecated)]
pub fn request_loan(
    env: &Env,
    borrower: Address,
    amount: i128,
    metadata_cid: Option<String>,
) -> Result<u32, Error> {
    util::require_initialized(env)?;
    util::require_not_paused(env)?;
    util::require_active_member(env, &borrower)?;

    if amount <= 0 {
        return Err(Error::InvalidAmount);
    }
    if let Some(ref cid) = metadata_cid {
        if cid.is_empty() {
            return Err(Error::InvalidMetadataCid);
        }
        if cid.len() > 64 {
            return Err(Error::DocumentTooLarge);
        }
    }
    is_eligible_for_loan(env, &borrower)?;

    let policy = storage::get_policy(env);
    let treasury = util::treasury_balance(env);
    let max_loan = treasury * policy.max_loan_to_treasury_ratio as i128 / BASIS_POINTS;
    if amount > max_loan {
        return Err(Error::ExceedsTreasuryRatio);
    }

    let terms = calculate_loan_terms(env, amount);
    let now = env.ledger().timestamp();
    let id = storage::next_id(env, storage::DataKey::NextProposalId);
    let proposal = LoanProposal {
        id,
        borrower: borrower.clone(),
        amount,
        interest_rate: terms.interest_rate,
        duration: terms.duration,
        total_repayment: terms.total_repayment,
        created_at: now,
        editing_period_end: now + policy.editing_period,
        phase: ProposalPhase::Editing,
        status: ProposalStatus::Pending,
        for_votes: 0,
        against_votes: 0,
        votes_cast: 0,
        voting_period: policy.voting_period,
        metadata_cid,
        last_edited_at: None,
    };
    storage::set_loan_proposal(env, &proposal);
    storage::extend_instance(env);

    env.events().publish(
        (symbol_short!("loan_req"),),
        (id, borrower, amount, terms.total_repayment),
    );
    Ok(id)
}

#[allow(deprecated)]
pub fn edit_loan_proposal(
    env: &Env,
    borrower: Address,
    proposal_id: u32,
    new_amount: i128,
) -> Result<(), Error> {
    util::require_initialized(env)?;
    util::require_not_paused(env)?;
    util::require_active_member(env, &borrower)?;
    let mut proposal =
        storage::get_loan_proposal(env, proposal_id).ok_or(Error::ProposalNotFound)?;
    if proposal.borrower != borrower {
        return Err(Error::NotBorrower);
    }
    let now = env.ledger().timestamp();
    if proposal.phase != ProposalPhase::Editing || now >= proposal.editing_period_end {
        return Err(Error::NotInEditingPhase);
    }
    if new_amount <= 0 {
        return Err(Error::InvalidAmount);
    }

    let prev_amount = proposal.amount;
    let prev_total_repayment = proposal.total_repayment;
    let terms = calculate_loan_terms(env, new_amount);
    proposal.amount = new_amount;
    proposal.interest_rate = terms.interest_rate;
    proposal.duration = terms.duration;
    proposal.total_repayment = terms.total_repayment;
    proposal.last_edited_at = Some(now);
    storage::set_loan_proposal(env, &proposal);

    let _structured = LoanTermsEdited {
        proposal_id,
        borrower: borrower.clone(),
        prev_amount,
        prev_total_repayment,
        new_amount,
        total_repayment: terms.total_repayment,
        edited_at: now,
    };
    env.events().publish(
        (symbol_short!("loan_edit"),),
        (proposal_id, borrower, new_amount, terms.total_repayment),
    );
    Ok(())
}

/// Advances a proposal's phase based on the clock. Returns the (possibly
/// mutated) proposal; the caller is responsible for persisting it.
pub fn refresh_phase(env: &Env, mut proposal: LoanProposal) -> LoanProposal {
    let now = env.ledger().timestamp();
    if proposal.phase == ProposalPhase::Editing && now >= proposal.editing_period_end {
        proposal.phase = ProposalPhase::Voting;
    }
    if proposal.phase == ProposalPhase::Voting
        && now > proposal.editing_period_end + proposal.voting_period
        && proposal.status == ProposalStatus::Pending
    {
        proposal.phase = ProposalPhase::Expired;
        proposal.status = ProposalStatus::Rejected;
    }
    proposal
}

#[allow(deprecated)]
pub fn vote_on_loan_proposal(
    env: &Env,
    voter: Address,
    proposal_id: u32,
    support: bool,
) -> Result<(), Error> {
    util::require_initialized(env)?;
    util::require_not_paused(env)?;
    util::require_active_member(env, &voter)?;

    let mut proposal = storage::get_loan_proposal(env, proposal_id)
        .ok_or(Error::ProposalNotFound)
        .map(|p| refresh_phase(env, p))?;

    if proposal.phase == ProposalPhase::Editing {
        return Err(Error::NotInVotingPhase);
    }
    if proposal.phase != ProposalPhase::Voting {
        return Err(Error::VotingEnded);
    }
    let now = env.ledger().timestamp();
    if now > proposal.editing_period_end + proposal.voting_period {
        return Err(Error::VotingEnded);
    }
    if storage::has_loan_voted(env, proposal_id, &voter) {
        return Err(Error::AlreadyVoted);
    }

    let weight = util::voting_weight(env, &voter);
    if support {
        proposal.for_votes += weight;
    } else {
        proposal.against_votes += weight;
    }
    proposal.votes_cast += 1;
    storage::set_loan_voted(env, proposal_id, &voter);
    env.events()
        .publish((symbol_short!("loan_vote"),), (proposal_id, voter, support));

    let policy = storage::get_policy(env);
    let threshold = if policy.quorum_bps > 0 {
        policy.quorum_bps
    } else {
        storage::get_threshold(env)
    };
    let required = util::required_votes(storage::get_active_members(env), threshold);
    if proposal.for_votes >= required && proposal.status == ProposalStatus::Pending {
        proposal.status = ProposalStatus::ApprovedPendingDisbursement;
        proposal.phase = ProposalPhase::Executed;
        storage::set_loan_proposal(env, &proposal);
        if approve_and_disburse(env, &proposal).is_ok() {
            proposal.status = ProposalStatus::Approved;
        } else {
            env.events().publish(
                (symbol_short!("loan_wait"),),
                (proposal.id, proposal.amount),
            );
        }
    } else if proposal.status == ProposalStatus::Pending {
        let remaining = storage::get_active_members(env).saturating_sub(proposal.votes_cast);
        let max_remaining = remaining as i128 * (1 + util::MAX_STAKE_BONUS);
        if proposal.for_votes + max_remaining < required {
            proposal.status = ProposalStatus::Rejected;
            proposal.phase = ProposalPhase::Expired;
            env.events().publish(
                (symbol_short!("loan_rej"),),
                (proposal.id, proposal.for_votes, proposal.against_votes),
            );
        }
    }
    storage::set_loan_proposal(env, &proposal);
    Ok(())
}

pub fn disburse_approved_loan(env: &Env, proposal_id: u32) -> Result<(), Error> {
    util::require_initialized(env)?;
    util::require_not_paused(env)?;
    let proposal = storage::get_loan_proposal(env, proposal_id).ok_or(Error::ProposalNotFound)?;
    if proposal.status != ProposalStatus::ApprovedPendingDisbursement {
        return Err(Error::NotInVotingPhase);
    }
    approve_and_disburse(env, &proposal)?;
    let mut proposal = proposal;
    proposal.status = ProposalStatus::Approved;
    storage::set_loan_proposal(env, &proposal);
    Ok(())
}

#[allow(deprecated)]
fn approve_and_disburse(env: &Env, proposal: &LoanProposal) -> Result<(), Error> {
    if util::treasury_balance(env) < proposal.amount {
        return Err(Error::InsufficientTreasury);
    }

    // Issue 61: re-quote at disbursement so rate reflects current treasury balance
    let terms = calculate_loan_terms(env, proposal.amount);

    let now = env.ledger().timestamp();
    // Reuse the proposal's own id rather than a separate counter: a proposal
    // produces at most one loan, so this keeps loan_id == proposal_id as an
    // invariant instead of two sequences that silently diverge the moment
    // any proposal is rejected or expires without disbursing (the off-chain
    // indexer has no other way to correlate a loan back to its proposal).
    let id = proposal.id;
    let loan = Loan {
        id,
        borrower: proposal.borrower.clone(),
        principal: proposal.amount,
        interest_rate: terms.interest_rate,
        total_repayment: terms.total_repayment,
        start_time: now,
        due_time: now + terms.duration,
        status: LoanStatus::Active,
        amount_repaid: 0,
    };
    storage::set_loan(env, &loan);

    let mut borrower = storage::get_member(env, &proposal.borrower).ok_or(Error::NotMember)?;
    borrower.has_active_loan = true;
    borrower.last_loan_time = now;
    borrower.total_loans += 1;
    borrower.active_loans += 1;
    storage::set_member(env, &borrower);

    util::token_client(env).transfer(
        &util::contract_address(env),
        &proposal.borrower,
        &proposal.amount,
    );

    env.events().publish(
        (symbol_short!("loan_appr"),),
        (
            id,
            proposal.borrower.clone(),
            proposal.amount,
            loan.due_time,
        ),
    );
    Ok(())
}

/// Repays a loan's entire remaining balance in one transaction. A thin
/// wrapper over the same path [`repay_loan_partial`] uses, so existing
/// integrations built against this zero-amount-argument entrypoint keep
/// working unchanged — see [`repay_loan_partial`] for why a breaking ABI
/// change to this entrypoint was avoided instead.
pub fn repay_loan(env: &Env, borrower: Address, loan_id: u32) -> Result<(), Error> {
    repay_loan_internal(env, borrower, loan_id, None)
}

/// Repays up to `amount` of a loan's outstanding balance. `amount` must be
/// positive and may not exceed what's currently outstanding.
///
/// Payments apply to accrued interest first, then principal: whatever
/// portion of `amount` falls within the loan's still-unpaid interest is
/// distributed to active members as yield immediately, exactly as a full
/// repayment always has. The remainder (principal) needs no separate
/// bookkeeping — it's already sitting in the contract's token balance the
/// moment it's transferred in, so it's counted in the treasury right away.
///
/// The loan only flips to `Repaid` (and `has_active_loan` only clears) once
/// the outstanding balance reaches exactly zero.
///
/// This is a new entrypoint rather than an added parameter on `repay_loan`:
/// the ABI isn't upgradeable once deployed, and a new entrypoint leaves
/// every existing caller of `repay_loan(borrower, loan_id)` — including
/// `ourdao-backend` and any already-deployed clients — untouched, at the
/// cost of two entrypoints sharing one code path instead of one.
pub fn repay_loan_partial(
    env: &Env,
    borrower: Address,
    loan_id: u32,
    amount: i128,
) -> Result<(), Error> {
    repay_loan_internal(env, borrower, loan_id, Some(amount))
}

#[allow(deprecated)]
fn repay_loan_internal(
    env: &Env,
    borrower: Address,
    loan_id: u32,
    amount: Option<i128>,
) -> Result<(), Error> {
    util::require_initialized(env)?;
    util::require_not_paused(env)?;
    borrower.require_auth();

    let mut loan = storage::get_loan(env, loan_id).ok_or(Error::LoanNotFound)?;
    if loan.borrower != borrower {
        return Err(Error::NotBorrower);
    }
    if loan.status != LoanStatus::Active {
        return Err(Error::LoanNotActive);
    }

    let mut outstanding = loan.total_repayment - loan.amount_repaid;

    let now = env.ledger().timestamp();
    if now > loan.due_time {
        let policy = storage::get_policy(env);
        let penalty = outstanding * (policy.default_penalty_bps as i128) / crate::types::BASIS_POINTS;
        outstanding += penalty;
        loan.total_repayment += penalty;
        loan.principal += penalty; // Keep penalty in treasury, don't distribute as interest
    }

    let amount = amount.unwrap_or(outstanding);
    if amount <= 0 || amount > outstanding {
        return Err(Error::InvalidAmount);
    }

    util::token_client(env).transfer(&borrower, util::contract_address(env), &amount);

    // Interest-first split: how much of the loan's total interest was
    // already covered before this payment vs. after it. The difference is
    // the interest component of *this* payment; everything else is principal.
    let interest_total = loan.total_repayment - loan.principal;
    let interest_before = loan.amount_repaid.min(interest_total);
    loan.amount_repaid += amount;
    let interest_after = loan.amount_repaid.min(interest_total);
    let interest_component = interest_after - interest_before;

    let remaining = loan.total_repayment - loan.amount_repaid;
    if remaining == 0 {
        loan.status = LoanStatus::Repaid;
        if let Some(mut member) = storage::get_member(env, &borrower) {
            member.has_active_loan = false;
            member.repaid_loans += 1;
            member.active_loans = member.active_loans.saturating_sub(1);
            storage::set_member(env, &member);
        }
    }
    storage::set_loan(env, &loan);

    distribute_interest(env, interest_component);

    env.events()
        .publish((symbol_short!("loan_rpy"),), (loan_id, borrower, remaining));
    Ok(())
}

/// Permissionless keeper call: persists the expired/rejected transition for a
/// loan proposal whose voting window has passed without reaching quorum.
/// Succeeds exactly once per proposal — subsequent calls are a no-op (no
/// double event).
#[allow(deprecated)]
pub fn expire_loan_proposal(env: &Env, proposal_id: u32) -> Result<(), Error> {
    util::require_initialized(env)?;
    util::require_not_paused(env)?;
    let mut proposal =
        storage::get_loan_proposal(env, proposal_id).ok_or(Error::ProposalNotFound)?;
    proposal = refresh_phase(env, proposal);
    if proposal.phase != ProposalPhase::Expired {
        return Err(Error::ProposalNotExpired);
    }
    // Check if already persisted (no-op for repeat calls).
    // Re-read from storage to compare: if already Expired, skip.
    if let Some(original) = storage::get_loan_proposal(env, proposal_id) {
        if original.phase == ProposalPhase::Expired {
            return Ok(());
        }
    }
    storage::set_loan_proposal(env, &proposal);
    storage::extend_instance(env);

    env.events().publish(
        (symbol_short!("loan_exp"),),
        (proposal_id, proposal.borrower),
    );
    Ok(())
}

/// Marks an overdue loan as defaulted. Permissionless and callable by anyone
/// once `due_time + policy.default_grace_period` has passed — this is an
/// objective, time-based state transition (a keeper call), not an admin
/// action, so there's nothing to authorize.
///
/// Consequence: the borrower's `contribution` (their pro-rata claim on the
/// treasury via `calculate_exit_share`) is slashed by `default_penalty_bps`,
/// and `has_active_loan` is cleared. Clearing the flag is deliberate: it lets
/// a defaulted borrower still exit the DAO with their reduced share rather
/// than being trapped (exit is blocked while `has_active_loan` is true), and
/// lets them request a new loan again after the normal cooldown. Like
/// `Repaid`, `Defaulted` is terminal — a defaulted loan can't later be repaid.
#[allow(deprecated)]
pub fn mark_loan_defaulted(env: &Env, loan_id: u32) -> Result<(), Error> {
    util::require_initialized(env)?;
    util::require_not_paused(env)?;
    let mut loan = storage::get_loan(env, loan_id).ok_or(Error::LoanNotFound)?;
    if loan.status != LoanStatus::Active {
        return Err(Error::LoanNotActive);
    }

    let policy = storage::get_policy(env);
    let now = env.ledger().timestamp();
    if now < loan.due_time + policy.default_grace_period {
        return Err(Error::LoanNotOverdue);
    }

    loan.status = LoanStatus::Defaulted;
    storage::set_loan(env, &loan);

    let mut penalty: i128 = 0;
    if let Some(mut member) = storage::get_member(env, &loan.borrower) {
        penalty = (member.contribution * policy.default_penalty_bps as i128 / BASIS_POINTS)
            .min(member.contribution);
        member.contribution -= penalty;
        member.has_active_loan = false;
        // Defaulted loans are terminal but were never repaid, so only the
        // outstanding count changes — `repaid_loans` deliberately stays put.
        member.active_loans = member.active_loans.saturating_sub(1);
        storage::set_member(env, &member);
        if penalty > 0 {
            storage::set_total_contributions(env, storage::get_total_contributions(env) - penalty);
        }
    }
    storage::extend_instance(env);

    env.events().publish(
        (symbol_short!("loan_dflt"),),
        (loan_id, loan.borrower.clone(), penalty),
    );
    Ok(())
}

/// Splits repaid interest equally across active members as claimable yield.
/// Any indivisible remainder is retained by the treasury.
///
/// Uses a pull-based accumulator: instead of iterating every member and
/// bumping their `PendingYield` (O(n)), we increment a global
/// `YieldAccumulator` by `interest / active_members`. Each member stores
/// a snapshot of the accumulator at their last interaction (join, claim,
/// or exit). Their pending yield is `(accumulator - snapshot) * 1`.
///
/// `pub(crate)` (rather than private) solely so the property tests in
/// `test.rs` can drive it directly with arbitrary `interest` values instead
/// of only the ones reachable through a real loan's computed interest.
#[allow(deprecated)]
pub(crate) fn distribute_interest(env: &Env, interest: i128) {
    let active = storage::get_active_members(env) as i128;
    if interest <= 0 || active == 0 {
        return;
    }

    // #60 — Carry the sub-divisible remainder forward instead of silently discarding
    let total_interest = interest + storage::get_yield_remainder(env);
    let per_member = total_interest / active;
    let remainder = total_interest % active;

    storage::set_yield_remainder(env, remainder);

    if per_member > 0 {
        let current = storage::get_yield_accumulator(env);
        storage::set_yield_accumulator(env, current + per_member);
    }

    // Unconditionally publish the event so the indexer sees the interest paid
    env.events()
        .publish((symbol_short!("interest"),), (interest, active));
}
