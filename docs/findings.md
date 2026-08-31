# Technical findings — Blend V2 flash-loan execution path

All findings below were derived from Blend V2 source (`blend-capital/blend-contracts-v2`)
and verified against live Stellar mainnet state or Blend's own test harness.

---

## 1. Flash-loan execution order

From `pool/src/pool/submit.rs::execute_submit_with_flash_loan`:

```
1. flash-loan liabilities are added to the user (real d-tokens minted)
2. requests are processed          -> builds the transfer maps
3. HEALTH CHECK                    <- here, BEFORE the receiver runs
4. flash amount transferred to the receiver contract
5. exec_op(from, asset, amount, 0) <- the swap happens here
6. handle_transfer_with_allowance  -> net settlement
```

Two consequences follow:

- The position must already be solvent from the *requests alone*. The receiver's
  output cannot contribute to passing the health check.
- The receiver cannot sell withdrawn collateral, because at step 5 that collateral
  has not been transferred to anyone yet.

## 2. The flash loan is not repayment-bound

Blend's flash loan opens a **real borrow position**. There is no "must repay in the
same call" constraint — only the health check at the end. Blend's own docs:

> `flash_loan()` allows users to borrow as much as they want from the pool without
> posting collateral as long as their position is healthy at the end of modification.

This is more permissive than ERC-3156 and means leverage entry is natively supported.

## 3. The fee is zero

`FlashLoanClient::exec_op(&from, &asset, &amount, &0)` — the fee argument is a
hardcoded literal `0`.

## 4. Same-asset flows are netted

From `handle_transfer_with_allowance`:

```rust
net_balances[token] -= spender_transfer   // what the user owes the pool
net_balances[token] += pool_transfer      // what the pool owes the user

if net < 0  -> pool pulls via transfer_from
if net > 0  -> pool sends
if net == 0 -> no transfer at all
```

This is the mechanism that makes atomic deleveraging possible. Withdrawing collateral
and repaying the flash loan in the same asset cancel out, so the user never needs to
hold the intermediate asset.

## 5. `max_positions` only binds when increasing

```rust
if new_num > previous_num && max_positions < new_num { panic }
```

A position-count-neutral operation (collateral swap) passes even at the cap.
Leverage loops that add a position do not.

## 6. Repayment overpayment is refunded

`apply_repay` burns all outstanding d-tokens and refunds the excess when the repay
amount exceeds the debt. Overpaying is therefore safe, which matters because the
flash liability is minted with `to_d_token_up` while repayment burns with
`to_d_token_down` — repaying the exact flash amount leaves one stroop of dust.

---

## The working recipe

```
flash_loan { asset: <COLLATERAL asset>, amount: A }

requests: [
  Repay            <debt asset>       <debt amount>   // funded by the receiver's swap
  WithdrawCollateral <collateral asset> A + buffer
  Repay            <collateral asset> A + buffer      // nets against the withdrawal
]
```

The receiver holds the collateral asset during `exec_op`, swaps it for the debt
asset, and transfers the proceeds to the user. Settlement then pulls only the debt
asset; the collateral leg nets to zero.

---

## Production authorization requirement

The validation test initially failed with:

```
Error(Auth, InvalidAction)
"authorization not tied to the root contract invocation for an address.
 Use require_auth() in the top invocation or enable non-root authorization."
```

The receiver must authorize its own sub-invocations (the AMM approve/swap) rather
than relying on invoker auth. In tests this is bypassed with
`mock_all_auths_allowing_non_root_auth()`; in production the receiver must call
`e.authorize_as_current_contract(...)` with explicit sub-invocation entries.

---

## On-chain measurements (Stellar mainnet)

Read via Soroban RPC from the pool addresses in `blend-capital/blend-utils`.

Blend V2 pool configuration:

| Pool | Oracle | max_positions |
|---|---|---|
| `Fixed V2` | `CCVTVW2CVA7JLH4ROQGP3CU4T3EXVCK66AZGSM4MUQPXAI4QHCZPOATS` | 6 |
| `YieldBlox V2` | `CD74A3C54EKUVEGUC6WNTUPOTHB624WFKXN3IYTFJGX3EHXDXHCYMXXR` | 6 |

Pools select their own oracle independently.

Approximate reserve state (interest-rate scaling is an estimate; utilisation ratios
are exact):

```
Fixed V2      XLM    supplied ~765,000,000   borrowed ~1,300,000    ( 0.2% util)
Fixed V2      USDC   supplied  ~49,000,000   borrowed ~39,300,000   (80.2% util)
YieldBlox V2  USDC   supplied      ~73,900   borrowed     ~63,100   (85.4% util)
```

Roughly $39M of USDC is actively borrowed against a large XLM collateral base —
the population Trims serves.

`YieldBlox V2` reserve list: XLM, USDC, EURC, AQUA, USDGLO, USTRY, CETES, PYUSD.
Collateral factors for USTRY and CETES are both `0`, disabling them as collateral.
