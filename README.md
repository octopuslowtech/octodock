# Octodock

A local desktop controller for coding agents, based on [Zeron](https://github.com/zeronsh/zeron).

## Desktop only

Octodock targets Linux, macOS, and Windows. Run coding agents, manage projects and conversations, inspect changes, and use local terminals and browser previews from one desktop application.

No Octodock account, cloud backend, mobile app, or multi-device synchronization is required or enabled. Coding agents still use their own authentication and network connections.

## Development

```sh
cargo run --locked -p octodock
```

Internal `zeron-*` crate names and existing storage paths are retained for compatibility and easier upstream integration. Existing local sessions are not migrated or uploaded. The default local IPC port is 27655. Legacy `ZERON_*` settings remain available for local configuration, but cloud credentials cannot enable synchronization in the desktop entry point.

## Packaging

- Linux: `scripts/package-linux.sh`
- macOS: `scripts/package-macos.sh`
- Windows: `scripts/package-windows.ps1` (requires Inno Setup 6)

Packages are installed and updated manually. Releases belong to [octopuslowtech/octodock](https://github.com/octopuslowtech/octodock/releases), not Zeron.

## Upstream updates

The Git `upstream` remote tracks Zeron. Preserve its history and integrate updates on a separate branch, resolving conflicts and testing before merging. Mobile and cloud entry points are intentionally excluded from this repository.

Licensed under the [MIT License](LICENSE). Original attribution and [third-party notices](THIRD_PARTY_NOTICES.md) are preserved.
