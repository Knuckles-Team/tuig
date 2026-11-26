"""Start the pinned Arbora release in its explicit no-auth sandbox mode.

The sandbox is real: the released ``medulla-server`` engine on a fresh store and
the released ``arbora`` entry point serving over loopback. Only the upstream
chat model is a local stand-in, as in Arbora's own boot qualification. The
interpreter that carries the pinned release comes from ``TUI_LIVE_ARBORA_PYTHON``.
"""

import contextlib
import hashlib
import json
import os
import shutil
import socket
import subprocess
import time
from collections.abc import Callable, Iterator
from dataclasses import dataclass
from pathlib import Path
from urllib.error import URLError
from urllib.request import ProxyHandler, Request, build_opener

import pytest

LIVE_ENVIRONMENT = "TUI_LIVE_ARBORA_PYTHON"
SKIP_REASON = (
    "Requires the live disposable Arbora fixture started by the live-sandbox job"
)
MODEL_SOURCE = Path(__file__).with_name("fixtures") / "sandbox_model.py"
IMPORTER_PRINCIPAL = hashlib.sha256(b"anonymous").hexdigest()
_OPENER = build_opener(ProxyHandler({}))


@dataclass(frozen=True)
class Sandbox:
    """Where the running sandbox listens and what it recorded."""

    url: str
    root: Path
    receipts: Path

    def environment(self) -> dict[str, str]:
        """The client environment that selects the no-auth sandbox."""
        return {
            **os.environ,
            "AGENT_URL": self.url,
            "AGENT_AUTH_MODE": "none",
            "HTTP_PROXY": "http://127.0.0.1:1",
            "HTTPS_PROXY": "http://127.0.0.1:1",
            "NO_PROXY": "localhost,127.0.0.1,::1",
        }

    def get(self, path: str) -> dict:
        with _OPENER.open(Request(self.url + path), timeout=30) as response:
            return json.load(response)

    def post(self, path: str, body: dict) -> dict:
        request = Request(
            self.url + path,
            data=json.dumps(body).encode(),
            headers={"Content-Type": "application/json"},
        )
        with _OPENER.open(request, timeout=60) as response:
            return json.load(response)

    def operations(self) -> set[str]:
        """The operation ids this release serves, from its registry."""
        return {operation["id"] for operation in self.get("/api/v1/registry")["ops"]}


def live_python() -> str | None:
    return os.environ.get(LIVE_ENVIRONMENT)


def require_live() -> None:
    if not live_python():
        pytest.skip(SKIP_REASON)


def _free_port() -> int:
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


def _listening(port: int) -> bool:
    with (
        contextlib.suppress(OSError),
        socket.create_connection(("127.0.0.1", port), 0.5),
    ):
        return True
    return False


def _wait(ready: Callable[[], bool], timeout: float, what: str) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if ready():
            return
        time.sleep(0.25)
    raise AssertionError(f"{what} did not become ready within {timeout}s")


def _terminate(process: subprocess.Popen) -> None:
    process.terminate()
    try:
        process.wait(timeout=15)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()


def _configuration(root: Path, engine_port: int, model_port: int) -> Path:
    config = root / "config"
    config.mkdir()
    (config / "config.json").write_text(
        json.dumps(
            {
                "ARBORA_AUTH_MODE": "none",
                "DEPLOYMENT_PROFILE": "single-node-prod",
                "GRAPH_SERVICE_ENDPOINTS": f"tcp://127.0.0.1:{engine_port}",
                "ENABLE_WEB_UI": False,
                "MESSAGING_INTAKE_ENABLED": False,
                "MCP_HTTP_ALLOWED_PRIVATE_HOSTS": "127.0.0.1",
                "MODEL_HTTP_ALLOWED_PRIVATE_HOSTS": "127.0.0.1",
                "EMBEDDING_MODELS": "[]",
                "CHAT_MODELS": json.dumps(
                    [
                        {
                            "id": "sandbox-model",
                            "base_url": f"http://127.0.0.1:{model_port}/v1",
                            "api_key_env": "SANDBOX_MODEL_PLACEHOLDER",
                        }
                    ]
                ),
            }
        )
    )
    (config / "mcp_config.json").write_text(json.dumps({"mcpServers": {}}))
    return config


@contextlib.contextmanager
def running_sandbox(root: Path) -> Iterator[Sandbox]:
    python = live_python()
    assert python, LIVE_ENVIRONMENT
    bindir = Path(python).parent
    engine_binary = subprocess.run(
        [
            python,
            "-c",
            "from medulla.server_binary import resolve_server_binary as r; print(r())",
        ],
        capture_output=True,
        text=True,
        check=True,
    ).stdout.strip()
    ports = {name: _free_port() for name in ("engine", "model", "arbora")}
    base_env = {
        key: value
        for key, value in os.environ.items()
        if key in {"PATH", "LANG", "LD_LIBRARY_PATH", "PYTHONPATH", "SSL_CERT_FILE"}
    }
    base_env.update(
        {
            "HOME": str(root),
            "XDG_CONFIG_HOME": str(root / "xdg-config"),
            "XDG_DATA_HOME": str(root / "xdg-data"),
            "XDG_CACHE_HOME": str(root / "cache"),
            "XDG_STATE_HOME": str(root / "state"),
            "HF_HUB_OFFLINE": "1",
            "TRANSFORMERS_OFFLINE": "1",
        }
    )
    config = _configuration(root, ports["engine"], ports["model"])
    env = {
        **base_env,
        "ARBORA_CONFIG_DIR": str(config),
        "ARBORA_AUTH_MODE": "none",
        "SANDBOX_MODEL_PLACEHOLDER": "inert-local-stand-in-not-a-credential",
    }
    receipts = root / "model-receipts.jsonl"
    url = f"http://127.0.0.1:{ports['arbora']}"
    processes: list[subprocess.Popen] = []
    logs = []

    def start(
        name: str, command: list[str], child_env: dict[str, str]
    ) -> subprocess.Popen:
        log = (root / f"{name}.log").open("w")
        logs.append(log)
        process = subprocess.Popen(
            command, env=child_env, stdout=log, stderr=subprocess.STDOUT
        )
        processes.append(process)
        return process

    def alive(port: int, process: subprocess.Popen) -> bool:
        assert process.poll() is None, f"process exited; logs in {root}"
        return _listening(port)

    try:
        engine = start(
            "engine",
            [
                engine_binary,
                "--auth-mode",
                "none",
                "--tcp-only",
                "--tcp-addr",
                f"127.0.0.1:{ports['engine']}",
                "--persist-dir",
                str(root / "store"),
            ],
            {
                **base_env,
                "MEDULLA_CONNECTOR_PACK_IMPORTERS": (
                    f"arbora=principal:sha256:{IMPORTER_PRINCIPAL}"
                ),
            },
        )
        _wait(lambda: alive(ports["engine"], engine), 60, "the engine")
        model = start(
            "model",
            [python, str(MODEL_SOURCE), str(ports["model"])],
            {**base_env, "SANDBOX_MODEL_RECEIPTS": str(receipts)},
        )
        _wait(lambda: alive(ports["model"], model), 30, "the model stand-in")
        operations = shutil.which("arbora-production-ops", path=str(bindir))
        assert operations is not None
        for step in ("provision-semantic-content", "onboard-fleet"):
            subprocess.run(
                [operations, step, "--served-url", url + "/mcp"],
                env=env,
                capture_output=True,
                text=True,
                timeout=180,
                check=True,
            )
        entry = shutil.which("arbora", path=str(bindir))
        assert entry is not None
        server = start(
            "arbora",
            [
                entry,
                "--transport",
                "streamable-http",
                "--host",
                "127.0.0.1",
                "--port",
                str(ports["arbora"]),
                "--auth-type",
                "none",
            ],
            env,
        )
        _wait(lambda: alive(ports["arbora"], server), 180, "Arbora")
        _wait(lambda: _healthy(url), 60, "Arbora health")
        yield Sandbox(url=url, root=root, receipts=receipts)
    finally:
        for process in reversed(processes):
            _terminate(process)
        for log in logs:
            log.close()


def _healthy(url: str) -> bool:
    try:
        with _OPENER.open(Request(url + "/health"), timeout=5):
            return True
    except (URLError, OSError):
        return False
