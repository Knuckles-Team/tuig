"""Quality acceptance v1 checks for the native terminal.

Each test proves one TUIG-RUNTIME-R015.* aggregate obligation from the tracked
files and rejects a counterexample that violates the same criterion.
"""

import configparser
import copy
import re
import tomllib

import pytest
import yaml

from tests.test_native_source_quality import (
    MAX_MODULE_LINES,
    MAX_TEST_FILE_LINES,
    MAX_TEST_ITEMS,
    NATIVE_SRC,
    NATIVE_TESTS,
    ROOT,
    WORKFLOW,
    _cli_help,
    _production_sources,
    _python_test_items,
    _rust_test_items,
    _test_only_modules,
)
from tests.test_native_test_quality import (
    REQUIREMENTS,
    _config,
    _direct_listener_binds,
    _every_test_asserts,
    _rust_tests,
    _rust_tests_with_only_weak_assertions,
    _workflow,
)

SPEC_ID = re.compile(r"TUIG-RUNTIME-R\d+(?:\.\d+)*")
ADVISORY = ROOT / ".github" / "workflows" / "advisory.yml"


def _hooks(config: dict) -> dict[str, dict]:
    return {hook["id"]: hook for repo in config["repos"] for hook in repo["hooks"]}


# --- R015.55: metadata, layout and public-surface checks ------------------


def _project_checks_configured(config: dict, pyproject: dict, advisory: dict) -> bool:
    hooks = _hooks(config)
    spec = hooks.get("spec", {})
    both_stages = {"pre-commit", "pre-push"} <= set(spec.get("stages", []))
    default_stages = set(config.get("default_stages", [])) >= {"pre-commit", "pre-push"}
    surface = pyproject.get("tool", {}).get("pipelines_hooks", {}).get("public_surface", {})
    surface_named = all(surface.get(key) for key in ("repository", "distribution", "pages_url"))
    gates = set(advisory["jobs"]["advisory"]["with"]["gates"].split())
    advisory_gates = {"public-surface", "root-hygiene", "gitignore-convergence"} <= gates
    return both_stages and default_stages and surface_named and advisory_gates


@pytest.mark.spec("TUIG-RUNTIME-R015.55")
def test_native_metadata_layout_and_public_surface_checks_configured():
    config = _config()
    pyproject = tomllib.loads((ROOT / "pyproject.toml").read_text())
    advisory = yaml.safe_load(ADVISORY.read_text())
    assert _project_checks_configured(config, pyproject, advisory)
    assert (ROOT / ".config" / "repo-layout.toml").is_file()
    no_spec = copy.deepcopy(config)
    for repo in no_spec["repos"]:
        repo["hooks"] = [h for h in repo["hooks"] if h["id"] != "spec"]
    assert not _project_checks_configured(no_spec, pyproject, advisory)
    no_surface = copy.deepcopy(pyproject)
    del no_surface["tool"]["pipelines_hooks"]["public_surface"]
    assert not _project_checks_configured(config, no_surface, advisory)
    no_layout = copy.deepcopy(advisory)
    no_layout["jobs"]["advisory"]["with"]["gates"] = "public-surface"
    assert not _project_checks_configured(config, pyproject, no_layout)


# --- R015.56: structural limits measured over the full tree ---------------


def _measured_all(tracked: set[str], measured: set[str]) -> bool:
    return tracked == measured


def _full_tree_gates(workflow: dict) -> bool:
    main = workflow["jobs"].get("main-gates", {})
    runs = [
        str(step.get("run", ""))
        for step in workflow["jobs"]["native-checks"]["steps"]
        if isinstance(step, dict)
    ]
    clippy = any("clippy" in run and "--all-targets" in run for run in runs)
    return main.get("with", {}).get("all-files") is True and clippy


@pytest.mark.spec("TUIG-RUNTIME-R015.56")
def test_native_structural_limits_measured_over_full_tree():
    tracked = {path.name for path in NATIVE_SRC.glob("*.rs")}
    measured = set(_production_sources()) | _test_only_modules()
    assert tracked and _measured_all(tracked, measured)
    assert not _measured_all(tracked, measured - {"main.rs"})
    assert _full_tree_gates(_workflow())
    changed_only = copy.deepcopy(_workflow())
    changed_only["jobs"]["main-gates"]["with"]["all-files"] = False
    assert not _full_tree_gates(changed_only)
    oversized = [
        path.name
        for path in NATIVE_TESTS.glob("*.rs")
        if len(path.read_text().splitlines()) > MAX_TEST_FILE_LINES
        or _rust_test_items(path.read_text()) > MAX_TEST_ITEMS
    ]
    assert oversized == []
    assert all(
        sum(1 for line in source.splitlines() if line.strip()) <= MAX_MODULE_LINES
        for source in _production_sources().values()
    )
    python = [path for path in (ROOT / "tests").glob("*.py")]
    assert all(_python_test_items(path.read_text()) <= MAX_TEST_ITEMS for path in python)


# --- R015.60: module boundaries and responsibility map --------------------

TRANSPORT = re.compile(r"\breqwest\b")
RENDERING = re.compile(r"\b(?:ratatui|crossterm)\b")


def _seam_violations(sources: dict[str, str]) -> list[str]:
    problems = []
    for name, text in sources.items():
        uses = "\n".join(line for line in text.splitlines() if line.lstrip().startswith("use "))
        transport = bool(TRANSPORT.search(text))
        rendering = bool(RENDERING.search(uses))
        if transport and rendering:
            problems.append(f"{name}: transport module imports rendering")
        if (name.endswith("_view.rs") or name in {"presentation.rs", "composer.rs"}) and transport:
            problems.append(f"{name}: view module reaches the transport directly")
    return problems


def _binary_modules(main: str) -> list[str]:
    lines = main.splitlines()
    return [
        match.group(1)
        for index, line in enumerate(lines)
        if (match := re.match(r"mod (\w+);", line))
        and not (index and lines[index - 1].startswith("#["))
    ]


@pytest.mark.spec("TUIG-RUNTIME-R015.60")
def test_native_module_boundaries_and_responsibility_map():
    sources = {path.name: path.read_text() for path in sorted(NATIVE_SRC.glob("*.rs"))}
    assert _seam_violations(sources) == []
    architecture = (ROOT / "docs" / "architecture.md").read_text()
    binary = _binary_modules(sources["main.rs"])
    assert len(binary) >= 10
    undocumented = [name for name in binary if f"`{name}`" not in architecture]
    assert undocumented == []
    assert _seam_violations({"x_view.rs": "use reqwest::Client;\n"}) == [
        "x_view.rs: view module reaches the transport directly"
    ]
    mixed = "use reqwest::Client;\nuse ratatui::Frame;\n"
    assert _seam_violations({"mixed.rs": mixed}) == ["mixed.rs: transport module imports rendering"]
    assert _binary_modules("mod ghost;\n") == ["ghost"]
    assert _binary_modules("#[cfg(test)]\nmod tests;\n") == []


# --- R015.61: concepts and test bindings reconcile ------------------------


def _declared() -> tuple[set[str], set[str]]:
    rows = re.findall(r"^\| `(TUIG-RUNTIME-R[\d.]+)` \| (.*)$", REQUIREMENTS.read_text(), re.M)
    retired = {row for row, text in rows if text.startswith("**RETIRED")}
    return {row for row, _ in rows}, retired


def _bound_ids() -> set[str]:
    bound: set[str] = set()
    for path in NATIVE_SRC.glob("*.rs"):
        bound.update(re.findall(r"^\s*// spec: (" + SPEC_ID.pattern + ")", path.read_text(), re.M))
    for path in NATIVE_TESTS.glob("*.rs"):
        bound.update(re.findall(r"^\s*// spec: (" + SPEC_ID.pattern + ")", path.read_text(), re.M))
    for path in (ROOT / "tests").glob("*.py"):
        bound.update(re.findall(r'mark\.spec\(\s*"(' + SPEC_ID.pattern + ')"', path.read_text()))
    return bound


def _reconciled(declared: set[str], retired: set[str], bound: set[str]) -> list[str]:
    problems = [f"{row}: bound but not declared" for row in sorted(bound - declared)]
    problems += [f"{row}: retired but still bound" for row in sorted(bound & retired)]
    return problems


@pytest.mark.spec("TUIG-RUNTIME-R015.61")
def test_native_concepts_and_test_bindings_reconcile():
    declared, retired = _declared()
    bound = _bound_ids()
    assert len(declared) > 100 and retired
    assert _reconciled(declared, retired, bound) == []
    assert "no-legacy-concepts" in _hooks(_config())
    concepts = (ROOT / "docs" / "concepts.md").read_text()
    assert "CONCEPT:TUIG-" in concepts
    orphan = "TUIG-RUNTIME-R" + "9999"
    assert _reconciled(declared, retired, bound | {orphan}) == [f"{orphan}: bound but not declared"]
    gone = sorted(retired)[0]
    assert _reconciled(declared, retired, bound | {gone}) == [f"{gone}: retired but still bound"]


# --- R015.64: version mirrors and locks agree -----------------------------


def _mirror_versions() -> dict[str, str]:
    bump = configparser.ConfigParser()
    bump.read(ROOT / ".bumpversion.cfg")
    project = tomllib.loads((ROOT / "pyproject.toml").read_text())["project"]["version"]
    cargo = tomllib.loads((ROOT / "native/tuig/Cargo.toml").read_text())["package"]["version"]
    concepts = re.search(r"\*\*Version\*\*: (\S+)", (ROOT / "docs/concepts.md").read_text())
    uv_lock = re.search(r'name = "tuig"\nversion = "([^"]+)"', (ROOT / "uv.lock").read_text())
    cargo_lock = re.search(
        r'name = "tuig-native"\nversion = "([^"]+)"', (ROOT / "native/tuig/Cargo.lock").read_text()
    )
    assert concepts and uv_lock and cargo_lock
    return {
        "bumpversion": bump["bumpversion"]["current_version"],
        "pyproject": project,
        "cargo": cargo,
        "concepts": concepts.group(1),
        "uv.lock": uv_lock.group(1),
        "cargo.lock": cargo_lock.group(1),
    }


@pytest.mark.spec("TUIG-RUNTIME-R015.64")
def test_version_mirrors_and_locks_agree():
    versions = _mirror_versions()
    assert len(set(versions.values())) == 1, versions
    bump = configparser.ConfigParser()
    bump.read(ROOT / ".bumpversion.cfg")
    mirrored = {name.split(":", 2)[2] for name in bump.sections() if name.startswith("bumpversion:file:")}
    assert {"pyproject.toml", "docs/concepts.md", "uv.lock", "native/tuig/Cargo.toml", "native/tuig/Cargo.lock"} <= mirrored
    assert "check-bumpversion" in _hooks(_config())
    stale = dict(versions, cargo="0.1.0")
    assert len(set(stale.values())) != 1
    historical = ROOT / "specs" / "terminal-runtime" / "test-spec.md"
    assert historical.is_file() and "bumpversion" not in {n for n in mirrored if n.startswith("specs")}


# --- R015.65: the test inventory is behavior specific ---------------------

REFUSAL = re.compile(r"\.is_err\(\)|\bassert_ne!|\bassert!\(\s*!|\bErr\(|\bis_none\(\)|\bmatches!")


def _checks_a_wrong_result(text: str) -> bool:
    return REFUSAL.search(text) is not None


@pytest.mark.spec("TUIG-RUNTIME-R015.65")
def test_test_inventory_is_behavior_specific():
    rust = _rust_tests()
    assert len(rust) >= 20
    assert _every_test_asserts() == ([], [])
    weak = [n for n, text in rust.items() if _rust_tests_with_only_weak_assertions(text)]
    assert weak == []
    direct = [n for n, text in rust.items() if _direct_listener_binds(text)]
    assert direct == []
    uncheck = [n for n, text in rust.items() if not _checks_a_wrong_result(text)]
    assert uncheck == [], f"test files that never reject a wrong result: {uncheck}"
    counts_only = "#[test]\nfn counted() {\n    assert!(items().len() == 3);\n}\n"
    assert not _checks_a_wrong_result(counts_only)
    assert _checks_a_wrong_result("#[test]\nfn refused() {\n    assert!(call().is_err());\n}\n")


# --- R015.66: environment documentation matches native inputs -------------

ENV_NAME = re.compile(r"\b(AGENT_[A-Z_]+|NO_COLOR)\b")
ENV_READ = re.compile(r"env::var(?:_os)?\(\s*\"([A-Z][A-Z0-9_]*)\"")


def _documented_names(config: str) -> set[str]:
    return set(re.findall(r"^\| `([A-Z][A-Z0-9_]*)` \|", config, re.M))


def _help_names(help_text: str) -> set[str]:
    section = help_text[help_text.index("Environment:") :]
    return set(re.findall(r"^  ([A-Z][A-Z0-9_]*)\s", section.replace("\\n", "\n"), re.M))


def _read_names() -> set[str]:
    names: set[str] = set()
    for path in NATIVE_SRC.glob("*.rs"):
        names.update(ENV_READ.findall(path.read_text()))
    return names


def _environment_reconciled(read: set[str], documented: set[str], helped: set[str]) -> bool:
    return read == documented == helped


@pytest.mark.spec("TUIG-RUNTIME-R015.66")
def test_environment_docs_match_native_inputs():
    config = (ROOT / "docs" / "configuration.md").read_text()
    read, documented, helped = _read_names(), _documented_names(config), _help_names(_cli_help())
    assert _environment_reconciled(read, documented, helped), (read, documented, helped)
    example = {
        line.split("=", 1)[0]
        for line in (ROOT / ".env.example").read_text().splitlines()
        if line and not line.startswith("#")
    }
    assert example <= documented
    main = (NATIVE_SRC / "main.rs").read_text()
    assert 'unwrap_or_else(|_| "http://localhost:8000".to_owned())' in main
    assert "default `http://localhost:8000`" in config and "http://localhost:8000" in _cli_help()
    assert "Duration::from_secs(300)" in (NATIVE_SRC / "lib.rs").read_text()
    assert "default 300" in config and "default: 300" in _cli_help()
    assert not _environment_reconciled(read | {"AGENT_UNDOCUMENTED"}, documented, helped)
    assert not _environment_reconciled(read, documented | {"AGENT_RETIRED"}, helped)
    assert _help_names("Environment:\\n  AGENT_URL   Endpoint.\\n") == {"AGENT_URL"}
