#![no_std]
//! # Trims receiver
//!
//! The contract Blend calls back during a flash loan. It is deliberately
//! separate from [`trims-manager`]: Soroban forbids contract re-entry, so the
//! contract that initiates `flash_loan` cannot also be the receiver.
//!
//! It is armed for exactly one callback, consumes that arming when invoked, and
//! sweeps every token it touches to the user. It never holds funds.

use soroban_sdk::{
    auth::{ContractContext, InvokerContractAuthEntry, SubContractInvocation},
    contract, contractclient, contracterror, contractimpl, contracttype, panic_with_error, token,
    vec, Address, Env, IntoVal, Symbol, Vec,
};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum ReceiverError {
    AlreadyInitialised = 1,
    NotInitialised = 2,
    /// `exec_op` ran without a matching arming.
    NotArmed = 3,
    /// The callback does not match what was armed.
    ArmingMismatch = 4,
    /// The swap returned less than the floor.
    SlippageExceeded = 5,
    /// The contract still held a balance after settling.
    ResidualBalance = 6,
}

#[contracttype]
enum DataKey {
    /// The manager permitted to arm this receiver.
    Manager,
    /// One-shot instructions for a single user, valid only inside the
    /// transaction that armed them.
    Armed(Address),
}

#[contracttype]
#[derive(Clone)]
pub struct Arming {
    pub router: Address,
    pub collateral: Address,
    pub debt: Address,
    pub flash_amount: i128,
    pub min_out: i128,
    pub deadline: u64,
}

#[allow(dead_code)] // only the generated client is used
#[contractclient(name = "RouterClient")]
pub trait SoroswapRouter {
    fn swap_exact_tokens_for_tokens(
        e: Env,
        amount_in: i128,
        amount_out_min: i128,
        path: Vec<Address>,
        to: Address,
        deadline: u64,
    ) -> Vec<i128>;

    /// Address of the pool the router will route a two-token path through.
    /// Needed to authorise the transfer the router makes on our behalf.
    fn router_pair_for(e: Env, token_a: Address, token_b: Address) -> Address;
}

#[contract]
pub struct Receiver;

#[contractimpl]
impl Receiver {
    pub fn init(e: Env, manager: Address) {
        if e.storage().instance().has(&DataKey::Manager) {
            panic_with_error!(&e, ReceiverError::AlreadyInitialised);
        }
        e.storage().instance().set(&DataKey::Manager, &manager);
    }

    pub fn manager(e: Env) -> Address {
        match e.storage().instance().get(&DataKey::Manager) {
            Some(m) => m,
            None => panic_with_error!(&e, ReceiverError::NotInitialised),
        }
    }

    /// Arm the receiver for one callback on `user`'s behalf. Manager-only.
    pub fn arm(e: Env, user: Address, arming: Arming) {
        Self::manager(e.clone()).require_auth();
        e.storage().temporary().set(&DataKey::Armed(user), &arming);
    }

    /// Blend's flash-loan callback. Consumes the arming, so it is one-shot and
    /// cannot be replayed inside or after the transaction.
    pub fn exec_op(e: Env, caller: Address, token_in: Address, amount: i128, _fee: i128) {
        let key = DataKey::Armed(caller.clone());
        let armed: Arming = match e.storage().temporary().get(&key) {
            Some(a) => a,
            None => panic_with_error!(&e, ReceiverError::NotArmed),
        };
        e.storage().temporary().remove(&key);

        if token_in != armed.collateral || amount != armed.flash_amount {
            panic_with_error!(&e, ReceiverError::ArmingMismatch);
        }

        let me = e.current_contract_address();
        let path = vec![&e, armed.collateral.clone(), armed.debt.clone()];
        let router = RouterClient::new(&e, &armed.router);

        // Soroswap moves the input with `transfer(from = us)`, not
        // `transfer_from`, so no allowance is granted — the router never gets
        // standing permission over our balance. But that transfer is issued by
        // the router rather than by us, so invoker auth does not cover it and
        // we have to authorise it ourselves.
        //
        // The entry is top-level, not nested under the swap: the router's own
        // `to.require_auth()` is already satisfied by invoker auth because we
        // call it directly, so nothing is consumed at that level and an entry
        // nested beneath it would never be reached.
        //
        // NOTE: this construction cannot be validated by any mocked test.
        // `mock_all_auths` rejects non-root invoker auth outright, and
        // `mock_all_auths_allowing_non_root_auth` accepts it without checking
        // the entries -- we confirmed a deliberately wrong entry still passes.
        // Only enforcing-mode execution on a network actually verifies this.
        let pair = router.router_pair_for(&armed.collateral, &armed.debt);
        e.authorize_as_current_contract(vec![
            &e,
            InvokerContractAuthEntry::Contract(SubContractInvocation {
                context: ContractContext {
                    contract: armed.collateral.clone(),
                    fn_name: Symbol::new(&e, "transfer"),
                    args: (me.clone(), pair, amount).into_val(&e),
                },
                sub_invocations: vec![&e],
            }),
        ]);

        // Measure what actually arrived rather than trusting the router's
        // return value. The router is caller-supplied and has just been handed
        // the collateral, so its self-report is the one thing that must not be
        // taken on faith: a hostile or non-conforming router could report a
        // healthy figure and deliver less, or nothing.
        let debt_token = token::TokenClient::new(&e, &armed.debt);
        let before = debt_token.balance(&me);

        router.swap_exact_tokens_for_tokens(&amount, &armed.min_out, &path, &me, &armed.deadline);

        let received = debt_token.balance(&me) - before;
        if received < armed.min_out {
            panic_with_error!(&e, ReceiverError::SlippageExceeded);
        }

        // Hand everything to the user and keep nothing. Sweeping the whole
        // balance also flushes any stray donation.
        Self::sweep(&e, &armed.debt, &caller);
        Self::sweep(&e, &armed.collateral, &caller);

        if token::TokenClient::new(&e, &armed.debt).balance(&me) != 0
            || token::TokenClient::new(&e, &armed.collateral).balance(&me) != 0
        {
            panic_with_error!(&e, ReceiverError::ResidualBalance);
        }
    }
}

impl Receiver {
    fn sweep(e: &Env, asset: &Address, to: &Address) {
        let client = token::TokenClient::new(e, asset);
        let balance = client.balance(&e.current_contract_address());
        if balance > 0 {
            client.transfer(&e.current_contract_address(), to, &balance);
        }
    }
}

#[cfg(test)]
mod test;
