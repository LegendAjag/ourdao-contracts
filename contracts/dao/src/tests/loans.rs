extern crate std;

use soroban_sdk::testutils::{Address as _, Events as _};
use soroban_sdk::xdr::{ContractEventBody, ScVal};
use soroban_sdk::{Address, String};

use super::common::*;
use crate::admin::TIMELOCK_DURATION;
use crate::storage::ProposalKind;
use crate::types::{LoanStatus, ProposalPhase, ProposalStatus};
use crate::Error;

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

    s.client
        .propose_policy_update(&s.admin, &high_quorum_policy);
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

// Issue #194: Custom metadata CID attachment to loan proposals
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
    let pid_some = s
        .client
        .request_loan(&borrower, &500, &Some(cid_str.clone()));
    let prop_some = s.client.get_loan_proposal(&pid_some).unwrap();
    assert_eq!(prop_some.metadata_cid, Some(cid_str));

    // 3. With valid CIDv1 (59 chars)
    let cid_v1 = String::from_str(
        &s.env,
        "bafybeicg2abbmanlpdgahgah744vyqeifqgndq7x2pzg7kmd3p7w4h2bfe",
    );
    let pid_v1 = s
        .client
        .request_loan(&borrower, &500, &Some(cid_v1.clone()));
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

#[test]
fn edit_loan_proposal_emits_loan_edit_event() {
    let s = setup(2);
    let borrower = s.members.get(0).unwrap();
    let pid = s.client.request_loan(&borrower, &500, &None);
    // Drain the loan_req event so only edit-related events remain.
    let _ = s.env.events().all();
    advance(&s.env, 10);
    let edit_ts = s.env.ledger().timestamp();
    s.client.edit_loan_proposal(&borrower, &pid, &600);
    // Capture events before any further contract call (all() drains).
    let events = s.env.events().all();
    let mut found = false;
    for e in events.events().iter() {
        let ContractEventBody::V0(body) = &e.body;
        let is_topic = matches!(body.topics.first(), Some(ScVal::Symbol(sym)) if sym.0.to_utf8_string_lossy() == "loan_edit");
        if !is_topic {
            continue;
        }
        let data_str = std::format!("{:?}", body.data);
        if data_str.contains("600") {
            found = true;
        }
    }
    assert!(found, "loan_edit event with new_amount 600 not found");
    // Verify storage separately, after event capture.
    let after = s.client.get_loan_proposal(&pid).unwrap();
    assert_eq!(after.amount, 600);
    assert_eq!(after.last_edited_at, Some(edit_ts));
    let expected = s.client.calculate_loan_terms(&600);
    assert_eq!(after.total_repayment, expected.total_repayment);
}

// ===========================================================================
// Issue #173: Off-by-one in voting period deadline comparison
// ===========================================================================

#[test]
fn voting_closed_exactly_at_deadline() {
    // At now == deadline a vote must be rejected (VotingEnded), not accepted.
    let s = setup(3);
    let borrower = s.members.get(0).unwrap();
    let voter = s.members.get(1).unwrap();

    let pid = s.client.request_loan(&borrower, &500, &None);
    // Advance to exactly the voting deadline (editing_period_end + voting_period).
    advance(&s.env, EDITING + VOTING_PERIOD);

    let res = s.client.try_vote_on_loan_proposal(&voter, &pid, &true);
    assert_eq!(
        res,
        Err(Ok(crate::Error::VotingEnded)),
        "vote at exact deadline must be rejected"
    );
}

#[test]
fn voting_open_one_second_before_deadline() {
    // One second before deadline a vote must still be accepted.
    let s = setup(3);
    let borrower = s.members.get(0).unwrap();
    let voter = s.members.get(1).unwrap();

    let pid = s.client.request_loan(&borrower, &500, &None);
    // Advance to one second before the voting deadline.
    advance(&s.env, EDITING + VOTING_PERIOD - 1);

    s.client.vote_on_loan_proposal(&voter, &pid, &true);
    let prop = s.client.get_loan_proposal(&pid).unwrap();
    assert!(prop.for_votes > 0, "vote should have been recorded");
// Issue #171: Reject zero-amount loan proposals
#[test]
fn zero_amount_loan_request_rejected() {
    let s = setup(1);
    let borrower = s.members.get(0).unwrap();

    let err = s.client.try_request_loan(&borrower, &0, &None);
    assert_eq!(err, Err(Ok(Error::InvalidAmount)));

    let err_neg = s.client.try_request_loan(&borrower, &-100, &None);
    assert_eq!(err_neg, Err(Ok(Error::InvalidAmount)));
// ==================== issue #190: member loan stats view ====================
#[test]
fn member_loan_stats_track_lifecycle() {
    let s = setup(3);
    let borrower = s.members.get(0).unwrap();
    let v1 = s.members.get(1).unwrap();
    let v2 = s.members.get(2).unwrap();

    let stats = s.client.get_member_loan_stats(&borrower);
    assert_eq!(stats.total_loans, 0);
    assert_eq!(stats.repaid_loans, 0);
    assert_eq!(stats.active_loans, 0);

    let pid = s.client.request_loan(&borrower, &1_000, &None);
    advance(&s.env, EDITING + 1);
    s.client.vote_on_loan_proposal(&v1, &pid, &true);
    s.client.vote_on_loan_proposal(&v2, &pid, &true);

    let stats = s.client.get_member_loan_stats(&borrower);
    assert_eq!(stats.total_loans, 1);
    assert_eq!(stats.repaid_loans, 0);
    assert_eq!(stats.active_loans, 1);

    // Partial repayment leaves the loan outstanding: stats must not move yet.
    let loan = s.client.get_loan(&pid).unwrap();
    let outstanding = loan.total_repayment - loan.amount_repaid;
    s.client
        .repay_loan_partial(&borrower, &pid, &(outstanding / 2));
    let stats = s.client.get_member_loan_stats(&borrower);
    assert_eq!(stats.total_loans, 1);
    assert_eq!(stats.repaid_loans, 0);
    assert_eq!(stats.active_loans, 1);

    // Full repayment settles the loan.
    s.client.repay_loan(&borrower, &pid);
    let stats = s.client.get_member_loan_stats(&borrower);
    assert_eq!(stats.total_loans, 1);
    assert_eq!(stats.repaid_loans, 1);
    assert_eq!(stats.active_loans, 0);

    // A second loan keeps accumulating on top of the first.
    let pid2 = s.client.request_loan(&borrower, &500, &None);
    advance(&s.env, EDITING + 1);
    s.client.vote_on_loan_proposal(&v1, &pid2, &true);
    s.client.vote_on_loan_proposal(&v2, &pid2, &true);
    let stats = s.client.get_member_loan_stats(&borrower);
    assert_eq!(stats.total_loans, 2);
    assert_eq!(stats.repaid_loans, 1);
    assert_eq!(stats.active_loans, 1);
}

#[test]
fn member_loan_stats_default_is_not_repaid() {
    let s = setup(3);
    let borrower = s.members.get(0).unwrap();
    let v1 = s.members.get(1).unwrap();
    let v2 = s.members.get(2).unwrap();

    let pid = s.client.request_loan(&borrower, &500, &None);
    advance(&s.env, EDITING + 1);
    s.client.vote_on_loan_proposal(&v1, &pid, &true);
    s.client.vote_on_loan_proposal(&v2, &pid, &true);
    advance(&s.env, LOAN_DURATION + 1);
    s.client.mark_loan_defaulted(&pid);

    let stats = s.client.get_member_loan_stats(&borrower);
    assert_eq!(stats.total_loans, 1);
    assert_eq!(stats.repaid_loans, 0);
    assert_eq!(stats.active_loans, 0);
}

#[test]
fn member_loan_stats_preserved_across_rejoin() {
    let s = setup(3);
    let borrower = s.members.get(0).unwrap();
    let v1 = s.members.get(1).unwrap();
    let v2 = s.members.get(2).unwrap();

    let pid = s.client.request_loan(&borrower, &500, &None);
    advance(&s.env, EDITING + 1);
    s.client.vote_on_loan_proposal(&v1, &pid, &true);
    s.client.vote_on_loan_proposal(&v2, &pid, &true);
    s.client.repay_loan(&borrower, &pid);

    let before = s.client.get_member_loan_stats(&borrower);
    assert_eq!(before.total_loans, 1);
    assert_eq!(before.repaid_loans, 1);

    s.client.exit_dao(&borrower);
    s.client.register_member(&borrower);

    assert_eq!(s.client.get_member_loan_stats(&borrower), before);
}

#[test]
fn member_loan_stats_unknown_member_rejected() {
    let s = setup(1);
    let stranger = Address::generate(&s.env);
    assert_eq!(
        s.client.try_get_member_loan_stats(&stranger),
        Err(Ok(Error::NotMember))
    );
}
