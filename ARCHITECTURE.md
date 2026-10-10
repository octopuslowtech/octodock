# Octodock Architecture

Octodock is a desktop-only, local controller for coding agents, derived from Zeron. Linux, macOS, and Windows share the Rust engine and GPUI desktop UI.

## Runtime

The desktop application embeds the engine, or attaches to an existing local instance over loopback IPC. The default port is 27655. The desktop entry point supplies no cloud endpoint, bearer, organization, or WorkOS client ID. Empty cloud endpoints select the local workspace profile without loading a cloud session.

The local engine owns agent subprocesses, transcripts, journals, repositories, workspace files, terminals, and previews. Engine operations run on Tokio rather than the UI thread. The MCP stdio command remains available for local agent integration. Server daemon, login/logout, and cloud sync CLI commands are removed.

## Modules

- `apps/octodock`: Octodock desktop binary and local utility commands.
- `crates/ui`: GPUI desktop shell and views.
- `crates/engine`, `crates/harness`: local execution and coding agent integrations.
- `crates/doc`, `crates/sync`: local document storage and persistence; shared transport code is retained where intertwined with the engine.
- `crates/proto`, `crates/rpc`, `crates/mcp`: shared contracts and local communication.
- `crates/preview`: local project preview discovery and browser support.
- Rendering, theme, text, markdown, syntax, audio, and voice crates support desktop features.

Native mobile apps, mobile-only Rust crates, the Cloudflare backend, public landing site, and their deployment workflows are removed. Shared internal cloud transport implementation has not all been deleted; the shipped desktop entry point does not enable it.

## Compatibility

Internal `zeron-*` crate names, local storage locations, and existing `ZERON_*` local settings are retained to avoid data migration and reduce upstream merge conflicts. User-facing branding, executable names, installers, and the conversation URL scheme use Octodock. Provider authentication is separate from the removed application cloud login.

Git `origin` is Octodock and `upstream` is Zeron. Integrating upstream changes must preserve local-only startup and existing storage contracts.
