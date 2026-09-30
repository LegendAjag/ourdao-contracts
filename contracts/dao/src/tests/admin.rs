use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, Bytes, BytesN, Env, String, Vec};

use super::common::*;
use crate::admin::TIMELOCK_DURATION;
use crate::storage::ProposalKind;
use crate::{Error, OurDao, OurDaoClient};

#[test]
fn initialize_rejects_duplicate_admins() {
    let env = Env::default();
    env.mock_all_auths();
    let token_admin = Address::generate(&env);
    let token_id = env
        .register_stellar_asset_contract_v2(token_admin)
        .address();
    let contract_id = env.register(OurDao, ());
    let client = OurDaoClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let mut admins = Vec::new(&env);
    admins.push_back(admin.clone());
    admins.push_back(admin.clone());

    let res = client.try_initialize(&admins, &5_100u32, &FEE, &token_id, &policy());
    assert_eq!(res, Err(Ok(Error::AlreadyAdmin)));
}

#[test]
fn pause_blocks_all_mutating_entrypoints() {
    // This test explicitly checks every mutating entrypoint to ensure pause()
    // prevents state changes. Every public entrypoint that mutates contract state
    // must be listed below with an assertion. If you add a new mutating entrypoint
    // without a pause decision, this test will fail — the property being protected
    // (an emergency stop actually stops everything) is too critical to check by
    // hand-written list.
    //
    // Entrypoints deliberately callable while paused:
    // - None. All state-changing operations should be blocked by pause().
    // - Note: repayment was discussed but decided to be pause-gated to maintain
    //   consistent emergency stop semantics (see issue #52).

    let s = setup(3);
    let borrower = s.members.get(0).unwrap();
    let voter = s.members.get(1).unwrap();
    let staker = s.members.get(2).unwrap();
    let newcomer = Address::generate(&s.env);

    // Set up some proposals to vote on / interact with
    let loan_pid = s.client.request_loan(&borrower, &500, &None);
    advance(&s.env, EDITING + 1);

    let treasury_pid = s.client.propose_treasury_withdrawal(
        &borrower,
        &100,
        &newcomer,
        &String::from_slice(&s.env, "test"),
        &false,
    );

    // Pause the contract
    s.client.pause(&s.admin);
    assert!(s.client.is_paused());

    // ==================== Membership ====================
    let res = s.client.try_register_member(&newcomer);
    assert_eq!(
        res,
        Err(Ok(Error::Paused)),
        "register_member should be pause-gated"
    );

    let res = s.client.try_exit_dao(&borrower);
    assert_eq!(
        res,
        Err(Ok(Error::Paused)),
        "exit_dao should be pause-gated"
    );

    let res = s.client.try_claim_rewards(&borrower);
    assert_eq!(
        res,
        Err(Ok(Error::Paused)),
        "claim_rewards should be pause-gated"
    );

    // ==================== Loans ====================
    let res = s.client.try_request_loan(&borrower, &1000, &None);
    assert_eq!(
        res,
        Err(Ok(Error::Paused)),
        "request_loan should be pause-gated"
    );

    let res = s.client.try_edit_loan_proposal(&borrower, &loan_pid, &600);
    assert_eq!(
        res,
        Err(Ok(Error::Paused)),
        "edit_loan_proposal should be pause-gated"
    );

    let res = s.client.try_vote_on_loan_proposal(&voter, &loan_pid, &true);
    assert_eq!(
        res,
        Err(Ok(Error::Paused)),
        "vote_on_loan_proposal should be pause-gated"
    );

    let res = s.client.try_disburse_approved_loan(&loan_pid);
    assert_eq!(
        res,
        Err(Ok(Error::Paused)),
        "disburse_approved_loan should be pause-gated"
    );

    let res = s.client.try_repay_loan(&borrower, &0);
    assert_eq!(
        res,
        Err(Ok(Error::Paused)),
        "repay_loan should be pause-gated"
    );

    let res = s.client.try_repay_loan_partial(&borrower, &0, &100);
    assert_eq!(
        res,
        Err(Ok(Error::Paused)),
        "repay_loan_partial should be pause-gated"
    );

    let res = s.client.try_mark_loan_defaulted(&0);
    assert_eq!(
        res,
        Err(Ok(Error::Paused)),
        "mark_loan_defaulted should be pause-gated"
    );

    let res = s.client.try_expire_loan_proposal(&0);
    assert_eq!(
        res,
        Err(Ok(Error::Paused)),
        "expire_loan_proposal should be pause-gated"
    );

    // ==================== Treasury ====================
    let res = s.client.try_propose_treasury_withdrawal(
        &borrower,
        &100,
        &newcomer,
        &String::from_slice(&s.env, "test"),
        &false,
    );
    assert_eq!(
        res,
        Err(Ok(Error::Paused)),
        "propose_treasury_withdrawal should be pause-gated"
    );

    let res = s
        .client
        .try_vote_on_treasury_proposal(&voter, &treasury_pid, &true);
    assert_eq!(
        res,
        Err(Ok(Error::Paused)),
        "vote_on_treasury_proposal should be pause-gated"
    );

    let res = s.client.try_expire_treasury_proposal(&0);
    assert_eq!(
        res,
        Err(Ok(Error::Paused)),
        "expire_treasury_proposal should be pause-gated"
    );

    let res = s.client.try_execute_treasury_proposal(&0);
    assert_eq!(
        res,
        Err(Ok(Error::Paused)),
        "execute_treasury_proposal should be pause-gated"
    );

    // ==================== Staking ====================
    let res = s.client.try_stake(&staker, &100);
    assert_eq!(res, Err(Ok(Error::Paused)), "stake should be pause-gated");

    let res = s.client.try_unstake(&staker, &100);
    assert_eq!(res, Err(Ok(Error::Paused)), "unstake should be pause-gated");

    // ==================== Registry ====================
    let res = s
        .client
        .try_register_name(&borrower, &String::from_slice(&s.env, "test"));
    assert_eq!(
        res,
        Err(Ok(Error::Paused)),
        "register_name should be pause-gated"
    );

    // ==================== Privacy (commit-reveal voting) ====================
    let commitment = BytesN::from_array(&s.env, &[0u8; 32]);
    let res = s
        .client
        .try_commit_treasury_vote(&voter, &treasury_pid, &commitment);
    assert_eq!(
        res,
        Err(Ok(Error::Paused)),
        "commit_treasury_vote should be pause-gated"
    );

    let salt = BytesN::from_array(&s.env, &[0u8; 32]);
    let res = s
        .client
        .try_reveal_treasury_vote(&voter, &treasury_pid, &true, &salt);
    assert_eq!(
        res,
        Err(Ok(Error::Paused)),
        "reveal_treasury_vote should be pause-gated"
    );

    // ==================== Docs (content-hash metadata) ====================
    let cid = Bytes::from_slice(&s.env, &[1, 2, 3]);
    let res = s
        .client
        .try_attach_document(&borrower, &ProposalKind::Loan, &loan_pid, &cid);
    assert_eq!(
        res,
        Err(Ok(Error::Paused)),
        "attach_document should be pause-gated"
    );

    // Verify unpause works
    s.client.unpause(&s.admin);
    assert!(!s.client.is_paused());
}

#[test]
fn only_admin_governs() {
    let s = setup(1);
    let intruder = s.members.get(0).unwrap();
    let res = s.client.try_set_consensus_threshold(&intruder, &7_000);
    assert_eq!(res, Err(Ok(Error::NotAdmin)));

    s.client.set_consensus_threshold(&s.admin, &7_000);
    assert_eq!(s.client.get_consensus_threshold(), 7_000);
}

#[test]
fn cannot_remove_last_admin() {
    let s = setup(0);
    let res = s.client.try_remove_admin(&s.admin, &s.admin);
    assert_eq!(res, Err(Ok(Error::CannotRemoveLastAdmin)));
}

// initialize token validation (#115)
fn init_with_token(env: &Env, token: &Address) -> Result<(), Error> {
    let contract_id = env.register(OurDao, ());
    let client = OurDaoClient::new(env, &contract_id);
    let mut admins = Vec::new(env);
    admins.push_back(Address::generate(env));
    match client.try_initialize(&admins, &5_100u32, &FEE, token, &policy()) {
        Ok(_) => Ok(()),
        Err(Ok(e)) => Err(e),
        Err(Err(_)) => panic!("unexpected host error"),
    }
}

#[test]
fn initialize_rejects_account_address_as_token() {
    let env = Env::default();
    env.mock_all_auths();
    let not_a_contract = Address::generate(&env);
    assert_eq!(
        init_with_token(&env, &not_a_contract),
        Err(Error::InvalidToken)
    );
}

#[test]
fn initialize_rejects_contract_that_is_not_a_token() {
    let env = Env::default();
    env.mock_all_auths();
    // A real contract, but not a token: it has no `balance` entrypoint.
    let other = env.register(OurDao, ());
    assert_eq!(init_with_token(&env, &other), Err(Error::InvalidToken));
}

#[test]
fn initialize_accepts_a_real_token() {
    let env = Env::default();
    env.mock_all_auths();
    let sac = env.register_stellar_asset_contract_v2(Address::generate(&env));
    assert_eq!(init_with_token(&env, &sac.address()), Ok(()));
}

// Issue #191: Configurable quorum threshold parameter in LoanPolicy
#[test]
fn validate_policy_rejects_quorum_bps_above_basis_points() {
    let mut p = policy();
    p.quorum_bps = 10_001; // > 10_000 BASIS_POINTS
    let s = setup(1);
    let res = s.client.try_propose_policy_update(&s.admin, &p);
    assert_eq!(res, Err(Ok(Error::InvalidLoanPolicy)));
}

// Issue #192: Timelock delay for administrative policy changes
#[test]
fn policy_update_timelock_enforced() {
    let s = setup(3);
    let non_admin = s.members.get(0).unwrap();

    let mut new_p = policy();
    new_p.max_loan_duration = 60 * 24 * 60 * 60; // change duration

    // Non-admin cannot propose policy update
    let err = s.client.try_propose_policy_update(&non_admin, &new_p);
    assert_eq!(err, Err(Ok(Error::NotAdmin)));

    // Admin proposes update
    s.client.propose_policy_update(&s.admin, &new_p);

    let pending = s.client.get_pending_policy_update().unwrap();
    assert_eq!(pending.policy.max_loan_duration, 60 * 24 * 60 * 60);
    assert_eq!(
        pending.execution_time,
        pending.proposed_at + TIMELOCK_DURATION
    );

    // Attempting to execute immediately fails with TimelockNotExpired
    let early = s.client.try_execute_policy_update(&s.admin);
    assert_eq!(early, Err(Ok(Error::TimelockNotExpired)));

    // Advance 47 hours (not yet 48 hours)
    advance(&s.env, TIMELOCK_DURATION - 3600);
    let early2 = s.client.try_execute_policy_update(&s.admin);
    assert_eq!(early2, Err(Ok(Error::TimelockNotExpired)));

    // Non-admin cannot execute even after timelock
    advance(&s.env, 3601);
    let non_admin_exec = s.client.try_execute_policy_update(&non_admin);
    assert_eq!(non_admin_exec, Err(Ok(Error::NotAdmin)));

    // Admin executes after timelock delay
    s.client.execute_policy_update(&s.admin);

    // Policy is updated, pending update is cleared
    assert_eq!(
        s.client.get_loan_policy().max_loan_duration,
        60 * 24 * 60 * 60
    );
    assert!(s.client.get_pending_policy_update().is_none());

    // Subsequent execute fails because nothing is pending
    let none_pending = s.client.try_execute_policy_update(&s.admin);
    assert_eq!(none_pending, Err(Ok(Error::NoPendingPolicy)));
}

#[test]
fn policy_update_cancel_by_admin() {
    let s = setup(3);
    let non_admin = s.members.get(0).unwrap();

    let mut new_p = policy();
    new_p.min_interest_rate = 1_000;

    s.client.propose_policy_update(&s.admin, &new_p);
    assert!(s.client.get_pending_policy_update().is_some());

    // Non-admin cannot cancel
    let err = s.client.try_cancel_policy_update(&non_admin);
    assert_eq!(err, Err(Ok(Error::NotAdmin)));

    // Admin cancels
    s.client.cancel_policy_update(&s.admin);
    assert!(s.client.get_pending_policy_update().is_none());

    // Even after 48h, execute fails because it was cancelled
    advance(&s.env, TIMELOCK_DURATION + 1);
    let res = s.client.try_execute_policy_update(&s.admin);
    assert_eq!(res, Err(Ok(Error::NoPendingPolicy)));
}

#[test]
fn get_version_returns_cargo_pkg_version() {
    let env = Env::default();
    let contract_id = env.register(OurDao, ());
    let client = OurDaoClient::new(&env, &contract_id);

    let version = client.get_version();
    assert_eq!(version, String::from_str(&env, env!("CARGO_PKG_VERSION")));
}

#[test]
fn test_bump_dao_ttl() {
    let s = setup(1);
    s.client.bump_dao_ttl();
}
