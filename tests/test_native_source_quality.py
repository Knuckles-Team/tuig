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


# --- Native source-quality census (R015.1, .2, .3, .4, .6, .7, .19, .20, .34, .35)
#
# The scanner below is self-contained: it parses the tracked Rust sources
# directly so the limits hold even where the external cccc binary is absent.
# Production sources are measured with every `#[cfg(test)]` item blanked and
# with files that are test-only modules (`#[cfg(test)] mod name;`) excluded;
# test files are measured separately at their own caps.

NATIVE_SRC = ROOT / "native" / "tuig" / "src"
NATIVE_TESTS = ROOT / "native" / "tuig" / "tests"

# v1 threshold table. MAX_CYCLOMATIC mirrors the shared pipelines complexity
# gate (pipelines/pipelines_hooks/complexity/__init__.py); the remaining caps
# are tuig v1 values recorded here for the module, method and test-file rows.
MAX_FUNCTION_LINES = 60
MAX_CYCLOMATIC = 10
MAX_NESTING = 5
MAX_MODULE_LINES = 900  # non-blank production lines per src module
MAX_IMPL_METHODS = 16  # fn items per impl block
MAX_TEST_FILE_LINES = 1200
MAX_TEST_ITEMS = 60


def _strip_comments_and_strings(text: str) -> str:
    """Blank line/block comments and string literals; keep newlines."""
    out = []
    i = 0
    n = len(text)
    while i < n:
        c = text[i]
        if c == "/" and i + 1 < n and text[i + 1] == "/":
            while i < n and text[i] != "\n":
                i += 1
        elif c == "/" and i + 1 < n and text[i + 1] == "*":
            depth = 0
            while i < n:
                if text[i] == "/" and i + 1 < n and text[i + 1] == "*":
                    depth += 1
                    i += 2
                elif text[i] == "*" and i + 1 < n and text[i + 1] == "/":
                    depth -= 1
                    i += 2
                    if depth == 0:
                        break
                else:
                    i += 1
        elif c == '"':
            j = i + 1
            while j < n:
                if text[j] == "\\":
                    j += 2
                    continue
                if text[j] == '"':
                    break
                j += 1
            out.append('""')
            i = j + 1
        elif c == "'":
            # Lifetimes and char literals are opaque to every metric.
            out.append("'")
            i += 1
        else:
            out.append(c)
            i += 1
    return "".join(out)


def _blank_range(text: str, start: int, end: int) -> str:
    """Blank [start, end) keeping newlines so line numbers survive."""
    mid = re.sub(r"[^\n]", " ", text[start:end])
    return text[:start] + mid + text[end:]


def _strip_cfg_test(text: str) -> str:
    """Blank every `#[cfg(test)]` item: attribute plus annotated item."""
    out = text
    for m in re.finditer(r"^#\[cfg\(test\)\]\s*\n", out, re.M):
        item_start = m.end()
        nxt = re.match(r"mod\s+\w+\s*(\([^)]*\))?\s*\{", out[item_start:])
        if nxt:
            depth, i = 0, item_start + nxt.end() - 1
            while i < len(out):
                if out[i] == "{":
                    depth += 1
                elif out[i] == "}":
                    depth -= 1
                    if depth == 0:
                        break
                i += 1
            item_end = i + 1
        else:
            m2 = re.match(
                r"(?:pub(?:\(crate\))?\s+)?(?:async\s+)?fn\s+\w+", out[item_start:]
            )
            if not m2:
                continue
            brace = out.find("{", item_start + m2.end())
            depth, i = 0, brace
            while i < len(out):
                if out[i] == "{":
                    depth += 1
                elif out[i] == "}":
                    depth -= 1
                    if depth == 0:
                        break
                i += 1
            item_end = i + 1
        out = _blank_range(out, m.start(), item_end)
    return out


def _test_only_modules() -> set[str]:
    """Stems of files that are test-only modules in a sibling declaration."""
    names = set()
    for path in NATIVE_SRC.glob("*.rs"):
        for m in re.finditer(r"#\[cfg\(test\)\]\s*\n\s*mod\s+(\w+)\s*;", path.read_text()):
            names.add(m.group(1) + ".rs")
    return names


def _production_sources() -> dict[str, str]:
    """Each production src module with its `#[cfg(test)]` items blanked."""
    test_only = _test_only_modules()
    return {
        path.name: _strip_cfg_test(path.read_text())
        for path in sorted(NATIVE_SRC.glob("*.rs"))
        if path.name not in test_only
    }


def _rust_functions(prod: str) -> list[tuple[str, int, str]]:
    """(name, line, body) for every fn item in a production source."""
    out = []
    for m in re.finditer(
        r"^\s*(?:pub(?:\(crate\))?\s+)?(?:async\s+)?fn\s+(\w+)", prod, re.M
    ):
        brace = prod.find("{", m.end())
        if brace == -1:
            continue
        depth, i = 0, brace
        while i < len(prod):
            if prod[i] == "{":
                depth += 1
            elif prod[i] == "}":
                depth -= 1
                if depth == 0:
                    break
            i += 1
        out.append((m.group(1), prod.count("\n", 0, m.start()) + 1, prod[brace : i + 1]))
    return out


def _cyclomatic(body: str) -> int:
    """Branches: 1 + if/while/for/loop + &&/|| + every `?` + match arms."""
    b = _strip_comments_and_strings(body)
    n = 1
    n += len(re.findall(r"\bif\b", b))
    n += len(re.findall(r"\bwhile\b", b))
    n += len(re.findall(r"\bfor\b", b))
    n += len(re.findall(r"\bloop\b", b))
    n += len(re.findall(r"&&", b)) + len(re.findall(r"\|\|", b))
    n += len(re.findall(r"\?", b))
    for mm in re.finditer(r"\bmatch\b[^{]*\{", b):
        brace = mm.end() - 1
        depth, i = 0, brace
        while i < len(b):
            if b[i] == "{":
                depth += 1
            elif b[i] == "}":
                depth -= 1
                if depth == 0:
                    break
            i += 1
        block = b[brace : i + 1]
        arms = len(
            re.findall(
                r"(?m)^\s*([^\s}).][^\n]*?)(?:\s*(?:if\s+[^{=]+)?)?\s*=>", block
            )
        )
        n += arms if arms else max(0, len(re.findall(r"=>", block)) - 1)
    return n


def _nesting(body: str) -> int:
    """Deepest brace nesting inside the function body."""
    depth = best = 0
    for c in _strip_comments_and_strings(body):
        if c == "{":
            depth += 1
            best = max(best, depth)
        elif c == "}":
            depth -= 1
    return best


def _fn_metrics(source: str) -> list[tuple[str, int, int, int, int]]:
    """(name, line, lines, cyc, nest) per fn item in a production source."""
    return [
        (name, line, body.count("\n") + 1, _cyclomatic(body), _nesting(body))
        for name, line, body in _rust_functions(source)
    ]


def _impl_spans(source: str) -> list[tuple[str, str]]:
    """(impl header, block text) per impl block in a production source."""
    stripped = _strip_comments_and_strings(source)
    spans = []
    for m in re.finditer(r"impl\s+([^{]*)\{", stripped):
        brace = m.end() - 1
        depth, i = 0, brace
        while i < len(stripped):
            if stripped[i] == "{":
                depth += 1
            elif stripped[i] == "}":
                depth -= 1
                if depth == 0:
                    break
            i += 1
        spans.append((m.group(1).strip(), stripped[brace : i + 1]))
    return spans


def _impl_blocks(source: str) -> list[tuple[str, int]]:
    """(impl header, fn count) per impl block in a production source."""
    return [
        (header, len(re.findall(r"\bfn\s+\w+", block)))
        for header, block in _impl_spans(source)
    ]


def _pinned_functions(source: str, names: list[str]) -> list[tuple[str, int]]:
    """(name, worst cyc) for the pinned names; raises when one is missing."""
    by_name: dict[str, list[int]] = {}
    for name, _line, _lines, cyc, _nest in _fn_metrics(source):
        by_name.setdefault(name, []).append(cyc)
    missing = [name for name in names if name not in by_name]
    assert not missing, f"pinned functions missing: {missing}"
    return [(name, max(by_name[name])) for name in names]


@pytest.mark.spec("TUIG-RUNTIME-R015.1")
def test_native_function_size():
    sources = _production_sources()
    assert sources
    over = [
        f"{name}:{fn}={lines} lines"
        for name, source in sources.items()
        for fn, line, lines, _cyc, _nest in _fn_metrics(source)
        if lines > MAX_FUNCTION_LINES
    ]
    assert not over, f"functions over {MAX_FUNCTION_LINES} lines: {over}"
    # Counterexample: a 62-line body must be flagged by the same criterion.
    counter = "fn sized() {\n" + "    let _x = 0;\n" * 60 + "}\n"
    over = [
        f"{fn}={lines} lines"
        for fn, _line, lines, _cyc, _nest in _fn_metrics(counter)
        if lines > MAX_FUNCTION_LINES
    ]
    assert over, "the counterexample must exceed the function-size cap"


@pytest.mark.spec("TUIG-RUNTIME-R015.2")
def test_native_cli_complexity():
    sources = _production_sources()
    # The processor is cli.rs in full plus the argument helpers in main.rs.
    cli_over = [
        f"cli.rs:{fn}: cyc {cyc}"
        for fn, _line, _lines, cyc, _nest in _fn_metrics(sources["cli.rs"])
        if cyc > MAX_CYCLOMATIC
    ]
    assert not cli_over, f"CLI functions over cyclomatic {MAX_CYCLOMATIC}: {cli_over}"
    helpers = [
        "help_requested",
        "launch_app",
        "launch_targets",
        "submission_from",
        "caller_credential",
        "require_credential",
        "remote_client",
        "selected_task",
        "selected_prompt",
        "selected_observation",
    ]
    main_over = [
        f"main.rs:{fn}: cyc {cyc}"
        for fn, cyc in _pinned_functions(sources["main.rs"], helpers)
        if cyc > MAX_CYCLOMATIC
    ]
    assert not main_over, f"argument helpers over cyclomatic {MAX_CYCLOMATIC}: {main_over}"
    # Counterexample: eleven branches push cyclomatic to 12 and must be flagged.
    counter = "fn branched(value: u32) -> u32 {\n"
    counter += "".join(f"    if value == {i} {{ out += 1; }}\n" for i in range(11))
    counter += "    0\n}\n"
    cycs = [cyc for _fn, _line, _lines, cyc, _nest in _fn_metrics(counter)]
    assert any(cyc > MAX_CYCLOMATIC for cyc in cycs), (
        "the counterexample must exceed the complexity cap"
    )


@pytest.mark.spec("TUIG-RUNTIME-R015.3")
def test_native_client_complexity():
    sources = _production_sources()
    spans = [
        block
        for header, block in _impl_spans(sources["lib.rs"])
        if header == "ArboraClient"
    ]
    assert spans, "ArboraClient impl blocks must exist in lib.rs"
    client_over = [
        f"lib.rs:{fn}: cyc {cyc}"
        for block in spans
        for fn, _line, _lines, cyc, _nest in _fn_metrics(block)
        if cyc > MAX_CYCLOMATIC
    ]
    inspector_over = [
        f"run_inspector_client.rs:{fn}: cyc {cyc}"
        for fn, _line, _lines, cyc, _nest in _fn_metrics(
            sources["run_inspector_client.rs"]
        )
        if cyc > MAX_CYCLOMATIC
    ]
    assert not client_over, (
        f"client methods over cyclomatic {MAX_CYCLOMATIC}: {client_over}"
    )
    assert not inspector_over, (
        f"inspector client fns over cyclomatic {MAX_CYCLOMATIC}: {inspector_over}"
    )
    # Counterexample: a 12-branch client method must be flagged.
    counter = "impl Sample {\n    fn send(value: u32) -> u32 {\n"
    counter += "".join(f"        if value == {i} {{ out += 1; }}\n" for i in range(12))
    counter += "        0\n    }\n}\n"
    flagged = any(
        cyc > MAX_CYCLOMATIC
        for _fn, _line, _lines, cyc, _nest in _fn_metrics(counter)
    )
    assert flagged, "the counterexample must exceed the complexity cap"


@pytest.mark.spec("TUIG-RUNTIME-R015.4")
def test_native_event_handler_complexity():
    sources = _production_sources()
    handlers = [
        ("session.rs", ["event_loop", "key_event"]),
        ("main.rs", ["route_mouse"]),
    ]
    over = []
    for file, names in handlers:
        for fn, cyc in _pinned_functions(sources[file], names):
            if cyc > MAX_CYCLOMATIC:
                over.append(f"{file}:{fn}: cyc {cyc}")
    assert not over, f"event handlers over cyclomatic {MAX_CYCLOMATIC}: {over}"
    # Counterexample: a 12-branch handler must be flagged.
    counter = "async fn handle(event: u32) -> u32 {\n"
    counter += "".join(f"    if event == {i} {{ out += 1; }}\n" for i in range(12))
    counter += "    0\n}\n"
    cycs = [cyc for _fn, _line, _lines, cyc, _nest in _fn_metrics(counter)]
    assert any(cyc > MAX_CYCLOMATIC for cyc in cycs), (
        "the counterexample must exceed the complexity cap"
    )


@pytest.mark.spec("TUIG-RUNTIME-R015.6")
def test_native_composer_complexity():
    sources = _production_sources()
    composer = sources["composer.rs"]
    over = [
        f"composer.rs:{fn}: cyc {cyc}"
        for fn, _line, _lines, cyc, _nest in _fn_metrics(composer)
        if cyc > MAX_CYCLOMATIC
    ]
    assert not over, f"composer functions over cyclomatic {MAX_CYCLOMATIC}: {over}"
    pinned = _pinned_functions(composer, ["key", "paste"])
    assert {"key", "paste"} <= {name for name, _cyc in pinned}
    # Counterexample: a 12-branch composer key handler must be flagged.
    counter = "fn key(code: u32) -> u32 {\n"
    counter += "".join(f"    if code == {i} {{ out += 1; }}\n" for i in range(12))
    counter += "    0\n}\n"
    cycs = [cyc for _fn, _line, _lines, cyc, _nest in _fn_metrics(counter)]
    assert any(cyc > MAX_CYCLOMATIC for cyc in cycs), (
        "the counterexample must exceed the complexity cap"
    )


@pytest.mark.spec("TUIG-RUNTIME-R015.7")
def test_native_nesting_depth():
    sources = _production_sources()
    over = [
        f"{name}:{fn}={nest} deep"
        for name, source in sources.items()
        for fn, line, _lines, _cyc, nest in _fn_metrics(source)
        if nest > MAX_NESTING
    ]
    assert not over, f"functions nested deeper than {MAX_NESTING}: {over}"
    # Counterexample: six nested blocks must be flagged.
    counter = "fn deep(value: bool) -> bool {\n"
    counter += "".join("    if value {\n" for _ in range(6))
    counter += "        true\n" + "".join("    }\n" for _ in range(6)) + "}\n"
    nests = [nest for _fn, _line, _lines, _cyc, nest in _fn_metrics(counter)]
    assert any(nest > MAX_NESTING for nest in nests), (
        "the counterexample must exceed the nesting cap"
    )


@pytest.mark.spec("TUIG-RUNTIME-R015.19")
def test_native_module_size():
    over = [
        f"{name}={sum(1 for ln in source.splitlines() if ln.strip())} non-blank lines"
        for name, source in _production_sources().items()
        if sum(1 for ln in source.splitlines() if ln.strip()) > MAX_MODULE_LINES
    ]
    assert not over, f"modules over {MAX_MODULE_LINES} non-blank production lines: {over}"
    # Counterexample: 901 non-blank lines must be flagged.
    counter = "".join(f"const C{i}: u32 = {i};\n" for i in range(901))
    nonblank = sum(1 for ln in counter.splitlines() if ln.strip())
    assert nonblank > MAX_MODULE_LINES, "the counterexample must exceed the module cap"


@pytest.mark.spec("TUIG-RUNTIME-R015.20")
def test_method_count():
    over = []
    for name, source in _production_sources().items():
        over += [
            f"{name} impl {header}: {count} fns"
            for header, count in _impl_blocks(source)
            if count > MAX_IMPL_METHODS
        ]
    assert not over, f"impl blocks over {MAX_IMPL_METHODS} methods: {over}"
    # Counterexample: a 17-method impl must be flagged.
    counter = "impl Sample {\n" + "".join(f"    fn m{i}() {{}}\n" for i in range(17)) + "}\n"
    counts = [count for _header, count in _impl_blocks(counter)]
    assert any(count > MAX_IMPL_METHODS for count in counts), (
        "the counterexample must exceed the method-count cap"
    )


def _rust_test_items(text: str) -> int:
    return len(re.findall(r"#\[(?:tokio::)?test\]", text))


def _python_test_items(text: str) -> int:
    return len(re.findall(r"^def test_", text, re.M))


@pytest.mark.spec("TUIG-RUNTIME-R015.34")
def test_test_file_size():
    rust = sorted(NATIVE_TESTS.glob("*.rs"))
    py = sorted(ROOT.glob("tests/*.py"))
    assert rust and py
    over = [
        f"{path.name}={len(path.read_text().splitlines())} lines"
        for path in rust + py
        if len(path.read_text().splitlines()) > MAX_TEST_FILE_LINES
    ]
    assert not over, f"test files over {MAX_TEST_FILE_LINES} lines: {over}"
    # Counterexample: a 1201-line test file must be flagged.
    counter = "".join(f"# test line {i}\n" for i in range(1201))
    assert len(counter.splitlines()) > MAX_TEST_FILE_LINES


@pytest.mark.spec("TUIG-RUNTIME-R015.35")
def test_test_count_per_file():
    rust = sorted(NATIVE_TESTS.glob("*.rs"))
    py = sorted(ROOT.glob("tests/*.py"))
    assert rust and py
    over = [
        f"{path.name}: {_rust_test_items(path.read_text())} tests"
        for path in rust
        if _rust_test_items(path.read_text()) > MAX_TEST_ITEMS
    ] + [
        f"{path.name}: {_python_test_items(path.read_text())} tests"
        for path in py
        if _python_test_items(path.read_text()) > MAX_TEST_ITEMS
    ]
    assert not over, f"test files over {MAX_TEST_ITEMS} test items: {over}"
    # Counterexample: 61 test items must be flagged.
    rust_counter = "".join(f"#[test]\nfn t{i}() {{}}\n" for i in range(61))
    py_counter = "".join(f"def test_t{i}():\n    assert True\n\n" for i in range(61))
    assert _rust_test_items(rust_counter) > MAX_TEST_ITEMS
    assert _python_test_items(py_counter) > MAX_TEST_ITEMS
