use soroban_sdk::{symbol_short, Address, Env, String};

use crate::error::Error;
use crate::storage;
use crate::types::{ProposalStatus, TreasuryProposal};
use crate::util;

pub fn propose_withdrawal(
    env: &Env,
    proposer: Address,
    amount: i128,
    destination: Address,
    reason: String,
    private: bool,
) -> Result<u32, Error> {
    util::require_initialized(env)?;
    util::require_not_paused(env)?;
    util::require_active_member(env, &proposer)?;

    if amount <= 0 {
        return Err(Error::InvalidAmount);
    }
    if amount > util::treasury_balance(env) {
        return Err(Error::InsufficientTreasury);
    }

    let id = storage::next_id(env, storage::DataKey::NextTreasuryId);
    let proposal = TreasuryProposal {
        id,
        proposer,
        amount,
        destination: destination.clone(),
        reason,
        created_at: env.ledger().timestamp(),
        status: ProposalStatus::Pending,
        for_votes: 0,
        against_votes: 0,
        votes_cast: 0,
        voting_period: storage::get_policy(env).voting_period,
        treasury_threshold: storage::get_policy(env).treasury_threshold,
        private,
    };
    storage::set_treasury_proposal(env, &proposal);
    storage::extend_instance(env);

    env.events().publish(
        (symbol_short!("tre_prop"),),
        (id, amount, destination, private),
    );
    Ok(id)
}

pub fn vote(env: &Env, voter: Address, proposal_id: u32, support: bool) -> Result<(), Error> {
    util::require_initialized(env)?;
    util::require_not_paused(env)?;
    util::require_active_member(env, &voter)?;

    let proposal =
        storage::get_treasury_proposal(env, proposal_id).ok_or(Error::TreasuryProposalNotFound)?;
    if proposal.private {
        // Private proposals must go through commit → reveal, not open voting.
        return Err(Error::NotAuthorized);
    }
    tally(env, proposal, &voter, support)
}

/// Shared vote-recording + execution path. Used by open voting and by the
/// commit-reveal privacy module once a vote is revealed. Assumes the caller has
/// already authorized `voter` and enforced any privacy-mode rules.
pub fn tally(
    env: &Env,
    mut proposal: TreasuryProposal,
    voter: &Address,
    support: bool,
) -> Result<(), Error> {
    if proposal.status != ProposalStatus::Pending {
        return Err(Error::NotInVotingPhase);
    }
    // Treasury proposals never expired before this check (#17) — a Pending
    // proposal could sit indefinitely and still be voted through/executed
    // long after the intended voting window. Mirror the loan proposal
    // lifecycle: once VOTING_PERIOD has elapsed since creation, persist the
    // Expired status and reject further votes.
    if env.ledger().timestamp() > proposal.created_at + proposal.voting_period {
        return Err(Error::NotInVotingPhase);
    }
    if storage::has_treasury_voted(env, proposal.id, voter) {
        return Err(Error::AlreadyVoted);
    }

    let weight = util::voting_weight(env, voter);
    if support {
        proposal.for_votes += weight;
    } else {
        proposal.against_votes += weight;
    }
    proposal.votes_cast += 1;
    storage::set_treasury_voted(env, proposal.id, voter);
    env.events().publish(
        (symbol_short!("tre_vote"),),
        (proposal.id, voter.clone(), support),
    );

    let required = util::required_votes(
        storage::get_active_members(env),
        proposal.treasury_threshold,
    );
    if proposal.for_votes >= required {
        proposal.status = ProposalStatus::ApprovedPendingDisbursement;
        storage::set_treasury_proposal(env, &proposal);
        if execute(env, &mut proposal).is_error() {
            env.events()
                .publish((symbol_short!("tre_wait"),), (proposal.id, proposal.amount));
        }
    } else {
        let remaining = storage::get_active_members(env).saturating_sub(proposal.votes_cast);
        let max_remaining = remaining as i128 * (1 + util::MAX_STAKE_BONUS);
        if proposal.for_votes + max_remaining < required {
            proposal.status = ProposalStatus::Rejected;
            env.events().publish(
                (symbol_short!("tre_rej"),),
                (proposal.id, proposal.for_votes, proposal.against_votes),
            );
        }
    }
    storage::set_treasury_proposal(env, &proposal);
    Ok(())
}

pub fn execute_approved(env: &Env, proposal_id: u32) -> Result<(), Error> {
    util::require_initialized(env)?;
    util::require_not_paused(env)?;
    let mut proposal =
        storage::get_treasury_proposal(env, proposal_id).ok_or(Error::TreasuryProposalNotFound)?;
    if proposal.status != ProposalStatus::ApprovedPendingDisbursement {
        return Err(Error::NotInVotingPhase);
    }
    execute(env, &mut proposal)?;
    storage::set_treasury_proposal(env, &proposal);
    Ok(())
}

fn execute(env: &Env, proposal: &mut TreasuryProposal) -> Result<(), Error> {
    if util::treasury_balance(env) < proposal.amount {
        return Err(Error::InsufficientTreasury);
    }
    proposal.status = ProposalStatus::Executed;
    util::token_client(env).transfer(
        &util::contract_address(env),
        &proposal.destination,
        &proposal.amount,
    );
    env.events().publish(
        (symbol_short!("tre_exec"),),
        (proposal.id, proposal.amount, proposal.destination.clone()),
    );
    Ok(())
}

/// Cancel a treasury proposal during its editing period. Only the original
/// proposer may cancel, only while the proposal is still Pending and within
/// the editing window. Emits a dedicated `ProposalCancelled` event so
/// off-chain indexers can track cancellations cleanly.
pub fn cancel_treasury_proposal(
    env: &Env,
    proposer: Address,
    proposal_id: u32,
) -> Result<(), Error> {
    util::require_initialized(env)?;
    util::require_not_paused(env)?;
    util::require_active_member(env, &proposer)?;
    let mut proposal = storage::get_treasury_proposal(env, proposal_id)
        .ok_or(Error::TreasuryProposalNotFound)?;
    if proposal.proposer != proposer {
        return Err(Error::NotAuthorized);
    }
    if proposal.status != ProposalStatus::Pending {
        return Err(Error::NotInVotingPhase);
    }
    let now = env.ledger().timestamp();
    // Treasury proposals don't carry an explicit editing window; treat the
    // editing period as the policy's editing_period from creation.
    let policy = storage::get_policy(env);
    if now > proposal.created_at + policy.editing_period {
        return Err(Error::NotInEditingPhase);
    }

    proposal.status = ProposalStatus::Cancelled;
    storage::set_treasury_proposal(env, &proposal);

    env.events().publish(
        (symbol_short!("prop_canc"),),
        (proposal_id, now),
    );
    Ok(())
}

pub fn expire_treasury_proposal(env: &Env, proposal_id: u32) -> Result<(), Error> {
    util::require_initialized(env)?;
    util::require_not_paused(env)?;
    let mut proposal = storage::get_treasury_proposal(env, proposal_id)
        .ok_or(Error::TreasuryProposalNotFound)?;
    
    if proposal.status != ProposalStatus::Pending {
        return Err(Error::NotInVotingPhase); // Re-using error
    }
    
    if env.ledger().timestamp() > proposal.created_at + proposal.voting_period {
        proposal.status = ProposalStatus::Expired;
        storage::set_treasury_proposal(env, &proposal);
        // Emitting an event would be nice but not strictly required.
        Ok(())
    } else {
        Err(Error::NotInVotingPhase) // Re-using error
    }
}
