# Trims

**A non-custodial position manager for Blend, Stellar's largest lending protocol.**

Borrowers on Blend can supply, withdraw, borrow and repay — each as a separate,
isolated transaction. What they cannot do is *restructure* a position: reduce
leverage, swap collateral, or refinance debt. Every one of those requires repaying
the loan first, which requires capital the borrower does not have. That is the
circularity Trims removes.

---

## Status

**Integration-proven prototype.** The compiled Trims contracts clear a real Blend
position by routing through Soroswap's deployed bytecode, starting from a wallet
holding nothing. Every counterparty in that test is the real thing.

What remains is a network round trip: authorisation is the one property no local
test can verify (see below), and only testnet then mainnet settle it.

See [`docs/findings.md`](docs/findings.md) for the analysis behind the design and
[`validation/`](validation/) to reproduce the Blend tests.

---

## The problem

A borrower holds XLM as collateral and owes USDC. The market drops and the position
approaches liquidation. To reduce the debt they need USDC. To get USDC they need to
sell collateral. To withdraw collateral they must first be healthy — which requires
reducing the debt.

Today the only exits are: find outside capital, or get liquidated.

## The insight

Blend V2 ships a **zero-fee flash loan** (`flash_loan` on the pool contract). Blend's
own technical documentation describes it as useful "for arbitrage bots, liquidation
bots, and for easily entering leveraged positions."

It is not reachable by end users. Blend's official interface exposes only four
actions — supply, withdraw, borrow, repay — and the capability appears nowhere in it.
The engine exists; the execution layer on top of it does not.

## The validated finding

The obvious recipe — flash-borrow the *debt* asset, repay, withdraw collateral, sell
it — does not work. Blend runs the health check *before* invoking the flash-loan
receiver, so the withdrawn collateral has not reached anyone by the time the
callback runs.

**Flash-borrowing the _collateral_ asset works.** The receiver then holds the
sellable asset during the callback. And because Blend nets same-asset flows before
settling:

```
net_balances[token] = pool_transfer − spender_transfer
net == 0  →  no transfer occurs at all
```

the withdrawn collateral and the flash repayment cancel out, so **the user never has
to hold the collateral to spend it.** (A dust remainder from Blend's repayment
refund does land in the wallet — a rounding artifact, not a funding requirement.)

### Test results

Both tests pass against unmodified Blend V2 contracts:

| Test | What it proves |
|---|---|
| `trims_deleverage_core.rs` | The recipe executes; debt is cleared; collateral is reduced |
| `trims_deleverage_real_amm.rs` | Same, but the wallet is **emptied of both assets first** and settlement runs through a live constant-product AMM with a 0.3% fee |
| `trims_integration_soroswap.rs` | **The full system.** Compiled Trims manager and receiver, driving a real Blend pool, settling through Soroswap's deployed factory/router/pair bytecode, from an empty wallet |

```
soroswap pair  ->  CA7YQMKDYTPLYWCUJAT4FARMZNT2BBU2GLH422G6PZ2E73FCZMP7BYUT
wallet emptied ->  0 XLM, 0 STABLE
before         ->  collateral 99,998 XLM | debt 999.99 STABLE
after          ->  collateral 88,998 XLM | debt <empty>
wallet after   ->  0.001 XLM, 94.30 STABLE
```

The debt is cleared without the user supplying any capital, and Trims retains
nothing of either asset.

---

## Contracts

| Crate | Role |
|---|---|
| [`contracts/manager`](contracts/manager) | User entry point. Validates the request, arms the receiver, initiates the flash loan. |
| [`contracts/receiver`](contracts/receiver) | Blend's flash-loan callback. Swaps the collateral, sweeps everything to the user, keeps nothing. |

They are separate because **Soroban forbids contract re-entry**: a contract that
calls `flash_loan` cannot also be the receiver Blend calls back into. See
[finding 8](docs/findings.md).

Invariants enforced in code and covered by tests:

- **No unbounded swaps.** `min_out` must be positive, and the receiver verifies it
  against the *measured* change in its own balance — never the router's self-report.
  A hostile router that claims a good fill and delivers a stroop is rejected.
- **The user never funds the repayment.** `min_out >= repay_amount` is enforced up
  front. Without it a thin swap leaves a shortfall that Blend pulls straight from
  the user's wallet, which is the one thing Trims exists to prevent.
- **The receiver is one-shot.** An arming is consumed on use and is bound to a
  single user, so the callback cannot be replayed or hijacked.
- **Nothing is retained.** Every call sweeps both assets to the user, including
  any stray donation, and asserts the contract ends empty.
- **The collateral legs must net.** `unwind_amount >= flash_amount`, or the
  withdrawal and the flash repayment would not cancel.

```bash
cd contracts && cargo test          # 17 tests
./validation/run.sh                 # 3 tests against real Blend + Soroswap
```

### Building

The `stellar` CLI is required. Plain `cargo build` emits `call_indirect`
immediates as padded LEBs, which the Soroban host rejects on upload — the
contracts compile, test, and lint clean while being undeployable.
`stellar contract optimize` rewrites them; `validation/run.sh` runs it. See
[finding 9](docs/findings.md).

## Roadmap

| Stage | Scope |
|---|---|
| **1 — One-click deleverage** | Stateless receiver contract *(built)*, validated request recipe *(built)*, Soroswap router integration, health-factor preview, mainnet transaction |
| **2** | Collateral swap · debt swap |
| **3** | Keeper automation — auto-deleverage on health-factor breach |
| **4** | Full platform, audit, production launch |

## What is not yet proven

- **Authorisation.** This is the one thing no local test reaches. The receiver
  authorises the transfer Soroswap makes on its behalf via
  `authorize_as_current_contract`; `mock_all_auths` refuses non-root invoker auth
  outright, and `mock_all_auths_allowing_non_root_auth` accepts it *without
  checking the entries* — we confirmed a deliberately wrong entry passes just as
  happily. Only enforcing-mode execution on a network verifies it.
- **Network execution** on testnet, then mainnet, with a real wallet signature.
- **Liquidity depth** for large positions.
- **Edge cases:** stale oracle, `max_positions` at the cap, token decimals.

## Hard limits (verified in Blend V2 source)

Two conditions make a position unreachable by Trims. Both are protocol-level and
cannot be worked around:

- **An active liquidation auction blocks everything.** `validate_submit` panics with
  `AuctionInProgress` if the user has an open `UserLiquidation` auction. Trims is a
  *preventive* tool — once liquidation has started, it cannot help.
- **A non-active pool blocks flash loans.** `require_action_allowed` rejects
  borrowing whenever pool status > 1 (on-ice or frozen), and the flash loan is a
  borrow. Trims stops working exactly when a pool is stressed, which may be when
  users most want it. Both live V2 pools are currently status 0 and 1.

## Licensing

Trims is Apache-2.0 (see [`LICENSE`](LICENSE)).

Blend V2 is AGPL-3.0. The validation tests under [`validation/`](validation/) run
inside Blend's own workspace and are derivative of it; they are research artifacts,
not shipped code. The Trims receiver contract makes cross-contract calls only and is
not a derivative work.
