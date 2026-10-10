# Repository Guide

## Project and References

Octodock is a local-only GPUI desktop controller for coding agents on Linux, macOS, and Windows, derived from Zeron. Internal crate names and storage paths retain upstream compatibility.

Read `README.md` for the product overview, `ARCHITECTURE.md` for design context, `CONTEXT.md` for domain vocabulary, and relevant documents under `docs/` before changing a subsystem. Some architecture sections describe earlier plans; verify current behavior against code, manifests, and CI.

## Repository Map

- `apps/octodock/`: desktop and CLI entry point, startup, and command dispatch.
- `crates/proto/`: shared wire types and frontend-independent view derivations.
- `crates/doc/`: Loro session and registry schemas, mirrors, and command ledger.
- `crates/sync/`: persistence, room transport, synchronization, and presence.
- `crates/harness/`: agent integrations, subprocess protocols, catalogs, and fixtures.
- `crates/engine/`: session execution, recovery, authentication, agent accounts, repositories, terminals, and workspace files.
- `crates/rpc/`: typed local and remote request/response/stream transports.
- `crates/ui/`: GPUI desktop views, interactions, assets, and rendering fixtures. This is not a web frontend.
- `crates/theme/`: theme schema, registry, validation, and imports.
- `crates/markdown/`, `crates/text/`, and `crates/syntax/`: parsing, layout, and highlighting.
- `crates/voice/`, `crates/voice-media/`, `crates/voice-session/`, `crates/audio/`, `crates/orb/`, and `crates/veil/`: voice, audio, and visual support.
- `crates/preview/` and `crates/mcp/`: preview and MCP integration.
- `scripts/`, `dist/`, and `.github/`: fixtures, packaging, platform integration, and CI.

## Working Rules

- Inspect the owning module and existing tests before editing. Keep changes small and follow local patterns.
- Preserve unrelated worktree changes. Do not commit, push, release, or deploy unless requested.
- Do not run builds or tests on your own initiative. Ask the user for approval first, naming the exact command, and wait for the answer.
- Keep code, identifiers, file names, and commit messages in English.
- Do not add explanatory source comments. Exceptions are shebangs, tool directives, required configuration comments, and license notices.
- Prefer existing workspace dependencies and standard-library facilities. Do not introduce speculative abstractions or update pinned GPUI dependencies incidentally.
- Keep engine work off the UI thread. Follow the existing Tokio and GPUI task bridge.
- Put shared behavior in the Rust engine rather than duplicating it in desktop views.

## Architectural Boundaries

- Preserve fully functional local-only operation without authentication or network access.
- Keep live authentication separate from the workspace storage/transport scope captured at engine startup. Login and logout must not silently switch an open store.
- Preserve profile isolation for transcripts, journals, and attachments. Signing in must not automatically upload local sessions.
- Keep durable commands host-executed and idempotent. Respect existing command-ledger rules and incremental Loro mirroring.
- Preserve shared serialization contracts when integrating upstream core changes.
- Enforce workspace path containment, symlink handling, and write-conflict checks in the owning engine. UI filtering is not an authorization boundary.

## Development and Verification

Run Rust commands from the repository root. Use the stable toolchain in `rust-toolchain.toml`; the workspace uses Rust edition 2024. Choose checks for affected packages rather than defaulting to every platform and live integration test.

### Rust

- Build: `cargo build --locked -p octodock`.
- Run desktop: `cargo run --locked -p octodock`.
- Check formatting: `cargo fmt --all -- --check`.
- Check an affected crate: `cargo check --locked -p <package>`.
- Run library tests: `cargo test --locked -p <package> --lib`.
- Run a named integration test: `cargo test --locked -p <package> --test <test_name>`.
- Lint: `cargo clippy --locked -p <package> --all-targets`.
- Linux core CI suite: `scripts/ci/test-core.sh` (requires cargo-nextest; enables harness native fixtures).
- UI CI suite: `cargo nextest run --config-file scripts/ci/nextest.toml --locked --no-fail-fast -p zeron-ui --lib --test voice_orb_geometry`.

Linux desktop builds require native libraries. Use the package lists in `.github/workflows/ui-tests.yml` and setup in `.github/actions/linux-ci/action.yml` as the source of truth. CI sets `CARGO_PROFILE_DEV_DEBUG=0` to reduce build size.

### Desktop UI

For desktop appearance changes, render affected states with the relevant examples under `crates/ui/examples/` and required features in `crates/ui/Cargo.toml`. Capture and inspect the result; compilation alone does not verify appearance. The Linux browser workflow builds `browser-fixture` with the `browser-fixture` feature, then runs `scripts/test-linux-browser.sh` with `BROWSER_FIXTURE_BINARY` pointing to the built binary.

Native macOS and Windows packaging must be checked on their respective platforms. Do not claim native platform behavior was verified by Linux-only checks.

After code changes, propose the relevant tests and run them only once the user approves; report what passed, failed, or was not run. Add regression tests for observable behavior, not implementation details. For documentation-only changes, validate paths, commands, references, and whitespace; a full application test run is unnecessary.
