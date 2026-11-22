"""Source-quality documentation checks for the native terminal.

Each test proves one TUIG-RUNTIME-R015.* obligation on the tracked files
and rejects a counterexample that violates the same criterion.
"""

import re
from pathlib import Path

import pytest

ROOT = Path(__file__).parents[1]
README = ROOT / "README.md"
CONTRIBUTING = ROOT / "CONTRIBUTING.md"


def _readme() -> str:
    return README.read_text()


def _cli_help() -> str:
    main = (ROOT / "native/tuig/src/main.rs").read_text()
    match = re.search(r'const CLI_HELP: &str = "([^"]*)"', main)
    assert match is not None, "CLI_HELP constant must exist in main.rs"
    return match.group(1)


@pytest.mark.spec("TUIG-RUNTIME-R015.11")
def test_readme_purpose_and_installation():
    text = _readme()
    assert _purpose_and_installation(text)
    assert not _purpose_and_installation(
        text.replace("terminal frontend for Arbora", "unexplained artifact")
    )
    assert not _purpose_and_installation(
        text.replace("pip install tuig", "pip install other")
    )


def _purpose_and_installation(text: str) -> bool:
    has_purpose = "terminal frontend for Arbora" in text
    has_installation = "## Installation" in text and "pip install tuig" in text
    return has_purpose and has_installation


@pytest.mark.spec("TUIG-RUNTIME-R015.12")
def test_readme_covers_each_entry_point():
    help_text = _cli_help()
    text = _readme()
    flags = [flag for flag in ("--prompt", "--observe-task", "--headless") if flag in help_text]
    assert flags
    assert _entry_points_covered(text, help_text)
    for flag in flags:
        assert flag in text
    assert re.search(r"^tuig$", text, re.M)
    assert not _entry_points_covered(text.replace("--headless", "--removed"), help_text)


def _entry_points_covered(readme: str, help_text: str) -> bool:
    for flag in ("--prompt", "--observe-task", "--headless"):
        if flag in help_text and flag not in readme:
            return False
    if "tuig" in help_text and not re.search(r"^tuig$", readme, re.M):
        return False
    return True


@pytest.mark.spec("TUIG-RUNTIME-R015.13")
def test_readme_navigation_reaches_each_section():
    text = _readme()
    sections = _sections(text)
    assert sections
    assert _every_section_navigated(text)
    stripped = text.replace("[Installation](#installation)", "")
    assert "Installation" in _sections(stripped)
    assert not _every_section_navigated(stripped)


def _sections(text: str) -> list[str]:
    return re.findall(r"^## (.+)$", text, re.M)


def _every_section_navigated(text: str) -> bool:
    for section in _sections(text):
        anchor = section.lower().replace(" ", "-")
        if f"[{section}](#{anchor})" not in text:
            return False
    return True


@pytest.mark.spec("TUIG-RUNTIME-R015.14")
def test_locked_install_instructions():
    docs = "\n".join(
        (ROOT / path).read_text()
        for path in ("README.md", "CONTRIBUTING.md", "docs/development.md")
    )
    assert _locked_installation(docs, (ROOT / "uv.lock").is_file())
    assert "uv sync --frozen" in docs
    assert re.search(r"cargo \S+ --locked", docs)
    assert (ROOT / "uv.lock").is_file()
    assert not _locked_installation("uv sync && cargo test", False)


def _locked_installation(docs: str, lockfile_present: bool) -> bool:
    return "uv sync --frozen" in docs and bool(re.search(r"cargo \S+ --locked", docs)) and lockfile_present


@pytest.mark.spec("TUIG-RUNTIME-R015.15")
def test_readme_explains_architecture():
    text = _readme()
    assert _architecture_named(text)
    for path in ("main.rs", "cli.rs", "lib.rs"):
        assert path in text
    assert re.search(r"\(docs/architecture\.md\)", text)
    assert not _architecture_named(text.replace("main.rs", "unknown.rs"))


def _architecture_named(text: str) -> bool:
    links_architecture = bool(re.search(r"\(docs/architecture\.md\)", text))
    names_paths = all(path in text for path in ("main.rs", "cli.rs", "lib.rs"))
    names_separate_process = "separate process" in text or "runs separately" in text
    return links_architecture and names_paths and names_separate_process


@pytest.mark.spec("TUIG-RUNTIME-R015.16")
def test_readme_links_existing_documentation():
    text = _readme()
    relative = _relative_links(text)
    assert relative
    assert _links_resolve(text)
    broken = text + "\nSee the [ghost page](docs/ghost.md).\n"
    assert "docs/ghost.md" in _relative_links(broken)
    assert not _links_resolve(broken)


def _relative_links(text: str) -> list[str]:
    links = re.findall(r"\]\(([^)]+)\)", text)
    return [
        link
        for link in links
        if not link.startswith(("http://", "https://", "#", "mailto:"))
    ]


def _links_resolve(text: str) -> bool:
    for link in _relative_links(text):
        target = (ROOT / "README.md").parent / link.split("#")[0]
        if not target.exists():
            return False
    return True


@pytest.mark.spec("TUIG-RUNTIME-R015.17")
def test_contributor_guide_tools_commands_and_source_map():
    text = CONTRIBUTING.read_text()
    assert "cargo fmt" in text
    assert "cargo test" in text
    assert "cargo clippy" in text
    paths = _source_map_paths(text)
    assert paths
    assert _source_map_paths_resolve(text)
    stale = text.replace("native/tuig/src/main.rs", "native/tuig/src/ghost.rs")
    assert "native/tuig/src/ghost.rs" in _source_map_paths(stale)
    assert not _source_map_paths_resolve(stale)


def _source_map_paths(text: str) -> list[str]:
    return re.findall(r"`((?:native|tests|specs)/[\w./-]*)`", text)


def _source_map_paths_resolve(text: str) -> bool:
    for path in _source_map_paths(text):
        if not (ROOT / path).exists():
            return False
    return True


@pytest.mark.spec("TUIG-RUNTIME-R015.18")
def test_declared_mit_license_is_tracked():
    license_text = (ROOT / "LICENSE").read_text()
    pyproject = (ROOT / "pyproject.toml").read_text()
    cargo = (ROOT / "native/tuig/Cargo.toml").read_text()
    assert _license_declarations_agree(license_text, pyproject, cargo, _readme())
    assert "Permission is hereby granted, free of charge" in license_text
    assert 'license = { file = "LICENSE" }' in pyproject
    assert 'license = "MIT"' in cargo
    assert "[MIT](LICENSE)" in _readme()
    mismatch = pyproject.replace('file = "LICENSE"', 'file = "NOTLICENSE"')
    assert not _license_declarations_agree(license_text, mismatch, cargo, _readme())


def _license_declarations_agree(
    license_text: str, pyproject: str, cargo: str, readme: str
) -> bool:
    is_mit = "Permission is hereby granted, free of charge" in license_text
    pyproject_names_license = 'license = { file = "LICENSE" }' in pyproject
    cargo_names_mit = 'license = "MIT"' in cargo
    readme_links_license = "[MIT](LICENSE)" in readme
    return is_mit and pyproject_names_license and cargo_names_mit and readme_links_license


import copy
import json
import os
import subprocess
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import yaml

SUPPORT_MODULE = ROOT / "native" / "tuig" / "tests" / "support" / "mod.rs"
ENV_EXAMPLE = ROOT / ".env.example"
WORKFLOW = ROOT / ".github" / "workflows" / "ci.yml"


def _shared_fixture_entry(source: str) -> bool:
    """The shared loopback fixture must stay one entry point."""
    return "pub async fn serve_raw" in source and "TcpListener::bind" in source


@pytest.mark.spec("TUIG-RUNTIME-R015.37")
def test_shared_fixture_entry():
    source = SUPPORT_MODULE.read_text()
    assert _shared_fixture_entry(source), "support/mod.rs must define serve_raw"
    importers = sorted(
        path.name
        for path in (ROOT / "native" / "tuig" / "tests").glob("*.rs")
        if "mod support" in path.read_text() or "support::" in path.read_text()
    )
    assert len(importers) >= 4, f"shared entry must be reused: {importers}"
    assert "env_clear()" in (ROOT / "native" / "tuig" / "tests" / "cli_help.rs").read_text()
    assert not _shared_fixture_entry("pub fn unrelated(value: u32) -> u32 { value }")


def _env_example_is_credential_free(text: str) -> bool:
    for line in text.splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        if not re.fullmatch(r"[A-Z][A-Z0-9_]*=[^\s]*", line):
            return False
        key, value = line.split("=", 1)
        if key == "AGENT_URL":
            if not re.fullmatch(r"https?://(localhost|127\.0\.0\.1):8000", value):
                return False
        elif key == "AGENT_THEME":
            if not re.fullmatch(r"[a-z][a-z-]*", value):
                return False
        else:
            return False
    return True


@pytest.mark.spec("TUIG-RUNTIME-R015.43")
def test_native_env_example_is_credential_free():
    text = ENV_EXAMPLE.read_text()
    assert _env_example_is_credential_free(text)
    assert re.search(r"^AGENT_URL=http://localhost:8000$", text, re.M)
    assert re.search(r"^AGENT_THEME=tokyo-night$", text, re.M)
    assert "AGENT_URL" in _readme()
    assert not _env_example_is_credential_free("AGENT_BEARER_TOKEN=secret-value\n")
    assert not _env_example_is_credential_free("AGENT_URL=https://remote.example.com:8443\n")
    assert not _env_example_is_credential_free("AGENT_URL=http://localhost:8000\nAGENT_THEME=run $(rm -rf /)\n")


def _native_ci_gates(workflow: dict) -> tuple[bool, bool]:
    job = workflow.get("jobs", {}).get("native-checks", {})
    runs = [str(step.get("run", "")) for step in job.get("steps", []) if isinstance(step, dict)]
    fmt = any("fmt" in run and "--check" in run for run in runs)
    clippy = any("clippy" in run and "-D warnings" in run for run in runs)
    return fmt, clippy


@pytest.mark.spec("TUIG-RUNTIME-R015.48")
def test_native_ci_runs_fmt_and_clippy_gates():
    workflow = yaml.safe_load(WORKFLOW.read_text())
    fmt, clippy = _native_ci_gates(workflow)
    assert fmt, "native-checks must run cargo fmt --check"
    assert clippy, "native-checks must run cargo clippy -D warnings"
    no_fmt = copy.deepcopy(workflow)
    for step in no_fmt["jobs"]["native-checks"]["steps"]:
        if isinstance(step, dict) and "fmt" in str(step.get("run", "")):
            step["run"] = str(step["run"]).replace("--check", "")
    assert not _native_ci_gates(no_fmt)[0]
    no_flag = copy.deepcopy(workflow)
    for step in no_flag["jobs"]["native-checks"]["steps"]:
        if isinstance(step, dict) and "clippy" in str(step.get("run", "")):
            step["run"] = str(step["run"]).replace("-D warnings", "-D nothing")
    assert not _native_ci_gates(no_flag)[1]


def _serve_task_server(state: str, status: int = 200) -> ThreadingHTTPServer:
    class Handler(BaseHTTPRequestHandler):
        def do_POST(self):
            length = int(self.headers["Content-Length"])
            self.rfile.read(length)
            if status != 200:
                self.send_response(status)
                self.end_headers()
                return
            task = {
                "kind": "task",
                "id": "task-1",
                "contextId": "context-1",
                "metadata": {"arbora": {"runId": "run-1"}},
                "status": {"state": state},
            }
            body = json.dumps(
                {"jsonrpc": "2.0", "id": "task-snapshot", "result": task}
            ).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *_args):
            pass

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


def _stop(server: ThreadingHTTPServer) -> None:
    server.shutdown()
    server.server_close()


def _observer_environment(outside: Path, server: ThreadingHTTPServer) -> dict[str, str]:
    # Minimal loopback environment: no ambient credentials, no ambient task.
    return {
        "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
        "HOME": str(outside),
        "AGENT_URL": f"http://127.0.0.1:{server.server_port}",
        "AGENT_AUTH_MODE": "none",
    }


def _observer_outcome(
    binary: str, outside: Path, server: ThreadingHTTPServer
) -> tuple[int | None, str, str]:
    """Launch the installed observer and report how it treats the served read.

    Returns the exit code (a negative signal code when the test stopped a
    still-observing process) plus the captured stdout and stderr.
    """
    process = subprocess.Popen(
        [binary, "--headless", "--observe-task", "task-1"],
        cwd=outside,
        env=_observer_environment(outside, server),
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    if process.stdout is None or process.stderr is None:
        raise AssertionError("piped stdout and stderr are required")
    stdout_pipe, stderr_pipe = process.stdout, process.stderr
    seen = ""
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        if process.poll() is not None:
            break
        line = stdout_pipe.readline()
        if not line:
            break
        seen += line
        if "State:" in line:
            break
    if process.poll() is None:
        # A finished observer exits on its own right after its last
        # snapshot; give it a moment before stopping a still-observing one.
        grace = time.monotonic() + 3
        while time.monotonic() < grace and process.poll() is None:
            time.sleep(0.05)
        if process.poll() is None:
            process.terminate()
    process.wait(timeout=10)
    stdout_text = seen + stdout_pipe.read()
    stderr_text = stderr_pipe.read()
    return process.returncode, stdout_text, stderr_text


@pytest.mark.spec("TUIG-RUNTIME-R015.70")
@pytest.mark.skipif(
    not os.environ.get("TUI_NATIVE_TEST_BINARY"),
    reason="Requires the installed native binary artifact",
)
def test_source_quality_entry_wiring(tmp_path: Path) -> None:
    binary = os.environ["TUI_NATIVE_TEST_BINARY"]
    outside = tmp_path / "outside"
    outside.mkdir()
    server = _serve_task_server("completed")
    try:
        help_result = subprocess.run(
            [binary, "--help"],
            cwd=outside,
            env=_observer_environment(outside, server),
            capture_output=True,
            text=True,
            timeout=30,
        )
        assert help_result.returncode == 0
        assert "Tuig: native Arbora terminal" in help_result.stdout
        code, out, err = _observer_outcome(binary, outside, server)
    finally:
        _stop(server)
    assert code == 0
    assert "Task ID: task-1" in out
    assert "State: completed" in out
    assert "Arbora task read refused or unavailable" not in out + err


@pytest.mark.spec("TUIG-RUNTIME-R015.71")
@pytest.mark.skipif(
    not os.environ.get("TUI_NATIVE_TEST_BINARY"),
    reason="Requires the installed native binary artifact",
)
def test_installed_observer_distinguishes_denied_and_empty_reads(tmp_path: Path) -> None:
    binary = os.environ["TUI_NATIVE_TEST_BINARY"]
    outside = tmp_path / "outside"
    outside.mkdir()
    # The authenticated read returns a task with no committed result yet:
    # an empty read, not a refusal.
    empty_server = _serve_task_server("submitted")
    try:
        empty_code, empty_out, empty_err = _observer_outcome(
            binary, outside, empty_server
        )
    finally:
        _stop(empty_server)
    denied_server = _serve_task_server("completed", status=401)
    try:
        denied_code, denied_out, denied_err = _observer_outcome(
            binary, outside, denied_server
        )
    finally:
        _stop(denied_server)
    # The empty read keeps the observer under observation: the snapshot is
    # shown, no denial surfaces, and the observer is still polling when the
    # test stops it.
    assert empty_code is not None and empty_code < 0, (
        "the empty-read observer must still be polling, "
        f"exited {empty_code}: {empty_out} {empty_err}"
    )
    assert "Task ID: task-1" in empty_out
    assert "State: submitted" in empty_out
    assert "Arbora task read refused or unavailable" not in empty_out + empty_err
    # The denied read fails fast with the refusal and no task data at all.
    assert denied_code == 1
    assert "Arbora task read refused or unavailable" in denied_err
    assert "Task ID:" not in denied_out
    # The installed observer shows the two reads differently.
    assert (empty_out, empty_err) != (denied_out, denied_err)
