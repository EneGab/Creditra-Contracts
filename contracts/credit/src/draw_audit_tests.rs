// SPDX-License-Identifier: MIT

use super::*;
use soroban_sdk::testutils::Address as _;
use soroban_sdk::testutils::Ledger;
use soroban_sdk::{token::StellarAssetClient, Vec};

fn setup_default() -> (Env, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let borrower = Address::generate(&env);
    let contract_id = env.register(Credit, ());
    let client = CreditClient::new(&env, &contract_id);

    client.init(&admin);

    let token_id = env.register_stellar_asset_contract_v2(Address::generate(&env));
    let token_address = token_id.address();
    client.set_liquidity_token(&token_address);
    client.set_liquidity_source(&contract_id);

    StellarAssetClient::new(&env, &token_address).mint(&contract_id, &100_000_i128);

    client.open_credit_line(&borrower, &100_000_i128, &300_u32, &50_u32);

    (env, borrower, contract_id)
}

#[test]
fn get_draw_audit_returns_recorded_amount() {
    let (env, borrower, contract_id) = setup_default();
    let client = CreditClient::new(&env, &contract_id);

    env.ledger().with_mut(|li| li.timestamp = 1000);
    client.draw_credit(&borrower, &5_000_i128);
    let ts = env.ledger().timestamp();

    let audit = client.get_draw_audit(&borrower, &ts);
    assert_eq!(audit, Some(5_000_i128));
}

#[test]
fn get_draw_audit_unknown_timestamp_returns_none() {
    let (env, borrower, contract_id) = setup_default();
    let client = CreditClient::new(&env, &contract_id);

    let audit = client.get_draw_audit(&borrower, &0_u64);
    assert_eq!(audit, None);
}

#[test]
fn get_draw_audit_nonexistent_borrower_returns_none() {
    let (env, _borrower, contract_id) = setup_default();
    let client = CreditClient::new(&env, &contract_id);

    let stranger = Address::generate(&env);
    let audit = client.get_draw_audit(&stranger, &100_u64);
    assert_eq!(audit, None);
}

#[test]
fn enumerate_draw_audit_returns_multiple_draws() {
    let (env, borrower, contract_id) = setup_default();
    let client = CreditClient::new(&env, &contract_id);

    env.ledger().with_mut(|li| li.timestamp = 1000);
    client.draw_credit(&borrower, &2_000_i128);
    let ts1 = env.ledger().timestamp();

    env.ledger().with_mut(|li| li.timestamp = 2000);
    client.draw_credit(&borrower, &3_000_i128);
    let ts2 = env.ledger().timestamp();

    env.ledger().with_mut(|li| li.timestamp = 3000);
    client.draw_credit(&borrower, &4_000_i128);
    let ts3 = env.ledger().timestamp();

    let audit = client.enumerate_draw_audit(&borrower, &None, &10_u32);
    let mut expected = Vec::new(&env);
    expected.push_back((ts3, 4_000_i128));
    expected.push_back((ts2, 3_000_i128));
    expected.push_back((ts1, 2_000_i128));
    assert_eq!(audit, expected);
}

#[test]
fn enumerate_draw_audit_cursor_excludes_upper_bound() {
    let (env, borrower, contract_id) = setup_default();
    let client = CreditClient::new(&env, &contract_id);

    env.ledger().with_mut(|li| li.timestamp = 1000);
    client.draw_credit(&borrower, &2_000_i128);
    let ts1 = env.ledger().timestamp();

    env.ledger().with_mut(|li| li.timestamp = 2000);
    client.draw_credit(&borrower, &3_000_i128);
    let ts2 = env.ledger().timestamp();

    env.ledger().with_mut(|li| li.timestamp = 3000);
    client.draw_credit(&borrower, &4_000_i128);
    let ts3 = env.ledger().timestamp();

    // cursor = ts3 means we exclude entries >= ts3, so only ts2, ts1
    let audit = client.enumerate_draw_audit(&borrower, &Some(ts3), &10_u32);
    let mut expected = Vec::new(&env);
    expected.push_back((ts2, 3_000_i128));
    expected.push_back((ts1, 2_000_i128));
    assert_eq!(audit, expected);
}

#[test]
fn enumerate_draw_audit_respects_limit() {
    let (env, borrower, contract_id) = setup_default();
    let client = CreditClient::new(&env, &contract_id);

    env.ledger().with_mut(|li| li.timestamp = 1000);
    client.draw_credit(&borrower, &2_000_i128);

    env.ledger().with_mut(|li| li.timestamp = 2000);
    client.draw_credit(&borrower, &3_000_i128);

    env.ledger().with_mut(|li| li.timestamp = 3000);
    client.draw_credit(&borrower, &4_000_i128);

    let audit = client.enumerate_draw_audit(&borrower, &None, &2_u32);
    assert_eq!(audit.len(), 2);
}

#[test]
fn enumerate_draw_audit_zero_limit_returns_empty() {
    let (env, borrower, contract_id) = setup_default();
    let client = CreditClient::new(&env, &contract_id);

    env.ledger().with_mut(|li| li.timestamp = 1000);
    client.draw_credit(&borrower, &2_000_i128);

    let audit = client.enumerate_draw_audit(&borrower, &None, &0_u32);
    assert_eq!(audit.len(), 0);
}

#[test]
fn enumerate_draw_audit_no_draws_returns_empty() {
    let (env, borrower, contract_id) = setup_default();
    let client = CreditClient::new(&env, &contract_id);

    let audit = client.enumerate_draw_audit(&borrower, &None, &10_u32);
    assert_eq!(audit.len(), 0);
}

#[test]
fn enumerate_draw_audit_nonexistent_borrower_returns_empty() {
    let (env, _borrower, contract_id) = setup_default();
    let client = CreditClient::new(&env, &contract_id);

    let stranger = Address::generate(&env);
    let audit = client.enumerate_draw_audit(&stranger, &None, &10_u32);
    assert_eq!(audit.len(), 0);
}
