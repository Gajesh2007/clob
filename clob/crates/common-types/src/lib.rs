//! Core shared types used across the CLOB system.
//!
//! This crate defines the canonical data model for requests, events, and
//! state that flow between services:
//! - Orders and signed orders submitted by clients
//! - Deposits and the unified [`Transaction`] log
//! - Trades and [`MarketEvent`]s emitted by the matching engine
//! - [`Account`] state and the [`OrderBook`] snapshot used for checkpointing
//!
//! All numeric quantities (prices and amounts) use [`rust_decimal::Decimal`]
//! for exact decimal arithmetic suitable for financial applications.
//!
//! Conventions
//! - Price represents quote per one unit of base for a market
//! - Quantity represents base-asset units
//! - Asset identifiers are opaque and application-defined
//!
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use std::fmt;

// --- Financial Primitives ---
/// Price in quote currency per one unit of base currency.
pub type Price = Decimal;
/// Quantity in base-asset units.
pub type Quantity = Decimal;

// --- Identifiers ---
/// Unique identifier for an order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub struct OrderID(pub u64);

/// Unique identifier for a user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub struct UserID(pub u64);

impl fmt::Display for UserID {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub struct TradeID(pub u64);

/// Opaque identifier for an asset (application-defined).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub struct AssetID(pub u32);

/// Identifier for a market (e.g., a base/quote pair).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub struct MarketID(pub u32);

// --- Cryptographic Primitives ---
use ed25519_dalek as ed25519;

pub type Hash = [u8; 32];

/// ED25519 verifying key associated with a [`UserID`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicKey(pub ed25519::VerifyingKey);

/// ED25519 signature over a bincode-serialized [`Order`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signature(pub ed25519::Signature);

// --- Core State & Order Properties ---

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Side {
    Buy,
    Sell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OrderType {
    Limit,
}

/// Account state tracked by the system.
///
/// Balances are held per [`AssetID`]. The settlement plane and verifier
/// reconstruct these from the execution log deterministically.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub user_id: UserID,
    pub balances: BTreeMap<AssetID, Quantity>,
}

/// Client order describing intent to buy or sell a quantity at a price.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Order {
    pub order_id: OrderID,
    pub user_id: UserID,
    pub market_id: MarketID,
    pub side: Side,
    pub price: Price,
    pub quantity: Quantity,
    pub order_type: OrderType,
    pub timestamp: u64,
}

/// An [`Order`] bundled with an ED25519 [`Signature`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedOrder {
    pub order: Order,
    pub signature: Signature,
}

/// Deposit of an amount for a given asset into a user's account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Deposit {
    pub user_id: UserID,
    pub asset_id: AssetID,
    pub amount: Quantity,
}

/// All transactions recorded in the execution log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Transaction {
    SignedOrder(SignedOrder),
    Deposit(Deposit),
}

/// Executed trade resulting from matching maker and taker orders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Trade {
    pub trade_id: TradeID,
    pub market_id: MarketID,
    pub maker_order_id: OrderID,
    pub taker_order_id: OrderID,
    pub maker_user_id: UserID,
    pub taker_user_id: UserID,
    pub quantity: Quantity,
    pub price: Price,
    pub timestamp: u64,
    pub taker_side: Side,
}

/// Events emitted by the matching engine.
///
/// - `OrderPlaced` is emitted when a residual order is added to the book.
/// - `OrderTraded` is emitted for each trade execution.
/// - `OrderCancelled` is reserved for future use and not currently emitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MarketEvent {
    OrderPlaced {
        order_id: OrderID,
        user_id: UserID,
        market_id: MarketID,
        side: Side,
        price: Price,
        quantity: Quantity,
        timestamp: u64,
    },
    OrderTraded(Trade),
    OrderCancelled {
        order_id: OrderID,
        timestamp: u64,
    },
}

// --- State Snapshot ---

/// Canonical snapshot of system state used for checkpointing and verification.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct StateSnapshot {
    pub accounts: Vec<(UserID, Account)>,
    pub order_book: OrderBook,
}

// --- Order Book ---

pub type PriceLevel = VecDeque<Order>;

/// In-memory order book structure used by the matching engine.
///
/// Bids are keyed by reverse price for max-heap semantics; asks by ascending
/// price. The matching engine mutates this structure and emits [`MarketEvent`]s.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct OrderBook {
    pub bids: BTreeMap<std::cmp::Reverse<Price>, PriceLevel>,
    pub asks: BTreeMap<Price, PriceLevel>,
    pub next_trade_id: u64,
}

impl OrderBook {
    /// Create an empty order book.
    pub fn new() -> Self {
        OrderBook {
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            next_trade_id: 0,
        }
    }
}