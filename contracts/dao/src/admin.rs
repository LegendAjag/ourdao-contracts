use soroban_sdk::{symbol_short, token, Address, Env, Vec};

use crate::error::Error;
use crate::storage::{self, extend_instance};
use crate::types::{LoanPolicy, PendingPolicyUpdate, BASIS_POINTS};
use crate::util;

/// Timelock delay between proposing and executing a policy update (48 hours in seconds).
pub const TIMELOCK_DURATION: u64 = 48 * 60 * 60;

fn validate_policy(policy: &LoanPolicy) -> Result<(), Error> {
    if policy.membership_contribution <= 0
        || policy.max_loan_duration == 0
        || policy.min_interest_rate as i128 > BASIS_POINTS
        || policy.max_interest_rate as i128 > BASIS_POINTS
        || policy.min_interest_rate > policy.max_interest_rate
        || policy.max_loan_to_treasury_ratio == 0
        || policy.max_loan_to_treasury_ratio as i128 > BASIS_POINTS
        || policy.default_penalty_bps as i128 > BASIS_POINTS
        || policy.editing_period == 0
        || policy.voting_period == 0
        || policy.editing_period > 30 * 24 * 60 * 60
        || policy.voting_period > 30 * 24 * 60 * 60
        || policy.min_membership_duration > 30 * 24 * 60 * 60
        || policy.cooldown_period > 30 * 24 * 60 * 60
        || policy.default_grace_period > 30 * 24 * 60 * 60
        || policy.treasury_threshold == 0
        || policy.treasury_threshold as i128 > BASIS_POINTS
        || policy.quorum_bps as i128 > BASIS_POINTS
    {
        return Err(Error::InvalidLoanPolicy);
    }
    Ok(())
}

/// Probe `token` with a read-only `balance` call so a wrong address (an
/// account, a non-token contract, or a typo) is rejected at initialization
/// instead of bricking every later transfer. `try_` calls turn a missing
/// contract or missing function into an error rather than a trap.
fn validate_token(env: &Env, token: &Address) -> Result<(), Error> {
    match token::Client::new(env, token).try_balance(&env.current_contract_address()) {
        Ok(Ok(_)) => Ok(()),
        _ => Err(Error::InvalidToken),
    }
}

// `env.events().publish` is deprecated in soroban-sdk in favour of
// `#[contractevent]`, but migration changes the wire format and is a
// coordinated, breaking change.  Suppress only in functions that publish events
// so unrelated future deprecations still surface via `cargo clippy -D warnings`.
#[allow(deprecated)]
pub fn initialize(
    env: &Env,
    admins: Vec<Address>,
    consensus_threshold: u32,
    membership_fee: i128,
    token: Address,
    policy: LoanPolicy,
) -> Result<(), Error> {
    if storage::is_initialized(env) {
        return Err(Error::AlreadyInitialized);
    }
    if consensus_threshold == 0 || consensus_threshold as i128 > BASIS_POINTS {
        return Err(Error::InvalidThreshold);
    }
    if membership_fee <= 0 {
        return Err(Error::InvalidAmount);
    }
    if admins.is_empty() {
        return Err(Error::NotAuthorized);
    }
    // Duplicates would let `remove_admin` (which drops every matching entry)
    // leave the contract with zero admins despite its last-admin guard (#42).
    for (i, a) in admins.iter().enumerate() {
        if admins.iter().skip(i + 1).any(|b| b == a) {
            return Err(Error::AlreadyAdmin);
        }
    }
    validate_policy(&policy)?;
    validate_token(env, &token)?;

    storage::set_admins(env, &admins);
    storage::set_threshold(env, consensus_threshold);
    storage::set_membership_fee(env, membership_fee);
    storage::set_token(env, &token);
    storage::set_policy(env, &policy);
    storage::set_paused(env, false);
    storage::set_members(env, &Vec::new(env));
    storage::set_total_members(env, 0);
    storage::set_active_members(env, 0);
    extend_instance(env);

    env.events().publish(
        (symbol_short!("init"),),
        (admins, consensus_threshold, membership_fee, token),
    );
    Ok(())
}

#[allow(deprecated)]
pub fn add_admin(env: &Env, caller: Address, admin: Address) -> Result<(), Error> {
    util::require_admin(env, &caller)?;
    let mut admins = storage::get_admins(env);
    if admins.iter().any(|a| a == admin) {
        return Err(Error::AlreadyAdmin);
    }
    admins.push_back(admin.clone());
    storage::set_admins(env, &admins);
    extend_instance(env);
    env.events().publish((symbol_short!("admin_add"),), admin);
    Ok(())
}

#[allow(deprecated)]
pub fn remove_admin(env: &Env, caller: Address, admin: Address) -> Result<(), Error> {
    util::require_admin(env, &caller)?;
    let admins = storage::get_admins(env);
    if admins.len() <= 1 {
        return Err(Error::CannotRemoveLastAdmin);
    }
    let mut next = Vec::new(env);
    let mut found = false;
    for a in admins.iter() {
        if a == admin {
            found = true;
        } else {
            next.push_back(a);
        }
    }
    if !found {
        return Err(Error::NotAdmin);
    }
    storage::set_admins(env, &next);
    extend_instance(env);
    env.events().publish((symbol_short!("admin_rem"),), admin);
    Ok(())
}

#[allow(deprecated)]
pub fn set_pauser(env: &Env, caller: Address, pauser: Address) -> Result<(), Error> {
    util::require_admin(env, &caller)?;
    if storage::get_pauser(env).as_ref() == Some(&pauser) {
        return Err(Error::AlreadyPauser);
    }
    storage::set_pauser(env, &pauser);
    extend_instance(env);
    env.events().publish((symbol_short!("pset"),), pauser);
    Ok(())
}

#[allow(deprecated)]
pub fn revoke_pauser(env: &Env, caller: Address) -> Result<(), Error> {
    util::require_admin(env, &caller)?;
    if storage::get_pauser(env).is_none() {
        return Err(Error::NotPauser);
    }
    storage::remove_pauser(env);
    extend_instance(env);
    env.events().publish((symbol_short!("prev"),), caller);
    Ok(())
}

#[allow(deprecated)]
pub fn set_consensus_threshold(env: &Env, caller: Address, threshold: u32) -> Result<(), Error> {
    util::require_admin(env, &caller)?;
    if threshold == 0 || threshold as i128 > BASIS_POINTS {
        return Err(Error::InvalidThreshold);
    }
    storage::set_threshold(env, threshold);
    extend_instance(env);
    env.events()
        .publish((symbol_short!("threshold"),), threshold);
    Ok(())
}

#[allow(deprecated)]
pub fn propose_policy_update(
    env: &Env,
    caller: Address,
    policy: LoanPolicy,
) -> Result<(), Error> {
    util::require_admin(env, &caller)?;
    validate_policy(&policy)?;
    let now = env.ledger().timestamp();
    let execution_time = now + TIMELOCK_DURATION;
    let update = PendingPolicyUpdate {
        policy,
        proposed_at: now,
        execution_time,
    };
    storage::set_pending_policy_update(env, &update);
    extend_instance(env);
    env.events()
        .publish((symbol_short!("pol_prop"),), (caller, execution_time));
    Ok(())
}

#[allow(deprecated)]
pub fn execute_policy_update(env: &Env, caller: Address) -> Result<(), Error> {
    util::require_admin(env, &caller)?;
    let update = storage::get_pending_policy_update(env).ok_or(Error::NoPendingPolicy)?;
    let now = env.ledger().timestamp();
    if now < update.execution_time {
        return Err(Error::TimelockNotExpired);
    }
    storage::set_policy(env, &update.policy);
    storage::remove_pending_policy_update(env);
    extend_instance(env);
    env.events().publish((symbol_short!("policy"),), ());
    Ok(())
}

#[allow(deprecated)]
pub fn cancel_policy_update(env: &Env, caller: Address) -> Result<(), Error> {
    util::require_admin(env, &caller)?;
    if storage::get_pending_policy_update(env).is_none() {
        return Err(Error::NoPendingPolicy);
    }
    storage::remove_pending_policy_update(env);
    extend_instance(env);
    env.events().publish((symbol_short!("pol_canc"),), caller);
    Ok(())
}

pub fn get_pending_policy_update(env: &Env) -> Option<PendingPolicyUpdate> {
    storage::get_pending_policy_update(env)
}

pub fn set_policy(env: &Env, caller: Address, policy: LoanPolicy) -> Result<(), Error> {
    propose_policy_update(env, caller, policy)
}

#[allow(deprecated)]
pub fn pause(env: &Env, caller: Address) -> Result<(), Error> {
    caller.require_auth();
    if !util::is_admin(env, &caller) && !util::is_pauser(env, &caller) {
        return Err(Error::NotAdmin);
    }
    if storage::is_paused(env) {
        return Err(Error::Paused);
    }
    storage::set_paused(env, true);
    extend_instance(env);
    env.events().publish((symbol_short!("paused"),), ());
    Ok(())
}

#[allow(deprecated)]
pub fn unpause(env: &Env, caller: Address) -> Result<(), Error> {
    util::require_admin(env, &caller)?;
    if !storage::is_paused(env) {
        return Err(Error::NotPaused);
    }
    storage::set_paused(env, false);
    extend_instance(env);
    env.events().publish((symbol_short!("unpaused"),), ());
    Ok(())
}
