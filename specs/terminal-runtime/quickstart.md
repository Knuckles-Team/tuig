# TUIG-RUNTIME — Quickstart

Run these commands from the repository root to prove the served and local behaviour of this spec.

## Native checks

```bash
cargo fmt --manifest-path native/tuig/Cargo.toml --check
cargo test --locked --manifest-path native/tuig/Cargo.toml
cargo clippy --locked --manifest-path native/tuig/Cargo.toml --all-targets -- -D warnings
```

## Compiled binary and installed entry tests

```bash
cargo build --locked --manifest-path native/tuig/Cargo.toml --bin tuig
uv run --frozen --no-sync pytest tests -q
```

## Scenario: operation round trip

Given a local Arbora fixture and a verified credential, run `tuig`, enter `/op <registered-id> {"key":"value"}`, then `/confirm <plan_ref>` for a side effect. The scenario passes when one typed result renders and a second `/confirm` is refused.

## Scenario: host logs

Given a valid host launch receipt, open the host log view with `l`. The scenario passes when bounded batches render and an altered receipt makes zero reads.
