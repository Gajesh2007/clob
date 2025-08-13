use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};

// --- Financial Primitives ---
pub type Price = Decimal;
pub type Quantity = Decimal;

// --- Identifiers ---
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub struct OrderID(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub struct UserID(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub struct TradeID(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub struct AssetID(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub struct MarketID(pub u32);

// --- Cryptographic Primitives ---
use ed25519_dalek as ed25519;

pub type Hash = [u8; 32];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicKey(pub ed25519::VerifyingKey);

// Signatures can be compared but not hashed, which is why `Hash` is not derived.
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub user_id: UserID,
    pub balances: BTreeMap<AssetID, Quantity>,
}

/// The fundamental order structure, containing all necessary details for matching.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Order {
    pub order_id: OrderID,
    pub user_id: UserID,
    pub market_id: MarketID,
    pub side: Side,
    pub price: Price,
    pub quantity: Quantity,
    pub order_type: OrderType,
    pub timestamp: u64, // Nanoseconds since epoch
}

/// A wrapper for an Order and its associated signature.
/// This struct cannot be hashed because the underlying signature type cannot be hashed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedOrder {
    pub order: Order,
    pub signature: Signature,
}

/// Represents a completed trade.
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
}

/// Public events broadcast by the matching engine.
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

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct StateSnapshot {
    pub accounts: Vec<(UserID, Account)>,
    pub order_book: OrderBook,
}

// --- Order Book ---

pub type PriceLevel = VecDeque<Order>;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct OrderBook {
    pub bids: BTreeMap<std::cmp::Reverse<Price>, PriceLevel>,
    pub asks: BTreeMap<Price, PriceLevel>,
    pub next_trade_id: u64,
}

impl OrderBook {
    pub fn new() -> Self {
        OrderBook {
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            next_trade_id: 0,
        }
    }
}