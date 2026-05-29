#!/usr/bin/env bash
# ION + bp-socket bring-up for an Earth or Mars DTChat node.
#
# Starts ION, builds and (idempotently) inserts the bp-socket kernel module,
# then runs the userspace daemon so DTChat's AF_BP endpoint (ipn:10.2 / ipn:30.2)
# works. Run this once before launching DTChat on that node.
#
# Usage:
#   sudo NODE=earth ./scripts/ion/start_ion.sh
#   sudo NODE=mars BP_SOCKET_DIR=/opt/bp-socket ./scripts/ion/start_ion.sh
#
# Env:
#   NODE           earth | mars (selects scripts/ion/<NODE>.ipn.rc)
#   BP_SOCKET_DIR  bp-socket checkout (default: /opt/bp-socket)
#   ION_LIB        ION shared-lib dir for LD_LIBRARY_PATH (default: /usr/local/lib)
#
# Note: bp.ko is an out-of-tree module. The target needs linux-headers for the
# running kernel and build-essential. The module is not persistent across
# reboots; re-run this script (insmod is skipped if already loaded).
set -euo pipefail

NODE="${NODE:?set NODE=earth or NODE=mars}"
ION_LIB="${ION_LIB:-/usr/local/lib}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$HERE/../.." && pwd)"
# Defaults to the bp-socket vendored in this repo; override for an external checkout.
BP_SOCKET_DIR="${BP_SOCKET_DIR:-$REPO_ROOT/bp-socket}"
RC="$HERE/${NODE}.ipn.rc"

[ "$(id -u)" -eq 0 ] || { echo "run as root (insmod needs it)"; exit 1; }
[ -f "$RC" ] || { echo "missing rc file: $RC"; exit 1; }
[ -d "$BP_SOCKET_DIR" ] || { echo "missing BP_SOCKET_DIR: $BP_SOCKET_DIR"; exit 1; }

export LD_LIBRARY_PATH="$ION_LIB${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"

echo "[1/4] starting ION with $RC"
ionstart -I "$RC"

echo "[2/4] building bp-socket in $BP_SOCKET_DIR"
make -C "$BP_SOCKET_DIR"

echo "[3/4] loading bp.ko"
if lsmod | grep -q "^bp "; then
    echo "  bp already loaded; skipping insmod"
else
    KO="$(find "$BP_SOCKET_DIR" -name bp.ko -print -quit)"
    [ -n "$KO" ] || { echo "bp.ko not found under $BP_SOCKET_DIR (did make succeed?)"; exit 1; }
    insmod "$KO"
fi

echo "[4/4] starting bp_daemon"
DAEMON="$(find "$BP_SOCKET_DIR" -name bp_daemon -type f -print -quit)"
[ -n "$DAEMON" ] || { echo "bp_daemon not found under $BP_SOCKET_DIR"; exit 1; }
exec "$DAEMON"
