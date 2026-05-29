#!/usr/bin/env python3
"""Deterministic tests for the aap2_bridge TCP relay plumbing.

These cover the parts that do not need a live uD3TN: the outbound route server
(DTChat TCP -> sender.send) and inbound delivery (_forward_inbound -> DTChat
listener). The AAP2 client itself is exercised against a real uD3TN on the rig.

Run: python3 scripts/aap2_bridge/test_aap2_bridge.py
"""

from __future__ import annotations

import socket
import threading
import time
import types
from typing import List, Tuple

import aap2_bridge


class _RecordingSender:
    def __init__(self) -> None:
        self.calls: List[Tuple[str, bytes]] = []

    def send(self, dst_eid: str, payload: bytes) -> None:
        self.calls.append((dst_eid, payload))


def _free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


def _wait_until(predicate, timeout: float = 2.0) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return True
        time.sleep(0.01)
    return predicate()


def test_route_server_relays_to_sender() -> None:
    sender = _RecordingSender()
    server = aap2_bridge._RouteServer(_free_port(), "ipn:10.2", sender)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        payload = b"base64-wrapped-protobuf-bytes"
        with socket.create_connection(server.server_address) as conn:
            conn.sendall(payload)
            conn.shutdown(socket.SHUT_WR)
        assert _wait_until(lambda: len(sender.calls) == 1), "sender was not called"
        assert sender.calls[0] == ("ipn:10.2", payload), sender.calls
    finally:
        server.shutdown()
        server.server_close()


def test_forward_inbound_delivers_to_listener() -> None:
    received: List[bytes] = []
    listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind(("127.0.0.1", 0))
    listener.listen(1)
    addr = listener.getsockname()

    def _accept_once() -> None:
        conn, _ = listener.accept()
        with conn:
            chunks = []
            while True:
                data = conn.recv(4096)
                if not data:
                    break
                chunks.append(data)
            received.append(b"".join(chunks))

    accept_thread = threading.Thread(target=_accept_once, daemon=True)
    accept_thread.start()
    try:
        payload = b"inbound-bundle-payload"
        aap2_bridge._forward_inbound(addr, payload)
        accept_thread.join(timeout=2.0)
        assert received == [payload], received
    finally:
        listener.close()


def test_run_receiver_unpacks_adu_tuple() -> None:
    # uD3TN's receive_adu returns (BundleADU, payload). Guard against treating
    # the tuple as the payload (which would crash on _forward_inbound).
    payload = b"inbound-adu-payload"
    stop = threading.Event()
    delivered: List[bytes] = []

    class _Msg:
        def __init__(self, kind: str) -> None:
            self._kind = kind
            self.adu = object()

        def WhichOneof(self, _field: str) -> str:
            return self._kind

    class _FakeClient:
        def __init__(self) -> None:
            self.calls = 0
            self.statuses: List[int] = []

        def receive_msg(self):
            self.calls += 1
            if self.calls == 1:
                return _Msg("adu")
            stop.set()
            return _Msg("keepalive")

        def receive_adu(self, adu):
            return (adu, payload)

        def send_response_status(self, status) -> None:
            self.statuses.append(status)

    client = _FakeClient()
    orig_forward = aap2_bridge._forward_inbound
    orig_status = aap2_bridge.ResponseStatus
    aap2_bridge._forward_inbound = lambda _target, data: delivered.append(data)
    aap2_bridge.ResponseStatus = types.SimpleNamespace(RESPONSE_STATUS_SUCCESS=1)
    try:
        aap2_bridge._run_receiver(client, ("127.0.0.1", 1), stop)
    finally:
        aap2_bridge._forward_inbound = orig_forward
        aap2_bridge.ResponseStatus = orig_status

    assert delivered == [payload], delivered
    assert client.statuses == [1], client.statuses


def test_parse_helpers() -> None:
    assert aap2_bridge._parse_route("7710=ipn:10.2") == (7710, "ipn:10.2")
    assert aap2_bridge._parse_host_port("127.0.0.1:7720") == ("127.0.0.1", 7720)


def main() -> int:
    tests = [
        test_parse_helpers,
        test_route_server_relays_to_sender,
        test_forward_inbound_delivers_to_listener,
        test_run_receiver_unpacks_adu_tuple,
    ]
    for test in tests:
        test()
        print(f"ok - {test.__name__}")
    print(f"\n{len(tests)} passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
