# Rulecheck

Rulecheck is a planned Rust CLI for deterministic repository checks authored in TypeScript. The repository currently contains a dependency-free starter, not a working policy evaluator. See [the implementation plan](PLAN.md).

## Development

Install [Rust via rustup](https://rustup.rs/). In the checkout, rustup selects stable and installs rustfmt and Clippy as specified by the repository.

```sh
cargo run -- --help
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
```

Run `cargo fmt --all` to apply formatting. GitHub Actions runs format, lint, and tests on pull requests and on pushes to main.
