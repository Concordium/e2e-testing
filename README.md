# Concordium Node E2E Test Suite

Automated end-to-end test suite for the Concordium node. Spins up a fresh
private single-validator chain from a programmatically generated genesis block,
runs a suite of tests against the node's full external interface, and tears
everything down — regardless of outcome.

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

For example, pull the latest testnet node image, then run the test suite with
it. The fixture automatically selects an available host port for the node's
gRPC endpoint:

```sh
docker pull concordium/testnet-node:latest
cargo run --release -- --image concordium/testnet-node:latest
```

Run the following command for a full description of the available options:

```sh
cargo run -- --help
```

## Exit codes

| Code | Meaning                                   |
|------|-------------------------------------------|
| `0`  | All tests passed (or no tests registered) |
| `1`  | One or more tests failed                  |
