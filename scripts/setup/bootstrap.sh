#!/usr/bin/env bash
# Bootstrap a fresh (Debian/Ubuntu/Pop!_OS) mini-PC for a DTChat demo node.
#
# Installs everything DTChat needs to build/run, plus the role's extra deps:
#   ROLE=ion    -> bp-socket build deps (kernel headers + toolchain). Assumes
#                  ION-DTN is already installed on the node.
#   ROLE=ud3tn  -> uD3TN built from source (latest) + python AAP2 utils.
#
# Usage:
#   sudo ROLE=ion   ./scripts/setup/bootstrap.sh
#   sudo ROLE=ud3tn ./scripts/setup/bootstrap.sh
#
# Optional env:
#   UD3TN_DIR=/opt/ud3tn   # where to clone uD3TN
#
# protoc is vendored by the build, so no system protobuf compiler is required.
set -euo pipefail

ROLE="${ROLE:?set ROLE=ion or ROLE=ud3tn}"
[ "$(id -u)" -eq 0 ] || { echo "run as root (apt/insmod/make install need it)"; exit 1; }
RUN_USER="${SUDO_USER:-$(whoami)}"

echo "[*] apt: common build + GUI (egui) dependencies"
export DEBIAN_FRONTEND=noninteractive
apt-get update
apt-get install -y \
    build-essential pkg-config cmake git curl ca-certificates \
    libssl-dev libgl1-mesa-dev \
    libx11-dev libxcb1-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev \
    libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev

# A-SABR needs edition2024 -> Rust/Cargo >= 1.85. Distro (apt) cargo is often
# older (e.g. 1.75) and would shadow rustup, so install/upgrade rustup and use
# its cargo explicitly. MIN_RUST can be raised if a dep bumps the requirement.
MIN_RUST="1.85"
echo "[*] Rust toolchain via rustup (need >= $MIN_RUST, as $RUN_USER)"
sudo -u "$RUN_USER" bash -lc '
    set -e
    if [ ! -x "$HOME/.cargo/bin/rustup" ]; then
        curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
    fi
    "$HOME/.cargo/bin/rustup" update stable
    "$HOME/.cargo/bin/rustup" default stable
'
CARGO="/home/$RUN_USER/.cargo/bin/cargo"
[ "$RUN_USER" = "root" ] && CARGO="/root/.cargo/bin/cargo"
ver="$($CARGO --version | awk '{print $2}')"
if [ "$(printf '%s\n%s\n' "$MIN_RUST" "$ver" | sort -V | head -1)" != "$MIN_RUST" ]; then
    echo "[error] cargo $ver < $MIN_RUST. Remove apt rust (sudo apt remove rustc cargo) and re-run." >&2
    exit 1
fi
echo "[*] using cargo $ver ($CARGO)"

case "$ROLE" in
ion)
    echo "[*] bp-socket build deps (kernel headers + toolchain)"
    apt-get install -y "linux-headers-$(uname -r)" automake libtool
    echo "[i] assuming ION-DTN is already installed (ionstart on PATH, libs in /usr/local/lib)."
    if ! command -v ionstart >/dev/null 2>&1; then
        echo "[warn] ionstart not found on PATH; make sure ION is installed before start_ion.sh"
    fi
    ;;
ud3tn)
    echo "[*] python + uD3TN build deps"
    apt-get install -y python3 python3-venv python3-pip
    UD3TN_DIR="${UD3TN_DIR:-/opt/ud3tn}"
    if [ ! -d "$UD3TN_DIR/.git" ]; then
        echo "[*] cloning uD3TN into $UD3TN_DIR"
        git clone --recursive https://gitlab.com/d3tn/ud3tn.git "$UD3TN_DIR"
    fi
    echo "[*] building uD3TN (posix)"
    make -C "$UD3TN_DIR" posix -j"$(nproc)"
    echo "[*] python venv + AAP2 utils (for aap2_bridge.py)"
    python3 -m venv "$UD3TN_DIR/.venv"
    "$UD3TN_DIR/.venv/bin/pip" install -U pip
    "$UD3TN_DIR/.venv/bin/pip" install "$UD3TN_DIR/pyd3tn" "$UD3TN_DIR/python-ud3tn-utils"
    chown -R "$RUN_USER":"$RUN_USER" "$UD3TN_DIR"
    echo "[i] activate the venv before running the bridge: source $UD3TN_DIR/.venv/bin/activate"
    ;;
*)
    echo "unknown ROLE=$ROLE (use ion or ud3tn)"; exit 1 ;;
esac

echo "[*] building DTChat (as $RUN_USER)"
sudo -u "$RUN_USER" bash -lc 'cd "'"$PWD"'" && "'"$CARGO"'" build --release'

echo "[done] $ROLE node ready. Next: bring up the stack, then run DTChat with its DTCHAT_CONFIG."
