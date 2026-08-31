# Validation

Two tests run against **unmodified Blend V2 contracts**, inside Blend's own test
harness, to prove that one-click deleveraging is mechanically possible.

```bash
./validation/run.sh
```

First run takes a few minutes (wasm build + dependency compile). Subsequent runs
are seconds.

## What each test proves

### `trims_deleverage_core.rs`

The recipe executes. A position of 100,000 XLM collateral / 1,000 STABLE debt is
deleveraged to zero debt in a single transaction. The DEX leg is mocked, so this
isolates the Blend mechanics.

### `trims_deleverage_real_amm.rs`

The strict version. Before deleveraging, the user's wallet is emptied of **both**
assets and asserted to be zero — so the repayment cannot come from their own funds.
The swap runs through a live Comet AMM (constant-product, 0.3% fee, real slippage)
rather than a mock.

```
wallet emptied  ->  0 XLM, 0 STABLE
before          ->  collateral 99,998 XLM | debt 999.99 STABLE
after           ->  collateral 88,998 XLM | debt <empty>
wallet after    ->  0.001 XLM, 95.49 STABLE
```

## Caveats

- The AMM is Comet, not Soroswap. Real router integration is Sprint 1 work.
- `mock_all_auths_allowing_non_root_auth()` stands in for the receiver authorising
  its own sub-invocations. Production requires `authorize_as_current_contract`.
- Amounts are sized to the fixture's pre-loaded utilisation (STABLE sits at 80%
  against a 95% cap), not to mainnet liquidity.

## Licensing

These tests execute inside `blend-contracts-v2`, which is AGPL-3.0, and are
derivative of Blend's test suite. They are validation artifacts, not part of the
Trims contract.
