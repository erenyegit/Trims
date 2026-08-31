#![cfg(test)]
//! Receiver guard rails. The receiver is the only contract Blend calls back
//! into, so its refusal paths matter more than its happy path.

use crate::{Arming, Receiver, ReceiverClient, ReceiverError};
use soroban_sdk::{
    contract, contractimpl, symbol_short,
    testutils::{Address as _, AuthorizedFunction, AuthorizedInvocation},
    token::{StellarAssetClient, TokenClient},
    vec, Address, Env, Vec,
};

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

/// Reports a healthy fill in its return value while delivering less. Models a
/// hostile or non-conforming router, which the receiver must not take on faith.
///
/// Lives in its own module: `contractimpl` emits module-level symbols, so two
/// contracts sharing a method name would collide.
mod liar {
    use super::*;

    #[contract]
    pub struct LyingRouter;

    #[contractimpl]
    impl LyingRouter {
        pub fn swap_exact_tokens_for_tokens(
            e: Env,
            amount_in: i128,
            amount_out_min: i128,
            path: Vec<Address>,
            to: Address,
            _deadline: u64,
        ) -> Vec<i128> {
            let me = e.current_contract_address();
            TokenClient::new(&e, &path.first().unwrap()).transfer_from(&me, &to, &me, &amount_in);
            // Hand over a single stroop...
            TokenClient::new(&e, &path.last().unwrap()).transfer(&me, &to, &1);
            // ...and claim the floor was met.
            vec![&e, amount_in, amount_out_min]
        }
    }
}
use liar::LyingRouter;

struct Fx {
    e: Env,
    rx: ReceiverClient<'static>,
    rx_id: Address,
    manager: Address,
    collateral: Address,
    debt: Address,
    user: Address,
    arming: Arming,
}

const FLASH: i128 = 100_000;

fn setup() -> Fx {
    let e = Env::default();
    e.mock_all_auths();

    let admin = Address::generate(&e);
    let collateral = e
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let debt = e.register_stellar_asset_contract_v2(admin).address();

    let manager = Address::generate(&e);
    let rx_id = e.register(Receiver, ());
    let rx = ReceiverClient::new(&e, &rx_id);
    rx.init(&manager);

    let router = e.register(StubRouter, ());
    StubRouterClient::new(&e, &router).init(&1, &100);
    StellarAssetClient::new(&e, &debt).mint(&router, &1_000_000_000);

    let user = Address::generate(&e);
    let arming = Arming {
        router,
        collateral: collateral.clone(),
        debt: debt.clone(),
        flash_amount: FLASH,
        min_out: 900,
        deadline: u64::MAX,
    };
    Fx {
        e,
        rx,
        rx_id,
        manager,
        collateral,
        debt,
        user,
        arming,
    }
}

impl Fx {
    /// Simulate Blend handing the receiver the flash amount.
    fn fund(&self) {
        StellarAssetClient::new(&self.e, &self.collateral).mint(&self.rx_id, &FLASH);
    }
}

#[test]
fn cold_callback_is_refused() {
    let fx = setup();
    fx.fund();
    assert_eq!(
        fx.rx.try_exec_op(&fx.user, &fx.collateral, &FLASH, &0),
        Err(Ok(ReceiverError::NotArmed.into()))
    );
}

#[test]
fn arming_is_one_shot() {
    let fx = setup();
    fx.rx.arm(&fx.user, &fx.arming);
    fx.fund();
    fx.rx.exec_op(&fx.user, &fx.collateral, &FLASH, &0);

    // A replay inside the same transaction finds nothing.
    fx.fund();
    assert_eq!(
        fx.rx.try_exec_op(&fx.user, &fx.collateral, &FLASH, &0),
        Err(Ok(ReceiverError::NotArmed.into()))
    );
}

#[test]
fn arming_is_bound_to_one_user() {
    let fx = setup();
    fx.rx.arm(&fx.user, &fx.arming);
    fx.fund();
    let stranger = Address::generate(&fx.e);
    assert_eq!(
        fx.rx.try_exec_op(&stranger, &fx.collateral, &FLASH, &0),
        Err(Ok(ReceiverError::NotArmed.into()))
    );
}

#[test]
fn callback_must_match_the_arming() {
    let fx = setup();

    fx.rx.arm(&fx.user, &fx.arming);
    fx.fund();
    assert_eq!(
        fx.rx.try_exec_op(&fx.user, &fx.debt, &FLASH, &0), // wrong asset
        Err(Ok(ReceiverError::ArmingMismatch.into()))
    );

    fx.rx.arm(&fx.user, &fx.arming);
    assert_eq!(
        fx.rx
            .try_exec_op(&fx.user, &fx.collateral, &(FLASH + 1), &0), // wrong amount
        Err(Ok(ReceiverError::ArmingMismatch.into()))
    );
}

#[test]
fn arming_requires_the_manager() {
    let fx = setup();
    fx.rx.arm(&fx.user, &fx.arming);

    let auths = fx.e.auths();
    let (who, invocation) = auths.first().expect("arm must require an authorisation");
    assert_eq!(who, &fx.manager, "only the manager may arm the receiver");
    assert!(matches!(
        invocation,
        AuthorizedInvocation {
            function: AuthorizedFunction::Contract(_),
            ..
        }
    ));
}

#[test]
fn settles_to_the_user_and_retains_nothing() {
    let fx = setup();
    fx.rx.arm(&fx.user, &fx.arming);
    fx.fund();
    fx.rx.exec_op(&fx.user, &fx.collateral, &FLASH, &0);

    assert_eq!(TokenClient::new(&fx.e, &fx.debt).balance(&fx.user), 1_000);
    assert_eq!(TokenClient::new(&fx.e, &fx.debt).balance(&fx.rx_id), 0);
    assert_eq!(
        TokenClient::new(&fx.e, &fx.collateral).balance(&fx.rx_id),
        0
    );
}

#[test]
fn a_donation_is_flushed_to_the_user_not_kept() {
    let fx = setup();
    fx.rx.arm(&fx.user, &fx.arming);
    fx.fund();
    // Someone dusts the receiver mid-flight.
    StellarAssetClient::new(&fx.e, &fx.debt).mint(&fx.rx_id, &42);

    fx.rx.exec_op(&fx.user, &fx.collateral, &FLASH, &0);

    assert_eq!(TokenClient::new(&fx.e, &fx.debt).balance(&fx.user), 1_042);
    assert_eq!(TokenClient::new(&fx.e, &fx.debt).balance(&fx.rx_id), 0);
}

#[test]
fn a_lying_router_cannot_under_deliver() {
    let fx = setup();
    let liar = fx.e.register(LyingRouter, ());
    StellarAssetClient::new(&fx.e, &fx.debt).mint(&liar, &1_000_000);

    let mut arming = fx.arming.clone();
    arming.router = liar;
    fx.rx.arm(&fx.user, &arming);
    fx.fund();

    // The router's return value claims the floor was met; only the balance
    // delta reveals that it was not.
    assert_eq!(
        fx.rx.try_exec_op(&fx.user, &fx.collateral, &FLASH, &0),
        Err(Ok(ReceiverError::SlippageExceeded.into()))
    );
}

#[test]
fn a_prior_donation_cannot_be_counted_as_swap_output() {
    let fx = setup();
    let liar = fx.e.register(LyingRouter, ());
    StellarAssetClient::new(&fx.e, &fx.debt).mint(&liar, &1_000_000);

    // Seed the receiver with more than min_out before the swap. A balance
    // *snapshot* check would pass here; a delta check must not.
    StellarAssetClient::new(&fx.e, &fx.debt).mint(&fx.rx_id, &5_000);

    let mut arming = fx.arming.clone();
    arming.router = liar;
    fx.rx.arm(&fx.user, &arming);
    fx.fund();

    assert_eq!(
        fx.rx.try_exec_op(&fx.user, &fx.collateral, &FLASH, &0),
        Err(Ok(ReceiverError::SlippageExceeded.into()))
    );
}

#[test]
fn cannot_be_reinitialised() {
    let fx = setup();
    let other = Address::generate(&fx.e);
    assert_eq!(
        fx.rx.try_init(&other),
        Err(Ok(ReceiverError::AlreadyInitialised.into()))
    );
    assert_eq!(fx.rx.manager(), fx.manager);
}
