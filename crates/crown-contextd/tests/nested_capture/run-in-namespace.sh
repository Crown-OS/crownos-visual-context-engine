#!/bin/sh
# Runs the nested capture tests as root of a user and mount namespace. An overlay on /usr/bin
# puts this build of crown-contextd at /usr/bin/crown-contextd. In the namespace the file and
# the overlay directory belong to root, so the nested compositor gives the daemon the Engine
# grant it migrates from agents.ron. Nothing changes outside the namespace.
#
# Usage: run-in-namespace.sh <nested_capture test binary> <crown-contextd binary> [test filter]
# Set VCGE_NESTED_COMPOSITOR, and VCGE_CONTEXT_PROBE for the integrated test.
set -eu

test_binary=$(realpath "$1")
contextd=$(realpath "$2")
filter=${3:-}

exec unshare --user --map-root-user --mount sh -eu -c '
overlay=$(mktemp -d /tmp/vcge-usr-bin.XXXXXX)
mount -t tmpfs tmpfs "$overlay"
mkdir "$overlay/upper" "$overlay/work"
cp "$2" "$overlay/upper/crown-contextd"
chmod 0755 "$overlay/upper" "$overlay/upper/crown-contextd"
mount -t overlay overlay -o "lowerdir=/usr/bin,upperdir=$overlay/upper,workdir=$overlay/work" /usr/bin
exec "$1" --ignored --nocapture --test-threads=1 $3
' run-in-namespace "$test_binary" "$contextd" "$filter"
