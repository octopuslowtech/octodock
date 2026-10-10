#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
VERSION=9.9.9
fail() { echo "FAIL: $*" >&2; exit 1; }

PKG="octodock-$VERSION-linux-x86_64"
mkdir -p "$WORK/pkg/$PKG"
printf '#!/bin/sh\nexit 0\n' >"$WORK/pkg/$PKG/octodock"
chmod 755 "$WORK/pkg/$PKG/octodock"
cp "$ROOT/dist/octodock.desktop" "$WORK/pkg/$PKG/octodock.desktop"
printf 'not-really-a-png' >"$WORK/pkg/$PKG/octodock.png"

sed -n "/<<'INSTALL'/,/^INSTALL\$/p" "$ROOT/scripts/package-linux.sh" | sed '1d;$d' \
  | sed "s/__VERSION__/$VERSION/" >"$WORK/pkg/$PKG/install.sh"
chmod 755 "$WORK/pkg/$PKG/install.sh"

mkdir -p "$WORK/tmp"
run_tarball() {
  local home="$1"; shift
  env -i HOME="$home" USER=tester PATH="/usr/bin:/bin" TMPDIR="$WORK/tmp" "$@" \
    bash "$WORK/pkg/$PKG/install.sh" >"$WORK/out.log" 2>&1 \
    || { cat "$WORK/out.log" >&2; fail "tarball installer exited non-zero"; }
  [ -z "$(ls -A "$WORK/tmp")" ] || fail "tarball installer left files in TMPDIR: $(ls -A "$WORK/tmp")"
}

check() {
  local home="$1" data="$2" entry="$2/applications/octodock.desktop"
  [ -f "$entry" ] || fail "missing $entry"
  [ -f "$data/icons/hicolor/1024x1024/apps/octodock.png" ] || fail "missing hicolor icon"
  grep -qxF "TryExec=$home/.local/share/octodock/app/current/octodock" "$entry" || fail "TryExec: $(grep '^TryExec' "$entry")"
  grep -qxF "Icon=$home/.local/share/octodock/app/current/octodock.png" "$entry" || fail "Icon: $(grep '^Icon' "$entry")"
  grep -qxF "StartupWMClass=octodock" "$entry" || fail "StartupWMClass changed"
  [ "$(grep -c '^\[Desktop Entry\]' "$entry")" = 1 ] || fail "duplicated entry"
  [ "$(grep -c '^Exec=' "$entry")" = 1 ] || fail "Exec lines"
  [ -z "$(find "$data" -name '.octodock*')" ] || fail "temp files left behind"
  [ ! -e "$data/icons/hicolor/icon-theme.cache" ] || fail "created a hicolor icon cache"
  if command -v desktop-file-validate >/dev/null 2>&1; then
    desktop-file-validate "$entry" || fail "desktop-file-validate"
  fi
}

for installer in tarball; do
  run() { "run_$installer" "$@"; }

  home="$WORK/$installer-a/home"; mkdir -p "$home"
  run "$home"
  check "$home" "$home/.local/share"
  grep -qxF "Exec=$home/.local/share/octodock/app/current/octodock %u" "$home/.local/share/applications/octodock.desktop" \
    || fail "$installer: Exec line"
  before="$(cat "$home/.local/share/applications/octodock.desktop")"
  run "$home"
  check "$home" "$home/.local/share"
  [ "$before" = "$(cat "$home/.local/share/applications/octodock.desktop")" ] || fail "$installer: re-run changed the entry"

  home="$WORK/$installer-b/home"; mkdir -p "$home"
  run "$home" XDG_DATA_HOME="$WORK/$installer-b/xdg"
  check "$home" "$WORK/$installer-b/xdg"
  [ ! -e "$home/.local/share/applications" ] || fail "$installer: wrote outside XDG_DATA_HOME"
  home="$WORK/$installer-c/home"; mkdir -p "$home"
  run "$home" XDG_DATA_HOME=relative/dir
  check "$home" "$home/.local/share"

  home="$WORK/$installer-d/h o\$me\"x%y"; mkdir -p "$home"
  run "$home"
  check "$home" "$home/.local/share"
  want="Exec=\"$WORK/$installer-d/"'h o\\$me\\"x%%y'"/.local/share/octodock/app/current/octodock\" %u"
  grep -qxF "$want" "$home/.local/share/applications/octodock.desktop" \
    || fail "$installer: Exec quoting: $(grep '^Exec=' "$home/.local/share/applications/octodock.desktop")"
  echo "ok: $installer installer"
done
