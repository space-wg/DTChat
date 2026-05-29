# DTChat — 3-Node DTN Interop Demo

DTChat is a GUI chat app for Delay Tolerant Networks (Bundle Protocol). This
repo is set up for a 3-node full-mesh demo: **Earth** and **Mars** run ION-DTN
(reached via bp-socket / `AF_BP`), **Moon** runs µD3TN (reached via a local AAP2
bridge). Each node runs the same DTChat binary with a different config and talks
only to its local DTN stack; the stacks route between planets over the network.

![DTChat](image.png)

## Topology

| Node  | Node ID | IP          | Stack   | DTChat endpoint | Transport            |
|-------|---------|-------------|---------|-----------------|----------------------|
| earth | ipn:10  | 192.168.1.1 | ION-DTN | ipn:10.2        | bp-socket (AF_BP)    |
| moon  | ipn:20  | 192.168.1.2 | µD3TN   | ipn:20.2        | TCP → AAP2 bridge    |
| mars  | ipn:30  | 192.168.2.2 | ION-DTN | ipn:30.2        | bp-socket (AF_BP)    |

One-way delays: earth↔moon ≈ 1 s, earth↔mars ≈ 240 s. Earth relays moon↔mars
(mars has no direct contact with moon), so moon→mars ≈ 241 s. These values live
in the ION `ipn.rc` (routing) and `db/contact_plan.rc` (DTChat's PBAT bars).

## Prerequisites

- **Rust** (all nodes): https://rustup.rs/ , plus `protobuf`.
- **Earth / Mars** (ION): ION-DTN installed, plus `linux-headers-$(uname -r)` and
  `build-essential` so the bp-socket kernel module builds.
- **Moon** (µD3TN): a running µD3TN and `python-ud3tn-utils` on `PYTHONPATH`.

On a fresh Debian/Ubuntu/Pop!_OS machine, `scripts/setup/bootstrap.sh` installs all
of the above (build + GUI libs, Rust, kernel headers, and the role's DTN stack):

```bash
sudo ROLE=ion   ./scripts/setup/bootstrap.sh   # Earth / Mars
sudo ROLE=ud3tn ./scripts/setup/bootstrap.sh   # Moon (builds uD3TN + AAP2 utils)
```

## Clone

```bash
git clone https://github.com/space-wg/DTChat.git
cd DTChat
cargo build --release
```

## Run the demo

### Earth / Mars (ION)

`start_ion.sh` starts ION, builds + inserts `bp.ko` (idempotent), and runs the
bp-socket daemon. `bp-socket` is vendored in this repo, so the default path works.

```bash
sudo NODE=earth ./scripts/ion/start_ion.sh        # use NODE=mars on Mars
# in a second terminal on the same node:
DTCHAT_CONFIG=db/earth.yaml cargo run --release    # db/mars.yaml on Mars
```

Override the bp-socket location with `BP_SOCKET_DIR=/path/to/bp-socket` and ION's
lib dir with `ION_LIB=/usr/local/lib` if needed.

### Moon (µD3TN)

Start µD3TN (see `db/moon.yaml` header for the exact `ud3tn` line), then the
bridge and DTChat:

```bash
export UD3TN_BDM_SECRET=<your 16+ char secret>
python3 scripts/aap2_bridge/aap2_bridge.py \
    --aap2-socket ud3tn.aap2.socket --agentid 2 \
    --recv-forward 127.0.0.1:7720 \
    --route 7710=ipn:10.2 --route 7730=ipn:30.2 &
DTCHAT_CONFIG=db/moon.yaml cargo run --release
```

## Local testing (no DTN stack)

Three loopback configs simulate the mesh over UDP on one machine:

```bash
DTCHAT_CONFIG=db/default.yaml cargo run   # earth (10)
DTCHAT_CONFIG=db/local2.yaml  cargo run   # moon  (20)
DTCHAT_CONFIG=db/local3.yaml  cargo run   # mars  (30)
```

This exercises fan-out, PBAT bars, and ACKs. Real per-hop delays only appear once
connected to the actual DTN stacks.

```bash
cargo test                                   # app + mesh integration tests
python3 scripts/aap2_bridge/test_aap2_bridge.py   # bridge relay tests
```

## What to edit for a different network

| Change            | Edit                                                        |
|-------------------|-------------------------------------------------------------|
| Ethernet IPs      | `scripts/ion/*.ipn.rc` (induct/outduct/plan), µD3TN contacts |
| EIDs              | `scripts/ion/*.ipn.rc` **and** `db/*.yaml`                  |
| Delays / topology | `scripts/ion/*.ipn.rc` (range) **and** `db/contact_plan.rc` |

## Layout

```
db/                 per-node configs + contact plan
scripts/ion/        ION ipn.rc files + start_ion.sh
scripts/aap2_bridge/ µD3TN AAP2 bridge + tests
bp-socket/          vendored AF_BP kernel module + daemon (ION nodes)
src/                DTChat application
```
