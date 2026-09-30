use soroban_sdk::contracterror;

/// Every failure mode the DAO can return. Numeric codes are stable and part of
/// the contract's public ABI, so append new variants rather than renumbering.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    // ---- lifecycle / config ----
    AlreadyInitialized = 1,
    NotInitialized = 2,
    InvalidThreshold = 3,
    InvalidAmount = 4,
    InvalidLoanPolicy = 5,
    Paused = 6,
    NotPaused = 7,

    // ---- authorization ----
    NotAuthorized = 10,
    NotAdmin = 11,
    NotMember = 12,
    AlreadyAdmin = 13,
    AlreadyMember = 14,
    CannotRemoveLastAdmin = 15,

    // ---- membership ----
    MemberNotActive = 20,
    HasActiveLoan = 21,

    // ---- loans ----
    ProposalNotFound = 30,
    NotBorrower = 31,
    NotInEditingPhase = 32,
    NotInVotingPhase = 33,
    VotingEnded = 34,
    AlreadyVoted = 35,
    NotEligibleForLoan = 36,
    CooldownActive = 37,
    LoanNotFound = 38,
    LoanNotActive = 39,
    ExceedsTreasuryRatio = 40,
    InsufficientTreasury = 41,
    LoanNotOverdue = 42,

    // ---- treasury ----
    TreasuryProposalNotFound = 50,

    // ---- native-swap modules ----
    NameTaken = 60,
    /// Reserved for future name registry use
    NameNotFound = 61,
    NoStake = 62,
    InsufficientStake = 63,
    NoCommitment = 64,
    CommitmentMismatch = 65,
    AlreadyRevealed = 66,
    NothingToClaim = 67,

    // ---- appended (keep at end) ----
    ProposalNotExpired = 70,
    InvalidName = 71,
    NotYetRevealed = 72,
    /// Caller is not the proposal's proposer/borrower and may not modify its
    /// attached document (#21).
    NotProposalOwner = 73,
    DocumentTooLarge = 74,
    /// The token passed to `initialize` is not a contract implementing the
    /// token interface (#115).
    InvalidToken = 75,
    /// Timelock delay has not expired yet (#192).
    TimelockNotExpired = 76,
    /// No policy update is currently pending (#192).
    NoPendingPolicy = 77,
    /// Proposal metadata CID is invalid (#194).
    InvalidMetadataCid = 78,
    /// Invalid delegation target (e.g. self-delegation) (#188).
    InvalidDelegation = 79,
}
