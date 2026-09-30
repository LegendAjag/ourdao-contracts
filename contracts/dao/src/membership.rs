use soroban_sdk::{symbol_short, Address, Env};

use crate::error::Error;
use crate::storage::{self, extend_instance};
use crate::types::{Member, MemberStatus, StakingRewardClaimed};
use crate::util;

pub fn register_member(env: &Env, member: Address) -> Result<(), Error> {
    util::require_initialized(env)?;
    util::require_not_paused(env)?;
    member.require_auth();

    // Reject only genuinely active members; a previously-exited member may rejoin.
    if let Some(existing) = storage::get_member(env, &member) {
        if existing.status == MemberStatus::ActiveMember {
            return Err(Error::AlreadyMember);
        }
    }

    let fee = storage::get_membership_fee(env);

    let is_returning = storage::get_member(env, &member).is_some();
    let record = Member {
        address: member.clone(),
        status: MemberStatus::ActiveMember,
        join_time: env.ledger().timestamp(),
        contribution: fee,
        share_balance: fee,
        has_active_loan: false,
        last_loan_time: 0,
    };
    storage::set_member(env, &record);

    // Snapshot the accumulator so the new member earns nothing from interest
    // repaid before they joined.
    let acc = storage::get_yield_accumulator(env);
    storage::set_yield_snapshot(env, &member, acc);

    if !is_returning {
        let mut members = storage::get_members(env);
        members.push_back(member.clone());
        storage::set_members(env, &members);
        storage::set_total_members(env, storage::get_total_members(env) + 1);
    }
    storage::set_active_members(env, storage::get_active_members(env) + 1);
    extend_instance(env);

    // Interaction last (checks-effects-interactions): the fee transfer only
    // happens once every state transition above has completed.
    util::token_client(env).transfer(&member, &util::contract_address(env), &fee);

    env.events()
        .publish((symbol_short!("joined"),), (member, fee));
    Ok(())
}

pub fn exit_dao(env: &Env, member: Address) -> Result<(), Error> {
    util::require_initialized(env)?;
    util::require_not_paused(env)?;
    let mut record = util::require_active_member(env, &member)?;
    if record.has_active_loan {
        return Err(Error::HasActiveLoan);
    }

    let share = calculate_exit_share(env, &member);
    let stake = storage::get_stake(env, &member);
    let pending = compute_pending_yield(env, &member);
    let payout = share + stake + pending;

    if payout > 0 {
        util::token_client(env).transfer(&util::contract_address(env), &member, &payout);
    }
    if stake > 0 {
        storage::set_stake(env, &member, 0);
        storage::set_total_staked(env, storage::get_total_staked(env) - stake);
    }
    // Settle yield: snapshot to current accumulator so rejoining starts clean.
    let acc = storage::get_yield_accumulator(env);
    storage::set_yield_snapshot(env, &member, acc);

    record.status = MemberStatus::Inactive;
    record.share_balance = 0;
    storage::set_member(env, &record);
    storage::set_active_members(env, storage::get_active_members(env) - 1);
    extend_instance(env);

    env.events()
        .publish((symbol_short!("exited"),), (member, share));
    Ok(())
}

pub fn claim_rewards(env: &Env, member: Address) -> Result<i128, Error> {
    util::require_initialized(env)?;
    util::require_not_paused(env)?;
    util::require_active_member(env, &member)?;
    let pending = compute_pending_yield(env, &member);
    if pending <= 0 {
        return Err(Error::NothingToClaim);
    }
    // Settle: advance snapshot to current accumulator.
    let acc = storage::get_yield_accumulator(env);
    storage::set_yield_snapshot(env, &member, acc);
    util::token_client(env).transfer(&util::contract_address(env), &member, &pending);
    let now = env.ledger().timestamp();
    env.events().publish(
        (symbol_short!("claimed"), member.clone(), pending),
        StakingRewardClaimed {
            member,
            amount: pending,
            timestamp: now,
        },
    );
    Ok(pending)
}

fn share_from_contributions(treasury: i128, contribution: i128, total_contributions: i128) -> i128 {
    if total_contributions == 0 || treasury <= 0 {
        return 0;
    }
    treasury * contribution / total_contributions
}

pub fn calculate_exit_share(env: &Env, member: &Address) -> i128 {
    let record = match storage::get_member(env, member) {
        Some(m) if m.status == MemberStatus::ActiveMember => m,
        _ => return 0,
    };
    let total_contributions = total_active_contributions(env);
    let treasury = util::treasury_balance(env);
    share_from_contributions(treasury, record.contribution, total_contributions)
}

fn total_active_contributions(env: &Env) -> i128 {
    storage::get_members(env)
        .iter()
        .filter_map(|addr| storage::get_member(env, &addr))
        .filter(|m| m.status == MemberStatus::ActiveMember)
        .map(|m| m.contribution)
        .sum()
}

fn compute_pending_yield(env: &Env, addr: &Address) -> i128 {
    let acc = storage::get_yield_accumulator(env);
    let snap = storage::get_yield_snapshot(env, addr);
    (acc - snap).max(0)
}
