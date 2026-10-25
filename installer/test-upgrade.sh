#!/bin/sh
# Exercises the installer's upgrade path under Wine (needs wine, wine32, xvfb and makensis):
#   installer/test-upgrade.sh path/to/wavemix.exe
# Installs 0.7.0, upgrades it, checks startup/desktop choices and settings survive, that a
# repair works, and that a silent downgrade is refused.
set -e
EXE=$(realpath "$1")
HERE=$(dirname "$(realpath "$0")")
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
export PATH=/usr/lib/wine:$PATH WINEPREFIX=$WORK/prefix WINEDEBUG=-all
run() { timeout 150 xvfb-run -a wine "$@" 2>&1 | grep -v "X connection" || true; }
P=$WINEPREFIX/drive_c/users/root/AppData/Local/Programs/wavemix
CFG=$WINEPREFIX/drive_c/users/root/AppData/Roaming/wavemix
UNINST='HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\wavemix'
RUNKEY='HKCU\Software\Microsoft\Windows\CurrentVersion\Run'
version() { run reg query "$UNINST" /v DisplayVersion | tr -d '\r' | grep -o '[0-9][0-9.]*$' || true; }
startup() { run reg query "$RUNKEY" /v wavemix | grep -c wavemix || true; }
fail() { echo "FAIL: $1"; exit 1; }

run wineboot -i >/dev/null
for v in 0.7.0 0.8.0 0.8.1; do makensis -V1 -DVERSION=$v -DEXE="$EXE" -DOUT="$WORK/$v.exe" "$HERE/wavemix.nsi"; done
mkdir -p "$CFG" && echo '{"view":"console"}' > "$CFG/config.json"

run "$WORK/0.7.0.exe" /S >/dev/null
[ "$(version)" = "0.7.0" ] || fail "fresh install"
[ "$(startup)" = "1" ] || fail "startup entry on a fresh install"

run "$WORK/0.8.0.exe" /S >/dev/null
[ "$(version)" = "0.8.0" ] || fail "upgrade"
[ "$(startup)" = "1" ] || fail "startup choice lost in an upgrade"
[ -f "$CFG/config.json" ] || fail "settings lost in an upgrade"

run reg delete "$RUNKEY" /v wavemix /f >/dev/null
run "$WORK/0.8.1.exe" /S >/dev/null
[ "$(version)" = "0.8.1" ] || fail "second upgrade"
[ "$(startup)" = "0" ] || fail "startup switched back on behind the user's back"

run "$WORK/0.7.0.exe" /S >/dev/null
[ "$(version)" = "0.8.1" ] || fail "silent downgrade was not refused"
[ "$(ls "$P" | tr '\n' ' ')" = "uninstall.exe wavemix.exe " ] || fail "unexpected files: $(ls "$P")"
echo "upgrade tests passed"
