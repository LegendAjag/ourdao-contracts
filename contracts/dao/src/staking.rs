//! Staking module. Members lock tokens to signal commitment and gain a
//! capped boost to their voting weight (see `util::voting_weight`). Staked
//! funds are tracked separately from the treasury and are never lent out or
//! distributed as yield.

use soroban_sdk::{symbol_short, Address, Env};

use crate::error::Error;
use crate::storage;
use crate::util;

pub fn stake(env: &Env, member: Address, amount: i128) -> Result<(), Error> {
    util::require_initialized(env)?;
    util::require_not_paused(env)?;
    util::require_active_member(env, &member)?;
    if amount <= 0 {
        return Err(Error::InvalidAmount);
    }

    util::token_client(env).transfer(&member, util::contract_address(env), &amount);

    let new_stake = storage::get_stake(env, &member) + amount;
    storage::set_stake(env, &member, new_stake);
    storage::set_total_staked(env, storage::get_total_staked(env) + amount);
    env.storage().persistent().set(
        &crate::storage::DataKey::StakeTime(member.clone()),
        &env.ledger().timestamp(),
    );
    storage::extend_instance(env);

    env.events()
        .publish((symbol_short!("staked"),), (member, amount, new_stake));
    Ok(())
}

pub fn unstake(env: &Env, member: Address, amount: i128) -> Result<(), Error> {
    util::require_initialized(env)?;
    util::require_not_paused(env)?;
    member.require_auth();
    if amount <= 0 {
        return Err(Error::InvalidAmount);
    }

    let current = storage::get_stake(env, &member);
    if current == 0 {
        return Err(Error::NoStake);
    }
    if amount > current {
        return Err(Error::InsufficientStake);
    }

    let last_stake: u64 = env
        .storage()
        .persistent()
        .get(&crate::storage::DataKey::StakeTime(member.clone()))
        .unwrap_or(0);
    let policy = storage::get_policy(env);
    if env.ledger().timestamp() < last_stake + policy.voting_period {
        return Err(Error::CooldownActive);
    }

    let new_stake = current - amount;
    storage::set_stake(env, &member, new_stake);
    storage::set_total_staked(env, storage::get_total_staked(env) - amount);
    util::token_client(env).transfer(&util::contract_address(env), &member, &amount);
    storage::extend_instance(env);

    env.events()
        .publish((symbol_short!("unstaked"),), (member, amount, new_stake));
    Ok(())
}
