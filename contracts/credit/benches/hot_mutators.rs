// SPDX-License-Identifier: MIT

use creditra_credit::types::CreditStatus;
use creditra_credit::{Credit, CreditClient};
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use soroban_sdk::testutils::{Address as _, Ledger};
use soroban_sdk::token::StellarAssetClient;
use soroban_sdk::{symbol_short, Address, Env, Symbol};

fn setup_bench(env: &Env) -> (CreditClient<'_>, Address, Address) {
    env.mock_all_auths_allowing_non_root_auth();
    env.ledger().with_mut(|ledger| {
        ledger.timestamp = 100;
    });

    let admin = Address::generate(env);
    let borrower = Address::generate(env);
    let credit_id = env.register(Credit, ());
    let token_id = env.register_stellar_asset_contract_v2(Address::generate(env));
    let token_address = token_id.address();

    let credit = CreditClient::new(env, &credit_id);
    credit.init(&admin);
    credit.set_liquidity_token(&token_address);
    credit.set_liquidity_source(&credit_id);

    StellarAssetClient::new(env, &token_address).mint(&credit_id, &100_000);

    credit.open_credit_line(&borrower, &10_000_i128, &300_u32, &50_u32);

    (credit, borrower, admin)
}

fn bench_draw_credit(c: &mut Criterion) {
    c.bench_function("draw_credit", |b| {
        b.iter_batched(
            || {
                let env = Env::default();
                let (credit, borrower, _) = setup_bench(&env);
                (env, credit, borrower)
            },
            |(env, credit, borrower)| {
                black_box(credit.draw_credit(&borrower, &1_000));
            },
            criterion::BatchSize::SmallInput,
        );
    });
}

fn bench_repay_credit(c: &mut Criterion) {
    c.bench_function("repay_credit", |b| {
        b.iter_batched(
            || {
                let env = Env::default();
                let (credit, borrower, _) = setup_bench(&env);
                credit.draw_credit(&borrower, &1_000);
                (env, credit, borrower)
            },
            |(env, credit, borrower)| {
                black_box(credit.repay_credit(&borrower, &1_000));
            },
            criterion::BatchSize::SmallInput,
        );
    });
}

fn bench_apply_accrual(c: &mut Criterion) {
    c.bench_function("apply_accrual", |b| {
        b.iter_batched(
            || {
                let env = Env::default();
                let (credit, borrower, _) = setup_bench(&env);
                credit.draw_credit(&borrower, &1_000);
                env.ledger().with_mut(|ledger| {
                    ledger.timestamp = 100 + 31_536_000; // 1 year later
                });
                (env, credit, borrower)
            },
            |(env, credit, borrower)| {
                // apply_accrual is called internally by draw/repay, but let's call it via a method that triggers it
                black_box(credit.get_credit_line(&borrower));
            },
            criterion::BatchSize::SmallInput,
        );
    });
}

fn bench_settle_default_liquidation(c: &mut Criterion) {
    c.bench_function("settle_default_liquidation", |b| {
        b.iter_batched(
            || {
                let env = Env::default();
                let (credit, borrower, _) = setup_bench(&env);
                credit.draw_credit(&borrower, &1_000);
                credit.default_credit_line(&borrower);
                let settlement_id = symbol_short!("test_auc");
                (env, credit, borrower, settlement_id)
            },
            |(env, credit, borrower, settlement_id)| {
                black_box(credit.settle_default_liquidation(&borrower, &1_000, &settlement_id, &None));
            },
            criterion::BatchSize::SmallInput,
        );
    });
}

criterion_group!(benches, bench_draw_credit, bench_repay_credit, bench_apply_accrual, bench_settle_default_liquidation);
criterion_main!(benches);
