#![cfg(test)]
//! Trims end-to-end integration.
//!
//! Every counterparty here is the real thing: Blend V2's own pool fixture,
//! Soroswap's deployed factory/router/pair bytecode, and the compiled Trims
//! manager and receiver wasm. Nothing is stubbed.
//!
//! The user's wallet is emptied of both assets before the call, so the
//! repayment provably comes from the flash-loaned collateral and nowhere else.
//!
//! ## What this cannot prove
//!
//! Authorisation. The receiver authorises the transfer Soroswap makes on its
//! behalf via `authorize_as_current_contract`, and no mocked auth mode checks
//! that: `mock_all_auths` refuses non-root invoker auth outright, while
//! `mock_all_auths_allowing_non_root_auth` accepts it without inspecting the
//! entries — a deliberately incorrect entry passes just as happily. Only
//! enforcing-mode execution on a network validates it.

use soroban_sdk::{testutils::Address as _, vec as svec, Address};
use test_suites::{
    create_fixture_with_data,
    test_fixture::{TokenIndex, SCALAR_7},
};

mod trims_manager {
    soroban_sdk::contractimport!(file = "../trims-artifacts/trims_manager.wasm");
}
mod trims_receiver {
    soroban_sdk::contractimport!(file = "../trims-artifacts/trims_receiver.wasm");
}
mod soroswap_factory {
    soroban_sdk::contractimport!(file = "../trims-artifacts/soroswap_factory.optimized.wasm");
}
mod soroswap_router {
    soroban_sdk::contractimport!(file = "../trims-artifacts/soroswap_router.optimized.wasm");
}
mod soroswap_pair {
    soroban_sdk::contractimport!(file = "../trims-artifacts/soroswap_pair.optimized.wasm");
}

#[test]
fn deleverages_through_real_soroswap_from_an_empty_wallet() {
    let fixture = create_fixture_with_data(false);
    let e = &fixture.env;
    let pool = &fixture.pools[0].pool;

    let xlm = &fixture.tokens[TokenIndex::XLM];
    let stable = &fixture.tokens[TokenIndex::STABLE];
    let unit6 = 10i128.pow(6);

    // ---------------------------------------------------------------- Soroswap
    // Real factory + router bytecode, and a real pair deployed by the factory.
    let factory_id = e.register(soroswap_factory::WASM, ());
    let factory = soroswap_factory::Client::new(e, &factory_id);
    let pair_hash = e.deployer().upload_contract_wasm(soroswap_pair::WASM);
    factory.initialize(&fixture.bombadil, &pair_hash);

    let router_id = e.register(soroswap_router::WASM, ());
    let router = soroswap_router::Client::new(e, &router_id);
    router.initialize(&factory_id);

    // Deep pool at the fixture's oracle rate: XLM $0.10, STABLE $1.
    let lp = Address::generate(e);
    xlm.mint(&lp, &(5_000_000 * SCALAR_7));
    stable.mint(&lp, &(500_000 * unit6));
    router.add_liquidity(
        &xlm.address,
        &stable.address,
        &(5_000_000 * SCALAR_7),
        &(500_000 * unit6),
        &0,
        &0,
        &lp,
        &u64::MAX,
    );
    println!("soroswap pair: {:?}", router.router_pair_for(&xlm.address, &stable.address));

    // ------------------------------------------------------------------ Trims
    let manager_id = e.register(trims_manager::WASM, ());
    let receiver_id = e.register(trims_receiver::WASM, ());
    trims_receiver::Client::new(e, &receiver_id).init(&manager_id);
    let manager = trims_manager::Client::new(e, &manager_id);
    manager.init(&receiver_id);

    // ------------------------------------------------------------- The position
    // 100,000 XLM collateral ($10,000) against 1,000 STABLE of debt.
    let user = Address::generate(e);
    xlm.mint(&user, &(100_000 * SCALAR_7));
    let until = e.ledger().sequence() + 17280;
    xlm.approve(&user, &pool.address, &i128::MAX, &until);
    stable.approve(&user, &pool.address, &i128::MAX, &until);

    pool.submit(
        &user,
        &user,
        &user,
        &svec![
            e,
            pool::Request {
                request_type: pool::RequestType::SupplyCollateral as u32,
                address: xlm.address.clone(),
                amount: 100_000 * SCALAR_7,
            },
            pool::Request {
                request_type: pool::RequestType::Borrow as u32,
                address: stable.address.clone(),
                amount: 1_000 * unit6,
            },
        ],
    );

    // The decisive step: leave the user holding nothing at all.
    let sink = Address::generate(e);
    stable.transfer(&user, &sink, &stable.balance(&user));
    xlm.transfer(&user, &sink, &xlm.balance(&user));
    assert_eq!(stable.balance(&user), 0, "wallet must hold no STABLE");
    assert_eq!(xlm.balance(&user), 0, "wallet must hold no XLM");
    println!("wallet emptied -> 0 XLM, 0 STABLE");

    let before = pool.get_positions(&user);
    println!(
        "before -> collateral {:?} | debt {:?}",
        before.collateral, before.liabilities
    );

    // The receiver authorises the transfer Soroswap makes on its behalf; see
    // the module docs for why no mocked mode can verify that.
    e.mock_all_auths_allowing_non_root_auth();

    // -------------------------------------------------------------- Deleverage
    // 11,000 XLM sells for roughly 1,094 STABLE after the 0.3% fee and curve
    // slippage, comfortably above the 1,005 floor.
    let flash = 11_000 * SCALAR_7;
    let repay = 1_005 * unit6;
    manager.deleverage(
        &user,
        &pool.address,
        &router_id,
        &xlm.address,
        &stable.address,
        &flash,
        &(flash + 10_000), // unwind: absorbs Blend's d-token rounding gap
        &repay,
        &repay, // min_out == repay: the swap must cover the repayment alone
        &u64::MAX,
    );

    let after = pool.get_positions(&user);
    println!(
        "after  -> collateral {:?} | debt {:?}",
        after.collateral, after.liabilities
    );
    println!(
        "wallet after -> {} XLM, {} STABLE",
        xlm.balance(&user) as f64 / SCALAR_7 as f64,
        stable.balance(&user) as f64 / unit6 as f64
    );

    // The debt is gone, paid for entirely out of collateral the user never held.
    assert_eq!(after.liabilities.len(), 0, "all debt should be cleared");
    assert!(after.collateral.len() > 0, "collateral should remain");

    // Trims kept nothing.
    assert_eq!(
        xlm.balance(&receiver_id),
        0,
        "receiver must not retain collateral"
    );
    assert_eq!(
        stable.balance(&receiver_id),
        0,
        "receiver must not retain proceeds"
    );
    assert_eq!(xlm.balance(&manager_id), 0, "manager must never hold funds");
    assert_eq!(stable.balance(&manager_id), 0, "manager must never hold funds");
}
