# P10 → P11 protocol update suite

This suite provides a repeatable environment for exercising a protocol update from P10 to P11:
it starts a network with validators using old node images and protocol, run transactions to establish state, upgrades node software before activate the new protocol, and continue using the same chain.

The tests hook into well-defined points before and after the transition, retain their own state across those points, so they can relate pre-update observations to post-update behavior without each implementing its own upgrade procedure.

## Run the suite

Run with a local Docker daemon and both node images available:

```sh
cargo run --release -- protocol-p10-p11 --old-image P10_NODE_IMAGE --new-image P11_NODE_IMAGE
```

Nodes use Docker's default bridge and publish gRPC on dynamically allocated localhost ports.
The runner needs access to Docker and those ports.
On controlled exit, it attempts to remove its containers and temporary data directories.

## Lifecycle

The suite owns four validators and their software/protocol transition. Tests own their private accounts, retained state, and any additional resources they create.

```text
Start and peer validators on old software / old protocol
    ↓
Run each test's prepare_before_update hook
    ↓
Run each eligible test's run_before_update hook
    ↓
Run each eligible test's finish_before_update hook
    ↓
Upgrade validator software one node at a time, retaining databases
    ↓
Submit the governance update and wait for finalized protocol activation
    ↓
Run each eligible test's prepare_after_update hook
    ↓
Run each eligible test's run_after_update hook
    ↓
Run each eligible test's finish_after_update hook
    ↓
Run cleanup hooks for tests whose preparation started, in reverse order
    ↓
Clean up validators
```

Each phase is a global barrier. The framework does not enter the next phase until every hook in the current phase is complete.

## Implement a test

Implement `ProtocolUpgradeTest` in `src/suites/protocol_p10_p11/tests/` with
`#[async_trait::async_trait(?Send)]`. Register an instance from its module's
`register` function, then call that function from `tests/mod.rs`:
