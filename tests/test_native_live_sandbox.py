"""Qualify the compiled terminal against the pinned Arbora release in no-auth mode.

Every operation the pinned release does not serve is asserted as the honest
unavailable state, so a release that starts serving it fails here and prompts
the owning spec row to be bound to a served proof.
"""

import json
import os
import subprocess
import uuid
from collections.abc import Iterator
from pathlib import Path
from urllib.error import HTTPError
from urllib.request import ProxyHandler, Request, build_opener

import pytest

from tests.live_sandbox import Sandbox, require_live, running_sandbox
from tests.test_native_public_details_pty import NativeTerminal

TOOL_TASK_EXTENSION = "https://knuckles-team.github.io/arbora/extensions/tool-task/v1"

pytestmark = pytest.mark.skipif(
    not os.environ.get("TUI_NATIVE_TEST_BINARY"),
    reason="Requires an explicitly built native executable",
)


@pytest.fixture(scope="module")
def sandbox(tmp_path_factory: pytest.TempPathFactory) -> Iterator[Sandbox]:
    require_live()
    with running_sandbox(tmp_path_factory.mktemp("live-sandbox")) as running:
        yield running


def native(
    *arguments: str, sandbox: Sandbox, stdin: str = ""
) -> subprocess.CompletedProcess:
    return subprocess.run(
        [str(Path(os.environ["TUI_NATIVE_TEST_BINARY"]).resolve()), *arguments],
        env={k: v for k, v in sandbox.environment().items() if k != "AGENT_TASK_ID"},
        input=stdin,
        capture_output=True,
        text=True,
        timeout=240,
        check=False,
    )


def completed_task(sandbox: Sandbox) -> tuple[str, str]:
    """Run one real chat turn; return its task id and the nonce its reply echoes."""
    nonce = "proof-" + uuid.uuid4().hex
    result = native("--headless", "--prompt", f"hello {nonce}", sandbox=sandbox)
    assert result.returncode == 0, result.stdout + result.stderr
    assert "State: completed" in result.stdout and nonce in result.stdout
    task_id = next(
        line.removeprefix("Task ID: ")
        for line in result.stdout.splitlines()
        if line.startswith("Task ID: ")
    )
    return task_id, nonce


@pytest.mark.spec("TUIG-RUNTIME-R002.1.3.1")
def test_live_chat_turn_reaches_the_model_and_returns_its_reply(
    sandbox: Sandbox,
) -> None:
    _task, nonce = completed_task(sandbox)
    receipts = [json.loads(line) for line in sandbox.receipts.read_text().splitlines()]
    assert any(row["nonce"] == nonce and nonce in row["reply"] for row in receipts)


@pytest.mark.spec("TUIG-RUNTIME-R002.8.4")
def test_live_one_shot_refuses_while_the_release_serves_no_tool_task_contract(
    sandbox: Sandbox,
) -> None:
    card = sandbox.get("/.well-known/agent-card.json")
    advertised = [item["uri"] for item in card["capabilities"].get("extensions", [])]
    assert TOOL_TASK_EXTENSION not in advertised, (
        "bind R001.8 and R002.8.6 to a served proof"
    )
    request = json.dumps(
        {
            "operation": "records.write",
            "parameters": {"name": "alpha"},
            "idempotency_key": "one-shot-key-0123456789",
            "preapproval_ref": "scope:receipt-0001",
        }
    )
    result = native("--one-shot", sandbox=sandbox, stdin=request)
    assert result.returncode != 0
    assert "nothing was submitted" in result.stderr
    assert "Task ID" not in result.stdout


@pytest.mark.spec("TUIG-RUNTIME-R010.8")
def test_live_run_surface_reads_unavailable_plan_without_fabrication(
    sandbox: Sandbox,
) -> None:
    task_id, _nonce = completed_task(sandbox)
    served = sandbox.post("/api/v1/ops/runs.get", {"run_id": task_id})["result"][
        "value"
    ]
    assert served["status"] == "completed"
    assert served["current_plan"] == {"availability": "unavailable", "snapshot": None}
    terminal = NativeTerminal(sandbox.url, [], task_id)
    try:
        terminal.wait_until(lambda: f"Task ID: {task_id}" in terminal.shown())
        terminal.send(b"i")
        terminal.wait_until(
            lambda: "unavailable" in terminal.shown().lower().split("run id")[-1]
        )
        assert "Current plan revision:" not in terminal.shown()
        terminal.quit()
    finally:
        terminal.close()


@pytest.mark.spec("TUIG-RUNTIME-R010.8")
def test_live_release_serves_no_execution_graph_for_a_plain_run(
    sandbox: Sandbox,
) -> None:
    task_id, _nonce = completed_task(sandbox)
    served = sandbox.post(
        "/api/v1/ops/runs.execution_graph",
        {"run_id": task_id, "schema_version": "arbora-execution-graph-v1"},
    )["result"]["value"]
    assert served["availability"] == "unavailable" and served["snapshot"] is None, (
        "bind R010.14 to a served nested-graph proof"
    )


@pytest.mark.spec("TUIG-RUNTIME-R002.11")
def test_live_release_serves_no_host_log_launch(sandbox: Sandbox) -> None:
    request = Request(
        sandbox.url + "/host/logs/reference",
        data=b"{}",
        headers={"Content-Type": "application/json"},
    )
    with pytest.raises(HTTPError) as refused:
        build_opener(ProxyHandler({})).open(request, timeout=30)
    assert refused.value.code in {404, 405}, "bind R002.4 and R002.6 to a served proof"
