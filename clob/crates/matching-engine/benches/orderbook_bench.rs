use criterion::{black_box, criterion_group, criterion_main, Criterion};
use matching_engine::MatchingEngine;
use common_types::{Order, Side, OrderType, OrderID, UserID, MarketID, OrderBook};
use rust_decimal::Decimal;
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

fn bench_simple_match(c: &mut Criterion) {
    c.bench_function("simple_full_match", |b| {
        b.iter_with_setup(
            || {
                let mut book = OrderBook::new();
                let maker = create_order(1, 1, Side::Sell, dec!(100.0), dec!(10.0));
                book.process_order(maker);
                let taker = create_order(2, 2, Side::Buy, dec!(100.0), dec!(10.0));
                (book, taker)
            },
            |(mut book, taker)| {
                black_box(book.process_order(taker));
            },
        );
    });
}

fn bench_one_to_many_match(c: &mut Criterion) {
    c.bench_function("one_to_many_match", |b| {
        b.iter_with_setup(
            || {
                let mut book = OrderBook::new();
                for i in 0..10 {
                    let maker = create_order(i + 1, 1, Side::Sell, dec!(100.0), dec!(1.0));
                    book.process_order(maker);
                }
                let taker = create_order(11, 2, Side::Buy, dec!(100.0), dec!(10.0));
                (book, taker)
            },
            |(mut book, taker)| {
                black_box(book.process_order(taker));
            },
        );
    });
}

fn bench_partial_fill_and_place(c: &mut Criterion) {
    c.bench_function("partial_fill_and_place", |b| {
        b.iter_with_setup(
            || {
                let mut book = OrderBook::new();
                let maker = create_order(1, 1, Side::Sell, dec!(100.0), dec!(5.0));
                book.process_order(maker);
                let taker = create_order(2, 2, Side::Buy, dec!(100.0), dec!(10.0));
                (book, taker)
            },
            |(mut book, taker)| {
                black_box(book.process_order(taker));
            },
        );
    });
}

fn bench_deep_book_match(c: &mut Criterion) {
    c.bench_function("deep_book_match", |b| {
        b.iter_with_setup(
            || {
                let mut book = OrderBook::new();
                // Create a deep book with 1000 bids and 1000 asks
                for i in 0..1000 {
                    let bid = create_order(i + 1, 1, Side::Buy, dec!(99.0) - Decimal::from(i), dec!(1.0));
                    let ask = create_order(i + 1001, 2, Side::Sell, dec!(101.0) + Decimal::from(i), dec!(1.0));
                    book.process_order(bid);
                    book.process_order(ask);
                }
                // The order that will cross the spread
                let taker = create_order(2002, 3, Side::Buy, dec!(101.0), dec!(1.0));
                (book, taker)
            },
            |(mut book, taker)| {
                black_box(book.process_order(taker));
            },
        );
    });
}

criterion_group!(
    benches,
    bench_simple_match,
    bench_one_to_many_match,
    bench_partial_fill_and_place,
    bench_deep_book_match
);
criterion_main!(benches);