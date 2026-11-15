#!/bin/sh
# With the package installed and its AppArmor profile loaded, on a virtual display: hushphone runs
# without a denial, starts no program, and opens no IP socket even with its seccomp filter off.
# Without a keyring it keeps no state at all; given one, the state is sealed with a key kept there.
set -eu

denials() {
    sudo journalctl -k --no-pager --since "@$1" | grep 'apparmor="DENIED"' | grep 'profile="hushphone"' || true
}

sudo aa-status --json | grep -q '"hushphone": *"enforce"'

# A process under the profile cannot start another program. (aa-exec applies the profile as it
# starts the shell; the shell, now confined, then tries to start /bin/true.)
if sudo aa-exec -p hushphone -- /bin/sh -c '/bin/true && echo STARTED' 2>/dev/null | grep -q STARTED; then
    echo "a program started under the profile" >&2
    exit 1
fi

export DISPLAY=:99
Xvfb :99 -screen 0 1280x800x24 >/dev/null 2>&1 &
xvfb=$!
trap 'kill $xvfb' EXIT
sleep 2

# Nothing was denied to hushphone since `$1`.
undenied() {
    seen=$(denials "$1")
    if [ -n "$seen" ]; then
        printf 'denied while running:\n%s\n' "$seen" >&2
        exit 1
    fi
}

# A sealed state is one sealed file, state.sealed, in place of the redb file state.redb.
state=$HOME/.local/share/hushphone/state
sealed() {
    test -e "$state/state.sealed" && test ! -e "$state/state.redb" &&
        test "$(head -c 8 "$state/state.sealed")" = PHSTATE1
}

# No keyring: hushphone runs, but opens no state, so none is written, sealed or not.
start=$(date +%s)
rc=0
env -u DBUS_SESSION_BUS_ADDRESS timeout 15 hushphone --memory --name ci || rc=$?
test "$rc" = 124 || { echo "hushphone stopped ($rc) under the profile" >&2; exit 1; }
undenied "$start"
if test -n "$(ls -A "$state" 2>/dev/null)" || test -e "$state/../settings.toml"; then
    echo "no keyring, yet state was written:" >&2
    ls -lA "$state" "$state/.." >&2 || true
    exit 1
fi

# With a keyring: the state is sealed in place, with a key of 32 bytes kept in the keyring.
start=$(date +%s)
dbus-run-session -- sh -eu -c '
    printf ci | gnome-keyring-daemon --unlock --components=secrets >/dev/null
    rc=0
    timeout 15 hushphone --memory --name ci || rc=$?
    test "$rc" = 124 || { echo "hushphone stopped ($rc) under the profile" >&2; exit 1; }
    n=$(secret-tool lookup application hushphone state "$1" | wc -c)
    test "$n" = 32 || { echo "the keyring holds no key of 32 bytes for the state ($n)" >&2; exit 1; }
' sh "$state"
undenied "$start"
sealed || { echo "the state was not sealed" >&2; exit 1; }
test ! -e "$state/.sealing" || { echo "the sealing was not finished" >&2; exit 1; }

# An IP socket is refused even with the seccomp filter off. The client starts only once the
# state's key is had, so the keyring made above is unlocked again for it.
start=$(date +%s)
dbus-run-session -- sh -c '
    printf ci | gnome-keyring-daemon --unlock --components=secrets >/dev/null
    HUSHPHONE_SANDBOX=off timeout 10 hushphone --daemon 127.0.0.1:9
' || true
denials "$start" | grep -q 'family="inet"' || { echo "an IP socket was not denied" >&2; exit 1; }

echo "apparmor: ok"
