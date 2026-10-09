# Repository Guide

## Project and References

Zeron is a local-first controller for coding agents with optional multi-device sync. Its Rust binary provides both a GPUI desktop application and a headless engine. The optional cloud backend is a TypeScript Cloudflare Worker with Durable Objects and R2. The iOS app is a native viewport backed by the Rust mobile core.

Read `README.md` for the product overview, `ARCHITECTURE.md` for design context, `CONTEXT.md` for domain vocabulary, and relevant documents under `docs/` before changing a subsystem. Some architecture sections describe earlier plans; verify current behavior against code, manifests, and CI.

## Repository Map

- `apps/zeron/`: desktop and CLI entry point, startup, and command dispatch.
- `crates/proto/`: shared wire types and frontend-independent view derivations.
- `crates/doc/`: Loro session and registry schemas, mirrors, and command ledger.
- `crates/sync/`: persistence, room transport, synchronization, and presence.
- `crates/harness/`: agent integrations, subprocess protocols, catalogs, and fixtures.
- `crates/engine/`: session execution, recovery, authentication, agent accounts, repositories, terminals, and workspace files.
- `crates/rpc/`: typed local and remote request/response/stream transports.
- `crates/ui/`: GPUI desktop views, interactions, assets, and rendering fixtures. This is not a web frontend.
- `crates/theme/`: theme schema, registry, validation, and imports.
- `crates/client/` and `crates/mobile/`: shared client state and UniFFI mobile core.
- `crates/markdown/`, `crates/text/`, and `crates/syntax/`: parsing, layout, and highlighting.
- `crates/voice/`, `crates/voice-media/`, `crates/voice-session/`, `crates/audio/`, `crates/orb/`, and `crates/veil/`: voice, audio, and visual support.
- `crates/preview/`, `crates/update/`, and `crates/mcp/`: preview, updates, and MCP integration.
- `edge/`: optional cloud backend and TypeScript tests.
- `apps/ios/`: UIKit application, Xcode project, and native tests; see its README.
- `apps/landing/`: public website assets.
- `scripts/`, `dist/`, and `.github/`: fixtures, packaging, platform integration, and CI.

## Working Rules

- Inspect the owning module and existing tests before editing. Keep changes small and follow local patterns.
- Preserve unrelated worktree changes. Do not commit, push, release, or deploy unless requested.
- Keep code, identifiers, file names, and commit messages in English.
- Do not add explanatory source comments. Exceptions are shebangs, tool directives, required configuration comments, and license notices.
- Prefer existing workspace dependencies and standard-library facilities. Do not introduce speculative abstractions or update pinned GPUI dependencies incidentally.
- Keep engine work off the UI thread. Follow the existing Tokio and GPUI task bridge.
- Put shared behavior in the shared Rust layer rather than duplicating it across desktop and mobile.
- Regenerate UniFFI bindings through the platform core build scripts rather than hand-editing generated files.

## Architectural Boundaries

- Preserve fully functional local-only operation without authentication or network access.
- Keep live authentication separate from the workspace storage/transport scope captured at engine startup. Login and logout must not silently switch an open store.
- Preserve profile isolation for transcripts, journals, and attachments. Signing in must not automatically upload local sessions.
- Keep durable commands host-executed and idempotent. Respect existing command-ledger rules and incremental Loro mirroring.
- Coordinate wire/schema changes across Rust and the TypeScript edge; do not change serialization contracts on only one side.
- Enforce workspace path containment, symlink handling, and write-conflict checks in the owning engine. UI filtering is not an authorization boundary.

## Development and Verification

Run Rust commands from the repository root. Use the stable toolchain in `rust-toolchain.toml`; the workspace uses Rust edition 2024. Choose checks for affected packages rather than defaulting to every platform and live integration test.

### Rust

- Build: `cargo build --locked -p zeron`.
- Run desktop: `cargo run --locked -p zeron`.
- Run headless: `cargo run --locked -p zeron -- headless`.
- Check formatting: `cargo fmt --all -- --check`.
- Check an affected crate: `cargo check --locked -p <package>`.
- Run library tests: `cargo test --locked -p <package> --lib`.
- Run a named integration test: `cargo test --locked -p <package> --test <test_name>`.
- Lint: `cargo clippy --locked -p <package> --all-targets`.
- Linux core CI suite: `scripts/ci/test-core.sh` (requires cargo-nextest; enables harness native fixtures).
- UI CI suite: `cargo nextest run --config-file scripts/ci/nextest.toml --locked --no-fail-fast -p zeron-ui --lib --test voice_orb_geometry`.

Linux desktop builds require native libraries. Use the package lists in `.github/workflows/ui-tests.yml` and setup in `.github/actions/linux-ci/action.yml` as the source of truth. CI sets `CARGO_PROFILE_DEV_DEBUG=0` to reduce build size.

### Edge

Run from `edge/`:

- Install locked dependencies: `npm ci`.
- Start local worker: `npm run dev` (port 27640, development authentication).
- Check types: `npm run typecheck`.
- Run unit and workerd tests: `npm test`.
- Validate bundling without deployment: `npm run build`.

Do not run `npm run deploy` or release workflows without explicit authorization.

### UI and Mobile

For desktop appearance changes, render affected states with the relevant examples under `crates/ui/examples/` and required features in `crates/ui/Cargo.toml`. Capture and inspect the result; compilation alone does not verify appearance. The Linux browser workflow builds `browser-fixture` with the `browser-fixture` feature, then runs `scripts/test-linux-browser.sh` with `BROWSER_FIXTURE_BINARY` pointing to the built binary.

For mobile core changes, run relevant tests in `zeron-client`, `zeron-mobile`, `zeron-text`, and `zeron-markdown`. Native iOS builds and tests require macOS and Xcode; follow `apps/ios/README.md` and `scripts/ios/build-core.sh`. Do not claim native platform or deployed cloud behavior was verified by Linux-only checks.

After code changes, run relevant tests and report what passed, failed, or could not run. Add regression tests for observable behavior, not implementation details. For documentation-only changes, validate paths, commands, references, and whitespace; a full application test run is unnecessary.
