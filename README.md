# Trims

**A non-custodial position manager for Blend, Stellar's largest lending protocol.**

Borrowers on Blend can supply, withdraw, borrow and repay — each as a separate,
isolated transaction. What they cannot do is *restructure* a position: reduce
leverage, swap collateral, or refinance debt. Every one of those requires repaying
the loan first, which requires capital the borrower does not have. That is the
circularity Trims removes.

---

## Status

**Pre-implementation. The core execution path has been validated against Blend V2's
own contracts.** See [`docs/findings.md`](docs/findings.md) for the full analysis and
[`validation/`](validation/) to reproduce the tests.

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

the withdrawn collateral and the flash repayment cancel out. **The collateral never
touches the user's wallet.**

### Test results

Both tests pass against unmodified Blend V2 contracts:

| Test | What it proves |
|---|---|
| `trims_deleverage_core.rs` | The recipe executes; debt is cleared; collateral is reduced |
| `trims_deleverage_real_amm.rs` | Same, but the user's wallet is **emptied of both assets first**, and settlement runs through a **live constant-product AMM** with a 0.3% fee and real slippage |

```
wallet emptied      ->  0 XLM, 0 STABLE
before              ->  collateral 99,998 XLM | debt 999.99 STABLE
after               ->  collateral 88,998 XLM | debt <empty>
wallet after        ->  0.001 XLM, 95.49 STABLE
```

The debt is cleared without the user supplying any capital.

---

## Roadmap

| Stage | Scope |
|---|---|
| **1 — One-click deleverage** | Stateless receiver contract, validated request recipe, Soroswap settlement, health-factor preview, mainnet transaction |
| **2** | Collateral swap · debt swap |
| **3** | Keeper automation — auto-deleverage on health-factor breach |
| **4** | Full platform, audit, production launch |

## Known open questions

- Soroswap router integration (validation used a Comet AMM, not Soroswap itself)
- Production authorization: the receiver must authorize its own sub-invocations via
  `authorize_as_current_contract` — surfaced by the validation tests
- Mainnet execution with a real wallet signature
- Liquidity depth for large positions
- Edge cases: stale oracle, active liquidation auction, `max_positions`, token decimals

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
