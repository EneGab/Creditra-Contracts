// SPDX-License-Identifier: MIT

use creditra_credit::events::FeeAccruedEvent;
use creditra_credit::{Credit, CreditClient};
use soroban_sdk::testutils::{Address as _, Events, Ledger};
use soroban_sdk::{symbol_short, token, Address, Env, Symbol, TryFromVal, TryIntoVal};

fn setup() -> (Env, Address, Address, Address, Address, Address) {
    let env = Env::default();
    env.mock_all_auths_allowing_non_root_auth();

    let admin = Address::generate(&env);
    let borrower = Address::generate(&env);
    let reserve = Address::generate(&env);
    let treasury = Address::generate(&env);
    let contract_id = env.register(Credit, ());
    let client = CreditClient::new(&env, &contract_id);
    client.init(&admin);

    let token_id = env.register_stellar_asset_contract_v2(Address::generate(&env));
    let token_address = token_id.address();

    client.set_liquidity_token(&token_address);
    client.set_liquidity_source(&reserve);
    client.set_treasury(&admin, &treasury);

    (env, contract_id, token_address, borrower, reserve, treasury)
}

fn prepare_repay<'a>(
    env: &'a Env,
    contract_id: &'a Address,
    token_address: &'a Address,
    borrower: &'a Address,
    reserve: &'a Address,
    draw_amount: i128,
    repay_amount: i128,
    interest_rate_bps: u32,
    fee_bps: u32,
) -> CreditClient<'a> {
    let client = CreditClient::new(env, contract_id);
    client.open_credit_line(borrower, &draw_amount, &interest_rate_bps, &50_u32);

    let asset = token::StellarAssetClient::new(env, token_address);
    asset.mint(reserve, &draw_amount);

    // Default collateral floor is 150%; over-collateralize in tests so the
    // repay assertions isolate protocol-fee behavior.
    let collateral_amount = draw_amount
        .checked_mul(2)
        .expect("test collateral amount overflow");
    asset.mint(borrower, &collateral_amount);
    client.deposit_collateral(borrower, &collateral_amount);

    client.draw_credit(borrower, &draw_amount);

    client.set_protocol_fee_bps(&fee_bps);

    env.ledger()
        .with_mut(|ledger| ledger.timestamp = 31_557_600);

    asset.mint(borrower, &repay_amount);
    token::Client::new(env, token_address).approve(
        borrower,
        contract_id,
        &repay_amount,
        &1_000_000_u32,
    );

    client
}

fn last_fee_event(env: &Env) -> Option<FeeAccruedEvent> {
    for (_contract, topics, data) in env.events().all().iter().rev() {
        if topics.len() < 2 {
            continue;
        }
        let Ok(topic0) = Symbol::try_from_val(env, &topics.get(0).unwrap()) else {
            continue;
        };
        let Ok(topic1) = Symbol::try_from_val(env, &topics.get(1).unwrap()) else {
            continue;
        };
        if topic0 == symbol_short!("credit") && topic1 == symbol_short!("fee_accrd") {
            return data.try_into_val(env).ok();
        }
    }
    None
}

#[test]
fn protocol_fee_zero_fee_keeps_treasury_balance_at_zero() {
    let (env, contract_id, token_address, borrower, reserve, treasury) = setup();
    let client = prepare_repay(
        &env,
        &contract_id,
        &token_address,
        &borrower,
        &reserve,
        1_000,
        1_100,
        1_000,
        0,
    );

    assert_eq!(client.get_protocol_fee_bps(), Some(0));
    assert_eq!(client.get_treasury(), Some(treasury.clone()));

    let token_client = token::Client::new(&env, &token_address);
    let contract_balance_before = token_client.balance(&contract_id);
    let reserve_balance_before = token_client.balance(&reserve);
    let treasury_balance_before = token_client.balance(&treasury);

    client.repay_credit(&borrower, &1_100);
    assert!(last_fee_event(&env).is_none());

    assert_eq!(client.get_treasury_balance(), 0);
    assert_eq!(
        client.get_credit_line(&borrower).unwrap().utilized_amount,
        0
    );
    assert_eq!(token_client.balance(&contract_id), contract_balance_before);
    assert_eq!(
        token_client.balance(&reserve),
        reserve_balance_before + 1_100
    );
    assert_eq!(token_client.balance(&treasury), treasury_balance_before);
}

mod repay {
    use super::*;

    #[test]
    fn fee_skim() {
        let (env, contract_id, token_address, borrower, reserve, treasury) = setup();
        let client = prepare_repay(
            &env,
            &contract_id,
            &token_address,
            &borrower,
            &reserve,
            1_000,
            1_100,
            1_000,
            1_000,
        );

        let token_client = token::Client::new(&env, &token_address);
        let contract_balance_before = token_client.balance(&contract_id);
        let reserve_balance_before = token_client.balance(&reserve);

        client.repay_credit(&borrower, &1_100);
        let event = last_fee_event(&env).expect("FeeAccruedEvent should be emitted");
        assert_eq!(event.borrower, borrower);
        assert_eq!(event.fee_amount, 10);
        assert_eq!(event.new_treasury_balance, 10);

        // One year at 10% on 1_000 accrues 100 interest; max protocol fee is
        // 1_000 bps (10%) of that interest = 10.
        assert_eq!(client.get_treasury_balance(), 10);
        assert_eq!(
            token_client.balance(&contract_id),
            contract_balance_before + 10
        );
        assert_eq!(
            token_client.balance(&reserve),
            reserve_balance_before + 1_090
        );
        assert_eq!(token_client.balance(&treasury), 0);

        let line = client.get_credit_line(&borrower).unwrap();
        assert_eq!(line.accrued_interest, 0);
        assert_eq!(line.utilized_amount, 10);
    }
}

#[test]
fn protocol_fee_rounding_edge_floors_small_fee_to_zero() {
    let (env, contract_id, token_address, borrower, reserve, treasury) = setup();
    let client = prepare_repay(
        &env,
        &contract_id,
        &token_address,
        &borrower,
        &reserve,
        10_000,
        10_001,
        1,
        1_000,
    );

    let token_client = token::Client::new(&env, &token_address);
    let contract_balance_before = token_client.balance(&contract_id);
    let reserve_balance_before = token_client.balance(&reserve);

    client.repay_credit(&borrower, &10_001);
    assert!(last_fee_event(&env).is_none());

    assert_eq!(client.get_treasury_balance(), 0);
    assert_eq!(token_client.balance(&contract_id), contract_balance_before);
    assert_eq!(
        token_client.balance(&reserve),
        reserve_balance_before + 10_001
    );
    assert_eq!(token_client.balance(&treasury), 0);
    assert_eq!(
        client.get_credit_line(&borrower).unwrap().utilized_amount,
        0
    );
}
