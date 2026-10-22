# Concordium Node E2E Test Suite

End-to-end suites for the Concordium node. Each suite generates a fresh private
chain and cleans up its Docker resources regardless of outcome:

- **Smoke:** single-validator chain with basic node-interface checks.
- **P10 → P11 continuity:** four validators and an observer, rolling software
  upgrades, prepared state, protocol activation and observer recovery.

## Prerequisites

| Tool   | Version                                                     |
|--------|-------------------------------------------------------------|
| Rust   | pinned via [`rust-toolchain.toml`](./rust-toolchain.toml)   |
| Docker | any recent version with access to the concordium-node image |

## Building

First, ensure the Rust SDK submodule is populated:

```sh
git submodule update --init --recursive
```

Then build the binary:

```sh
cargo build --release
```

The binary is written to `target/release/concordium-e2e`.

## Usage

Make the required node images available to Docker before running a suite.
Replace the image placeholders below with Docker image references.

### Smoke

Checks basic node behavior and connectivity. Use this suite for a quick check
of a node image.

```sh
cargo run --release -- smoke --image NODE_IMAGE
```

### P10 → P11 continuity

The suite starts a private P10 chain with four validators and an observer, then
upgrades validators one at a time while checking that finalization and CCD
transfers continue. During mixed-version operation, it prepares CCD, delegation
and token state; after all validators are upgraded, it submits a governance
update to activate P11 and checks that this state is preserved. It then runs fresh
CCD and token transfers at P11, verifies that the old observer cannot follow the
update, and upgrades the observer using its existing database. Finally, it checks
that all five nodes agree on saved finalized state and cleans up the network.

```sh
cargo run --release -- protocol-p10-p11 \
  --old-image P10_NODE_IMAGE --new-image P11_NODE_IMAGE
```

See the [protocol suite documentation](docs/protocol-p10-p11.md) for image
compatibility requirements.

### Available options

List the suites and options with `--help`. Add `--help` to a suite command for
its options.

```sh
cargo run -- --help
```

## Exit codes

| Code | Meaning                                   |
|------|-------------------------------------------|
| `0`  | All tests passed (or no tests registered) |
| `1`  | One or more tests failed                  |
