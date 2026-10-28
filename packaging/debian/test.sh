#!/bin/sh
set -eu

test -x /usr/bin/hushphone
hushphone --help | grep -q -- --daemon
test -f /usr/share/applications/network.katzenpost.hushphone.desktop
test -f /usr/share/icons/hicolor/scalable/apps/network.katzenpost.hushphone.svg
apparmor_parser -Q -K /etc/apparmor.d/hushphone
