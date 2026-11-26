"""The compiled terminal serves an agent's write request behind a shown diff and approval."""

import json
import os
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import pytest

from tests.test_native_public_details_pty import NativeTerminal

pytestmark = pytest.mark.skipif(
    not os.environ.get("TUI_NATIVE_TEST_BINARY"),
    reason="Requires compiled native executable",
)

BACKEND = r"""
import json, os, sys
wire = os.environ["TUIG_TEST_WIRE"]

def send(value):
    print(json.dumps({"jsonrpc": "2.0", **value}), flush=True)

def record(message):
    with open(wire, "a") as out:
        out.write(json.dumps(message) + "\n")

def host_request(identifier, method, params):
    send({"id": identifier, "method": method, "params": params})
    answer = json.loads(sys.stdin.readline())
    record(answer)
    return answer

for line in sys.stdin:
    message = json.loads(line)
    record(message)
    method = message["method"]
    if method == "initialize":
        send({"id": message["id"], "result": {"protocolVersion": 1}})
    elif method == "session/new":
        send({"id": message["id"], "result": {"sessionId": "coding-session"}})
    elif method == "session/prompt":
        host_request(50, "fs/write_text_file", {"sessionId": "coding-session", "path": "note.txt", "content": "changed\n"})
        host_request(51, "fs/write_text_file", {"sessionId": "coding-session", "path": "../outside.txt", "content": "escaped\n"})
        send({"id": message["id"], "result": {"stopReason": "end_turn"}})
    elif method == "session/close":
        send({"id": message["id"], "result": {}})
        break
"""


@pytest.fixture
def coding_terminal(tmp_path, monkeypatch):
    backend = tmp_path / "bin" / "arbora-acp"
    backend.parent.mkdir()
    backend.write_text(f"#!{sys.executable}\n{BACKEND}")
    backend.chmod(0o700)
    root = tmp_path / "project"
    root.mkdir()
    (root / "note.txt").write_text("original\n")
    wire = tmp_path / "wire.jsonl"
    monkeypatch.setenv(
        "PATH", f"{backend.parent}{os.pathsep}{os.environ.get('PATH', '')}"
    )
    monkeypatch.setenv("TUIG_TEST_WIRE", str(wire))
    server = ThreadingHTTPServer(("127.0.0.1", 0), BaseHTTPRequestHandler)
    terminal = NativeTerminal(
        server,
        [],
        task_id=None,
        arguments=(
            "--local-runtime",
            "--local-model",
            "test:model",
            "--workspace-root",
            str(root),
        ),
    )
    try:
        terminal.wait_until(lambda: "Compose local turn" in terminal.shown())
        yield terminal, root, wire
    finally:
        if terminal.process.poll() is None:
            terminal.send(b"\x03")
            terminal.process.wait(timeout=10)
        terminal.close()
        server.server_close()


def answers(wire):
    sent = [json.loads(line) for line in wire.read_text().splitlines()]
    return {
        message["id"]: message
        for message in sent
        if "id" in message and "method" not in message
    }


@pytest.mark.spec("TUIG-RUNTIME-R011.8", "TUIG-RUNTIME-R011.12")
def test_write_shows_diff_applies_on_approval_and_refuses_escape(coding_terminal):
    terminal, root, wire = coding_terminal
    terminal.send(b"edit\r")
    terminal.wait_until(
        lambda: (
            "Pending approval" in terminal.shown() and "+changed" in terminal.shown()
        )
    )
    assert "-original" in terminal.shown()
    assert (root / "note.txt").read_text() == "original\n"
    terminal.send(b"y")
    terminal.wait_until(lambda: "Turn completed" in terminal.shown())
    assert (root / "note.txt").read_text() == "changed\n"
    assert not (root.parent / "outside.txt").exists()
    replies = answers(wire)
    assert replies[50]["result"] is None
    assert replies[51]["error"]["data"]["reason"] == "PATH_OUTSIDE_ROOT"
    assert "Pending approval" not in terminal.shown()
    terminal.quit()


@pytest.mark.spec("TUIG-RUNTIME-R011.8", "TUIG-RUNTIME-R011.12")
def test_denied_write_leaves_the_file_and_reaches_the_gateway_as_an_error(
    coding_terminal,
):
    terminal, root, wire = coding_terminal
    terminal.send(b"edit\r")
    terminal.wait_until(lambda: "Pending approval" in terminal.shown())
    terminal.send(b"n")
    terminal.wait_until(lambda: "Turn completed" in terminal.shown())
    assert (root / "note.txt").read_text() == "original\n"
    assert answers(wire)[50]["error"]["data"]["reason"] == "APPROVAL_DENIED"
    terminal.quit()
