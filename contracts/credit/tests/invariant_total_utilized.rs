// SPDX-License-Identifier: MIT

//! Proptest-based invariant test for `stored_total_utilized == σ utilized_amount`.
//!
//! Invariant: `get_total_utilized() == sum(enumerate_credit_lines().utilized_amount)`
//!
//! Generators drive ≥20 random operations across ≥4 borrowers, covering draw,
//! repay, forgive, default, and close transitions. After every operation the
//! invariant is re-checked. proptest's shrinking guarantees a minimal
//! reproducing sequence on any failure.
//!
//! Covered storage path: `persist_credit_line` (the single chokepoint that
//! atomically adjusts `DataKey::TotalUtilized` from the caller-captured
//! `previous_utilized`).

use creditra_credit::{Credit, CreditClient};
use proptest::prelude::*;
use soroban_sdk::testutils::{Address as _, Ledger};
use soroban_sdk::token::StellarAssetClient;
use soroban_sdk::{vec, Address, Env, Vec};

const BORROWER_COUNT: u32 = 4;
const PAGE_SIZE: u32 = 10;

/// Actions that touch the total-utilized accumulator.
#[derive(Debug, Clone)]
enum Action {
    Draw(u32, i128),
    Repay(u32, i128),
    Forgive(u32, i128),
    Default(u32),
    Close(u32),
}

// ── Environment setup ─────────────────────────────────────────────────────────

fn setup_env() -> (Env, CreditClient<'static>, Address, Vec<Address>) {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let contract_id = env.register(Credit, ());
    let client = CreditClient::new(&env, &contract_id);
    client.init(&admin);

    let token_id = env.register_stellar_asset_contract_v2(Address::generate(&env));
    let token = token_id.address();
    client.set_liquidity_token(&token);
    client.set_liquidity_source(&contract_id);

    let sac = StellarAssetClient::new(&env, &token);
    sac.mint(&contract_id, &50_000_000_i128);

    let mut borrowers = Vec::new(&env);
    for i in 0..BORROWER_COUNT {
        let borrower = Address::generate(&env);
        sac.mint(&borrower, &10_000_000_i128);
        let credit_limit = 100_000_i128 + (i as i128 * 20_000_i128);
        let interest_rate = 1_500_u32 + (i as u32 * 500_u32);
        let risk_score = 30_u32 + (i as u32 * 10_u32);
        client.open_credit_line(&borrower, &credit_limit, &interest_rate, &risk_score);
        borrowers.push_back(borrower);
    }

    (env, client, admin, borrowers)
}

// ── Invariant checker ─────────────────────────────────────────────────────────

fn assert_total_utilized_invariant(client: &CreditClient<'_>) {
    let total_count = client.get_credit_line_count();

    let mut cursor: Option<u32> = None;
    let mut recomputed: i128 = 0;

    loop {
        let page = client.enumerate_credit_lines(&cursor, &PAGE_SIZE);
        if page.is_empty() {
            break;
        }
        for (id, line) in page.iter() {
            recomputed += line.utilized_amount;
            cursor = Some(id);
        }
    }

    let stored = client.get_total_utilized();
    assert_eq!(
        stored, recomputed,
        "TotalUtilized mismatch: stored={stored}, recomputed={recomputed}, count={total_count}"
    );
}

// ── Proptest strategies ───────────────────────────────────────────────────────

fn action_strategy() -> impl Strategy<Value = Action> {
    prop_oneof![
        // Draw from any borrower
        (0u32..BORROWER_COUNT, 1i128..25_000i128).prop_map(|(b, a)| Action::Draw(b, a)),
        // Repay any borrower
        (0u32..BORROWER_COUNT, 1i128..25_000i128).prop_map(|(b, a)| Action::Repay(b, a)),
        // Forgive any borrower
        (0u32..BORROWER_COUNT, 1i128..10_000i128).prop_map(|(b, a)| Action::Forgive(b, a)),
        // Default any borrower
        (0u32..BORROWER_COUNT).prop_map(Action::Default),
        // Close any borrower
        (0u32..BORROWER_COUNT).prop_map(Action::Close),
    ]
}

// ── Action execution ──────────────────────────────────────────────────────────

fn try_execute(
    client: &CreditClient<'_>,
    admin: &Address,
    borrower: &Address,
    action: &Action,
) -> Result<(), ()> {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match action {
        Action::Draw(_, amount) => client.draw_credit(borrower, amount),
        Action::Repay(_, amount) => client.repay_credit(borrower, amount),
        Action::Forgive(_, amount) => client.forgive_debt(borrower, amount),
        Action::Default(..) => client.default_credit_line(borrower),
        Action::Close(..) => client.close_credit_line(borrower, admin),
    }));
    result.map(|_| ()).map_err(|_| ())
}

// ── Proptest invariant test ───────────────────────────────────────────────────

proptest! {
    #![proptest_config = ProptestConfig {
        cases: 1024,
        .. ProptestConfig::default()
    }]

    /// Verify that the sum of every credit line's `utilized_amount` matches
    /// the global `TotalUtilized` accumulator after every random transition.
    #[test]
    fn total_utilized_invariant_holds(
        actions in prop::collection::vec(action_strategy(), 20..=60)
    ) {
        let (_env, client, admin, borrowers) = setup_env();

        // Initial invariant: all lines at zero utilization.
        assert_total_utilized_invariant(&client);

        for action in &actions {
            let idx = match action {
                Action::Draw(i, _)
                | Action::Repay(i, _)
                | Action::Forgive(i, _)
                | Action::Default(i)
                | Action::Close(i) => *i,
            };

            // The strategy guarantees idx < BORROWER_COUNT.
            let borrower = borrowers.get(idx).expect("valid borrower index");

            // Advance the ledger so lazy accrual fires on the next mutation.
            _env.ledger().with_mut(|l| l.timestamp += 3600 + (l.timestamp % 86400));

            // Execute — may fail due to preconditions, which is fine.
            let _ = try_execute(&client, &admin, &borrower, action);

            // Invariant must hold after every transition, success or failure.
            assert_total_utilized_invariant(&client);
        }
    }
}
