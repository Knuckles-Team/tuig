"""Quality acceptance evidence jobs: scans, coverage, strict docs and suite outcomes.

Each test proves that a CI job produces its evidence from the tracked files and
rejects a counterexample that would let the evidence go missing.
"""

import json
import subprocess
import sys
import tomllib

import pytest
import yaml

from tests.test_native_source_quality import ROOT, WORKFLOW

SCRIPTS = ROOT / "scripts"


def _jobs() -> dict:
    return yaml.safe_load(WORKFLOW.read_text())["jobs"]


def _run_text(job: dict) -> str:
    return "\n".join(str(step.get("run", "")) for step in job["steps"])


def _required(jobs: dict) -> set[str]:
    return set(jobs["ci-required"]["needs"])


@pytest.mark.spec("TUIG-RUNTIME-R015.57")
def test_dependency_scan_is_pinned_fail_closed_and_required():
    jobs = _jobs()
    job = jobs["cargo-advisories"]
    assert "cargo-advisories" in _required(jobs)
    assert any("cargo-deny@" in str(step.get("with", {}).get("tool", "")) for step in job["steps"])
    assert "check advisories" in _run_text(job)
    assert "--locked" in _run_text(job)
    deny = tomllib.loads((ROOT / ".config" / "deny.toml").read_text())
    assert deny["advisories"]["ignore"] == []
    assert deny["advisories"]["yanked"] == "deny"
    assert deny["graph"]["all-features"] is True


def _coverage(lines: float, branches: float, branch_count: int = 10) -> str:
    totals = {
        "lines": {"count": 100, "covered": lines, "percent": lines},
        "branches": {"count": branch_count, "covered": 0, "percent": branches},
    }
    return json.dumps({"data": [{"totals": totals}]})


def _verify_coverage(tmp_path, body: str, *flags: str) -> subprocess.CompletedProcess:
    report = tmp_path / "coverage.json"
    report.write_text(body)
    return subprocess.run(
        [sys.executable, str(SCRIPTS / "verify_native_coverage.py"), str(report), *flags],
        capture_output=True,
        text=True,
        check=False,
    )


@pytest.mark.spec("TUIG-RUNTIME-R015.58")
def test_native_line_and_branch_coverage_are_measured_separately(tmp_path):
    jobs = _jobs()
    job = jobs["native-coverage"]
    assert "native-coverage" in _required(jobs)
    run = _run_text(job)
    assert "llvm-cov --branch" in run
    assert "verify_native_coverage.py" in run
    assert any(step.get("uses", "").startswith("actions/upload-artifact") for step in job["steps"])
    assert _verify_coverage(tmp_path, _coverage(90, 85)).returncode == 0
    assert _verify_coverage(tmp_path, _coverage(79.9, 95)).returncode == 1
    assert _verify_coverage(tmp_path, _coverage(95, 79.9)).returncode == 1
    assert _verify_coverage(tmp_path, _coverage(95, 0, branch_count=0)).returncode == 1
    advisory = _verify_coverage(tmp_path, _coverage(95, 68), "--branches-advisory")
    assert advisory.returncode == 0 and "::warning::" in advisory.stdout
    assert _verify_coverage(tmp_path, _coverage(70, 90), "--branches-advisory").returncode == 1


@pytest.mark.spec("TUIG-RUNTIME-R015.59")
def test_documentation_builds_strictly_with_the_shared_theme():
    jobs = _jobs()
    job = jobs["docs-strict"]
    assert "docs-strict" in _required(jobs)
    run = _run_text(job)
    assert "mkdocs build --strict" in run
    assert "sync_mkdocs_theme.py check" in run
    assert any(
        step.get("with", {}).get("repository") == "Knuckles-Team/pipelines"
        and step["with"].get("path") == ".pipeline-theme"
        for step in job["steps"]
    )
    mkdocs = (ROOT / "mkdocs.yml").read_text()
    assert mkdocs.startswith("INHERIT: .pipeline-theme/templates/mkdocs-theme/base.mkdocs.yml")
    status = (ROOT / "docs" / "status.md").read_text()
    assert "](../specs/" not in status, "docs must not link outside docs_dir"


def _suite(tmp_path, cases: str) -> subprocess.CompletedProcess:
    report = tmp_path / "suite.xml"
    report.write_text(f"<testsuites><testsuite>{cases}</testsuite></testsuites>")
    return subprocess.run(
        [sys.executable, str(SCRIPTS / "verify_suite_outcomes.py"), str(report)],
        capture_output=True,
        text=True,
        check=False,
    )


@pytest.mark.spec("TUIG-RUNTIME-R015.63")
def test_full_suite_outcomes_are_recorded_without_unexplained_skips(tmp_path):
    native = _jobs()["native-checks"]
    run = _run_text(native)
    assert "pytest tests" in run and "verify_suite_outcomes.py" in run
    ok = '<testcase classname="a" name="b"/>'
    explained = (
        '<testcase classname="a" name="c"><skipped message="Requires the isolated '
        'installed-provider qualification job"/></testcase>'
    )
    unexplained = '<testcase classname="a" name="d"><skipped message="flaky"/></testcase>'
    failing = '<testcase classname="a" name="e"><failure message="x"/></testcase>'
    passed = _suite(tmp_path, ok + explained)
    assert passed.returncode == 0 and "collected=2" in passed.stdout
    assert _suite(tmp_path, ok + unexplained).returncode == 1
    assert _suite(tmp_path, ok + failing).returncode == 1
    assert _suite(tmp_path, "").returncode == 1
