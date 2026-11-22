"""Environment inventory and theme documentation for the native binary."""

import re
from pathlib import Path

import pytest

ROOT = Path(__file__).parents[1]
NATIVE_SRC = ROOT / "native" / "tuig" / "src"

ENV_READ_RE = re.compile(
    r"\b(?:std::)?env::(?:var|var_os)\(\s*\"([A-Z][A-Z0-9_]*)\"\s*\)"
)


def _env_reads() -> set[str]:
    names: set[str] = set()
    for path in NATIVE_SRC.rglob("*.rs"):
        names.update(ENV_READ_RE.findall(path.read_text()))
    return names


def _cli_help() -> str:
    src = (NATIVE_SRC / "main.rs").read_text()
    match = re.search(r"const CLI_HELP: &str = \"((?:[^\"\\]|\\.)*)\"", src, re.S)
    assert match is not None, "CLI_HELP constant must exist in main.rs"
    return match.group(1)


@pytest.mark.spec("TUIG-RUNTIME-R015.41.2")
def test_native_environment_reads_are_documented():
    reads = _env_reads()
    assert reads, "the native binary must read at least one environment input"
    config = (ROOT / "docs" / "configuration.md").read_text()
    help_text = _cli_help()
    for name in sorted(reads):
        assert name in config, (
            f"{name} is read by the native binary but missing from docs/configuration.md"
        )
        assert name in help_text, (
            f"{name} is read by the native binary but missing from the CLI help"
        )


@pytest.mark.spec("TUIG-RUNTIME-R015.42.1")
def test_theme_fallback_is_documented():
    config = (ROOT / "docs" / "configuration.md").read_text()
    theme_rows = [line for line in config.splitlines() if "`AGENT_THEME`" in line]
    assert theme_rows, "docs/configuration.md must carry an AGENT_THEME row"
    row = " ".join(theme_rows)
    assert re.search(r"first registered theme", row, re.I), (
        "the AGENT_THEME row must explain that an unknown value selects the "
        "first registered theme"
    )
    color_rows = [line for line in config.splitlines() if "`NO_COLOR`" in line]
    assert color_rows, "docs/configuration.md must carry a NO_COLOR row"
    assert re.search(r"disabl\w+ (theme )?colors", " ".join(color_rows), re.I), (
        "the NO_COLOR row must explain that a nonempty value disables theme colors"
    )
