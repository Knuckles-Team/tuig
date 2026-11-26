"""Drive the compiled `tuig --one-shot` entry point against a protocol stub."""

import json
import os
import subprocess
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import pytest

pytestmark = pytest.mark.skipif(
    not os.environ.get("TUI_NATIVE_TEST_BINARY"),
    reason="Requires an explicitly built native executable",
)

EXTENSION = "https://knuckles-team.github.io/arbora/extensions/tool-task/v1"
KEY = "one-shot-key-0123456789"
REF = "scope:receipt-0001"
FAR_FUTURE_MS = 4_000_000_000_000


def _task(state: str, attestation: dict | None) -> dict:
    arbora: dict[str, object] = {"runId": "run-one-shot", "admissionReceipt": "admit-7"}
    if attestation is not None:
        arbora["preapproval"] = attestation
    task: dict[str, object] = {
        "kind": "task",
        "id": "task-one-shot",
        "contextId": "context-one-shot",
        "metadata": {"arbora": arbora},
        "status": {"state": state},
    }
    if state == "completed":
        task["artifacts"] = [{"parts": [{"kind": "text", "text": "tool output line"}]}]
    return task


def _attested(**changes) -> dict:
    claim = {
        "ref": REF,
        "status": "attested",
        "taskId": "task-one-shot",
        "expiresAtMs": FAR_FUTURE_MS,
    }
    return claim | changes


def _run(final_state: str, attestation: dict | None, *, extension: bool, stdin: str):
    requests: list[dict] = []
    cards = []

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass

        def _send(self, payload: dict, activation: bool = False):
            data = json.dumps(payload).encode()
            self.send_response(200)
            self.send_header("Content-Length", str(len(data)))
            if activation:
                self.send_header("X-A2A-Extensions", EXTENSION)
            self.end_headers()
            self.wfile.write(data)

        def do_GET(self):
            cards.append(self.path)
            listed = [{"uri": EXTENSION, "required": False}] if extension else []
            self._send({"name": "arbora", "capabilities": {"extensions": listed}})

        def do_POST(self):
            body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            body["_headers"] = {key.lower(): value for key, value in self.headers.items()}
            requests.append(body)
            submitting = body["method"] == "message/send"
            state = "submitted" if submitting else final_state
            self._send(
                {"jsonrpc": "2.0", "id": body["id"], "result": _task(state, attestation)},
                activation=submitting,
            )

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    environment = {k: v for k, v in os.environ.items() if not k.startswith("AGENT_")}
    environment.update(AGENT_URL=f"http://127.0.0.1:{server.server_port}", AGENT_AUTH_MODE="none")
    try:
        result = subprocess.run(
            [str(Path(os.environ["TUI_NATIVE_TEST_BINARY"]).resolve()), "--one-shot"],
            env=environment,
            input=stdin,
            capture_output=True,
            text=True,
            timeout=20,
            check=False,
        )
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)
    return result, requests, cards


def _request(**changes) -> str:
    body = {
        "operation": "records.write",
        "parameters": {"name": "alpha"},
        "idempotency_key": KEY,
        "preapproval_ref": REF,
    }
    return json.dumps(body | changes)


@pytest.mark.spec("TUIG-RUNTIME-R001.8", "TUIG-RUNTIME-R002.8.2", "TUIG-RUNTIME-R002.8.3", "TUIG-RUNTIME-R002.8.5")  # fmt: skip
def test_one_shot_submits_once_and_reports_lifecycle_and_evidence():
    result, requests, cards = _run("completed", _attested(), extension=True, stdin=_request())
    assert result.returncode == 0, result.stderr
    assert [r["method"] for r in requests] == ["message/send", "tasks/get"]
    assert len(cards) == 1
    send = requests[0]
    assert send["_headers"]["idempotency-key"] == KEY
    assert send["_headers"]["x-a2a-extensions"] == EXTENSION
    part = send["params"]["message"]["parts"][0]
    assert part == {
        "kind": "data",
        "data": {
            "toolTask": {
                "operation": "records.write",
                "parameters": {"name": "alpha"},
                "preapprovalRef": REF,
            }
        },
    }
    assert "Task ID: task-one-shot" in result.stdout
    assert "State: completed" in result.stdout
    assert "tool output line" in result.stdout
    assert "Preapproval: attested by Arbora for this task." in result.stdout
    assert f"Idempotency key: {KEY}" in result.stdout
    assert "Admission receipt: admit-7" in result.stdout
    assert "Backend effect count: unavailable" in result.stdout
    assert "alpha" not in result.stdout
    assert REF not in result.stdout


@pytest.mark.spec("TUIG-RUNTIME-R002.8.4")
@pytest.mark.parametrize(
    ("attestation", "reason"),
    [
        (None, "attestation unavailable"),
        (_attested(status="revoked"), "revoked or not attested"),
        (_attested(expiresAtMs=1), "expired"),
        (_attested(taskId="another-task"), "bound to another task"),
        (_attested(ref="scope:other-receipt"), "not the requested one"),
    ],
)
def test_one_shot_never_reports_an_unattested_preapproval(attestation, reason):
    result, _requests, _cards = _run(
        "input-required", attestation, extension=True, stdin=_request()
    )
    assert result.returncode != 0
    assert "Approval: pending; not granted locally." in result.stdout
    assert "the permission prompt is retained" in result.stdout
    assert reason in result.stdout
    assert "attested by Arbora" not in result.stdout
    assert "no approval was sent" in result.stderr


@pytest.mark.spec("TUIG-RUNTIME-R002.8.4")
def test_one_shot_refuses_before_submission_without_the_producer_contract():
    result, requests, cards = _run("completed", None, extension=False, stdin=_request())
    assert result.returncode != 0
    assert requests == []
    assert len(cards) == 1
    assert "nothing was submitted" in result.stderr


@pytest.mark.spec("TUIG-RUNTIME-R002.8.1")
@pytest.mark.parametrize(
    "stdin",
    ["", "[]", "not json", _request(extra=1), _request(parameters=[]), _request(operation="")],
)
def test_one_shot_rejects_malformed_requests_before_any_request(stdin):
    result, requests, cards = _run("completed", None, extension=True, stdin=stdin)
    assert result.returncode != 0
    assert requests == [] and cards == []
    assert result.stderr.strip()
