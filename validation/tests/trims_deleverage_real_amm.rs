#![cfg(test)]
//! Trims — strict validation.
//!
//! Closes the two gaps left by the core test:
//!   1. The user's wallet is EMPTIED of both assets before deleveraging, so the
//!      repayment provably cannot come from their own funds.
//!   2. No mock DEX. Settlement runs through a live Comet AMM (constant-product,
//!      0.3% swap fee) with real slippage.

use pool::{FlashLoan, Request, RequestType};
use soroban_sdk::{
    contract, contractimpl, testutils::Address as _, token, vec as svec, Address, Env, Symbol,
};
use test_suites::{
    create_fixture_with_data,
    liquidity_pool::{LPClient, LP_WASM},
    test_fixture::{TokenIndex, SCALAR_7},
};

/// Flash-loan receiver: swaps the borrowed collateral asset for the debt asset
/// on a real AMM and forwards the proceeds to the user.
#[contract]
pub struct AmmReceiver;

#[contractimpl]
impl AmmReceiver {
    pub fn init(e: Env, amm: Address, stable: Address) {
        e.storage().instance().set(&Symbol::new(&e, "amm"), &amm);
        e.storage().instance().set(&Symbol::new(&e, "stable"), &stable);
    }

    pub fn exec_op(e: Env, caller: Address, token_in: Address, amount: i128, _fee: i128) {
        let amm: Address = e.storage().instance().get(&Symbol::new(&e, "amm")).unwrap();
        let stable: Address = e.storage().instance().get(&Symbol::new(&e, "stable")).unwrap();
        let me = e.current_contract_address();

        // NOTE: in production this sub-invocation must be authorised via
        // e.authorize_as_current_contract(...). The test harness uses
        // mock_all_auths_allowing_non_root_auth() instead.
        token::TokenClient::new(&e, &token_in)
            .approve(&me, &amm, &amount, &(e.ledger().sequence() + 1000));

        let (out, _price) = LPClient::new(&e, &amm)
            .swap_exact_amount_in(&token_in, &amount, &stable, &0_i128, &i128::MAX, &me);

        token::TokenClient::new(&e, &stable).transfer(&me, &caller, &out);
    }
}

#[test]
fn trims_deleverage_real_amm_empty_wallet() {
    let fixture = create_fixture_with_data(false);
    let pool = &fixture.pools[0].pool;
    let e = &fixture.env;

    let xlm = &fixture.tokens[TokenIndex::XLM];
    let stable = &fixture.tokens[TokenIndex::STABLE];
    let unit6 = 10i128.pow(6);

    // Real AMM: 10M XLM / 1M STABLE, 50-50 weights, 0.3% fee.
    let amm_id = Address::generate(e);
    e.register_at(&amm_id, LP_WASM, ());
    let amm = LPClient::new(e, &amm_id);
    let lp_admin = fixture.bombadil.clone();
    xlm.mint(&lp_admin, &(10_000_000 * SCALAR_7));
    stable.mint(&lp_admin, &(1_000_000 * unit6));
    amm.init(
        &lp_admin,
        &svec![e, xlm.address.clone(), stable.address.clone()],
        &svec![e, 0_5000000_i128, 0_5000000_i128],
        &svec![e, 10_000_000 * SCALAR_7, 1_000_000 * unit6],
        &0_0030000_i128,
    );

    let receiver_id = Address::generate(e);
    e.register_at(&receiver_id, AmmReceiver {}, ());
    AmmReceiverClient::new(e, &receiver_id).init(&amm_id, &stable.address);

    // Position: 100,000 XLM collateral, 1,000 STABLE debt.
    let user = Address::generate(e);
    xlm.mint(&user, &(100_000 * SCALAR_7));
    let until = e.ledger().sequence() + 17280;
    xlm.approve(&user, &pool.address, &i128::MAX, &until);
    stable.approve(&user, &pool.address, &i128::MAX, &until);

    pool.submit(&user, &user, &user, &svec![e,
        Request { request_type: RequestType::SupplyCollateral as u32,
                  address: xlm.address.clone(), amount: 100_000 * SCALAR_7 },
        Request { request_type: RequestType::Borrow as u32,
                  address: stable.address.clone(), amount: 1_000 * unit6 },
    ]);

    // ---- The decisive step: strip the wallet bare ----
    let sink = Address::generate(e);
    stable.transfer(&user, &sink, &stable.balance(&user));
    xlm.transfer(&user, &sink, &xlm.balance(&user));
    assert_eq!(stable.balance(&user), 0, "wallet must hold no STABLE");
    assert_eq!(xlm.balance(&user), 0, "wallet must hold no XLM");
    println!("wallet emptied -> 0 XLM, 0 STABLE");

    let before = pool.get_positions(&user);
    println!("before -> collateral {:?} | debt {:?}", before.collateral, before.liabilities);

    // The receiver authorises its own AMM sub-invocations.
    e.mock_all_auths_allowing_non_root_auth();

    let flash_xlm = 11_000 * SCALAR_7;
    let result = pool.flash_loan(
        &user,
        &FlashLoan { contract: receiver_id.clone(), asset: xlm.address.clone(), amount: flash_xlm },
        &svec![e,
            Request { request_type: RequestType::Repay as u32,
                      address: stable.address.clone(), amount: 1_050 * unit6 },
            Request { request_type: RequestType::WithdrawCollateral as u32,
                      address: xlm.address.clone(), amount: flash_xlm + 10_000 },
            Request { request_type: RequestType::Repay as u32,
                      address: xlm.address.clone(), amount: flash_xlm + 10_000 },
        ],
    );

    println!("after  -> collateral {:?} | debt {:?}", result.collateral, result.liabilities);
    println!("wallet after -> {} XLM, {} STABLE",
             xlm.balance(&user) as f64 / SCALAR_7 as f64,
             stable.balance(&user) as f64 / unit6 as f64);

    assert_eq!(result.liabilities.len(), 0, "all debt should be cleared");
    assert!(result.collateral.len() > 0, "collateral should remain");
}
