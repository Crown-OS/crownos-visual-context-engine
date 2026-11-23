#!/bin/sh
# Runs nested session tests as root of a user, mount and pid namespace. An overlay on
# /usr/bin, on a tmpfs at /mnt, puts this build of crown-contextd at /usr/bin/crown-contextd.
# A second overlay lets a test install a desktop entry in /usr/share/applications.
#
# In the namespace the user's own files and the overlay look owned by root, and the real
# root's files look owned by nobody. The tests run without capabilities, as a session does, so a
# process that turned off PR_SET_DUMPABLE is closed to them. So the nested compositor gives the daemon the Engine grant
# it migrates from agents.ron. Nothing changes outside the namespace, and every process of the
# test ends with it.
#
# The test root comes from mkdtemp. HOME and the XDG base directories point into it, so no
# test process writes to the user's home.
#
# Usage: run-in-namespace.sh <nested_capture test binary> <crown-contextd binary> [test filter]
# Set VCGE_NESTED_COMPOSITOR, and VCGE_CONTEXT_PROBE for the integrated test.
set -eu

test_binary=$(realpath "$1")
contextd=$(realpath "$2")
filter=${3:-}
root=$(mktemp -d /tmp/vcge-nested.XXXXXX)
mkdir -p "$root/home/.config" "$root/home/.local/share" "$root/home/.cache" "$root/home/.local/state"

export VCGE_TEST_ROOT="$root"
export HOME="$root/home"
export XDG_CONFIG_HOME="$HOME/.config"
export XDG_DATA_HOME="$HOME/.local/share"
export XDG_CACHE_HOME="$HOME/.cache"
export XDG_STATE_HOME="$HOME/.local/state"

exec unshare --user --map-root-user --mount --pid --fork --kill-child --mount-proc sh -eu -c '
mount -t tmpfs tmpfs /mnt
mkdir /mnt/upper /mnt/work
cp "$2" /mnt/upper/crown-contextd
chmod 0755 /mnt/upper /mnt/upper/crown-contextd
mount -t overlay overlay -o "lowerdir=/usr/bin,upperdir=/mnt/upper,workdir=/mnt/work" /usr/bin
mkdir /mnt/applications /mnt/applications-work
mount -t overlay overlay -o "lowerdir=/usr/share/applications,upperdir=/mnt/applications,workdir=/mnt/applications-work" /usr/share/applications
exec setpriv --inh-caps=-all --ambient-caps=-all --bounding-set=-all -- "$1" --ignored --nocapture --test-threads=1 $3
' run-in-namespace "$test_binary" "$contextd" "$filter"
