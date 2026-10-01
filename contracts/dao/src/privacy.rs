//! Commit-reveal voting for private treasury proposals. On-chain encrypted
//! voting isn't available on Soroban, so privacy is achieved with a two-phase
//! scheme: members first submit a hash of `(support, salt)`, hiding their
//! choice while voting is open, then reveal it later. The tally only runs on
//! reveal, so no one sees the running result influence how others vote.
//!
//! Applies to treasury proposals created with `private = true`.

use soroban_sdk::{symbol_short, Address, Bytes, BytesN, Env};

use crate::error::Error;
use crate::storage;
use crate::treasury;
use crate::types::ProposalStatus;
use crate::util;

/// The commitment a voter must submit: `sha256([support_byte] ++ salt)`.
pub fn compute_commitment(env: &Env, support: bool, salt: &BytesN<32>) -> BytesN<32> {
    let mut preimage = Bytes::new(env);
    preimage.push_back(if support { 1 } else { 0 });
    preimage.append(&Bytes::from_array(env, &salt.to_array()));
    env.crypto().sha256(&preimage).to_bytes()
}

// `env.events().publish` is deprecated in soroban-sdk in favour of
// `#[contractevent]`, but migration is a coordinated, breaking wire-format
// change (#85).  Suppress per-function so unrelated deprecations still surface.
#[allow(deprecated)]
pub fn commit_vote(
    env: &Env,
    voter: Address,
    proposal_id: u32,
    commitment: BytesN<32>,
) -> Result<(), Error> {
    util::require_initialized(env)?;
    util::require_not_paused(env)?;
    util::require_active_member(env, &voter)?;

    let proposal =
        storage::get_treasury_proposal(env, proposal_id).ok_or(Error::TreasuryProposalNotFound)?;
    if !proposal.private {
        return Err(Error::NotAuthorized);
    }
    if proposal.status != ProposalStatus::Pending {
        return Err(Error::NotInVotingPhase);
    }
    if env.ledger().timestamp() > proposal.created_at + proposal.voting_period {
        return Err(Error::VotingEnded);
    }
    if storage::has_treasury_voted(env, proposal_id, &voter) {
        return Err(Error::AlreadyVoted);
    }
    // A commitment is binding: overwriting it would let a member keep changing
    // their hidden vote until the reveal phase (#49).
    if storage::get_commit(env, proposal_id, &voter).is_some() {
        return Err(Error::AlreadyVoted);
    }

    storage::set_commit(env, proposal_id, &voter, &commitment);
    env.events()
        .publish((symbol_short!("committed"),), (proposal_id, voter));
    Ok(())
}

#[allow(deprecated)]
pub fn reveal_vote(
    env: &Env,
    voter: Address,
    proposal_id: u32,
    support: bool,
    salt: BytesN<32>,
) -> Result<(), Error> {
    util::require_initialized(env)?;
    util::require_not_paused(env)?;
    util::require_active_member(env, &voter)?;

    let proposal =
        storage::get_treasury_proposal(env, proposal_id).ok_or(Error::TreasuryProposalNotFound)?;
    if !proposal.private {
        return Err(Error::NotAuthorized);
    }
    let commit_end = proposal.created_at + proposal.voting_period;
    let reveal_end = commit_end + proposal.voting_period;
    if env.ledger().timestamp() <= commit_end {
        return Err(Error::NotYetRevealed);
    }
    if env.ledger().timestamp() > reveal_end {
        return Err(Error::VotingEnded);
    }
    if storage::has_treasury_voted(env, proposal_id, &voter) {
        return Err(Error::AlreadyRevealed);
    }

    let stored = storage::get_commit(env, proposal_id, &voter).ok_or(Error::NoCommitment)?;
    let expected = compute_commitment(env, support, &salt);
    if stored != expected {
        return Err(Error::CommitmentMismatch);
    }

    // Consume the commitment; the shared tally records the vote and, if the
    // threshold is met, executes the withdrawal.
    storage::remove_commit(env, proposal_id, &voter);
    env.events().publish(
        (symbol_short!("revealed"),),
        (proposal_id, voter.clone(), support),
    );
    treasury::tally(env, proposal, &voter, support)
}
