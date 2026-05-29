#!/usr/bin/env python3
"""AAP2 bridge: connect a DTChat (Moon) node to a local uD3TN over AAP2.

DTChat speaks plain TCP; uD3TN speaks AAP2. This sidecar relays between them so
the Moon node needs no native AAP2 code in DTChat:

    outbound: DTChat --(localhost TCP)--> bridge --(AAP2 send_adu)--> uD3TN
    inbound : uD3TN  --(AAP2 subscribe)--> bridge --(localhost TCP)--> DTChat

Payloads are opaque bytes (DTChat sends base64-wrapped protobuf), so the bridge
never inspects or reframes them.

Example -- Moon node is ipn:20, local AAP2 agent id "2":

    python3 aap2_bridge.py \\
        --aap2-socket ud3tn.aap2.socket \\
        --agentid 2 \\
        --recv-forward 127.0.0.1:7720 \\
        --route 7710=ipn:10.2 \\
        --route 7730=ipn:30.2

The matching Moon DTChat config then uses:

    local_peer endpoint  Tcp 127.0.0.1:7720   # inbound delivery target
    peer "earth" endpoint Tcp 127.0.0.1:7710  # relayed to ipn:10.2
    peer "mars"  endpoint Tcp 127.0.0.1:7730  # relayed to ipn:30.2

Requirements:
    uD3TN's python-ud3tn-utils package (module ``ud3tn_utils.aap2``) must be on
    PYTHONPATH. Run this from the uD3TN checkout (the same place you would run
    ``tools/aap2/aap2_send.py``) or ``pip install`` the utils package. The AAP2
    client API used here (configure / send_adu / receive_msg / receive_adu /
    send_response_status) matches the stock ``aap2_send.py`` / ``aap2_receive.py``
    tools; if your uD3TN version differs, align the calls with those tools.
"""

from __future__ import annotations

import argparse
import logging
import socket
import socketserver
import sys
import threading
from typing import Optional, Tuple

# Imported lazily so the relay plumbing stays importable/testable on hosts
# without uD3TN; main() turns a missing library into a clear runtime error.
try:
    from ud3tn_utils.aap2 import (  # type: ignore
        AAP2TCPClient,
        AAP2UnixClient,
        BundleADU,
        ResponseStatus,
    )

    _AAP2_IMPORT_ERROR: Optional[ImportError] = None
except ImportError as exc:  # pragma: no cover - environment dependent
    AAP2TCPClient = AAP2UnixClient = BundleADU = ResponseStatus = None  # type: ignore
    _AAP2_IMPORT_ERROR = exc


LOGGER = logging.getLogger("aap2_bridge")

_RECV_CHUNK = 65536


def _read_to_eof(conn: socket.socket) -> bytes:
    """Read a single DTChat datagram: bytes until the peer half-closes."""
    chunks = []
    while True:
        data = conn.recv(_RECV_CHUNK)
        if not data:
            break
        chunks.append(data)
    return b"".join(chunks)


def _parse_host_port(value: str) -> Tuple[str, int]:
    host, sep, port = value.rpartition(":")
    if not sep:
        raise argparse.ArgumentTypeError(f"expected host:port, got {value!r}")
    try:
        return host, int(port)
    except ValueError as exc:
        raise argparse.ArgumentTypeError(f"invalid port in {value!r}") from exc


def _parse_route(value: str) -> Tuple[int, str]:
    port, sep, eid = value.partition("=")
    if not sep or not eid:
        raise argparse.ArgumentTypeError(
            f"expected PORT=EID (e.g. 7710=ipn:10.2), got {value!r}"
        )
    try:
        return int(port), eid
    except ValueError as exc:
        raise argparse.ArgumentTypeError(f"invalid port in {value!r}") from exc


class Aap2Sender:
    """Thread-safe wrapper around a single AAP2 sending session."""

    def __init__(self, client) -> None:
        self._client = client
        self._lock = threading.Lock()

    def send(self, dst_eid: str, payload: bytes) -> None:
        with self._lock:
            self._client.send_adu(
                BundleADU(dst_eid=dst_eid, payload_length=len(payload)),
                payload,
            )
            self._client.receive_response()


class _RouteHandler(socketserver.BaseRequestHandler):
    """Handle one DTChat outbound connection and relay it to a fixed EID."""

    def handle(self) -> None:
        payload = _read_to_eof(self.request)
        if not payload:
            return
        dst_eid: str = self.server.dst_eid  # type: ignore[attr-defined]
        sender: Aap2Sender = self.server.sender  # type: ignore[attr-defined]
        try:
            sender.send(dst_eid, payload)
            LOGGER.info("relayed %d bytes -> %s", len(payload), dst_eid)
        except Exception:  # noqa: BLE001 - log and keep the bridge alive
            LOGGER.exception("failed to relay %d bytes to %s", len(payload), dst_eid)


class _RouteServer(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True

    def __init__(self, port: int, dst_eid: str, sender: Aap2Sender) -> None:
        super().__init__(("127.0.0.1", port), _RouteHandler)
        self.dst_eid = dst_eid
        self.sender = sender


def _forward_inbound(target: Tuple[str, int], payload: bytes) -> None:
    """Deliver one received bundle to DTChat's local TCP listener."""
    with socket.create_connection(target) as conn:
        conn.sendall(payload)
        conn.shutdown(socket.SHUT_WR)


def _run_receiver(client, target: Tuple[str, int], stop: threading.Event) -> None:
    """Subscribe to inbound bundles and forward each payload to DTChat."""
    LOGGER.info("listening for inbound bundles, forwarding to %s:%d", *target)
    while not stop.is_set():
        msg = client.receive_msg()
        if msg is None:
            break
        if msg.WhichOneof("msg") != "adu":
            # Keepalives and status messages carry no payload to relay.
            continue
        # receive_adu returns (BundleADU, payload); we only relay the payload.
        _adu, payload = client.receive_adu(msg.adu)
        client.send_response_status(ResponseStatus.RESPONSE_STATUS_SUCCESS)
        try:
            _forward_inbound(target, payload)
            LOGGER.info("delivered %d bytes -> DTChat", len(payload))
        except OSError:
            LOGGER.exception("failed to deliver %d bytes to DTChat", len(payload))


def _make_client(args: argparse.Namespace):
    if args.aap2_socket is not None:
        return AAP2UnixClient(address=args.aap2_socket)
    return AAP2TCPClient(address=args.aap2_tcp)


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Relay between a DTChat node (TCP) and a local uD3TN (AAP2).",
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    transport = parser.add_mutually_exclusive_group(required=True)
    transport.add_argument(
        "--aap2-socket",
        metavar="PATH",
        help="uD3TN AAP2 unix socket (e.g. ud3tn.aap2.socket)",
    )
    transport.add_argument(
        "--aap2-tcp",
        metavar="HOST:PORT",
        type=_parse_host_port,
        help="uD3TN AAP2 TCP endpoint",
    )
    parser.add_argument(
        "--agentid",
        required=True,
        help="local AAP2 agent id to register (e.g. 2 -> ipn:<node>.2)",
    )
    parser.add_argument(
        "--secret",
        default=None,
        help="AAP2 agent secret (shared by send/receive sessions; auto if unset)",
    )
    parser.add_argument(
        "--recv-forward",
        required=True,
        metavar="HOST:PORT",
        type=_parse_host_port,
        help="DTChat local TCP listener that receives inbound bundles",
    )
    parser.add_argument(
        "--route",
        action="append",
        default=[],
        metavar="PORT=EID",
        type=_parse_route,
        help="outbound route: listen on localhost PORT, relay to EID (repeatable)",
    )
    parser.add_argument(
        "--log-level",
        default="INFO",
        choices=["DEBUG", "INFO", "WARNING", "ERROR"],
    )
    return parser


def main(argv: Optional[list] = None) -> int:
    args = _build_parser().parse_args(argv)
    if _AAP2_IMPORT_ERROR is not None:
        raise SystemExit(
            "could not import ud3tn_utils.aap2; run from the uD3TN checkout or "
            "pip install python-ud3tn-utils (see module docstring)"
        )
    logging.basicConfig(
        level=getattr(logging, args.log_level),
        format="%(asctime)s %(levelname)s %(name)s: %(message)s",
    )

    stop = threading.Event()

    # Configure the sender first to obtain the agent secret, then the receiver
    # reuses that secret so both sessions share the same registered agent id.
    send_client = _make_client(args)
    send_client.connect()
    secret = send_client.configure(args.agentid, subscribe=False, secret=args.secret)
    sender = Aap2Sender(send_client)

    recv_client = _make_client(args)
    recv_client.connect()
    recv_client.configure(args.agentid, subscribe=True, secret=secret)

    servers = [
        _RouteServer(port, eid, sender) for port, eid in args.route
    ]
    server_threads = []
    for server in servers:
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        server_threads.append(thread)
        LOGGER.info("route: 127.0.0.1:%d -> %s", server.server_address[1], server.dst_eid)

    receiver_thread = threading.Thread(
        target=_run_receiver,
        args=(recv_client, args.recv_forward, stop),
        daemon=True,
    )
    receiver_thread.start()

    LOGGER.info("aap2 bridge ready (agent id %s)", args.agentid)
    try:
        receiver_thread.join()
    except KeyboardInterrupt:
        LOGGER.info("shutting down")
    finally:
        stop.set()
        for server in servers:
            server.shutdown()
        send_client.disconnect()
        recv_client.disconnect()
    return 0


if __name__ == "__main__":
    sys.exit(main())
