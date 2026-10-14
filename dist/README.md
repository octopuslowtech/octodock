# Packaging

## Linux

Run `scripts/package-linux.sh` for a release package or `PROFILE=debug scripts/package-linux.sh` for a debug package. Output: `target/package/octodock-<version>-linux-<arch>.tar.gz`.

The archive contains `octodock`, `octodock.desktop`, `octodock.png`, licenses, and `install.sh`. Binaries are installed under `~/.local/share/octodock/app/<version>` with a `current` symlink, leaving the existing Zeron installation untouched. The installer links `~/.local/bin/octodock` and writes absolute launcher paths under `$XDG_DATA_HOME` (default `~/.local/share`). Run `scripts/test-linux-desktop-entry.sh` to check the tarball installer offline.

## macOS

Run `scripts/package-macos.sh` on macOS. Output: `target/package/octodock-<version>-macos-<arch>.dmg`, containing `Octodock.app`. Set `CODESIGN_IDENTITY` for Developer ID signing; otherwise the bundle is ad-hoc signed. Set `NOTARY_KEY_PATH`, `NOTARY_KEY_ID`, and `NOTARY_ISSUER_ID` together to notarize and staple the app and DMG.

The committed DMG backgrounds retain the existing artwork. `scripts/run-macos-dev.sh` prepares an isolated `Octodock Dev.app`; its existing `ZERON_DEV_*` overrides and data directory remain supported.

## Windows

Run `./scripts/package-windows.ps1` with Inno Setup 6 installed. Outputs under `target/package/`:

- `octodock-<version>-windows-<arch>-setup.exe`: per-user installer from `dist/windows/octodock.iss`.
- `octodock-<version>-windows-<arch>.zip`: portable package.

The installer registers `octodock://` and installs Octodock shortcuts. Existing session storage is untouched. `scripts/test-windows-installer.ps1` checks install/uninstall behavior; it requires CI or `-Force`. Startup and lifecycle probes live beside it.

Desktop application identity is `io.github.octopuslowtech.octodock`. No package carries an updater feed, updater marker, or updater-only artifact.

## Icons

`python3 scripts/draw-octopus-logo.py .` redraws the application logo from the octopus mascot's outline. It writes `dist/octodock.svg` (vector source), `dist/octodock.png` (Linux), `dist/macos/icon-1024.png` (macOS, with the system margin and shadow baked in), `dist/windows/octodock.ico`, and the in-app mark `crates/ui/assets/icons/zeron-logo.svg`. It requires PyGObject with the librsvg introspection data.
