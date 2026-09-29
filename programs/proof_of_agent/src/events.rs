use anchor_lang::prelude::*;

use crate::state::{AgentTerms, Breach, PositionStatus};

#[event]
pub struct AgentCreated {
    pub agent: Pubkey,
    pub operator: Pubkey,
    pub agent_id: u64,
}

#[event]
pub struct AgentPublished {
    pub agent: Pubkey,
    pub operator: Pubkey,
    pub terms: AgentTerms,
    pub collateral: u64,
}

#[event]
pub struct ExecutorBound {
    pub agent: Pubkey,
    pub executor: Pubkey,
}

#[event]
pub struct CollateralChanged {
    pub agent: Pubkey,
    pub delta: i64,
    pub total_collateral: u64,
}

#[event]
pub struct PositionOpened {
    pub position: Pubkey,
    pub agent: Pubkey,
    pub trader: Pubkey,
    pub principal: u64,
    pub locked_collateral: u64,
    pub deadline: i64,
}

#[event]
pub struct PositionClosed {
    pub position: Pubkey,
    pub agent: Pubkey,
    pub trader: Pubkey,
    pub status: PositionStatus,
    pub breach: Breach,
    pub returned: u64,
    pub slashed: u64,
    pub fee_paid: u64,
    /// Lamports paid to the trader for this position: returned principal
    /// minus fee plus any slash (settle), the principal (cancel or decline),
    /// or the slashed bond (default). Excludes the position vault's rent
    /// floor, which is also refunded to the trader when the vault is drained.
    pub trader_payout: u64,
}

/// The agent declined a position it never drew: the trader got the principal
/// back and the agent's reputation counters were not changed. Emitted
/// alongside `PositionClosed` (status `Settled`, breach `None`) so indexers can
/// tell a decline apart from a settle that returned exactly the principal.
#[event]
pub struct PositionDeclined {
    pub position: Pubkey,
    pub agent: Pubkey,
    pub trader: Pubkey,
    pub principal: u64,
}

/// The agent's trading key drew a position's principal to trade with it.
#[event]
pub struct FundsDrawn {
    pub position: Pubkey,
    pub agent: Pubkey,
    pub trader: Pubkey,
    pub executor: Pubkey,
    pub principal: u64,
    pub deadline: i64,
}

/// The operator paused or resumed new positions on a published agent.
#[event]
pub struct AcceptingChanged {
    pub agent: Pubkey,
    pub operator: Pubkey,
    pub accepting: bool,
}

#[event]
pub struct BreachRecorded {
    pub agent: Pubkey,
    pub position: Pubkey,
    pub breach: Breach,
    pub slashed: u64,
}

/// The upgrade authority created or changed the protocol config.
#[event]
pub struct ConfigChanged {
    pub paused: bool,
    pub max_position: u64,
    pub max_agent_capital: u64,
}
