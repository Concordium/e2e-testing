# Concordium Node E2E Test Suite

End-to-end suites for the Concordium node. Each suite generates a fresh private
chain and cleans up its Docker resources regardless of outcome:

- **Smoke:** verify core functionality and new-feature behavior.
- **P10 → P11 continuity:** verify that protocol activation preserves existing
  state and capabilities while enabling new functionality.

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

Smoke starts directly under P11 and requires a node image that supports P11.

```sh
cargo run --release -- smoke --image NODE_IMAGE
```

### P10 → P11 continuity

Checks rolling validator upgrades, preservation of CCD, staking, and token state
across P11 activation, and database-preserving observer recovery.

```sh
cargo run --release -- protocol-p10-p11 \
  --old-image P10_NODE_IMAGE --new-image P11_NODE_IMAGE
```

See the [protocol suite documentation](docs/protocol-p10-p11.md) for image
compatibility requirements, lifecycle checks, and instructions for adding tests.

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
