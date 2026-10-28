#!/bin/sh
# With the package installed and its AppArmor profile loaded, on a virtual display: hushphone runs
# without a denial, starts no program, and opens no IP socket even with its seccomp filter off.
set -eu

denials() {
    sudo journalctl -k --no-pager --since "@$1" | grep 'apparmor="DENIED"' | grep 'profile="hushphone"' || true
}

sudo aa-status --json | grep -q '"hushphone": *"enforce"'

if sudo aa-exec -p hushphone -- /bin/true 2>/dev/null; then
    echo "a program started under the profile" >&2
    exit 1
fi

export DISPLAY=:99
Xvfb :99 -screen 0 1280x800x24 >/dev/null 2>&1 &
xvfb=$!
trap 'kill $xvfb' EXIT
sleep 2

start=$(date +%s)
rc=0
timeout 15 hushphone --memory --name ci || rc=$?
test "$rc" = 124 || { echo "hushphone stopped ($rc) under the profile" >&2; exit 1; }
seen=$(denials "$start")
if [ -n "$seen" ]; then
    printf 'denied while running:\n%s\n' "$seen" >&2
    exit 1
fi

start=$(date +%s)
HUSHPHONE_SANDBOX=off timeout 10 hushphone --daemon 127.0.0.1:9 --name ci || true
denials "$start" | grep -q 'family="inet"' || { echo "an IP socket was not denied" >&2; exit 1; }
echo "apparmor: ok"
