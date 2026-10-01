use anchor_lang::prelude::*;

#[error_code]
pub enum ErrorCode {
    #[msg("Collateral ratio must be between the protocol minimum and maximum")]
    InvalidCollateralRatio,
    #[msg("Max drawdown exceeds the protocol limit")]
    InvalidDrawdown,
    #[msg("Name is empty or too long")]
    InvalidName,
    #[msg("Description is too long")]
    InvalidDescription,
    #[msg("Amount must be greater than zero")]
    ZeroAmount,
    #[msg("Agent does not have enough free collateral to guarantee this position")]
    InsufficientFreeCollateral,
    #[msg("Only the agent operator may perform this action")]
    UnauthorizedOperator,
    #[msg("Only the position trader may perform this action")]
    UnauthorizedTrader,
    #[msg("Position is not in the expected status")]
    InvalidStatus,
    #[msg("Position duration is outside the agent's published trading window")]
    InvalidDuration,
    #[msg("Position deadline has not been reached yet")]
    DeadlineNotReached,
    #[msg("Position deadline has passed; the trader may claim default")]
    DeadlinePassed,
    #[msg("Agent is not accepting new positions")]
    AgentNotAccepting,
    #[msg("Arithmetic overflow")]
    Overflow,
    #[msg("Fee exceeds the cap allowed by the collateral ratio")]
    FeeTooHigh,
    #[msg("Trading window is invalid")]
    InvalidTradingWindow,
    #[msg("Allowed assets must list between one and eight unique mints")]
    InvalidAllowedAssets,
    #[msg("Rules must be published and fit the size limit")]
    InvalidRules,
    #[msg("Terms can only be changed while the agent is a draft")]
    TermsLocked,
    #[msg("Agent has not been published")]
    NotPublished,
    #[msg("Agent is already published")]
    AlreadyPublished,
    #[msg("Deposit collateral before publishing the agent")]
    NoCollateral,
    #[msg("Signer is neither the agent's operator nor its bound trading key")]
    UnauthorizedExecutor,
    #[msg("Collateral ratio plus max drawdown must not exceed 100% of principal")]
    RatioPlusDrawdownTooHigh,
    #[msg("The protocol is paused: new positions, draws and deposits are stopped")]
    ProtocolPaused,
    #[msg("Position is larger than the protocol allows")]
    PositionTooLarge,
    #[msg("Agent would manage more capital than the protocol allows")]
    AgentCapReached,
    #[msg("Only the program's upgrade authority may change the protocol config")]
    UnauthorizedAdmin,
    #[msg("Caps must be greater than zero")]
    InvalidCaps,
    // ---- vault custody (appended; codes are permanent) ----
    #[msg("draw_funds is disabled: start trading with begin_trading, which keeps the principal in the vault")]
    DrawDisabled,
    #[msg("This position holds its principal in the vault: unwind it with execute_swap and settle it instead of claiming a default")]
    UseSettle,
    #[msg("This DEX program is not on the protocol's allowlist")]
    DexNotAllowed,
    #[msg("Mint is neither wrapped SOL nor one of the agent's allowed assets")]
    MintNotAllowed,
    #[msg("The vault still holds tokens other than wrapped SOL; swap them back before settling")]
    VaultNotUnwound,
    #[msg("The swap took more of the input token than amount_in_max")]
    SwapTooMuchIn,
    #[msg("The swap returned less than min_out")]
    SwapTooLittleOut,
    #[msg("The swap returned less than the oracle value allows after the maximum deviation")]
    SwapBelowOracle,
    #[msg("Price update account is not owned by the oracle program or has the wrong layout")]
    OracleAccountInvalid,
    #[msg("Price update is for a different feed than the config maps this mint to")]
    OracleFeedMismatch,
    #[msg("Price update is older than the protocol allows")]
    OraclePriceStale,
    #[msg("Price update is not fully verified")]
    OracleNotVerified,
    #[msg("Oracle price is zero or negative")]
    OraclePriceInvalid,
    #[msg("A vault account was changed by the swap in a way that is not a balance change")]
    VaultAccountTampered,
    #[msg("The swap instruction references a vault token account other than vault_in and vault_out")]
    ExtraVaultAccount,
    #[msg("Input and output mints must differ")]
    SameMint,
    #[msg("Token account still holds a balance")]
    TokenAccountNotEmpty,
    #[msg("Trading config is invalid")]
    InvalidTradingConfig,
    #[msg("After the deadline a position may only be swapped back into wrapped SOL")]
    AfterDeadlineOnlyUnwind,
    #[msg("No price feed is configured for this mint")]
    NoFeedForMint,
    #[msg("The wrapped SOL account cannot be closed while the position is trading")]
    WsolAccountInUse,
    #[msg("Custody accounts are missing: pass custody, vault_wsol, rent_payer and token_program")]
    CustodyAccountsMissing,
}
