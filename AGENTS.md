# AGENTS.md (Rust Development)

This file defines how coding agents should work in this repository for Rust projects.

## 1) Core Principles

- Prefer correctness and maintainability over cleverness.
- Keep changes small, reviewable, and well-tested.
- Use idiomatic Rust and follow ecosystem conventions.
- Treat warnings as work items, not noise.

## 2) Standard Execution Order

Run commands from the repository root.

### If this is a single crate

```bash
cargo check --all-targets --all-features
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

### If this is a workspace

```bash
cargo check --workspace --all-targets --all-features
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
```

If `rustfmt` or `clippy` are missing, install them:

```bash
rustup component add rustfmt
rustup component add clippy
```

## 3) Formatting and Linting

- Use `rustfmt` as the source of truth for formatting.
- Keep formatting checks in CI via `cargo fmt --all -- --check`.
- Run `clippy` in CI with `-D warnings`.
- Do not enable the whole `clippy::restriction` group globally; opt in lint-by-lint when needed.

## 4) Cargo and Toolchain Policy

- Set `edition` explicitly in `Cargo.toml` (prefer latest stable edition for new crates).
- Set `rust-version` (MSRV) explicitly in `Cargo.toml`.
- Keep `rust-version`, CI toolchains, and documentation aligned.
- Use workspace-level shared settings where applicable (`[workspace]`, shared `Cargo.lock`, root-level profiles/lints).

## 5) API and Code Design

- Follow the Rust API Guidelines for naming, interoperability, documentation, and future-proofing.
- Prefer explicit types and ownership boundaries over implicit behavior.
- Keep public APIs minimal and stable.
- Avoid unnecessary macros when normal functions/types are clearer.

## 6) Error Handling Rules

- Use `Result<T, E>` for recoverable errors.
- Use `panic!` only for unrecoverable bug conditions or impossible invariants.
- Prefer propagating errors with context instead of panicking in library code.
- Keep error messages actionable and specific.

## 7) Testing Rules

- Add unit tests for local logic changes.
- Add integration tests for public behavior and cross-module behavior.
- Keep doctests valid for public-facing examples.
- For bug fixes, add a regression test that fails before and passes after.

## 8) Documentation Rules

- Public items should have clear rustdoc comments.
- Include examples for public APIs when practical.
- Build docs in CI when changes touch public APIs:

```bash
cargo doc --no-deps
```

## 9) Unsafe Code Policy

- Avoid `unsafe` unless required.
- For each `unsafe` block, document the safety invariant and why it holds.
- Keep `unsafe` scope as small as possible.

## 10) Definition of Done (Agent Checklist)

- Code compiles (`cargo check`).
- Formatting passes (`cargo fmt --check` path).
- Lint passes (`cargo clippy ... -D warnings`).
- Tests pass (`cargo test` including doctests by default).
- New/changed public behavior is documented and tested.

## References (Primary Sources)

- Rust Book (testing): https://doc.rust-lang.org/book/ch11-00-testing.html
- Rust Book (error handling): https://doc.rust-lang.org/book/ch09-00-error-handling.html
- Rust Book (profiles): https://doc.rust-lang.org/book/ch14-01-release-profiles.html
- Cargo Book (`cargo check`): https://doc.rust-lang.org/cargo/commands/cargo-check.html
- Cargo Book (`cargo test`): https://doc.rust-lang.org/cargo/commands/cargo-test.html
- Cargo Book (`cargo fmt`): https://doc.rust-lang.org/cargo/commands/cargo-fmt.html
- Cargo Book (`cargo clippy`): https://doc.rust-lang.org/cargo/commands/cargo-clippy.html
- Cargo Book (`rust-version`): https://doc.rust-lang.org/cargo/reference/rust-version.html
- Cargo Book (workspaces): https://doc.rust-lang.org/cargo/reference/workspaces.html
- Cargo Book (CI): https://doc.rust-lang.org/cargo/guide/continuous-integration.html
- Rust Style Guide: https://doc.rust-lang.org/style-guide/
- Clippy docs (usage/config): https://doc.rust-lang.org/clippy/usage.html
- Rust API Guidelines: https://rust-lang.github.io/api-guidelines/about.html
- rustfmt (CI and style-edition notes): https://github.com/rust-lang/rustfmt
