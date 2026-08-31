#![no_std]
//! # Trims manager
//!
//! The user-facing entry point. Validates the request, arms the receiver, and
//! initiates the Blend flash loan.
//!
//! ## Why the collateral asset is flash-borrowed, not the debt asset
//!
//! Blend validates position health *before* invoking the flash-loan receiver.
//! By then, withdrawn collateral has not been transferred to anyone, so a
//! receiver cannot sell it to fund the repayment. Flash-borrowing the
//! **collateral** asset inverts this: the receiver holds a sellable asset the
//! moment it is called. Blend nets same-asset flows before settling, so the
//! collateral withdrawal and the flash repayment cancel and the collateral
//! never touches the user's wallet.
//!
//! ## Why this is two contracts
//!
//! Soroban forbids contract re-entry. `manager -> pool -> manager` would trap,
//! so the receiver is a separate contract that is not on the call stack when
//! Blend calls back.

use soroban_sdk::{
    contract, contractclient, contracterror, contractimpl, contracttype, panic_with_error, vec,
    Address, Env, Map, Vec,
};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum ManagerError {
    AlreadyInitialised = 1,
    NotInitialised = 2,
    /// An amount argument was zero or negative.
    InvalidAmount = 3,
    /// `min_out` was not set. Trims never executes an unbounded swap.
    MissingSlippageBound = 4,
    /// Collateral unwound must cover the flash loan, or the legs will not net.
    UnwindBelowFlash = 5,
    /// Collateral and debt must be different assets.
    SameAsset = 6,
}

// -- Blend interface. Mirrors `blend-contracts-v2` exactly; Soroban encodes
// -- `contracttype` structs by field name, so the names are load-bearing.
// -- Re-declared rather than imported so Trims carries no AGPL dependency.

#[contracttype]
#[derive(Clone)]
pub struct Request {
    pub request_type: u32,
    pub address: Address,
    pub amount: i128,
}

#[contracttype]
#[derive(Clone)]
pub struct FlashLoan {
    pub contract: Address,
    pub asset: Address,
    pub amount: i128,
}

#[contracttype]
#[derive(Clone)]
pub struct Positions {
    pub liabilities: Map<u32, i128>,
    pub collateral: Map<u32, i128>,
    pub supply: Map<u32, i128>,
}

const REQ_WITHDRAW_COLLATERAL: u32 = 3;
const REQ_REPAY: u32 = 5;

#[allow(dead_code)] // only the generated client is used
#[contractclient(name = "PoolClient")]
pub trait BlendPool {
    fn flash_loan(
        e: Env,
        from: Address,
        flash_loan: FlashLoan,
        requests: Vec<Request>,
    ) -> Positions;
}

/// Mirrors `trims_receiver::Arming`.
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
#[contractclient(name = "ReceiverClient")]
pub trait TrimsReceiver {
    fn arm(e: Env, user: Address, arming: Arming);
}

#[contracttype]
enum DataKey {
    Receiver,
}

#[contract]
pub struct Manager;

#[contractimpl]
impl Manager {
    pub fn init(e: Env, receiver: Address) {
        if e.storage().instance().has(&DataKey::Receiver) {
            panic_with_error!(&e, ManagerError::AlreadyInitialised);
        }
        e.storage().instance().set(&DataKey::Receiver, &receiver);
    }

    pub fn receiver(e: Env) -> Address {
        match e.storage().instance().get(&DataKey::Receiver) {
            Some(r) => r,
            None => panic_with_error!(&e, ManagerError::NotInitialised),
        }
    }

    /// Reduce or clear a Blend debt position by selling collateral, without the
    /// user providing any capital.
    ///
    /// Set `repay_amount` below the outstanding debt for a partial deleverage.
    /// Blend refunds any overage, so passing more than is owed is safe.
    ///
    /// * `flash_amount`  — collateral to flash-borrow and sell.
    /// * `unwind_amount` — collateral withdrawn *and* repaid; the two legs net
    ///   to zero. Must be at least `flash_amount`. A small excess absorbs
    ///   Blend's `to_d_token_up`/`to_d_token_down` rounding gap, which otherwise
    ///   leaves dust debt behind.
    /// * `min_out`       — floor on swap output. Must be positive.
    pub fn deleverage(
        e: Env,
        user: Address,
        pool: Address,
        router: Address,
        collateral: Address,
        debt: Address,
        flash_amount: i128,
        unwind_amount: i128,
        repay_amount: i128,
        min_out: i128,
        deadline: u64,
    ) -> Positions {
        user.require_auth();

        if flash_amount <= 0 || unwind_amount <= 0 || repay_amount <= 0 {
            panic_with_error!(&e, ManagerError::InvalidAmount);
        }
        if min_out <= 0 {
            panic_with_error!(&e, ManagerError::MissingSlippageBound);
        }
        if unwind_amount < flash_amount {
            panic_with_error!(&e, ManagerError::UnwindBelowFlash);
        }
        if collateral == debt {
            panic_with_error!(&e, ManagerError::SameAsset);
        }

        let receiver = Self::receiver(e.clone());
        ReceiverClient::new(&e, &receiver).arm(
            &user,
            &Arming {
                router,
                collateral: collateral.clone(),
                debt: debt.clone(),
                flash_amount,
                min_out,
                deadline,
            },
        );

        // Repay the debt from the swap proceeds, free the collateral, then
        // settle the flash loan against it. The two collateral legs are equal,
        // so Blend nets them and no collateral transfer occurs.
        let requests = vec![
            &e,
            Request {
                request_type: REQ_REPAY,
                address: debt,
                amount: repay_amount,
            },
            Request {
                request_type: REQ_WITHDRAW_COLLATERAL,
                address: collateral.clone(),
                amount: unwind_amount,
            },
            Request {
                request_type: REQ_REPAY,
                address: collateral.clone(),
                amount: unwind_amount,
            },
        ];

        PoolClient::new(&e, &pool).flash_loan(
            &user,
            &FlashLoan {
                contract: receiver,
                asset: collateral,
                amount: flash_amount,
            },
            &requests,
        )
    }
}

#[cfg(test)]
mod test;
