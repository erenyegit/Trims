#![cfg(test)]
//! Guard-rail and end-to-end tests using stub counterparties, so each rule is
//! isolated. Behaviour against real Blend and Soroswap contracts lives in
//! `validation/`.

use crate::{Manager, ManagerClient, ManagerError};
use soroban_sdk::{
    contract, contractimpl, symbol_short,
    testutils::Address as _,
    token::{StellarAssetClient, TokenClient},
    vec, Address, Env, IntoVal, Symbol, Vec,
};
use trims_receiver::{Receiver, ReceiverClient};

/// Mirrors the real pool's ordering: hand the flash amount to the receiver,
/// then invoke its callback.
#[contract]
pub struct StubPool;

#[contractimpl]
impl StubPool {
    pub fn flash_loan(
        e: Env,
        from: Address,
        flash_loan: crate::FlashLoan,
        _requests: Vec<crate::Request>,
    ) -> crate::Positions {
        TokenClient::new(&e, &flash_loan.asset).transfer(
            &e.current_contract_address(),
            &flash_loan.contract,
            &flash_loan.amount,
        );
        e.invoke_contract::<()>(
            &flash_loan.contract,
            &Symbol::new(&e, "exec_op"),
            vec![
                &e,
                from.into_val(&e),
                flash_loan.asset.into_val(&e),
                flash_loan.amount.into_val(&e),
                0i128.into_val(&e),
            ],
        );
        crate::Positions {
            liabilities: soroban_sdk::Map::new(&e),
            collateral: soroban_sdk::Map::new(&e),
            supply: soroban_sdk::Map::new(&e),
        }
    }
}

/// Fixed-rate router that honours `amount_out_min`.
#[contract]
pub struct StubRouter;

#[contractimpl]
impl StubRouter {
    pub fn init(e: Env, num: i128, den: i128) {
        e.storage().instance().set(&symbol_short!("num"), &num);
        e.storage().instance().set(&symbol_short!("den"), &den);
    }

    pub fn swap_exact_tokens_for_tokens(
        e: Env,
        amount_in: i128,
        amount_out_min: i128,
        path: Vec<Address>,
        to: Address,
        _deadline: u64,
    ) -> Vec<i128> {
        let num: i128 = e.storage().instance().get(&symbol_short!("num")).unwrap();
        let den: i128 = e.storage().instance().get(&symbol_short!("den")).unwrap();
        let out = amount_in * num / den;
        assert!(out >= amount_out_min, "router: below floor");
        let me = e.current_contract_address();
        TokenClient::new(&e, &path.first().unwrap()).transfer_from(&me, &to, &me, &amount_in);
        TokenClient::new(&e, &path.last().unwrap()).transfer(&me, &to, &out);
        vec![&e, amount_in, out]
    }
}

struct Fx {
    e: Env,
    mgr: ManagerClient<'static>,
    receiver: Address,
    pool: Address,
    router: Address,
    collateral: Address,
    debt: Address,
    user: Address,
}

fn setup() -> Fx {
    let e = Env::default();
    e.mock_all_auths();

    let admin = Address::generate(&e);
    let collateral = e
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let debt = e.register_stellar_asset_contract_v2(admin).address();

    let mgr_id = e.register(Manager, ());
    let receiver = e.register(Receiver, ());
    ReceiverClient::new(&e, &receiver).init(&mgr_id);
    let mgr = ManagerClient::new(&e, &mgr_id);
    mgr.init(&receiver);

    let pool = e.register(StubPool, ());
    let router = e.register(StubRouter, ());
    StubRouterClient::new(&e, &router).init(&1, &100); // 100 collateral -> 1 debt

    StellarAssetClient::new(&e, &collateral).mint(&pool, &1_000_000_000);
    StellarAssetClient::new(&e, &debt).mint(&router, &1_000_000_000);

    let user = Address::generate(&e);
    Fx {
        e,
        mgr,
        receiver,
        pool,
        router,
        collateral,
        debt,
        user,
    }
}

impl Fx {
    fn call(&self, flash: i128, unwind: i128, repay: i128, min_out: i128) {
        self.mgr.deleverage(
            &self.user,
            &self.pool,
            &self.router,
            &self.collateral,
            &self.debt,
            &flash,
            &unwind,
            &repay,
            &min_out,
            &u64::MAX,
        );
    }

    /// Returns the contract error a rejected call produced, or `None` if the
    /// call did not fail with one.
    fn try_call(
        &self,
        debt: &Address,
        flash: i128,
        unwind: i128,
        repay: i128,
        min_out: i128,
    ) -> Option<soroban_sdk::Error> {
        match self.mgr.try_deleverage(
            &self.user,
            &self.pool,
            &self.router,
            &self.collateral,
            debt,
            &flash,
            &unwind,
            &repay,
            &min_out,
            &u64::MAX,
        ) {
            Err(Ok(e)) => Some(e),
            _ => None,
        }
    }
}

#[test]
fn proceeds_reach_the_user_and_nothing_is_retained() {
    let fx = setup();
    fx.call(100_000, 100_000, 900, 900);

    assert_eq!(TokenClient::new(&fx.e, &fx.debt).balance(&fx.user), 1_000);
    assert_eq!(TokenClient::new(&fx.e, &fx.debt).balance(&fx.receiver), 0);
    assert_eq!(
        TokenClient::new(&fx.e, &fx.collateral).balance(&fx.receiver),
        0
    );
}

#[test]
fn rejects_unbounded_swap() {
    let fx = setup();
    assert_eq!(
        fx.try_call(&fx.debt, 100_000, 100_000, 900, 0),
        Some(ManagerError::MissingSlippageBound.into())
    );
}

#[test]
fn rejects_unwind_below_flash() {
    let fx = setup();
    assert_eq!(
        fx.try_call(&fx.debt, 100_000, 99_999, 900, 900),
        Some(ManagerError::UnwindBelowFlash.into())
    );
}

#[test]
fn rejects_non_positive_amounts() {
    let fx = setup();
    for (f, u, r) in [
        (0i128, 100_000i128, 900i128),
        (100_000, 0, 900),
        (100_000, 100_000, 0),
    ] {
        assert_eq!(
            fx.try_call(&fx.debt, f, u, r, 900),
            Some(ManagerError::InvalidAmount.into())
        );
    }
}

#[test]
fn rejects_same_asset_on_both_legs() {
    let fx = setup();
    let collateral = fx.collateral.clone();
    assert_eq!(
        fx.try_call(&collateral, 100_000, 100_000, 900, 900),
        Some(ManagerError::SameAsset.into())
    );
}

#[test]
fn refuses_when_proceeds_would_not_cover_the_repayment() {
    let fx = setup();
    // Guaranteeing 899 while repaying 900 would leave a 1-unit shortfall for
    // Blend to pull from the user's own wallet.
    assert_eq!(
        fx.try_call(&fx.debt, 100_000, 100_000, 900, 899),
        Some(ManagerError::ProceedsBelowRepayment.into())
    );
}

#[test]
fn slippage_floor_blocks_a_bad_swap() {
    // Router yields 1_000; demand 1_001.
    let fx = setup();
    assert!(
        fx.try_call(&fx.debt, 100_000, 100_000, 900, 1_001)
            .is_some(),
        "a swap below the floor must not settle"
    );
}
