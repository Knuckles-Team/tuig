"""Open a local coding session at an explicit workspace root, not the invoking directory."""

import json
import os
import subprocess
import sys

import pytest

pytestmark = pytest.mark.skipif(
    not os.environ.get("TUI_NATIVE_TEST_BINARY") or os.name != "posix",
    reason="Requires an explicitly built native executable and POSIX process signals",
)

BACKEND = r"""
import json, os, sys
from pathlib import Path

report = Path(os.environ["ACP_TEST_REPORT"])
report.write_text(json.dumps({"argv": sys.argv[1:], "cwd": os.getcwd()}))

def send(value):
    print(json.dumps(value), flush=True)

for line in sys.stdin:
    request = json.loads(line)
    method = request["method"]
    if method == "initialize":
        send({"jsonrpc":"2.0","id":request["id"],"result":{"protocolVersion":1}})
    elif method == "session/new":
        with report.with_suffix(".session").open("w") as record:
            record.write(json.dumps(request["params"]))
        send({"jsonrpc":"2.0","id":request["id"],"result":{"sessionId":"root-session"}})
    elif method == "session/prompt":
        send({"jsonrpc":"2.0","id":request["id"],"result":{"stopReason":"end_turn"}})
"""


def run_native(tmp_path, *arguments):
    backend = tmp_path / "bin" / "arbora-acp"
    backend.parent.mkdir()
    backend.write_text(f"#!{sys.executable}\n{BACKEND}")
    backend.chmod(0o700)
    invoking = tmp_path / "invoking"
    invoking.mkdir()
    environment = {k: v for k, v in os.environ.items() if not k.startswith("AGENT_")}
    environment.update(
        PATH=f"{backend.parent}{os.pathsep}{os.environ.get('PATH', '')}",
        ACP_TEST_REPORT=str(tmp_path / "report.json"),
    )
    return subprocess.run(
        [
            os.environ["TUI_NATIVE_TEST_BINARY"],
            "--local-runtime",
            "--local-model",
            "test:model",
            *arguments,
        ],
        cwd=invoking,
        env=environment,
        capture_output=True,
        text=True,
        timeout=30,
        check=False,
    )


@pytest.mark.spec("TUIG-RUNTIME-R011.9")
def test_workspace_root_argument_opens_the_session_at_that_root(tmp_path):
    root = tmp_path / "project"
    root.mkdir()
    result = run_native(
        tmp_path, "--workspace-root", str(root), "--headless", "--prompt", "hello"
    )
    assert result.returncode == 0, result.stderr
    resolved = str(root.resolve())
    report = json.loads((tmp_path / "report.json").read_text())
    assert report["argv"][-2:] == ["--workspace-root", resolved]
    assert report["cwd"] == resolved
    session = json.loads((tmp_path / "report.session").read_text())
    assert session == {"cwd": resolved, "mcpServers": []}


@pytest.mark.spec("TUIG-RUNTIME-R011.9")
def test_default_workspace_root_is_the_invoking_directory(tmp_path):
    result = run_native(tmp_path, "--headless", "--prompt", "hello")
    assert result.returncode == 0, result.stderr
    session = json.loads((tmp_path / "report.session").read_text())
    assert session["cwd"] == str((tmp_path / "invoking").resolve())


@pytest.mark.spec("TUIG-RUNTIME-R011.9")
@pytest.mark.parametrize("missing", ["absent", "file"])
def test_workspace_root_must_be_an_existing_directory(tmp_path, missing):
    target = tmp_path / "target"
    if missing == "file":
        target.write_text("not a directory")
    result = run_native(
        tmp_path, "--workspace-root", str(target), "--headless", "--prompt", "hello"
    )
    assert result.returncode != 0
    assert "Workspace root must be an existing directory" in result.stderr
    assert not (tmp_path / "report.json").exists()
