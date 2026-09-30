#![cfg(test)]

extern crate std;
use std::println;

use soroban_sdk::testutils::{Address as _, Events as _, Ledger as _};
use soroban_sdk::xdr::{ContractEventBody, ScVal};
use soroban_sdk::{token, Address, Bytes, BytesN, Env, String, Vec};

use crate::privacy::compute_commitment;
use crate::storage::ProposalKind;
use crate::storage::TokenWhitelist;
use crate::admin::TIMELOCK_DURATION;
use crate::types::{LoanPolicy, LoanStatus, MemberStatus, ProposalPhase, ProposalStatus};
use crate::{Error, OurDao, OurDaoClient};

const FEE: i128 = 1_000;
const MINT: i128 = 1_000_000;
const EDITING: u64 = 3 * 24 * 60 * 60;
const VOTING_PERIOD: u64 = 3 * 24 * 60 * 60;
const LOAN_DURATION: u64 = 30 * 24 * 60 * 60;

#[soroban_sdk::contracttype]
#[derive(Clone)]
enum RejectingTokenKey {
    Balance(Address),
    RejectTransfers,
}

#[soroban_sdk::contract]
struct RejectingToken;

#[soroban_sdk::contractimpl]
impl RejectingToken {
    pub fn mint(env: Env, to: Address, amount: i128) {
        let key = RejectingTokenKey::Balance(to);
        let current: i128 = env.storage().instance().get(&key).unwrap_or(0);
        env.storage().instance().set(&key, &(current + amount));
    }

    pub fn set_reject_transfers(env: Env, reject: bool) {
        env.storage()
            .instance()
            .set(&RejectingTokenKey::RejectTransfers, &reject);
    }

    pub fn balance(env: Env, id: Address) -> i128 {
        env.storage()
            .instance()
            .get(&RejectingTokenKey::Balance(id))
            .unwrap_or(0)
    }

    pub fn transfer(env: Env, from: Address, to: Address, amount: i128) {
        let reject: bool = env
            .storage()
            .instance()
            .get(&RejectingTokenKey::RejectTransfers)
            .unwrap_or(false);
        if reject {
            panic!("mock token transfer rejected");
        }
        if amount < 0 {
            panic!("negative transfer");
        }

        let from_key = RejectingTokenKey::Balance(from);
        let to_key = RejectingTokenKey::Balance(to);
        let from_balance: i128 = env.storage().instance().get(&from_key).unwrap_or(0);
        if from_balance < amount {
            panic!("insufficient balance");
        }
        let to_balance: i128 = env.storage().instance().get(&to_key).unwrap_or(0);
        env.storage()
            .instance()
            .set(&from_key, &(from_balance - amount));
        env.storage().instance().set(&to_key, &(to_balance + amount));
    }
}

struct RejectingSetup<'a> {
    env: Env,
    client: OurDaoClient<'a>,
    token: RejectingTokenClient<'a>,
    members: Vec<Address>,
}

fn rejecting_setup(num_members: u32) -> RejectingSetup<'static> {
    let env = Env::default();
    env.mock_all_auths();

    let token_id = env.register(RejectingToken, ());
    let token = RejectingTokenClient::new(&env, &token_id);
    let admin = Address::generate(&env);
    let contract_id = env.register(OurDao, ());
    let client = OurDaoClient::new(&env, &contract_id);

    let mut admins = Vec::new(&env);
    admins.push_back(admin);
    client.initialize(&admins, &5_100u32, &FEE, &token_id, &policy());

    let mut members = Vec::new(&env);
    for _ in 0..num_members {
        let member = Address::generate(&env);
        token.mint(&member, &MINT);
        client.register_member(&member);
        members.push_back(member);
    }

    RejectingSetup {
        env,
        client,
        token,
        members,
    }
}

struct Setup<'a> {
    env: Env,
    client: OurDaoClient<'a>,
    token: token::Client<'a>,
    admin: Address,
    members: Vec<Address>,
}

fn policy() -> LoanPolicy {
    // placeholder
    LoanPolicy {
        min_membership_duration: 0,
        membership_contribution: FEE,
        max_loan_duration: 30 * 24 * 60 * 60,
        min_interest_rate: 500,   // 5%
        max_interest_rate: 2_000, // 20%
        cooldown_period: 0,
        max_loan_to_treasury_ratio: 5_000, // 50%
        default_grace_period: 0,
        default_penalty_bps: 2_000, // 20%
        editing_period: EDITING,
        voting_period: VOTING_PERIOD,
        treasury_threshold: 5_100, // 51%
        quorum_bps: 0,
    }
}

fn setup(num_members: u32) -> Setup<'static> {
    let env = Env::default();
    env.mock_all_auths();

    let token_admin = Address::generate(&env);
    let sac = env.register_stellar_asset_contract_v2(token_admin.clone());
    let token_id = sac.address();
    let token = token::Client::new(&env, &token_id);
    let token_mint = token::StellarAssetClient::new(&env, &token_id);

    let admin = Address::generate(&env);
    let contract_id = env.register(OurDao, ());
    let client = OurDaoClient::new(&env, &contract_id);

    let mut admins = Vec::new(&env);
    admins.push_back(admin.clone());
    client.initialize(&admins, &5_100u32, &FEE, &token_id, &policy());

    let mut members = Vec::new(&env);
    for _ in 0..num_members {
        let m = Address::generate(&env);
        token_mint.mint(&m, &MINT);
        client.register_member(&m);
        members.push_back(m);
    }

    Setup {
        env,
        client,
        token,
        admin,
        members,
    }
}

fn advance(env: &Env, secs: u64) {
    env.ledger().with_mut(|li| li.timestamp += secs);
}

// ---------------------------------------------------------------------------

#[test]
fn init_and_membership() {
    let s = setup(3);
    assert_eq!(s.client.get_total_members(), 3);
    assert_eq!(s.client.get_active_members(), 3);
    assert_eq!(s.client.get_treasury_balance(), 3 * FEE);
    assert!(s.client.is_member(&s.members.get(0).unwrap()));
    assert!(s.client.is_admin(&s.admin));
    assert_eq!(s.client.get_consensus_threshold(), 5_100);

    let m = s.client.get_member(&s.members.get(0).unwrap()).unwrap();
    assert_eq!(m.status, MemberStatus::ActiveMember);
    assert_eq!(m.contribution, FEE);
}

#[test]
fn double_join_rejected() {
    let s = setup(1);
    let m = s.members.get(0).unwrap();
    let res = s.client.try_register_member(&m);
    assert_eq!(res, Err(Ok(Error::AlreadyMember)));
}

#[test]
fn exit_returns_share() {
    let s = setup(2);
    let m = s.members.get(0).unwrap();
    let before = s.token.balance(&m);
    let share = s.client.calculate_exit_share(&m);
    assert!(share > 0);
    s.client.exit_dao(&m);
    assert_eq!(s.token.balance(&m), before + share);
    assert_eq!(s.client.get_active_members(), 1);
    assert!(!s.client.is_member(&m));
}

#[test]
fn full_loan_lifecycle() {
    let s = setup(3);
    let borrower = s.members.get(0).unwrap();
    let v1 = s.members.get(1).unwrap();
    let v2 = s.members.get(2).unwrap();

    let terms = s.client.calculate_loan_terms(&1_000);
    assert!(terms.interest_rate >= 500 && terms.interest_rate <= 2_000);

    let pid = s.client.request_loan(&borrower, &1_000, &None);
    let prop = s.client.get_loan_proposal(&pid).unwrap();
    assert_eq!(prop.total_repayment, terms.total_repayment);

    // Cannot vote during the editing phase.
    let early = s.client.try_vote_on_loan_proposal(&v1, &pid, &true);
    assert_eq!(early, Err(Ok(Error::NotInVotingPhase)));

    advance(&s.env, EDITING + 1);

    let treasury_before = s.client.get_treasury_balance();
    let bal_before = s.token.balance(&borrower);
    s.client.vote_on_loan_proposal(&v1, &pid, &true);
    s.client.vote_on_loan_proposal(&v2, &pid, &true); // 2/3 >= ceil(51%) => approved

    let prop = s.client.get_loan_proposal(&pid).unwrap();
    assert_eq!(prop.status, ProposalStatus::Approved);
    assert_eq!(s.token.balance(&borrower), bal_before + 1_000);
    assert_eq!(s.client.get_treasury_balance(), treasury_before - 1_000);

    let loan = s.client.get_loan(&0).unwrap();
    assert_eq!(loan.status, LoanStatus::Active);
    assert!(s.client.get_member(&borrower).unwrap().has_active_loan);

    // Repay and verify interest becomes claimable yield for active members.
    s.client.repay_loan(&borrower, &loan.id);
    let loan = s.client.get_loan(&0).unwrap();
    assert_eq!(loan.status, LoanStatus::Repaid);
    assert!(!s.client.get_member(&borrower).unwrap().has_active_loan);

    let interest = loan.total_repayment - loan.principal;
    let per = interest / 3;
    assert!(per > 0);
    assert_eq!(s.client.get_pending_yield(&v1), per);

    let claim_before = s.token.balance(&v1);
    let claimed = s.client.claim_rewards(&v1);
    assert_eq!(claimed, per);
    assert_eq!(s.token.balance(&v1), claim_before + per);
    assert_eq!(s.client.get_pending_yield(&v1), 0);
}

#[test]
fn loan_rejected_when_ineligible_active_loan() {
    let s = setup(3);
    let borrower = s.members.get(0).unwrap();
    let v1 = s.members.get(1).unwrap();
    let v2 = s.members.get(2).unwrap();

    let pid = s.client.request_loan(&borrower, &500, &None);
    advance(&s.env, EDITING + 1);
    s.client.vote_on_loan_proposal(&v1, &pid, &true);
    s.client.vote_on_loan_proposal(&v2, &pid, &true);

    // Borrower now has an active loan; a second request must fail.
    let res = s.client.try_request_loan(&borrower, &200, &None);
    assert_eq!(res, Err(Ok(Error::HasActiveLoan)));
}

#[test]
fn loan_exceeds_treasury_ratio() {
    let s = setup(3); // treasury = 3000, max ratio 50% => max loan 1500
    let borrower = s.members.get(0).unwrap();
    let res = s.client.try_request_loan(&borrower, &2_000, &None);
    assert_eq!(res, Err(Ok(Error::ExceedsTreasuryRatio)));
}

#[test]
fn loan_default_before_due_rejected() {
    let s = setup(3);
    let borrower = s.members.get(0).unwrap();
    let v1 = s.members.get(1).unwrap();
    let v2 = s.members.get(2).unwrap();

    let pid = s.client.request_loan(&borrower, &500, &None);
    advance(&s.env, EDITING + 1);
    s.client.vote_on_loan_proposal(&v1, &pid, &true);
    s.client.vote_on_loan_proposal(&v2, &pid, &true);

    // Loan is Active and not yet overdue.
    let res = s.client.try_mark_loan_defaulted(&0);
    assert_eq!(res, Err(Ok(Error::LoanNotOverdue)));

    // A loan that doesn't exist can't be defaulted either.
    let missing = s.client.try_mark_loan_defaulted(&99);
    assert_eq!(missing, Err(Ok(Error::LoanNotFound)));
}

#[test]
fn loan_default_applies_penalty_and_frees_borrower() {
    let s = setup(3);
    let borrower = s.members.get(0).unwrap();
    let v1 = s.members.get(1).unwrap();
    let v2 = s.members.get(2).unwrap();

    let pid = s.client.request_loan(&borrower, &500, &None);
    advance(&s.env, EDITING + 1);
    s.client.vote_on_loan_proposal(&v1, &pid, &true);
    s.client.vote_on_loan_proposal(&v2, &pid, &true);
    assert!(s.client.get_member(&borrower).unwrap().has_active_loan);

    advance(&s.env, LOAN_DURATION + 1); // past due_time, grace period is 0

    let contribution_before = s.client.get_member(&borrower).unwrap().contribution;
    s.client.mark_loan_defaulted(&0);

    let loan = s.client.get_loan(&0).unwrap();
    assert_eq!(loan.status, LoanStatus::Defaulted);

    let member = s.client.get_member(&borrower).unwrap();
    assert!(!member.has_active_loan);
    let expected_penalty = contribution_before * 2_000 / 10_000; // 20% per policy()
    assert_eq!(member.contribution, contribution_before - expected_penalty);
}

#[test]
fn defaulted_loan_is_terminal() {
    let s = setup(3);
    let borrower = s.members.get(0).unwrap();
    let v1 = s.members.get(1).unwrap();
    let v2 = s.members.get(2).unwrap();

    let pid = s.client.request_loan(&borrower, &500, &None);
    advance(&s.env, EDITING + 1);
    s.client.vote_on_loan_proposal(&v1, &pid, &true);
    s.client.vote_on_loan_proposal(&v2, &pid, &true);
    advance(&s.env, LOAN_DURATION + 1);
    s.client.mark_loan_defaulted(&0);

    // Can't default it twice.
    let again = s.client.try_mark_loan_defaulted(&0);
    assert_eq!(again, Err(Ok(Error::LoanNotActive)));

    // Can't repay a defaulted loan.
    let repay = s.client.try_repay_loan(&borrower, &0);
    assert_eq!(repay, Err(Ok(Error::LoanNotActive)));
}

#[test]
fn defaulted_borrower_can_exit() {
    let s = setup(3);
    let borrower = s.members.get(0).unwrap();
    let v1 = s.members.get(1).unwrap();
    let v2 = s.members.get(2).unwrap();

    let pid = s.client.request_loan(&borrower, &500, &None);
    advance(&s.env, EDITING + 1);
    s.client.vote_on_loan_proposal(&v1, &pid, &true);
    s.client.vote_on_loan_proposal(&v2, &pid, &true);
    advance(&s.env, LOAN_DURATION + 1);

    // Before default, has_active_loan blocks exit.
    let blocked = s.client.try_exit_dao(&borrower);
    assert_eq!(blocked, Err(Ok(Error::HasActiveLoan)));

    s.client.mark_loan_defaulted(&0);

    // After default, has_active_loan is cleared and exit succeeds (with the
    // already-slashed, reduced share).
    s.client.exit_dao(&borrower);
    assert!(!s.client.is_member(&borrower));
}

#[test]
fn loan_id_matches_its_originating_proposal_id() {
    let s = setup(3);
    let borrower = s.members.get(0).unwrap();
    let v1 = s.members.get(1).unwrap();
    let v2 = s.members.get(2).unwrap();

    // First proposal (id 0) never reaches approval — consumes a proposal id
    // without ever producing a loan, so a separate loan-id counter would lag
    // behind the proposal-id counter from here on.
    let pid0 = s.client.request_loan(&borrower, &200, &None);
    advance(&s.env, EDITING + 1);
    s.client.vote_on_loan_proposal(&v1, &pid0, &false);
    assert_eq!(
        s.client.get_loan_proposal(&pid0).unwrap().status,
        ProposalStatus::Pending
    );

    // Second proposal (id 1) is approved. Its loan must carry id 1 too, not
    // whatever a separate counter would have handed out (0).
    let pid1 = s.client.request_loan(&borrower, &300, &None);
    advance(&s.env, EDITING + 1);
    s.client.vote_on_loan_proposal(&v1, &pid1, &true);
    s.client.vote_on_loan_proposal(&v2, &pid1, &true);

    assert_eq!(pid1, 1);
    let loan = s.client.get_loan(&pid1).unwrap();
    assert_eq!(loan.id, pid1);
    assert_eq!(loan.borrower, borrower);
}

#[test]
fn treasury_withdrawal_open_vote() {
    let s = setup(3);
    let proposer = s.members.get(0).unwrap();
    let v1 = s.members.get(1).unwrap();
    let v2 = s.members.get(2).unwrap();
    let dest = Address::generate(&s.env);

    let reason = String::from_str(&s.env, "grant");
    let pid = s
        .client
        .propose_treasury_withdrawal(&proposer, &600, &dest, &reason, &false);

    s.client.vote_on_treasury_proposal(&v1, &pid, &true);
    let mid = s.client.get_treasury_proposal(&pid).unwrap();
    assert_eq!(mid.status, ProposalStatus::Pending); // 1 vote not enough (needs 2)

    s.client.vote_on_treasury_proposal(&v2, &pid, &true);
    let done = s.client.get_treasury_proposal(&pid).unwrap();
    assert_eq!(done.status, ProposalStatus::Executed);
    assert_eq!(s.token.balance(&dest), 600);
}

#[test]
fn staking_boosts_voting_weight() {
    let s = setup(2); // required for loan = ceil(2*51%) = 2
    let borrower = s.members.get(0).unwrap();
    let staker = s.members.get(1).unwrap();

    // Stake enough for +2 weight (200 / 100). One staked yes-vote = weight 3 >= 2.
    s.client.stake(&staker, &200);
    assert_eq!(s.client.get_stake(&staker), 200);

    let pid = s.client.request_loan(&borrower, &500, &None);
    advance(&s.env, EDITING + 1);
    s.client.vote_on_loan_proposal(&staker, &pid, &true);

    let prop = s.client.get_loan_proposal(&pid).unwrap();
    assert_eq!(prop.for_votes, 3);
    assert_eq!(prop.status, ProposalStatus::Approved);

    // Unstake returns tokens.
    let before = s.token.balance(&staker);
    s.client.unstake(&staker, &200);
    assert_eq!(s.token.balance(&staker), before + 200);
    assert_eq!(s.client.get_stake(&staker), 0);
}

#[test]
fn name_registry() {
    let s = setup(1);
    let owner = s.members.get(0).unwrap();
    let name = String::from_str(&s.env, "alice_dao");
    s.client.register_name(&owner, &name);
    assert_eq!(s.client.resolve_name(&name), Some(owner.clone()));
    assert_eq!(s.client.name_of(&owner), Some(name.clone()));

    // A different owner cannot claim the same name.
    let other = Address::generate(&s.env);
    let res = s.client.try_register_name(&other, &name);
    assert_eq!(res, Err(Ok(Error::NameTaken)));
}

#[test]
fn releasing_a_name_emits_an_event() {
    use soroban_sdk::testutils::Events as _;

    let s = setup(1);
    let owner = s.members.get(0).unwrap();
    let old = String::from_str(&s.env, "alice_dao");
    let new = String::from_str(&s.env, "alice_v2");

    s.client.register_name(&owner, &old);
    // First registration frees nothing: only `name_reg` is emitted.
    assert_eq!(s.env.events().all().events().len(), 1);

    // Re-registering under a new name releases the old one: `name_rel`
    // (old name, previous owner) is emitted alongside `name_reg` (#124).
    s.client.register_name(&owner, &new);
    assert_eq!(s.env.events().all().events().len(), 2);
    assert_eq!(s.client.resolve_name(&old), None);

    // Re-registering the same name releases nothing.
    s.client.register_name(&owner, &new);
    assert_eq!(s.env.events().all().events().len(), 1);
}

#[test]
fn commit_reveal_private_treasury_vote() {
    let s = setup(3);
    let proposer = s.members.get(0).unwrap();
    let v1 = s.members.get(1).unwrap();
    let v2 = s.members.get(2).unwrap();
    let dest = Address::generate(&s.env);

    let reason = String::from_str(&s.env, "secret grant");
    let pid = s
        .client
        .propose_treasury_withdrawal(&proposer, &600, &dest, &reason, &true);

    // Open voting is refused on a private proposal.
    let open = s.client.try_vote_on_treasury_proposal(&v1, &pid, &true);
    assert_eq!(open, Err(Ok(Error::NotAuthorized)));

    let salt1 = BytesN::from_array(&s.env, &[7u8; 32]);
    let salt2 = BytesN::from_array(&s.env, &[9u8; 32]);
    let c1 = compute_commitment(&s.env, true, &salt1);
    let c2 = compute_commitment(&s.env, true, &salt2);

    s.client.commit_treasury_vote(&v1, &pid, &c1);
    s.client.commit_treasury_vote(&v2, &pid, &c2);

    // A reveal that doesn't match the commitment is rejected.
    let bad = s.client.try_reveal_treasury_vote(&v1, &pid, &false, &salt1);
    assert_eq!(bad, Err(Ok(Error::CommitmentMismatch)));

    s.client.reveal_treasury_vote(&v1, &pid, &true, &salt1);
    assert_eq!(
        s.client.get_treasury_proposal(&pid).unwrap().status,
        ProposalStatus::Pending
    );
    s.client.reveal_treasury_vote(&v2, &pid, &true, &salt2);
    assert_eq!(
        s.client.get_treasury_proposal(&pid).unwrap().status,
        ProposalStatus::Executed
    );
    assert_eq!(s.token.balance(&dest), 600);
}

#[test]
fn commit_vote_cannot_be_overwritten() {
    let s = setup(2);
    let proposer = s.members.get(0).unwrap();
    let voter = s.members.get(1).unwrap();
    let dest = Address::generate(&s.env);
    let reason = String::from_str(&s.env, "secret grant");
    let pid = s
        .client
        .propose_treasury_withdrawal(&proposer, &600, &dest, &reason, &true);

    let salt = BytesN::from_array(&s.env, &[7u8; 32]);
    let other = BytesN::from_array(&s.env, &[9u8; 32]);
    let c1 = compute_commitment(&s.env, true, &salt);
    let c2 = compute_commitment(&s.env, false, &other);

    s.client.commit_treasury_vote(&voter, &pid, &c1);
    let again = s.client.try_commit_treasury_vote(&voter, &pid, &c2);
    assert_eq!(again, Err(Ok(Error::AlreadyVoted)));

    // The original commitment is still the one that reveals successfully.
    advance(&s.env, VOTING_PERIOD + 1);
    s.client.reveal_treasury_vote(&voter, &pid, &true, &salt);
}

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
fn content_hash_document() {
    let s = setup(1);
    let member = s.members.get(0).unwrap();
    let pid = s.client.request_loan(&member, &500, &None);

    let cid = Bytes::from_array(&s.env, b"QmExampleCid1234567890");
    s.client
        .attach_document(&member, &ProposalKind::Loan, &pid, &cid);
    assert_eq!(s.client.get_document(&ProposalKind::Loan, &pid), Some(cid));
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
    assert_eq!(res, Err(Ok(Error::Paused)), "register_member should be pause-gated");

    let res = s.client.try_exit_dao(&borrower);
    assert_eq!(res, Err(Ok(Error::Paused)), "exit_dao should be pause-gated");

    let res = s.client.try_claim_rewards(&borrower);
    assert_eq!(res, Err(Ok(Error::Paused)), "claim_rewards should be pause-gated");

    // ==================== Loans ====================
    let res = s.client.try_request_loan(&borrower, &1000, &None);
    assert_eq!(res, Err(Ok(Error::Paused)), "request_loan should be pause-gated");

    let res = s.client.try_edit_loan_proposal(&borrower, &loan_pid, &600);
    assert_eq!(res, Err(Ok(Error::Paused)), "edit_loan_proposal should be pause-gated");

    let res = s.client.try_vote_on_loan_proposal(&voter, &loan_pid, &true);
    assert_eq!(res, Err(Ok(Error::Paused)), "vote_on_loan_proposal should be pause-gated");

    let res = s.client.try_disburse_approved_loan(&loan_pid);
    assert_eq!(res, Err(Ok(Error::Paused)), "disburse_approved_loan should be pause-gated");

    let res = s.client.try_repay_loan(&borrower, &0);
    assert_eq!(res, Err(Ok(Error::Paused)), "repay_loan should be pause-gated");

    let res = s.client.try_repay_loan_partial(&borrower, &0, &100);
    assert_eq!(res, Err(Ok(Error::Paused)), "repay_loan_partial should be pause-gated");

    let res = s.client.try_mark_loan_defaulted(&0);
    assert_eq!(res, Err(Ok(Error::Paused)), "mark_loan_defaulted should be pause-gated");

    let res = s.client.try_expire_loan_proposal(&0);
    assert_eq!(res, Err(Ok(Error::Paused)), "expire_loan_proposal should be pause-gated");

    // ==================== Treasury ====================
    let res = s.client.try_propose_treasury_withdrawal(
        &borrower,
        &100,
        &newcomer,
        &String::from_slice(&s.env, "test"),
        &false,
    );
    assert_eq!(res, Err(Ok(Error::Paused)), "propose_treasury_withdrawal should be pause-gated");

    let res = s.client.try_vote_on_treasury_proposal(&voter, &treasury_pid, &true);
    assert_eq!(res, Err(Ok(Error::Paused)), "vote_on_treasury_proposal should be pause-gated");

    let res = s.client.try_expire_treasury_proposal(&0);
    assert_eq!(res, Err(Ok(Error::Paused)), "expire_treasury_proposal should be pause-gated");

    let res = s.client.try_execute_treasury_proposal(&0);
    assert_eq!(res, Err(Ok(Error::Paused)), "execute_treasury_proposal should be pause-gated");

    // ==================== Staking ====================
    let res = s.client.try_stake(&staker, &100);
    assert_eq!(res, Err(Ok(Error::Paused)), "stake should be pause-gated");

    let res = s.client.try_unstake(&staker, &100);
    assert_eq!(res, Err(Ok(Error::Paused)), "unstake should be pause-gated");

    // ==================== Registry ====================
    let res = s.client.try_register_name(&borrower, &String::from_slice(&s.env, "test"));
    assert_eq!(res, Err(Ok(Error::Paused)), "register_name should be pause-gated");

    // ==================== Privacy (commit-reveal voting) ====================
    let commitment = BytesN::from_array(&s.env, &[0u8; 32]);
    let res = s.client.try_commit_treasury_vote(&voter, &treasury_pid, &commitment);
    assert_eq!(res, Err(Ok(Error::Paused)), "commit_treasury_vote should be pause-gated");

    let salt = BytesN::from_array(&s.env, &[0u8; 32]);
    let res = s.client.try_reveal_treasury_vote(&voter, &treasury_pid, &true, &salt);
    assert_eq!(res, Err(Ok(Error::Paused)), "reveal_treasury_vote should be pause-gated");

    // ==================== Docs (content-hash metadata) ====================
    let cid = Bytes::from_slice(&s.env, &[1, 2, 3]);
    let res = s.client.try_attach_document(&borrower, &ProposalKind::Loan, &loan_pid, &cid);
    assert_eq!(res, Err(Ok(Error::Paused)), "attach_document should be pause-gated");

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

// ==================== issue #1: expire_loan_proposal ====================

#[test]
fn expired_proposal_shows_expired_phase_in_view() {
    let s = setup(3);
    let borrower = s.members.get(0).unwrap();
    let pid = s.client.request_loan(&borrower, &500, &None);

    // Before voting window expires, view returns the live phase.
    let prop = s.client.get_loan_proposal(&pid).unwrap();
    assert_eq!(prop.phase, ProposalPhase::Editing);

    advance(&s.env, EDITING + 1);
    let prop = s.client.get_loan_proposal(&pid).unwrap();
    assert_eq!(prop.phase, ProposalPhase::Voting);

    // After voting window passes without reaching quorum, view auto-expires.
    advance(&s.env, VOTING_PERIOD + 1);
    let prop = s.client.get_loan_proposal(&pid).unwrap();
    assert_eq!(prop.phase, ProposalPhase::Expired);
    assert_eq!(prop.status, ProposalStatus::Rejected);
}

#[test]
fn expire_before_deadline_rejected() {
    let s = setup(3);
    let borrower = s.members.get(0).unwrap();
    let pid = s.client.request_loan(&borrower, &500, &None);

    // Proposal is still in editing phase — not expired yet.
    let res = s.client.try_expire_loan_proposal(&pid);
    assert_eq!(res, Err(Ok(Error::ProposalNotExpired)));
}

#[test]
fn double_expire_is_noop() {
    let s = setup(3);
    let borrower = s.members.get(0).unwrap();
    let pid = s.client.request_loan(&borrower, &500, &None);

    advance(&s.env, EDITING + VOTING_PERIOD + 2);

    // First call persists the expired state.
    s.client.expire_loan_proposal(&pid);
    let prop = s.client.get_loan_proposal(&pid).unwrap();
    assert_eq!(prop.phase, ProposalPhase::Expired);

    // Second call is a no-op (already persisted).
    s.client.expire_loan_proposal(&pid);
    let prop = s.client.get_loan_proposal(&pid).unwrap();
    assert_eq!(prop.phase, ProposalPhase::Expired);
}

// ==================== issue #2: has_voted view ====================

#[test]
fn has_voted_loan_before_and_after() {
    let s = setup(3);
    let borrower = s.members.get(0).unwrap();
    let v1 = s.members.get(1).unwrap();
    let pid = s.client.request_loan(&borrower, &500, &None);
    advance(&s.env, EDITING + 1);

    assert!(!s.client.has_voted(&ProposalKind::Loan, &pid, &v1));
    s.client.vote_on_loan_proposal(&v1, &pid, &true);
    assert!(s.client.has_voted(&ProposalKind::Loan, &pid, &v1));
}

#[test]
fn has_voted_treasury_commit_reveal() {
    let s = setup(3);
    let proposer = s.members.get(0).unwrap();
    let v1 = s.members.get(1).unwrap();
    let dest = Address::generate(&s.env);
    let reason = String::from_str(&s.env, "grant");

    let pid = s
        .client
        .propose_treasury_withdrawal(&proposer, &100, &dest, &reason, &true);

    // Before commit: not voted.
    assert!(!s.client.has_voted(&ProposalKind::Treasury, &pid, &v1));

    // After commit: voted (commitment counts as a vote for dedup).
    let salt = BytesN::from_array(&s.env, &[1u8; 32]);
    let commitment = compute_commitment(&s.env, true, &salt);
    s.client.commit_treasury_vote(&v1, &pid, &commitment);
    assert!(s.client.has_voted(&ProposalKind::Treasury, &pid, &v1));
}

// ==================== issue #3: name validation ====================

#[test]
fn name_too_short_rejected() {
    let s = setup(1);
    let owner = s.members.get(0).unwrap();
    let name = String::from_str(&s.env, "ab");
    let res = s.client.try_register_name(&owner, &name);
    assert_eq!(res, Err(Ok(Error::InvalidName)));
}

#[test]
fn name_too_long_rejected() {
    let s = setup(1);
    let owner = s.members.get(0).unwrap();
    // 33 characters.
    let name = String::from_str(&s.env, "abcdefghijklmnopqrstuvwxyz1234567");
    let res = s.client.try_register_name(&owner, &name);
    assert_eq!(res, Err(Ok(Error::InvalidName)));
}

#[test]
fn name_uppercase_rejected() {
    let s = setup(1);
    let owner = s.members.get(0).unwrap();
    let name = String::from_str(&s.env, "Alice_dao");
    let res = s.client.try_register_name(&owner, &name);
    assert_eq!(res, Err(Ok(Error::InvalidName)));
}

#[test]
fn name_dot_or_space_rejected() {
    let s = setup(1);
    let owner = s.members.get(0).unwrap();

    let dot = String::from_str(&s.env, "alice.dao");
    let res = s.client.try_register_name(&owner, &dot);
    assert_eq!(res, Err(Ok(Error::InvalidName)));

    let space = String::from_str(&s.env, "alice dao");
    let res = s.client.try_register_name(&owner, &space);
    assert_eq!(res, Err(Ok(Error::InvalidName)));
}

#[test]
fn name_leading_trailing_separator_rejected() {
    let s = setup(1);
    let owner = s.members.get(0).unwrap();

    let lead = String::from_str(&s.env, "-alice");
    let res = s.client.try_register_name(&owner, &lead);
    assert_eq!(res, Err(Ok(Error::InvalidName)));

    let trail = String::from_str(&s.env, "alice-");
    let res = s.client.try_register_name(&owner, &trail);
    assert_eq!(res, Err(Ok(Error::InvalidName)));

    let lead2 = String::from_str(&s.env, "_alice");
    let res = s.client.try_register_name(&owner, &lead2);
    assert_eq!(res, Err(Ok(Error::InvalidName)));

    let trail2 = String::from_str(&s.env, "alice_");
    let res = s.client.try_register_name(&owner, &trail2);
    assert_eq!(res, Err(Ok(Error::InvalidName)));
}

#[test]
fn name_valid_with_digits_and_separators() {
    let s = setup(1);
    let owner = s.members.get(0).unwrap();
    let name = String::from_str(&s.env, "alice-123_dao");
    s.client.register_name(&owner, &name);
    assert_eq!(s.client.resolve_name(&name), Some(owner.clone()));
}

// ==================== issue #4: yield accumulator ====================

#[test]
fn yield_accumulator_join_claim_exit_rejoin() {
    let s = setup(3);
    let borrower = s.members.get(0).unwrap();
    let v1 = s.members.get(1).unwrap();
    let v2 = s.members.get(2).unwrap();

    // Get a loan approved and repaid so interest is distributed.
    let pid = s.client.request_loan(&borrower, &1_000, &None);
    advance(&s.env, EDITING + 1);
    s.client.vote_on_loan_proposal(&v1, &pid, &true);
    s.client.vote_on_loan_proposal(&v2, &pid, &true);
    s.client.repay_loan(&borrower, &0);

    let loan = s.client.get_loan(&0).unwrap();
    let interest = loan.total_repayment - loan.principal;
    let per_member = interest / 3;

    // Both non-borrower members can claim their share.
    assert_eq!(s.client.get_pending_yield(&v1), per_member);
    assert_eq!(s.client.get_pending_yield(&v2), per_member);
    s.client.claim_rewards(&v1);
    assert_eq!(s.client.get_pending_yield(&v1), 0);

    // v1 exits — their snapshot is settled so they don't double-claim.
    s.client.exit_dao(&v1);
    assert_eq!(s.client.get_pending_yield(&v1), 0);

    // v1 rejoins — snapshot is set to current accumulator, earning nothing
    // from interest that accrued before they rejoined.
    s.client.register_member(&v1);
    assert_eq!(s.client.get_pending_yield(&v1), 0);
}

// ==================== issue #5: partial loan repayment ====================

#[test]
fn partial_repayment_then_full() {
    let s = setup(3);
    let borrower = s.members.get(0).unwrap();
    let v1 = s.members.get(1).unwrap();
    let v2 = s.members.get(2).unwrap();

    let pid = s.client.request_loan(&borrower, &1_000, &None);
    advance(&s.env, EDITING + 1);
    s.client.vote_on_loan_proposal(&v1, &pid, &true);
    s.client.vote_on_loan_proposal(&v2, &pid, &true);

    let loan = s.client.get_loan(&pid).unwrap();
    let outstanding = loan.total_repayment - loan.amount_repaid;
    let half = outstanding / 2;
    assert!(half > 0);

    let bal_before = s.token.balance(&borrower);
    s.client.repay_loan_partial(&borrower, &pid, &half);

    let loan = s.client.get_loan(&pid).unwrap();
    assert_eq!(loan.status, LoanStatus::Active);
    assert_eq!(loan.amount_repaid, half);
    assert_eq!(s.token.balance(&borrower), bal_before - half);
    assert!(s.client.get_member(&borrower).unwrap().has_active_loan);

    // Interest-first: whatever portion of `half` falls inside the loan's
    // total interest is claimable right away, exactly as a full repayment.
    let interest_total = loan.total_repayment - loan.principal;
    let interest_component = half.min(interest_total);
    let per = interest_component / 3;
    assert_eq!(s.client.get_pending_yield(&v1), per);

    // Repaying the rest via the full-repayment entrypoint clears the loan.
    s.client.repay_loan(&borrower, &pid);
    let loan = s.client.get_loan(&pid).unwrap();
    assert_eq!(loan.status, LoanStatus::Repaid);
    assert_eq!(loan.amount_repaid, loan.total_repayment);
    assert!(!s.client.get_member(&borrower).unwrap().has_active_loan);
}

#[test]
fn partial_repayment_overpay_rejected() {
    let s = setup(3);
    let borrower = s.members.get(0).unwrap();
    let v1 = s.members.get(1).unwrap();
    let v2 = s.members.get(2).unwrap();

    let pid = s.client.request_loan(&borrower, &500, &None);
    advance(&s.env, EDITING + 1);
    s.client.vote_on_loan_proposal(&v1, &pid, &true);
    s.client.vote_on_loan_proposal(&v2, &pid, &true);

    let loan = s.client.get_loan(&pid).unwrap();
    let outstanding = loan.total_repayment - loan.amount_repaid;
    let bal_before = s.token.balance(&borrower);

    let over = s
        .client
        .try_repay_loan_partial(&borrower, &pid, &(outstanding + 1));
    assert_eq!(over, Err(Ok(Error::InvalidAmount)));

    let zero = s.client.try_repay_loan_partial(&borrower, &pid, &0);
    assert_eq!(zero, Err(Ok(Error::InvalidAmount)));

    let negative = s.client.try_repay_loan_partial(&borrower, &pid, &-10);
    assert_eq!(negative, Err(Ok(Error::InvalidAmount)));

    // None of the rejected attempts moved any funds or touched the loan.
    assert_eq!(s.token.balance(&borrower), bal_before);
    assert_eq!(s.client.get_loan(&pid).unwrap().amount_repaid, 0);
}

#[test]
fn partial_repayments_sum_to_exact_total_marks_repaid() {
    let s = setup(3);
    let borrower = s.members.get(0).unwrap();
    let v1 = s.members.get(1).unwrap();
    let v2 = s.members.get(2).unwrap();

    let pid = s.client.request_loan(&borrower, &1_000, &None);
    advance(&s.env, EDITING + 1);
    s.client.vote_on_loan_proposal(&v1, &pid, &true);
    s.client.vote_on_loan_proposal(&v2, &pid, &true);

    let total = s.client.get_loan(&pid).unwrap().total_repayment;
    let a = total / 3;
    let b = total / 3;
    let c = total - a - b; // remainder, so a + b + c == total exactly

    s.client.repay_loan_partial(&borrower, &pid, &a);
    assert_eq!(s.client.get_loan(&pid).unwrap().status, LoanStatus::Active);
    s.client.repay_loan_partial(&borrower, &pid, &b);
    assert_eq!(s.client.get_loan(&pid).unwrap().status, LoanStatus::Active);
    s.client.repay_loan_partial(&borrower, &pid, &c);

    let loan = s.client.get_loan(&pid).unwrap();
    assert_eq!(loan.status, LoanStatus::Repaid);
    assert_eq!(loan.amount_repaid, total);
    assert!(!s.client.get_member(&borrower).unwrap().has_active_loan);
}

#[test]
fn partial_payment_overdue_loan_still_defaultable() {
    let s = setup(3);
    let borrower = s.members.get(0).unwrap();
    let v1 = s.members.get(1).unwrap();
    let v2 = s.members.get(2).unwrap();

    let pid = s.client.request_loan(&borrower, &500, &None);
    advance(&s.env, EDITING + 1);
    s.client.vote_on_loan_proposal(&v1, &pid, &true);
    s.client.vote_on_loan_proposal(&v2, &pid, &true);

    let outstanding = s.client.get_loan(&pid).unwrap().total_repayment;
    let partial = outstanding / 4;
    assert!(partial > 0);
    s.client.repay_loan_partial(&borrower, &pid, &partial);

    advance(&s.env, LOAN_DURATION + 1); // past due_time, grace period is 0

    let contribution_before = s.client.get_member(&borrower).unwrap().contribution;
    s.client.mark_loan_defaulted(&pid);

    let loan = s.client.get_loan(&pid).unwrap();
    assert_eq!(loan.status, LoanStatus::Defaulted);
    let member = s.client.get_member(&borrower).unwrap();
    assert!(!member.has_active_loan);
    let expected_penalty = contribution_before * 2_000 / 10_000; // 20% per policy()
    assert_eq!(member.contribution, contribution_before - expected_penalty);

    // Defaulted loans are terminal — no further repayment, partial or full.
    let res = s.client.try_repay_loan_partial(&borrower, &pid, &1);
    assert_eq!(res, Err(Ok(Error::LoanNotActive)));
}

#[test]
fn exit_blocked_while_partial_balance_remains() {
    let s = setup(3);
    let borrower = s.members.get(0).unwrap();
    let v1 = s.members.get(1).unwrap();
    let v2 = s.members.get(2).unwrap();

    let pid = s.client.request_loan(&borrower, &500, &None);
    advance(&s.env, EDITING + 1);
    s.client.vote_on_loan_proposal(&v1, &pid, &true);
    s.client.vote_on_loan_proposal(&v2, &pid, &true);

    let outstanding = s.client.get_loan(&pid).unwrap().total_repayment;
    let partial = outstanding / 2;
    assert!(partial > 0 && partial < outstanding);
    s.client.repay_loan_partial(&borrower, &pid, &partial);

    let blocked = s.client.try_exit_dao(&borrower);
    assert_eq!(blocked, Err(Ok(Error::HasActiveLoan)));

    s.client.repay_loan(&borrower, &pid);
    s.client.exit_dao(&borrower);
    assert!(!s.client.is_member(&borrower));
}

// ==================== issue: total_contributions ====================

#[test]
fn ten_join_five_exit_pays_remaining_members_full_share() {
    let s = setup(10);

    for i in 0..5u32 {
        let m = s.members.get(i).unwrap();
        assert_eq!(s.client.calculate_exit_share(&m), FEE);
        s.client.exit_dao(&m);
    }

    assert_eq!(s.client.get_total_members(), 10);
    assert_eq!(s.client.get_active_members(), 5);
    assert_eq!(s.client.get_treasury_balance(), 5 * FEE);

    for i in 5..10u32 {
        let m = s.members.get(i).unwrap();
        assert_eq!(s.client.calculate_exit_share(&m), FEE);
    }
}

#[test]
fn rejoin_after_exit_is_counted_once_not_twice() {
    let s = setup(3);
    let m = s.members.get(0).unwrap();

    s.client.exit_dao(&m);
    assert_eq!(s.client.get_active_members(), 2);

    s.client.register_member(&m);
    assert_eq!(s.client.get_total_members(), 4);
    assert_eq!(s.client.get_active_members(), 3);
    assert_eq!(s.client.calculate_exit_share(&m), FEE);
}

#[test]
fn mixed_joins_exits_defaults_never_strand_value() {
    let s = setup(3);
    let a = s.members.get(0).unwrap();
    let b = s.members.get(1).unwrap();
    let c = s.members.get(2).unwrap();

    let pid = s.client.request_loan(&a, &500, &None);
    advance(&s.env, EDITING + 1);
    s.client.vote_on_loan_proposal(&b, &pid, &true);
    s.client.vote_on_loan_proposal(&c, &pid, &true);

    s.client.exit_dao(&c);
    let b_share_before_default = s.client.calculate_exit_share(&b);

    advance(&s.env, LOAN_DURATION + 1);
    s.client.mark_loan_defaulted(&pid);

    let b_share_after_default = s.client.calculate_exit_share(&b);
    assert!(b_share_after_default > b_share_before_default);

    s.client.exit_dao(&b);
    s.client.register_member(&c);

    assert_eq!(s.client.get_total_members(), 4);
    assert_eq!(s.client.get_active_members(), 2);

    let mut sum = 0i128;
    for i in 0..3u32 {
        let m = s.members.get(i).unwrap();
        if s.client.is_member(&m) {
            sum += s.client.calculate_exit_share(&m);
        }
    }
    assert!(sum <= s.client.get_treasury_balance());
}

// ==================== issue #7: property tests ====================
//
// Example tests above pin down behavior at specific, hand-picked numbers.
// These instead generate wide ranges of inputs (amounts, treasury sizes,
// stakes, contributions — including near-`i128::MAX` boundaries) and check
// invariants that must hold for *any* input, not just the ones a human
// happened to write down. `amount` inputs are capped at `AMOUNT_BOUND`
// (rather than the full `i128` range) specifically to stay clear of the
// *intermediate* overflow in `amount * BASIS_POINTS` — the invariants below
// are about the post-clamp behavior of these functions, not about auditing
// every arithmetic op in isolation.
mod proptests {
    use super::*;
    use proptest::prelude::*;

    /// Keeps `amount * BASIS_POINTS` (BASIS_POINTS == 10_000) well clear of
    /// i128 overflow while still exercising values many orders of magnitude
    /// larger than any real loan or treasury.
    const AMOUNT_BOUND: i128 = i128::MAX / 20_000;

    fn contract_with_treasury(treasury: i128) -> (Env, OurDaoClient<'static>) {
        let env = Env::default();
        env.mock_all_auths();
        let token_admin = Address::generate(&env);
        let sac = env.register_stellar_asset_contract_v2(token_admin);
        let token_id = sac.address();
        let token_mint = token::StellarAssetClient::new(&env, &token_id);

        let admin = Address::generate(&env);
        let contract_id = env.register(OurDao, ());
        let client = OurDaoClient::new(&env, &contract_id);
        let mut admins = Vec::new(&env);
        admins.push_back(admin);
        client.initialize(&admins, &5_100u32, &FEE, &token_id, &policy());

        if treasury > 0 {
            token_mint.mint(&contract_id, &treasury);
        }
        (env, client)
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(40))]

        /// `calculate_loan_terms`'s rate is a linear curve clamped to
        /// `[min_interest_rate, max_interest_rate]` — no amount or treasury
        /// size should ever push it outside that band, and the quoted
        /// repayment should never be less than the amount requested.
        #[test]
        fn loan_terms_rate_stays_within_policy_bounds(
            amount in 0i128..=AMOUNT_BOUND,
            treasury in 0i128..=AMOUNT_BOUND,
        ) {
            let (_env, client) = contract_with_treasury(treasury);
            let terms = client.calculate_loan_terms(&amount);
            let p = policy();
            prop_assert!(terms.interest_rate >= p.min_interest_rate);
            prop_assert!(terms.interest_rate <= p.max_interest_rate);
            prop_assert!(terms.total_repayment >= amount);
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(80))]

        /// One base vote plus up to `MAX_STAKE_BONUS` (5) bonus votes at
        /// `STAKE_WEIGHT_UNIT` (100) tokens per bonus vote — so weight is
        /// always in `[1, 6]` for any non-negative stake.
        #[test]
        fn voting_weight_stays_in_bounds(stake in 0i128..=i128::MAX) {
            let env = Env::default();
            let contract_id = env.register(OurDao, ());
            let who = Address::generate(&env);
            let weight = env.as_contract(&contract_id, || {
                crate::storage::set_stake(&env, &who, stake);
                crate::util::voting_weight(&env, &who)
            });
            prop_assert!((1..=6).contains(&weight));
        }

        /// More stake never costs voting weight.
        #[test]
        fn voting_weight_is_monotonic_in_stake(
            a in 0i128..=i128::MAX,
            delta in 0i128..=i128::MAX,
        ) {
            let b = a.saturating_add(delta);
            let env = Env::default();
            let contract_id = env.register(OurDao, ());
            let who = Address::generate(&env);
            let (wa, wb) = env.as_contract(&contract_id, || {
                crate::storage::set_stake(&env, &who, a);
                let wa = crate::util::voting_weight(&env, &who);
                crate::storage::set_stake(&env, &who, b);
                let wb = crate::util::voting_weight(&env, &who);
                (wa, wb)
            });
            prop_assert!(wb >= wa);
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(32))]

        /// The pull-based accumulator splits `interest` equally across
        /// `active` members: every member ends up with exactly
        /// `interest / active` claimable, the sum never exceeds `interest`,
        /// and whatever isn't evenly divisible (`interest % active`) is
        /// exactly what's left retained by the treasury.
        #[test]
        fn distributed_interest_never_exceeds_collected(
            active in 1u32..=8,
            interest in 0i128..=i128::MAX,
        ) {
            let (env, client) = contract_with_treasury(0);
            let contract_id = client.address.clone();

            let mut members = Vec::new(&env);
            for _ in 0..active {
                let m = Address::generate(&env);
                let sac = token::StellarAssetClient::new(&env, &client.get_token());
                sac.mint(&m, &FEE);
                client.register_member(&m);
                members.push_back(m);
            }

            env.as_contract(&contract_id, || {
                crate::loans::distribute_interest(&env, interest);
            });

            let per_member = interest / active as i128;
            let mut sum = 0i128;
            for m in members.iter() {
                let pending = client.get_pending_yield(&m);
                prop_assert_eq!(pending, per_member);
                sum += pending;
            }
            prop_assert!(sum <= interest);
            prop_assert_eq!(interest - sum, interest % active as i128);
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(20))]

        /// `calculate_exit_share` is a pro-rata slice of the treasury.
        /// However a member's `contribution` got to its current value
        /// (join at the fee, then possibly reduced by default slashing —
        /// see issue's rejoin-after-exit note), each member's share is
        /// bounded by their contribution's fraction of the total, so the
        /// sum across every member can never exceed the treasury itself.
        #[test]
        fn exit_shares_never_exceed_treasury(
            contributions in prop::collection::vec(0i128..=FEE, 1..=5),
            extra_treasury in 0i128..=AMOUNT_BOUND,
        ) {
            let (env, client) = contract_with_treasury(extra_treasury);
            let contract_id = client.address.clone();
            let token_id = client.get_token();

            let mut members = Vec::new(&env);
            for &c in contributions.iter() {
                let m = Address::generate(&env);
                let sac = token::StellarAssetClient::new(&env, &token_id);
                sac.mint(&m, &FEE);
                client.register_member(&m);
                env.as_contract(&contract_id, || {
                    let mut rec = crate::storage::get_member(&env, &m).unwrap();
                    rec.contribution = c;
                    crate::storage::set_member(&env, &rec);
                });
                members.push_back(m);
            }

            let treasury = client.get_treasury_balance();
            let sum: i128 = members.iter().map(|m| client.calculate_exit_share(&m)).sum();
            prop_assert!(sum <= treasury);
        }
    }
}

#[test]
fn bench_exit_dao_scaling() {
    // #138: Measure cost of exit_dao at different sizes
    for size in [10, 100, 1000] {
        let s = setup(size);
        let m = s.members.get(0).unwrap();
        
        s.env.budget().reset_unlimited();
        let cpu_before = s.env.budget().cpu_instruction_cost();
        s.client.exit_dao(&m);
        let cpu_after = s.env.budget().cpu_instruction_cost();
        
        let cost = cpu_after - cpu_before;

        // Fail if cost scales poorly (O(n) check). Keep this no_std-compatible
        // instead of printing from the contract crate.
        assert!(
            size != 1000 || cost <= 50_000_000,
            "exit_dao cost exceeds O(1) bound"
        );
    }
}

// ---------------------------------------------------------------------------
// Approved-but-unfundable path (#108): a proposal passes its vote while the
// treasury can't cover it, so it parks in ApprovedPendingDisbursement and
// emits `loan_wait` / `tre_wait` instead of paying out.
// ---------------------------------------------------------------------------

/// True if any event from the most recent invocation has `name` as its first topic.
fn emitted(env: &Env, name: &str) -> bool {
    env.events().all().events().iter().any(|e| {
        let ContractEventBody::V0(body) = &e.body;
        matches!(body.topics.first(), Some(ScVal::Symbol(sym)) if sym.0.to_utf8_string_lossy() == name)
    })
}

/// Join a new member (mints their fee first), growing the treasury by FEE.
fn refill_treasury(s: &Setup) {
    let m = Address::generate(&s.env);
    token::StellarAssetClient::new(&s.env, &s.token.address).mint(&m, &MINT);
    s.client.register_member(&m);
}

#[test]
fn approved_but_unfundable_loan_waits_then_disburses_after_refill() {
    // 4 members => treasury 4000, max loan 2000 (50%), 3 of 4 votes required.
    let s = setup(4);
    let borrower = s.members.get(0).unwrap();
    let v1 = s.members.get(1).unwrap();
    let v2 = s.members.get(2).unwrap();
    let v3 = s.members.get(3).unwrap();

    // Request passes the pre-vote ratio check against the full treasury...
    let pid = s.client.request_loan(&borrower, &1_500, &None);

    // ...then the treasury is drained by a passed withdrawal before the loan vote closes.
    let dest = Address::generate(&s.env);
    let reason = String::from_str(&s.env, "drain");
    let tid = s
        .client
        .propose_treasury_withdrawal(&v1, &3_000, &dest, &reason, &false);
    s.client.vote_on_treasury_proposal(&v1, &tid, &true);
    s.client.vote_on_treasury_proposal(&v2, &tid, &true);
    s.client.vote_on_treasury_proposal(&v3, &tid, &true);
    assert_eq!(s.client.get_treasury_balance(), 1_000);

    advance(&s.env, EDITING + 1);
    s.client.vote_on_loan_proposal(&v1, &pid, &true);
    s.client.vote_on_loan_proposal(&v2, &pid, &true);
    let borrower_bal = s.token.balance(&borrower);
    s.client.vote_on_loan_proposal(&v3, &pid, &true); // reaches the threshold; treasury (1000) < 1500

    let prop = s.client.get_loan_proposal(&pid).unwrap();
    assert_eq!(prop.status, ProposalStatus::ApprovedPendingDisbursement);
    assert!(emitted(&s.env, "loan_wait"));
    // No tokens moved, and no loan/active-loan state was created.
    assert_eq!(s.token.balance(&borrower), borrower_bal);
    assert_eq!(s.client.get_treasury_balance(), 1_000);
    assert!(s.client.get_loan(&pid).is_none());
    assert!(!s.client.get_member(&borrower).unwrap().has_active_loan);

    // Still unfundable until the treasury is refilled.
    assert_eq!(
        s.client.try_disburse_approved_loan(&pid),
        Err(Ok(Error::InsufficientTreasury))
    );

    // Refill, then the very same proposal can be disbursed.
    refill_treasury(&s);
    refill_treasury(&s);
    assert!(s.client.get_treasury_balance() >= 1_500);
    s.client.disburse_approved_loan(&pid);

    let prop = s.client.get_loan_proposal(&pid).unwrap();
    assert_eq!(prop.status, ProposalStatus::Approved);
    assert_eq!(s.token.balance(&borrower), borrower_bal + 1_500);
    assert!(s.client.get_member(&borrower).unwrap().has_active_loan);
    assert_eq!(s.client.get_loan(&pid).unwrap().status, LoanStatus::Active);
}

#[test]
fn approved_but_unfundable_treasury_withdrawal_waits_then_executes_after_refill() {
    // 4 members => treasury 4000, 3 of 4 votes required.
    let s = setup(4);
    let p = s.members.get(0).unwrap();
    let v1 = s.members.get(1).unwrap();
    let v2 = s.members.get(2).unwrap();
    let dest = Address::generate(&s.env);
    let reason = String::from_str(&s.env, "grant");

    // Two withdrawals that each fit today but not both together.
    let first = s
        .client
        .propose_treasury_withdrawal(&p, &2_000, &dest, &reason, &false);
    let second = s
        .client
        .propose_treasury_withdrawal(&p, &2_500, &dest, &reason, &false);

    for voter in [&p, &v1, &v2] {
        s.client.vote_on_treasury_proposal(voter, &first, &true);
    }
    assert_eq!(s.client.get_treasury_balance(), 2_000);
    assert_eq!(s.token.balance(&dest), 2_000);

    s.client.vote_on_treasury_proposal(&p, &second, &true);
    s.client.vote_on_treasury_proposal(&v1, &second, &true);
    s.client.vote_on_treasury_proposal(&v2, &second, &true); // approved, but only 2000 left

    let prop = s.client.get_treasury_proposal(&second).unwrap();
    assert_eq!(prop.status, ProposalStatus::ApprovedPendingDisbursement);
    assert!(emitted(&s.env, "tre_wait"));
    assert_eq!(s.token.balance(&dest), 2_000); // nothing more moved
    assert_eq!(s.client.get_treasury_balance(), 2_000);
    assert_eq!(
        s.client.try_execute_treasury_proposal(&second),
        Err(Ok(Error::InsufficientTreasury))
    );

    refill_treasury(&s);
    refill_treasury(&s);
    s.client.execute_treasury_proposal(&second);

    let prop = s.client.get_treasury_proposal(&second).unwrap();
    assert_eq!(prop.status, ProposalStatus::Executed);
    assert_eq!(s.token.balance(&dest), 4_500);
}

// initialize token validation (#115)
// ---------------------------------------------------------------------------

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
    assert_eq!(init_with_token(&env, &not_a_contract), Err(Error::InvalidToken));
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

// ===========================================================================
// Issue #191: Configurable quorum threshold parameter in LoanPolicy
// ===========================================================================

#[test]
fn validate_policy_rejects_quorum_bps_above_basis_points() {
    let mut p = policy();
    p.quorum_bps = 10_001; // > 10_000 BASIS_POINTS
    let s = setup(1);
    let res = s.client.try_propose_policy_update(&s.admin, &p);
    assert_eq!(res, Err(Ok(Error::InvalidLoanPolicy)));
}

#[test]
fn loan_proposal_uses_dynamic_quorum_threshold() {
    // 4 members. Threshold set to 5100 (needs 3 votes normally).
    // If policy has quorum_bps = 2500 (needs ceil(4 * 25%) = 1 vote), 1 vote is enough to approve!
    let s = setup(4);
    let borrower = s.members.get(0).unwrap();
    let v1 = s.members.get(1).unwrap();

    let mut low_quorum_policy = policy();
    low_quorum_policy.quorum_bps = 2_500; // 25% => 1 vote required for 4 members

    // Propose and execute policy update after timelock
    s.client.propose_policy_update(&s.admin, &low_quorum_policy);
    advance(&s.env, TIMELOCK_DURATION + 1);
    s.client.execute_policy_update(&s.admin);
    assert_eq!(s.client.get_loan_policy().quorum_bps, 2_500);

    let pid = s.client.request_loan(&borrower, &500, &None);
    advance(&s.env, EDITING + 1);

    // Single vote is enough with 25% quorum!
    s.client.vote_on_loan_proposal(&v1, &pid, &true);
    let prop = s.client.get_loan_proposal(&pid).unwrap();
    assert_eq!(prop.status, ProposalStatus::Approved);
}

#[test]
fn loan_proposal_quorum_higher_threshold_requires_more_votes() {
    // 4 members. Threshold set to 5100 (3 votes).
    // If policy has quorum_bps = 10_000 (needs ceil(4 * 100%) = 4 votes).
    let s = setup(4);
    let borrower = s.members.get(0).unwrap();
    let v1 = s.members.get(1).unwrap();
    let v2 = s.members.get(2).unwrap();
    let v3 = s.members.get(3).unwrap();

    let mut high_quorum_policy = policy();
    high_quorum_policy.quorum_bps = 10_000; // 100% => all 4 votes required

    s.client.propose_policy_update(&s.admin, &high_quorum_policy);
    advance(&s.env, TIMELOCK_DURATION + 1);
    s.client.execute_policy_update(&s.admin);

    let pid = s.client.request_loan(&borrower, &500, &None);
    advance(&s.env, EDITING + 1);

    // 2 votes was enough with 51% (for 3 members), but for 4 members with 100% quorum, 2 votes is still pending.
    s.client.vote_on_loan_proposal(&v1, &pid, &true);
    s.client.vote_on_loan_proposal(&v2, &pid, &true);
    assert_eq!(
        s.client.get_loan_proposal(&pid).unwrap().status,
        ProposalStatus::Pending
    );

    s.client.vote_on_loan_proposal(&v3, &pid, &true);
    assert_eq!(
        s.client.get_loan_proposal(&pid).unwrap().status,
        ProposalStatus::Pending
    );

    // Borrower votes yes (4th vote) => 4/4 approved!
    s.client.vote_on_loan_proposal(&borrower, &pid, &true);
    assert_eq!(
        s.client.get_loan_proposal(&pid).unwrap().status,
        ProposalStatus::Approved
#[test]
fn rejected_register_member_transfer_rolls_back_all_membership_state() {
    let s = rejecting_setup(0);
    let member = Address::generate(&s.env);
    s.token.mint(&member, &MINT);
    s.token.set_reject_transfers(&true);

    let result = s.client.try_register_member(&member);
    assert!(result.is_err());

    assert_eq!(s.client.get_total_members(), 0);
    assert_eq!(s.client.get_active_members(), 0);
    assert!(!s.client.is_member(&member));
    assert!(s.client.get_member(&member).is_none());
    assert_eq!(s.token.balance(&s.client.address), 0);
}

#[test]
fn rejected_stake_transfer_leaves_stake_storage_unchanged() {
    let s = rejecting_setup(1);
    let member = s.members.get(0).unwrap();
    let dao_balance_before = s.token.balance(&s.client.address);

    s.token.set_reject_transfers(&true);
    let result = s.client.try_stake(&member, &500);
    assert!(result.is_err());

    assert_eq!(s.client.get_stake(&member), 0);
    let total_staked = s
        .env
        .as_contract(&s.client.address, || crate::storage::get_total_staked(&s.env));
    assert_eq!(total_staked, 0);
    assert_eq!(s.token.balance(&s.client.address), dao_balance_before);
    let has_stake_time = s.env.as_contract(&s.client.address, || {
        s.env
            .storage()
            .persistent()
            .has(&crate::storage::DataKey::StakeTime(member.clone()))
    });
    assert!(
        !has_stake_time,
        "stake timestamp must roll back with the rejected transfer"
    );
}

#[test]
fn loan_proposal_backwards_compatible_with_zero_quorum_bps() {
    // With quorum_bps == 0, it falls back to global consensus_threshold
    let s = setup(3); // 51% of 3 members is 2 votes
    let borrower = s.members.get(0).unwrap();
    let v1 = s.members.get(1).unwrap();
    let v2 = s.members.get(2).unwrap();

    assert_eq!(s.client.get_loan_policy().quorum_bps, 0);

    let pid = s.client.request_loan(&borrower, &500, &None);
    advance(&s.env, EDITING + 1);

    s.client.vote_on_loan_proposal(&v1, &pid, &true);
    assert_eq!(
        s.client.get_loan_proposal(&pid).unwrap().status,
        ProposalStatus::Pending
    );

    s.client.vote_on_loan_proposal(&v2, &pid, &true);
    assert_eq!(
        s.client.get_loan_proposal(&pid).unwrap().status,
        ProposalStatus::Approved
    );
}

// ===========================================================================
// Issue #192: Timelock delay for administrative policy changes
// ===========================================================================

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
    assert_eq!(pending.execution_time, pending.proposed_at + TIMELOCK_DURATION);

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

// ===========================================================================
// Issue #193: StakingRewardClaimed event on yield distribution
// ===========================================================================

#[test]
fn claim_rewards_emits_staking_reward_claimed_event_and_updates_snapshot() {
    let s = setup(3);
    let borrower = s.members.get(0).unwrap();
    let v1 = s.members.get(1).unwrap();
    let v2 = s.members.get(2).unwrap();

    let pid = s.client.request_loan(&borrower, &1_000, &None);
    advance(&s.env, EDITING + 1);
    s.client.vote_on_loan_proposal(&v1, &pid, &true);
    s.client.vote_on_loan_proposal(&v2, &pid, &true);
    s.client.repay_loan(&borrower, &0);

    let loan = s.client.get_loan(&0).unwrap();
    let interest = loan.total_repayment - loan.principal;
    let expected_share = interest / 3;
    assert!(expected_share > 0);

    assert_eq!(s.client.get_pending_yield(&v1), expected_share);

    // Claim rewards
    let claimed = s.client.claim_rewards(&v1);
    assert_eq!(claimed, expected_share);

    // Verify event payload and topics
    assert!(emitted(&s.env, "claimed"));

    // Find the claimed event in event log
    let all_events = s.env.events().all();
    let events_vec = all_events.events();
    let event = events_vec
        .iter()
        .rev()
        .find(|e| {
            let ContractEventBody::V0(body) = &e.body;
            matches!(body.topics.first(), Some(ScVal::Symbol(sym)) if sym.0.to_utf8_string_lossy() == "claimed")
        })
        .expect("claimed event not found");

    let ContractEventBody::V0(body) = &event.body;
    assert_eq!(body.topics.len(), 3);
    // Topic 0: symbol "claimed"
    match &body.topics[0] {
        ScVal::Symbol(sym) => assert_eq!(sym.0.to_utf8_string_lossy(), "claimed"),
        _ => panic!("unexpected topic 0"),
    }
    // Verify topic 2 has the claimed amount
    match &body.topics[2] {
        ScVal::I128(amount) => {
            let val = ((amount.hi as i128) << 64) | (amount.lo as i128);
            assert_eq!(val, expected_share);
        }
        _ => panic!("unexpected topic 2"),
    }

    // Verify accumulator snapshot updated on member record
    assert_eq!(s.client.get_pending_yield(&v1), 0);
    assert_eq!(s.client.try_claim_rewards(&v1), Err(Ok(Error::NothingToClaim)));
}

// ===========================================================================
// Issue #194: Custom metadata CID attachment to loan proposals
// ===========================================================================

#[test]
fn proposal_creation_with_and_without_cid() {
    let s = setup(2);
    let borrower = s.members.get(0).unwrap();

    // 1. Without CID
    let pid_none = s.client.request_loan(&borrower, &500, &None);
    let prop_none = s.client.get_loan_proposal(&pid_none).unwrap();
    assert_eq!(prop_none.metadata_cid, None);

    // 2. With valid CID (IPFS CIDv0: 46 chars)
    let cid_str = String::from_str(&s.env, "QmXoypizjW3WknFiJnKLwHCnL72vedxjQkDDP1mXWo6uco");
    let pid_some = s.client.request_loan(&borrower, &500, &Some(cid_str.clone()));
    let prop_some = s.client.get_loan_proposal(&pid_some).unwrap();
    assert_eq!(prop_some.metadata_cid, Some(cid_str));

    // 3. With valid CIDv1 (59 chars)
    let cid_v1 = String::from_str(
        &s.env,
        "bafybeicg2abbmanlpdgahgah744vyqeifqgndq7x2pzg7kmd3p7w4h2bfe",
    );
    let pid_v1 = s.client.request_loan(&borrower, &500, &Some(cid_v1.clone()));
    let prop_v1 = s.client.get_loan_proposal(&pid_v1).unwrap();
    assert_eq!(prop_v1.metadata_cid, Some(cid_v1));
}

#[test]
fn proposal_creation_rejects_invalid_cid() {
    let s = setup(2);
    let borrower = s.members.get(0).unwrap();

    // Empty CID string
    let empty_cid = String::from_str(&s.env, "");
    let err_empty = s.client.try_request_loan(&borrower, &500, &Some(empty_cid));
    assert_eq!(err_empty, Err(Ok(Error::InvalidMetadataCid)));

    // Too long CID (> 64 chars)
    let long_cid = String::from_str(
        &s.env,
        "QmXoypizjW3WknFiJnKLwHCnL72vedxjQkDDP1mXWo6ucoExtraLongContentHashExceeding64Characters",
    );
    let err_long = s.client.try_request_loan(&borrower, &500, &Some(long_cid));
    assert_eq!(err_long, Err(Ok(Error::DocumentTooLarge)));
}
fn rejected_treasury_transfer_rolls_back_approval_vote_and_execution_state() {
    let s = rejecting_setup(3);
    let proposer = s.members.get(0).unwrap();
    let voter_one = s.members.get(1).unwrap();
    let voter_two = s.members.get(2).unwrap();
    let destination = Address::generate(&s.env);

    let proposal_id = s.client.propose_treasury_withdrawal(
        &proposer,
        &500,
        &destination,
        &String::from_str(&s.env, "rollback test"),
        &false,
    );

    s.client
        .vote_on_treasury_proposal(&voter_one, &proposal_id, &true);
    let before = s.client.get_treasury_proposal(&proposal_id).unwrap();
    assert_eq!(before.status, ProposalStatus::Pending);
    assert_eq!(before.for_votes, 1);
    assert_eq!(before.votes_cast, 1);

    s.token.set_reject_transfers(&true);
    let result = s
        .client
        .try_vote_on_treasury_proposal(&voter_two, &proposal_id, &true);
    assert!(result.is_err());

    let after = s.client.get_treasury_proposal(&proposal_id).unwrap();
    assert_eq!(after, before, "proposal changes must roll back");
    let has_vote = s.env.as_contract(&s.client.address, || {
        crate::storage::has_treasury_voted(&s.env, proposal_id, &voter_two)
    });
    assert!(
        !has_vote,
        "vote marker must not survive a rejected execution transfer"
    );
    assert_eq!(s.token.balance(&destination), 0);
}
