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
