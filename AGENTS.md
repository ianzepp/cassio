# Cassio

Rust CLI. Parsers build a session AST and a parallel training export; formatters
render these into transcripts and structured outputs. Keep behavioral tests in
`*_test.rs` companion files, not inline production test modules.

## Validation

Use `cargo test --locked --lib parser::codex` for Codex parser changes. Before a
release, run `cargo test --locked` and `cargo clippy --locked --all-targets`.
Scope formatting to changed files with `rustfmt --edition 2024`.

Measured locally on 2026-10-30 with warm dependency caches:

| Command | Elapsed | Result |
| --- | --- | --- |
| `cargo test --locked --lib parser::codex` | 0.75 s | 25 passed |
| `cargo test --locked` | 3.52 s | 315 passed |
| `cargo build --locked` | 1.72 s | success |
| `cargo build --release --locked` | 18.51 s | success; OpenSSL 3 selected |
| `cargo clippy --locked --all-targets` | 1.96 s | success; six existing warnings |

A native-dependency rebuild took about 23 s locally. The pinned llama.cpp
HTTP dependency requires OpenSSL 3; OpenSSL 4 fails its C++ compilation. For a
fresh macOS build where OpenSSL 4 is the default, use
`OPENSSL_ROOT_DIR="$(brew --prefix openssl@3)" cargo ...`. Existing CMake caches
that selected OpenSSL 4 must also be reconfigured to use OpenSSL 3.

## Release

Update the root package version in `Cargo.toml` and its package entry in
`Cargo.lock`. Add release notes under `docs/releases/`. Push the release commit
to `master`, then its matching `vX.Y.Z` tag. `.github/workflows/release.yml`
builds three platform archives, publishes the GitHub release, and updates the
Homebrew tap. Verify every platform build, publication, and tap update completes; a tag alone
is not a release.
