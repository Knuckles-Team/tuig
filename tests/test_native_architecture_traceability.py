"""Architecture and traceability checks for the native terminal.

Each test proves one TUIG-RUNTIME-R015.* obligation on the tracked files
and rejects a counterexample that violates the same criterion.
"""

import re
from pathlib import Path

import pytest

ROOT = Path(__file__).parents[1]
NATIVE_SRC = ROOT / "native/tuig/src"
NATIVE_TESTS = ROOT / "native/tuig/tests"

_FORBIDDEN_PRESENTATION_USE = re.compile(r"\b(ArboraClient|reqwest|hyper|tokio::net|tuig_native)\b")


def _use_lines(source: str) -> list[str]:
    return [line for line in source.splitlines() if line.lstrip().startswith("use ")]


def _presentation_is_separate(source: str) -> bool:
    return not any(_FORBIDDEN_PRESENTATION_USE.search(line) for line in _use_lines(source))


@pytest.mark.spec("TUIG-RUNTIME-R015.21")
def test_presentation_adapter_boundary():
    source = (NATIVE_SRC / "presentation.rs").read_text()
    assert _use_lines(source)
    assert _presentation_is_separate(source)
    assert not _presentation_is_separate("use crate::ArboraClient;\n" + source)
    assert not _presentation_is_separate("use reqwest::Client;\n" + source)


def _public_modules(lib: str) -> list[str]:
    return re.findall(r"^pub mod (\w+)\b", lib, re.M)


def _architecture_names(doc: str, modules: list[str]) -> list[str]:
    return [name for name in modules if f"`{name}`" not in doc]


@pytest.mark.spec("TUIG-RUNTIME-R015.23")
def test_module_responsibility():
    modules = _public_modules((NATIVE_SRC / "lib.rs").read_text())
    doc = (ROOT / "docs/architecture.md").read_text()
    assert len(modules) == len(set(modules)) > 10
    assert _architecture_names(doc, modules) == []
    assert _architecture_names(doc, [*modules, "unlisted_module"]) == ["unlisted_module"]


def _concept_owner_rows(concepts: str) -> list[tuple[str, str]]:
    section = concepts.split("## Native concept owners", 1)[-1].split("\n## ", 1)[0]
    return re.findall(r"^\|\s*([^|`]+?)\s*\|\s*`([^`]+)`\s*\|", section, re.M)


def _unowned_concepts(rows: list[tuple[str, str]], root: Path) -> list[str]:
    return [name for name, path in rows if not (root / path).is_file() or not path.startswith("native/tuig/src/")]


@pytest.mark.spec("TUIG-RUNTIME-R015.24")
def test_concept_owner_trace():
    rows = [r for r in _concept_owner_rows((ROOT / "docs/concepts.md").read_text()) if r[0] != "Concept"]
    assert {name for name, _ in rows} >= {"tuig", "A2A", "ACP", "Headless"}
    assert _unowned_concepts(rows, ROOT) == []
    assert _unowned_concepts([("Ghost", "native/tuig/src/ghost.rs")], ROOT) == ["Ghost"]


_CONCEPT_REF = re.compile(r"CONCEPT:[A-Za-z0-9_.*-]+")


def _orphan_refs(files: dict[str, str], registry: str) -> list[str]:
    declared = set(_CONCEPT_REF.findall(registry))
    return sorted(
        f"{name}:{ref}" for name, text in files.items() for ref in _CONCEPT_REF.findall(text) if ref not in declared
    )


@pytest.mark.spec("TUIG-RUNTIME-R015.25")
def test_orphan_concept_refs():
    registry = (ROOT / "docs/concepts.md").read_text()
    paths = [*sorted((ROOT / "docs").glob("*.md")), *sorted(NATIVE_SRC.glob("*.rs"))]
    files = {str(p.relative_to(ROOT)): p.read_text() for p in paths}
    assert _orphan_refs(files, registry) == []
    assert _orphan_refs({"docs/x.md": "see CONCEPT:UNDECLARED.thing"}, registry) == [
        "docs/x.md:CONCEPT:UNDECLARED.thing"
    ]


_SPEC_ID = re.compile(r"TUIG-RUNTIME-R\d+(?:\.\d+)*")


def _unbound_rust_tests(files: dict[str, str]) -> list[str]:
    return sorted(name for name, text in files.items() if not re.search(r"^\s*// spec: " + _SPEC_ID.pattern, text, re.M))


def _unbound_python_tests(files: dict[str, str]) -> list[str]:
    return sorted(name for name, text in files.items() if not re.search(r"mark\.spec\(\s*\"" + _SPEC_ID.pattern, text))


@pytest.mark.spec("TUIG-RUNTIME-R015.26")
def test_tests_bind_spec_ids():
    rows = set(_SPEC_ID.findall((ROOT / "specs/terminal-runtime/requirements.md").read_text()))
    rust = {p.name: p.read_text() for p in sorted(NATIVE_TESTS.glob("*.rs"))}
    python = {p.name: p.read_text() for p in sorted((ROOT / "tests").glob("test_native_*.py"))}
    assert rust and python
    assert _unbound_rust_tests(rust) == []
    assert _unbound_python_tests(python) == []
    bound = {i for text in [*rust.values(), *python.values()] for i in _SPEC_ID.findall(text)}
    assert bound - rows == set()
    assert _unbound_rust_tests({"loose.rs": "#[test]\nfn t() {}\n"}) == ["loose.rs"]
    assert _unbound_python_tests({"test_native_loose.py": "def test_t():\n    assert 1\n"}) == ["test_native_loose.py"]
