#!/usr/bin/env bash
# Builds the program and holds it to the deployed build, instruction by instruction: every result,
# log, return value and account must match. See tests/differential.rs.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build-sbf
RUST_LOG=off cargo test --test differential -- --ignored --nocapture "$@"
