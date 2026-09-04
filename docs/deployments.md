# Deployments

## Stellar Testnet

| Contract | ID |
|---|---|
| Manager | [`CA3SMX3NJEJG5YWGB7VJZOBG4SQTBU6WPH7PQC7BU4JN2LQKPR66LDUB`](https://stellar.expert/explorer/testnet/contract/CA3SMX3NJEJG5YWGB7VJZOBG4SQTBU6WPH7PQC7BU4JN2LQKPR66LDUB) |
| Receiver | [`CAHEXZDMXDOWKD7UOBLMUJVN3DM65H75RD6NS5TUUMJYRJYM6JV5N2ZG`](https://stellar.expert/explorer/testnet/contract/CAHEXZDMXDOWKD7UOBLMUJVN3DM65H75RD6NS5TUUMJYRJYM6JV5N2ZG) |

Initialisation transactions:

- `receiver.init(manager)` —
  [`e83577…f83736`](https://stellar.expert/explorer/testnet/tx/e83577235b66fbd6fb68c6dc57b6f282fdcf006b7203445b59e54be81cf83736)
- `manager.init(receiver)` —
  [`451cda…ab0281`](https://stellar.expert/explorer/testnet/tx/451cda4640abcccc61dae4fafa4a91429ef9f5946c97f7b2c1817fa006ab0281)

Read back from chain after wiring:

```
receiver.manager()  -> CA3SMX3NJEJG5YWGB7VJZOBG4SQTBU6WPH7PQC7BU4JN2LQKPR66LDUB
manager.receiver()  -> CAHEXZDMXDOWKD7UOBLMUJVN3DM65H75RD6NS5TUUMJYRJYM6JV5N2ZG
```

### What the deployment establishes

**The wasm actually deploys.** The encoding problem in
[finding 9](findings.md) was fixed locally, but a local fix is not evidence:
only an upload to a real network settles whether the host accepts the module.
It does.

**Manager-gating holds under enforcing mode.** Calling `arm()` from an account
that is not the manager is refused on-chain:

```
$ stellar contract invoke --id <receiver> --source <not-the-manager> -- arm ...
error: Missing signing key for account CA3SMX3N…LDUB      # the manager contract
```

The receiver's `manager.require_auth()` is doing its job against a real
network, not a mocked auth mode. Our unit tests could only assert this under
`mock_all_auths`; this is the enforcing-mode confirmation.

### What it does not establish

The authorisation path that matters most is still unproven: the receiver's
`authorize_as_current_contract` entry covering the transfer Soroswap makes on
its behalf. Reaching it requires a funded Blend testnet position and Soroswap
liquidity — that is the first deliverable of the sprint, not something a bare
deployment can show.

## Reproducing

```bash
cd contracts
cargo build --release --target wasm32-unknown-unknown
stellar contract optimize --wasm target/wasm32-unknown-unknown/release/trims_manager.wasm \
                          --wasm-out target/optimized/trims_manager.wasm
stellar contract deploy --wasm target/optimized/trims_manager.wasm \
                        --source <identity> --network testnet
```

The `optimize` step is not optional — see [finding 9](findings.md).

## Mainnet

Not yet deployed.
