// SPDX-License-Identifier: MIT

//! Regression tests for `RateChangeConfig::rate_change_min_interval` cadence.
//!
//! A same-block double-update (`now == last_rate_update_ts`) is a known gotcha:
//! the elapsed interval is zero, so the second rate change must revert before any
//! state mutation. These tests pin that behavior and the exact error code.

use creditra_credit::types::ContractError;
use creditra_credit::{Credit, CreditClient};
use soroban_sdk::testutils::{Address as _, Ledger};
use soroban_sdk::{Address, Env};

const START_TS: u64 = 1_000;
const MIN_INTERVAL_SECONDS: u64 = 60;
const CREDIT_LIMIT: i128 = 10_000;
const INITIAL_RATE_BPS: u32 = 300;
const FIRST_UPDATE_RATE_BPS: u32 = 350;
const SAME_BLOCK_RATE_BPS: u32 = 380;

fn setup(start_ts: u64) -> (Env, Address, CreditClient<'_>) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().with_mut(|li| li.timestamp = start_ts);

    let admin = Address::generate(&env);
    let borrower = Address::generate(&env);
    let contract_id = env.register(Credit, ());
    let client = CreditClient::new(&env, &contract_id);
    client.init(&admin);
    client.open_credit_line(&borrower, &CREDIT_LIMIT, &INITIAL_RATE_BPS, &70_u32);

    // Large delta cap so cadence — not magnitude — is what fails on the second call.
    client.set_rate_change_limits(&500_u32, &MIN_INTERVAL_SECONDS);

    (env, borrower, client)
}

#[test]
fn rate_change_same_block_double_update_reverts_with_timestamp_regression() {
    let (env, borrower, client) = setup(START_TS);

    client.update_risk_parameters(&borrower, &CREDIT_LIMIT, &FIRST_UPDATE_RATE_BPS, &70_u32);

    let line_after_first = client.get_credit_line(&borrower).unwrap();
    assert_eq!(line_after_first.interest_rate_bps, FIRST_UPDATE_RATE_BPS);
    assert_eq!(line_after_first.last_rate_update_ts, START_TS);

    // Identical ledger timestamp: elapsed = 0 < rate_change_min_interval.
    env.ledger().with_mut(|li| li.timestamp = START_TS);
    let result =
        client.try_update_risk_parameters(&borrower, &CREDIT_LIMIT, &SAME_BLOCK_RATE_BPS, &70_u32);

    assert!(result.is_err(), "same-block second rate update must revert");
    assert_eq!(
        result.err().unwrap().unwrap(),
        ContractError::TimestampRegression,
        "cadence breach must map to TimestampRegression (#33)"
    );

    let line_after_reject = client.get_credit_line(&borrower).unwrap();
    assert_eq!(
        line_after_reject.interest_rate_bps, FIRST_UPDATE_RATE_BPS,
        "failed update must leave the prior rate unchanged"
    );
    assert_eq!(
        line_after_reject.last_rate_update_ts, START_TS,
        "failed update must not refresh last_rate_update_ts"
    );
}

#[test]
fn rate_change_exact_interval_boundary_allows_second_update() {
    let (env, borrower, client) = setup(START_TS);

    client.update_risk_parameters(&borrower, &CREDIT_LIMIT, &FIRST_UPDATE_RATE_BPS, &70_u32);

    env.ledger()
        .with_mut(|li| li.timestamp = START_TS + MIN_INTERVAL_SECONDS);
    client.update_risk_parameters(&borrower, &CREDIT_LIMIT, &SAME_BLOCK_RATE_BPS, &65_u32);

    let line = client.get_credit_line(&borrower).unwrap();
    assert_eq!(line.interest_rate_bps, SAME_BLOCK_RATE_BPS);
    assert_eq!(line.risk_score, 65);
    assert_eq!(line.last_rate_update_ts, START_TS + MIN_INTERVAL_SECONDS);
}
