"""Test-quality, injection and gate-configuration checks for the native terminal.

Each test proves one TUIG-RUNTIME-R015.* obligation on the tracked files and
rejects a counterexample that violates the same criterion.
"""

import ast
import copy
import re

import pytest
import yaml

from tests.test_native_source_quality import (
    NATIVE_SRC,
    NATIVE_TESTS,
    ROOT,
    WORKFLOW,
    _cli_help,
    _production_sources,
    _readme,
    _rust_functions,
    _strip_comments_and_strings,
)

PRE_COMMIT = ROOT / ".pre-commit-config.yaml"
REQUIREMENTS = ROOT / "specs" / "terminal-runtime" / "requirements.md"

# A function is significant for contract naming from half the 60-line cap.
SIGNIFICANT_FUNCTION_LINES = 30
SIGNATURE = r"\s*(?:pub(?:\(crate\))?\s+)?(?:async\s+)?fn\s+(\w+)"
HOOK_REVISION = re.compile(r"[0-9a-f]{40}")


def _config() -> dict:
    return yaml.safe_load(PRE_COMMIT.read_text())


def _workflow() -> dict:
    return yaml.safe_load(WORKFLOW.read_text())


def _native_steps(workflow: dict) -> list[str]:
    job = workflow["jobs"]["native-checks"]
    return [str(step.get("run", "")) for step in job["steps"] if isinstance(step, dict)]


# --- R015.8: security scan gates ------------------------------------------

SECURITY_HOOKS = ("detect-private-key", "secrets", "bandit")


def _security_gates_configured(workflow: dict, config: dict) -> bool:
    for name in ("gates", "main-gates"):
        job = workflow["jobs"].get(name, {})
        if not str(job.get("uses", "")).endswith("gates_pipeline.yml@main"):
            return False
        if job.get("with", {}).get("rust") is not True:
            return False
        skipped = str(job.get("with", {}).get("skip-hooks", "")).split(",")
        if any(hook in skipped for hook in SECURITY_HOOKS):
            return False
    hooks = {
        hook["id"]: hook for repo in config["repos"] for hook in repo["hooks"]
    }
    for hook_id in SECURITY_HOOKS:
        stages = hooks.get(hook_id, {}).get("stages", ["pre-commit"])
        if hook_id not in hooks or stages == ["manual"]:
            return False
    return True


@pytest.mark.spec("TUIG-RUNTIME-R015.8")
def test_security_scan_gates_configured():
    workflow, config = _workflow(), _config()
    assert _security_gates_configured(workflow, config)
    skipping = copy.deepcopy(workflow)
    skipping["jobs"]["gates"]["with"]["skip-hooks"] += ",secrets"
    assert not _security_gates_configured(skipping, config)
    no_rust = copy.deepcopy(workflow)
    no_rust["jobs"]["main-gates"]["with"]["rust"] = False
    assert not _security_gates_configured(no_rust, config)
    dropped = copy.deepcopy(config)
    for repo in dropped["repos"]:
        repo["hooks"] = [h for h in repo["hooks"] if h["id"] != "detect-private-key"]
    assert not _security_gates_configured(workflow, dropped)


# --- R015.9, .39, .40: native and Python tests assert results -------------

RUST_TEST = re.compile(
    r"#\[(?:tokio::)?test(?:\([^)]*\))?\]\s*(?:#\[[^\]]*\]\s*)*"
    r"(?:pub\s+)?(?:async\s+)?fn\s+(\w+)[^{]*\{"
)
RUST_ASSERTION = re.compile(r"\bassert(?:_eq|_ne)?!|\bpanic!|\bmatches!")
RUST_ASSERT_MACRO = re.compile(r"\bassert!\(\s*([^;]*?)\)\s*;", re.S)
WEAK_ASSERTION = re.compile(r"true|[\w.\s():&]*\.is_ok\(\)")


def _rust_test_bodies(text: str) -> dict[str, str]:
    stripped = _strip_comments_and_strings(text)
    bodies = {}
    for match in RUST_TEST.finditer(stripped):
        depth, index = 0, match.end() - 1
        while index < len(stripped):
            if stripped[index] == "{":
                depth += 1
            elif stripped[index] == "}":
                depth -= 1
                if depth == 0:
                    break
            index += 1
        bodies[match.group(1)] = stripped[match.end() - 1 : index]
    return bodies


def _rust_tests_without_assertion(text: str) -> list[str]:
    return [
        name
        for name, body in _rust_test_bodies(text).items()
        if not RUST_ASSERTION.search(body)
    ]


def _rust_tests_with_only_weak_assertions(text: str) -> list[str]:
    weak = []
    for name, body in _rust_test_bodies(text).items():
        macros = RUST_ASSERT_MACRO.findall(body)
        strong = len(RUST_ASSERTION.findall(body)) - len(macros)
        if macros and strong == 0 and all(WEAK_ASSERTION.fullmatch(m.strip()) for m in macros):
            weak.append(name)
    return weak


def _is_assertion_call(node: ast.AST) -> bool:
    if not isinstance(node, ast.Call):
        return False
    func = node.func
    name = func.attr if isinstance(func, ast.Attribute) else getattr(func, "id", "")
    return "assert" in name.lower() or name in {"raises", "fail"}


def _asserting_functions(trees: list[ast.AST]) -> set[str]:
    names = set()
    for tree in trees:
        for node in ast.walk(tree):
            if isinstance(node, ast.FunctionDef) and any(
                isinstance(inner, ast.Assert) or _is_assertion_call(inner)
                for inner in ast.walk(node)
            ):
                names.add(node.name)
    return names


def _python_tests_without_assertion(source: str, known: set[str]) -> list[str]:
    tree = ast.parse(source)
    missing = []
    for node in ast.walk(tree):
        if not (isinstance(node, ast.FunctionDef) and node.name.startswith("test_")):
            continue
        asserts = any(
            isinstance(inner, ast.Assert)
            or _is_assertion_call(inner)
            or (
                isinstance(inner, ast.Call)
                and getattr(inner.func, "id", getattr(inner.func, "attr", "")) in known
            )
            for inner in ast.walk(node)
        )
        if not asserts:
            missing.append(node.name)
    return missing


def _python_trees() -> dict[str, str]:
    return {path.name: path.read_text() for path in sorted((ROOT / "tests").glob("*.py"))}


def _rust_tests() -> dict[str, str]:
    return {path.name: path.read_text() for path in sorted(NATIVE_TESTS.glob("*.rs"))}


def _every_test_asserts() -> tuple[list[str], list[str]]:
    sources = _python_trees()
    known = _asserting_functions([ast.parse(text) for text in sources.values()])
    rust = [
        f"{name}:{test}"
        for name, text in _rust_tests().items()
        for test in _rust_tests_without_assertion(text)
    ]
    python = [
        f"{name}:{test}"
        for name, text in sources.items()
        for test in _python_tests_without_assertion(text, known)
    ]
    return rust, python


@pytest.mark.spec("TUIG-RUNTIME-R015.9")
def test_every_test_asserts_an_observable_result():
    rust, python = _every_test_asserts()
    assert not rust, f"native tests without an assertion: {rust}"
    assert not python, f"python tests without an assertion: {python}"
    assert sum(len(_rust_test_bodies(t)) for t in _rust_tests().values()) > 100
    bare = "#[test]\nfn quiet() {\n    let _value = compute();\n}\n"
    assert _rust_tests_without_assertion(bare) == ["quiet"]
    assert _python_tests_without_assertion("def test_quiet():\n    compute()\n", set()) == [
        "test_quiet"
    ]


@pytest.mark.spec("TUIG-RUNTIME-R015.39")
def test_tests_assert_results():
    rust, python = _every_test_asserts()
    assert (rust, python) == ([], [])
    cli_help = _rust_test_bodies(_rust_tests()["cli_help.rs"])
    assert "help_does_not_swallow_values_or_invalid_argument_combinations" in cli_help
    assert all(RUST_ASSERTION.search(body) for body in cli_help.values())
    asserting = "#[tokio::test]\nasync fn checks() {\n    assert_eq!(compute(), 3);\n}\n"
    assert _rust_tests_without_assertion(asserting) == []
    helper_only = "def test_delegates():\n    exercise_flow()\n"
    assert _python_tests_without_assertion(helper_only, {"exercise_flow"}) == []
    assert _python_tests_without_assertion(helper_only, set()) == ["test_delegates"]


@pytest.mark.spec("TUIG-RUNTIME-R015.40")
def test_behavior_specific_assertions():
    weak = [
        f"{name}:{test}"
        for name, text in _rust_tests().items()
        for test in _rust_tests_with_only_weak_assertions(text)
    ]
    assert not weak, f"tests with only non-specific assertions: {weak}"
    compared = sum(
        len(re.findall(r"\bassert_eq!", text)) for text in _rust_tests().values()
    )
    assert compared > 100
    assert _rust_tests_with_only_weak_assertions(
        "#[test]\nfn vacuous() {\n    assert!(true);\n}\n"
    ) == ["vacuous"]
    assert _rust_tests_with_only_weak_assertions(
        "#[test]\nfn ok_only() {\n    assert!(build().is_ok());\n}\n"
    ) == ["ok_only"]
    specific = "#[test]\nfn exact() {\n    assert!(build().is_ok());\n    assert_eq!(value(), 4);\n}\n"
    assert _rust_tests_with_only_weak_assertions(specific) == []


# --- R015.10: documentation examples match the CLI ------------------------


def _readme_commands(text: str) -> list[str]:
    block = re.search(r"## Connect.*?```sh\n(.*?)```", text, re.S)
    assert block is not None, "README must keep a command block under Connect"
    return [line for line in block.group(1).splitlines() if line.startswith("tuig")]


def _unknown_flags(commands: list[str], help_text: str) -> list[str]:
    return sorted(
        {
            flag
            for command in commands
            for flag in re.findall(r"(?<!\S)(--[a-z][a-z-]*)", command)
            if flag not in help_text
        }
    )


@pytest.mark.spec("TUIG-RUNTIME-R015.10")
def test_readme_examples_match_cli_help():
    commands = _readme_commands(_readme())
    help_text = _cli_help()
    assert len(commands) >= 4
    assert _unknown_flags(commands, help_text) == []
    flags = {flag for command in commands for flag in re.findall(r"--[a-z-]+", command)}
    assert {"--prompt", "--observe-task", "--headless"} <= flags
    assert _unknown_flags(commands + ["tuig --made-up-flag"], help_text) == ["--made-up-flag"]


# --- R015.22: explicit dependency injection -------------------------------

CLOCK_SOURCES = ("SystemTime", "UNIX_EPOCH")


def _reads_ambient_clock(source: str) -> bool:
    stripped = _strip_comments_and_strings(source)
    return any(token in stripped for token in CLOCK_SOURCES)


@pytest.mark.spec("TUIG-RUNTIME-R015.22")
def test_dependency_injection():
    lib = (NATIVE_SRC / "lib.rs").read_text()
    assert re.search(
        r"pub fn new\(endpoint: &str, token: Option<&str>\)", lib
    ), "ArboraClient::new must take endpoint and token"
    assert "pub fn with_clock(mut self, clock: clock::Clock)" in lib
    clock = (NATIVE_SRC / "clock.rs").read_text()
    assert _reads_ambient_clock(clock)
    ambient = [
        path.name
        for path in sorted(NATIVE_SRC.glob("*.rs"))
        if path.name != "clock.rs" and _reads_ambient_clock(path.read_text())
    ]
    assert ambient == [], f"only clock.rs may read the system clock: {ambient}"
    host_logs = (NATIVE_SRC / "host_logs.rs").read_text()
    assert "pub fn parse(value: &Value, now_ms: u64)" in host_logs
    assert "client.now_ms()" in host_logs
    plan = (NATIVE_SRC / "plan_confirm.rs").read_text()
    assert "plan.is_expired(self.now_ms())" in plan
    tests = _rust_tests()
    for name in ("host_logs.rs", "plan_confirm.rs"):
        text = tests[name]
        assert "Clock::from_fn" in text
        assert not re.search(r"\bsleep\(|SystemTime", text), f"{name} must not wait on time"
    assert "serve_raw" in (NATIVE_TESTS / "support" / "mod.rs").read_text()
    assert _reads_ambient_clock("let now = SystemTime::now();")
    assert not _reads_ambient_clock("let now = client.now_ms(); // SystemTime")


# --- R015.27: significant functions name contracts ------------------------

CONTRACT = re.compile(r"\s*// contract: (TUIG-RUNTIME-R\d+(?:\.\d+)*)$")


def _requirement_ids() -> set[str]:
    return set(re.findall(r"^\| `(TUIG-RUNTIME-R[\d.]+)`", REQUIREMENTS.read_text(), re.M))


def _significant_without_contract(source: str, known: set[str]) -> list[str]:
    lines = source.splitlines()
    signatures = [
        index for index, line in enumerate(lines) if re.match(SIGNATURE, line)
    ]
    functions = _rust_functions(source)
    assert len(signatures) == len(functions)
    missing = []
    for index, (name, _line, body) in zip(signatures, functions):
        public = re.match(r"\s*pub\b", lines[index]) is not None
        if body.count("\n") + 1 < SIGNIFICANT_FUNCTION_LINES:
            continue
        if not (public or name in {"run", "main"}):
            continue
        above = index
        while above > 0 and re.match(r"\s*(///|#\[)", lines[above - 1]):
            above -= 1
        found = CONTRACT.match(lines[above - 1]) if above else None
        if found is None or found.group(1) not in known:
            missing.append(name)
    return missing


@pytest.mark.spec("TUIG-RUNTIME-R015.27")
def test_functions_identify_contracts():
    known = _requirement_ids()
    sources = _production_sources()
    missing = {
        name: _significant_without_contract(source, known)
        for name, source in sources.items()
    }
    assert {name: fns for name, fns in missing.items() if fns} == {}
    marked = sum(
        len(re.findall(r"^\s*// contract: ", source, re.M)) for source in sources.values()
    )
    assert marked >= 15
    body = "    let _x = 0;\n" * 31
    bare = f"pub fn long_one() {{\n{body}}}\n"
    assert _significant_without_contract(bare, known) == ["long_one"]
    named = "// contract: TUIG-RUNTIME-R002.1.3\n" + bare
    assert _significant_without_contract(named, known) == []
    unknown = "// contract: TUIG-RUNTIME-R" + "999\n" + bare
    assert _significant_without_contract(unknown, known) == ["long_one"]
    private = f"fn helper() {{\n{body}}}\n"
    assert _significant_without_contract(private, known) == []
    entry = f"async fn run() {{\n{body}}}\n"
    assert _significant_without_contract(entry, known) == ["run"]


# --- R015.28: lint checks pass --------------------------------------------


def _lint_gates(steps: list[str]) -> bool:
    manifest = "--manifest-path native/tuig/Cargo.toml"
    fmt = any(f"fmt {manifest} --check" in run for run in steps)
    clippy = any(
        "clippy --locked" in run
        and manifest in run
        and "--all-targets" in run
        and run.rstrip().endswith("-D warnings")
        for run in steps
    )
    return fmt and clippy


def _blanket_lint_suppressions(source: str) -> bool:
    return bool(re.search(r"#!\[allow\((?:warnings|clippy::all)", source))


@pytest.mark.spec("TUIG-RUNTIME-R015.28")
def test_lint_checks():
    steps = _native_steps(_workflow())
    assert _lint_gates(steps)
    workflow = _workflow()["jobs"]["native-checks"]
    toolchain = next(
        str(step["run"]) for step in workflow["steps"] if "toolchain install" in str(step.get("run"))
    )
    assert "--component rustfmt" in toolchain and "--component clippy" in toolchain
    suppressed = [
        path.name
        for path in sorted(NATIVE_SRC.glob("*.rs"))
        if _blanket_lint_suppressions(path.read_text())
    ]
    assert suppressed == []
    assert not _lint_gates([run.replace("-D warnings", "") for run in steps])
    assert not _lint_gates([run.replace("--all-targets", "") for run in steps])
    assert _blanket_lint_suppressions("#![allow(clippy::all)]\nfn main() {}")


# --- R015.29.2: remaining hook pins ---------------------------------------

REMAINING_HOOKS = {
    "ruff": "https://github.com/astral-sh/ruff-pre-commit",
    "mypy": "https://github.com/pre-commit/mirrors-mypy",
    "vulture": "https://github.com/jendrikseipp/vulture",
    "codespell": "https://github.com/codespell-project/codespell",
    "bandit": "https://github.com/PyCQA/bandit",
    "nbqa-ruff": "https://github.com/nbQA-dev/nbQA",
}


def _unpinned_hooks(config: dict) -> list[str]:
    repos = {repo["repo"]: repo for repo in config["repos"]}
    problems = []
    for hook_id, url in REMAINING_HOOKS.items():
        repo = repos.get(url)
        if repo is None or hook_id not in {hook["id"] for hook in repo["hooks"]}:
            problems.append(f"{hook_id}: missing")
        elif not HOOK_REVISION.fullmatch(str(repo["rev"])):
            problems.append(f"{hook_id}: rev {repo['rev']} is not a commit SHA")
    readiness = [
        hook
        for repo in config["repos"]
        for hook in repo["hooks"]
        if hook["id"] == "dependency-readiness"
    ]
    if len(readiness) != 1 or "pre-push" not in readiness[0].get("stages", []):
        problems.append("dependency-readiness: not a pre-push hook")
    return problems


@pytest.mark.spec("TUIG-RUNTIME-R015.29.2")
def test_remaining_hook_pins():
    config = _config()
    assert _unpinned_hooks(config) == []
    floating = copy.deepcopy(config)
    for repo in floating["repos"]:
        if repo["repo"] == REMAINING_HOOKS["bandit"]:
            repo["rev"] = "1.9.4"
    assert _unpinned_hooks(floating) == ["bandit: rev 1.9.4 is not a commit SHA"]
    removed = copy.deepcopy(config)
    removed["repos"] = [r for r in removed["repos"] if r["repo"] != REMAINING_HOOKS["ruff"]]
    assert _unpinned_hooks(removed) == ["ruff: missing"]


# --- R015.36: test layers --------------------------------------------------


def _layers_organized(unit: list[str], integration: list[str], served: list[str]) -> bool:
    return bool(unit) and bool(integration) and bool(served)


@pytest.mark.spec("TUIG-RUNTIME-R015.36")
def test_test_layers():
    unit = [
        path.name
        for path in sorted(NATIVE_SRC.glob("*.rs"))
        if "#[cfg(test)]" in path.read_text()
    ]
    integration = [path.name for path in sorted(NATIVE_TESTS.glob("*.rs"))]
    served = [path.name for path in sorted((ROOT / "tests").glob("test_native_*_pty.py"))]
    assert _layers_organized(unit, integration, served)
    assert "main.rs" in unit and "lib.rs" in unit
    assert "host_logs.rs" in integration
    assert "test_native_composer_pty.py" in served
    assert all("#[test]" not in path.read_text() for path in NATIVE_SRC.glob("*.rs") if "#[cfg(test)]" not in path.read_text())
    assert not _layers_organized(unit, [], served)
    assert not _layers_organized([], integration, served)
    assert not _layers_organized(unit, integration, [])


# --- R015.38: shared fixtures ---------------------------------------------


def _direct_listener_binds(text: str) -> list[str]:
    stripped = _strip_comments_and_strings(text)
    return re.findall(r"TcpListener\s*::\s*bind", stripped)


@pytest.mark.spec("TUIG-RUNTIME-R015.38")
def test_shared_fixture_reuse():
    support = (NATIVE_TESTS / "support" / "mod.rs").read_text()
    assert "pub async fn bind()" in support and "pub fn bind_blocking()" in support
    offenders = [
        path.name
        for path in sorted(NATIVE_TESTS.glob("*.rs"))
        if _direct_listener_binds(path.read_text())
    ] + [
        path.name
        for path in sorted(NATIVE_SRC.glob("*.rs"))
        if _direct_listener_binds(path.read_text())
    ]
    assert offenders == [], f"bind listeners through tests/support: {offenders}"
    users = [
        path.name
        for path in sorted(NATIVE_TESTS.glob("*.rs"))
        if "support::bind" in path.read_text() or "support::serve" in path.read_text()
    ]
    assert len(users) >= 10
    assert _direct_listener_binds(support) != []
    assert _direct_listener_binds('let l = TcpListener::bind("127.0.0.1:0").await.unwrap();')
    assert not _direct_listener_binds("let (l, url) = support::bind().await;")


# --- R015.52: native CI executes tests and journeys -----------------------


def _native_ci_executes(workflow: dict) -> bool:
    job = workflow["jobs"].get("native-checks", {})
    runs = [str(step.get("run", "")) for step in job.get("steps", []) if isinstance(step, dict)]
    tests = any("cargo +1.96.0 test --locked" in run for run in runs)
    journeys = any(
        "test_native_local_session_pty.py" in run
        and "--junitxml" in run
        and 'case.find(tag) is None for case in cases for tag in ("skipped", "failure", "error")' in run
        for run in runs
    )
    acp = any("tests/test_native_acp.py" in run and "--junitxml" in run for run in runs)
    return tests and journeys and acp


@pytest.mark.spec("TUIG-RUNTIME-R015.52")
def test_native_ci_executes_native_tests_and_journeys():
    workflow = _workflow()
    assert _native_ci_executes(workflow)
    assert "ci-required" in workflow["jobs"]
    assert "native-checks" in workflow["jobs"]["ci-required"]["needs"]
    no_tests = copy.deepcopy(workflow)
    for step in no_tests["jobs"]["native-checks"]["steps"]:
        if isinstance(step, dict) and "run" in step:
            step["run"] = str(step["run"]).replace("test --locked", "check --locked")
    assert not _native_ci_executes(no_tests)
    tolerant = copy.deepcopy(workflow)
    for step in tolerant["jobs"]["native-checks"]["steps"]:
        if isinstance(step, dict) and "run" in step:
            step["run"] = str(step["run"]).replace('("skipped", "failure", "error")', '("failure",)')
    assert not _native_ci_executes(tolerant)
