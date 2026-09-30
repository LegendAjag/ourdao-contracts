use soroban::{contracttype, Address, String};

/// Basis-points denominator (100% == 10_000).
pub const BASIS_POINTS: i128 = 10_000;
/// Minimum length for a registered name (inclusive).
pub const NAME_MIN_LEN: u32 = 3;
/// Maximum length for a registered name (inclusive). Keeps the on-chain
/// storage and every `name_reg` event payload bounded.
pub const NAME_MAX_LEN: u32 = 32;

#contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemberStatus {
    ActiveMember,
    Inactive,
}

#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProposalStatus {
    Pending,
    ApprovedPendingDisbursement,
    Approved,
    Rejected,
    Executed,
    Expired,
}

#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProposalPhase {
    Editing,
    Voting,
    Executed,
    Expired,
}

#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoanStatus {
    Active,
    Repaid,
    Defaulted,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Member {
    pub address: Address,
    pub status: MemberStatus,
    pub join_time: u64,
    pub contribution: i128,
    pub share_balance: i128,
    pub has_active_loan: bool,
    pub last_loan_time: u64,
}

/// Tunable lending parameters. Durations are in ledger seconds; rates in bps.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoanPolicy {
    pub min_membership_duration: u64,
    pub membership_contribution: i128,
    pub max_loan_duration: u64,
    pub min_interest_rate: u32,
    pub max_interest_rate: u32,
    pub cooldown_period: u64,
    pub max_loan_to_treasury_ratio: u32,
    /// Extra time past `Loan.due_time` before a loan becomes markable as defaulted.
    pub default_grace_period: u64,
    /// Basis points of the defaulting borrower's `contribution` slashed on default.
    pub default_penalty_bps: u32,
    pub editing_period: u64,
    pub voting_period: u64,
    pub treasury_threshold: u32,
    pub quorum_bps: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoanProposal {
    pub id: u32,
    pub borrower: Address,
    pub amount: i128,
    pub interest_rate: u32,
    pub duration: u64,
    pub total_repayment: i128,
    pub created_at: u64,
    pub editing_period_end: u64,
    pub phase: ProposalPhase,
    pub status: ProposalStatus,
    pub for_votes: i128,
    pub against_votes: i128,
    pub votes_cast: u32,
    pub voting_period: u64,
    pub metadata_cid: Option<String>,
    /// Stellar asset address the loan is denominated in.
    pub asset: Address,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Loan {
    pub id: u32,
    pub borrower: Address,
    pub principal: i128,
    pub interest_rate: u32,
    pub total_repayment: i128,
    pub start_time: u64,
    pub due_time: u64,
    pub status: LoanStatus,
    pub amount_repaid: i128,
    /// Stellar asset the loan was issued in.
    pub asset: Address,
    /// Stellar asset the loan must be repaid in.
    pub repayment_asset: Address,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TreasuryProposal {
    pub id: u32,
    pub proposer: Address,
    pub amount: i128,
    pub destination: Address,
    pub reason: String,
    pub created_at: u64,
    pub status: ProposalStatus,
    pub for_votes: i128,
    pub against_votes: i128,
    pub votes_cast: u32,
    pub voting_period: u64,
    pub treasury_threshold: u32,
    /// When true, votes must be committed then revealed (commit-reveal privacy).
    pub private: bool,
    /// Stellar asset the treasury payout is denominated in.
    pub asset: Address,
}

/// Computed loan terms returned by the read-only quote helper.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoanTerms {
    pub interest_rate: u32,
    pub total_repayment: i128,
    pub duration: u64,
}

/// Pending policy update undergoing timelock delay (#192).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingPolicyUpdate {
    pub policy: LoanPolicy,
    pub proposed_at: u64,
    pub execution_time: u64,
}

/// Structured event emitted upon staking reward claim (#193).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StakingRewardClaimed {
    pub member: Address,
    pub amount: i128,
    pub timestamp: u64,
}

/// An approved Stellar asset that the treasury may accept deposits in and
/// issue loans in. Admins propose and add tokens to the whitelist.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TokenWhitelist {
    /// Stellar asset contract address.
    pub asset: Address,
    /// Human-readable symbol (e.g. "USDX", "XEL").
    pub symbol: String,
    /// Ledger timestamp the asset was added.
    pub added_at: u64,
    /// Whether the asset is currently accepted for deposits and loans.
    pub active: bool,
}

/// Proposal to add a new token to the treasury whitelist.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TokenWhitelistProposal {
    pub id: u32,
    pub proposer: Address,
    pub asset: Address,
    pub symbol: String,
    pub created_at: u64,
    pub status: ProposalStatus,
    pub for_votes: i128,
    pub against_votes: i128,
    pub votes_cast: u32,
    pub voting_period: u64,
    pub treasury_threshold: u32,
}
