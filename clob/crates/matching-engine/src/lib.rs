//! Matching engine library.
//!
//! This crate implements price-time priority matching for limit orders over the
//! shared [`common_types::OrderBook`] structure. It is intentionally minimal and
//! free of networking or persistence concerns.
//!
//! Key properties
//! - Deterministic: given the same input sequence, produces the same events
//! - Price-time priority: best price first; FIFO within each price level
//! - Emits [`common_types::MarketEvent`]s for placements and trades
//!
use common_types::{MarketEvent, Order, Side, Trade, TradeID, OrderBook, PriceLevel};
use rust_decimal::Decimal;

/// Core trait for processing orders against an order book.
///
/// Implementations must consume an input [`Order`] and return the sequence of
/// [`MarketEvent`]s generated (zero or more trades, and possibly an order
/// placement if residual quantity remains).
pub trait MatchingEngine {
    fn process_order(&mut self, order: Order) -> Vec<MarketEvent>;
}

impl MatchingEngine for OrderBook {
    /// Match and place a single order against the order book.
    fn process_order(&mut self, mut order: Order) -> Vec<MarketEvent> {
        let mut events = Vec::new();

        if order.side == Side::Buy {
            match_buy_order(self, &mut order, &mut events);
        } else {
            match_sell_order(self, &mut order, &mut events);
        }

        if order.quantity > Decimal::ZERO {
            place_order(self, order, &mut events);
        }

        events
    }
}

fn match_buy_order(
    book: &mut OrderBook,
    order: &mut Order,
    events: &mut Vec<MarketEvent>,
) {
    while order.quantity > Decimal::ZERO {
        let best_ask_price = book.asks.first_key_value().map(|(p, _)| *p);

        if let Some(ask_price) = best_ask_price {
            if order.price >= ask_price {
                let mut entry = book.asks.first_entry().unwrap();
                let price_level = entry.get_mut();
                execute_trades(order, price_level, events, &mut book.next_trade_id);
                if price_level.is_empty() {
                    entry.remove();
                }
            } else {
                break;
            }
        } else {
            break;
        }
    }
}

fn match_sell_order(
    book: &mut OrderBook,
    order: &mut Order,
    events: &mut Vec<MarketEvent>,
) {
    while order.quantity > Decimal::ZERO {
        let best_bid_price = book.bids.first_key_value().map(|(p, _)| p.0);

        if let Some(bid_price) = best_bid_price {
            if order.price <= bid_price {
                let mut entry = book.bids.first_entry().unwrap();
                let price_level = entry.get_mut();
                execute_trades(order, price_level, events, &mut book.next_trade_id);
                if price_level.is_empty() {
                    entry.remove();
                }
            } else {
                break;
            }
        } else {
            break;
        }
    }
}

fn place_order(
    book: &mut OrderBook,
    order: Order,
    events: &mut Vec<MarketEvent>,
) {
    let price_level = match order.side {
        Side::Buy => book.bids.entry(std::cmp::Reverse(order.price)).or_default(),
        Side::Sell => book.asks.entry(order.price).or_default(),
    };
    price_level.push_back(order);

    events.push(MarketEvent::OrderPlaced {
        order_id: order.order_id,
        user_id: order.user_id,
        market_id: order.market_id,
        side: order.side,
        price: order.price,
        quantity: order.quantity,
        timestamp: order.timestamp,
    });
}

fn execute_trades(
    taker_order: &mut Order,
    maker_price_level: &mut PriceLevel,
    events: &mut Vec<MarketEvent>,
    next_trade_id: &mut u64,
) {
    let mut filled_maker_orders = Vec::new();

    for (i, maker_order) in maker_price_level.iter_mut().enumerate() {
        if taker_order.quantity == Decimal::ZERO {
            break;
        }

        let trade_qty = std::cmp::min(taker_order.quantity, maker_order.quantity);
        taker_order.quantity -= trade_qty;
        maker_order.quantity -= trade_qty;

        let trade = Trade {
            trade_id: TradeID(*next_trade_id),
            market_id: taker_order.market_id,
            maker_order_id: maker_order.order_id,
            taker_order_id: taker_order.order_id,
            maker_user_id: maker_order.user_id,
            taker_user_id: taker_order.user_id,
            quantity: trade_qty,
            price: maker_order.price,
            timestamp: taker_order.timestamp,
            taker_side: taker_order.side,
        };
        *next_trade_id += 1;
        events.push(MarketEvent::OrderTraded(trade));

        if maker_order.quantity == Decimal::ZERO {
            filled_maker_orders.push(i);
        }
    }

    for i in filled_maker_orders.iter().rev() {
        maker_price_level.remove(*i);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common_types::{MarketID, OrderID, OrderType, UserID};
    use rust_decimal_macros::dec;

    fn create_order(id: u64, user_id: u64, side: Side, price: Decimal, qty: Decimal) -> Order {
        Order {
            order_id: OrderID(id),
            user_id: UserID(user_id),
            market_id: MarketID(1),
            side,
            price,
            quantity: qty,
            order_type: OrderType::Limit,
            timestamp: 0,
        }
    }

    #[test]
    fn test_place_order_no_match() {
        let mut book = OrderBook::new();
        let order = create_order(1, 1, Side::Buy, dec!(100.0), dec!(10.0));
        let events = book.process_order(order);

        assert_eq!(book.bids.len(), 1);
        assert_eq!(book.asks.len(), 0);
        assert_eq!(book.bids.get(&std::cmp::Reverse(dec!(100.0))).unwrap().len(), 1);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], MarketEvent::OrderPlaced { .. }));
    }

    #[test]
    fn test_simple_full_match() {
        let mut book = OrderBook::new();
        let maker_order = create_order(1, 1, Side::Sell, dec!(100.0), dec!(10.0));
        book.process_order(maker_order);
        let taker_order = create_order(2, 2, Side::Buy, dec!(100.0), dec!(10.0));
        let events = book.process_order(taker_order);

        assert!(book.bids.is_empty());
        assert!(book.asks.is_empty());
        assert_eq!(events.len(), 1);
        match &events[0] {
            MarketEvent::OrderTraded(trade) => {
                assert_eq!(trade.maker_user_id, maker_order.user_id);
                assert_eq!(trade.taker_user_id, taker_order.user_id);
            }
            _ => panic!("Expected OrderTraded event"),
        }
    }
}