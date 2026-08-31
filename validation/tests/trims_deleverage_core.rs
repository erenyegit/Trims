#![cfg(test)]
//! Trims — core mechanic validation.
//!
//! Question: does one-click deleveraging work against Blend V2's actual
//! flash-loan execution order?
//!
//! Hypothesis: flash-borrow the COLLATERAL asset, not the debt asset. The
//! receiver then holds a sellable asset during `exec_op`, and Blend's
//! same-asset netting cancels the withdrawal against the flash repayment.

use pool::{FlashLoan, Request, RequestType};
use soroban_sdk::{
    contract, contractimpl, testutils::Address as _, token, vec as svec, Address, Env, Symbol,
};
use test_suites::{
    create_fixture_with_data,
    test_fixture::{TokenIndex, SCALAR_7},
};

/// Stands in for a DEX: receives the flash-loaned XLM and hands the caller
/// STABLE at the oracle rate (XLM $0.10, STABLE $1; 7 vs 6 decimals).
#[contract]
pub struct MockSwapReceiver;

#[contractimpl]
impl MockSwapReceiver {
    pub fn init(e: Env, stable: Address) {
        e.storage().instance().set(&Symbol::new(&e, "stable"), &stable);
    }

    pub fn exec_op(e: Env, caller: Address, _token_in: Address, amount: i128, _fee: i128) {
        let stable: Address = e.storage().instance().get(&Symbol::new(&e, "stable")).unwrap();
        let proceeds = amount / 100;
        token::TokenClient::new(&e, &stable)
            .transfer(&e.current_contract_address(), &caller, &proceeds);
    }
}

#[test]
fn trims_deleverage_with_collateral_flash() {
    let fixture = create_fixture_with_data(false);
    let pool = &fixture.pools[0].pool;
    let e = &fixture.env;

    let xlm = &fixture.tokens[TokenIndex::XLM];
    let stable = &fixture.tokens[TokenIndex::STABLE];
    let unit6 = 10i128.pow(6);

    let receiver_id = Address::generate(e);
    e.register_at(&receiver_id, MockSwapReceiver {}, ());
    MockSwapReceiverClient::new(e, &receiver_id).init(&stable.address);
    stable.mint(&receiver_id, &(1_000_000 * unit6));

    // Position: 100,000 XLM collateral, 1,000 STABLE debt.
    // (Borrow is sized to the fixture's STABLE utilisation headroom — the pool
    // is pre-loaded to 80% and max_util is 95%.)
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

    let before = pool.get_positions(&user);
    println!("before -> collateral {:?} | debt {:?}", before.collateral, before.liabilities);
    assert!(before.liabilities.len() > 0, "expected a debt position");

    // Flash the COLLATERAL asset. The +10_000 stroop buffer absorbs the
    // rounding gap between to_d_token_up (mint) and to_d_token_down (burn);
    // Blend refunds any repayment overage.
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
    assert_eq!(result.liabilities.len(), 0, "all debt should be cleared");
    assert!(result.collateral.len() > 0, "collateral should remain");
}
