"""Compiled-native theme startup acceptance through a real terminal."""

import json
import os
import re
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import pytest

from tests.test_native_public_details_pty import NativeTerminal

pytestmark = [
    pytest.mark.spec("TUIG-RUNTIME-R015.42.2"),
    pytest.mark.skipif(
        not os.environ.get("TUI_NATIVE_TEST_BINARY"),
        reason="Requires compiled native executable",
    ),
]

ROOT = Path(__file__).parents[1]


def _startup_themes_and_palettes() -> tuple[str, dict[str, str]]:
    source = (ROOT / "native" / "tuig" / "src" / "presentation.rs").read_text()
    themes = re.search(r"const THEMES: \[&str; \d+\] = \[([^\]]*)\]", source)
    assert themes is not None, "presentation.rs must register the startup themes"
    first = re.findall(r'"([a-z-]+)"', themes.group(1))[0]
    palettes = {
        name: foreground
        for name, foreground in re.findall(
            r'"([a-z-]+)" => \(0x([0-9a-f]{6}), 0x[0-9a-f]{6}\)', source
        )
    }
    default = re.search(r"_ => \(0x([0-9a-f]{6}), 0x[0-9a-f]{6}\)", source)
    assert default is not None, "presentation.rs must keep the default palette arm"
    palettes["_default"] = default.group(1)
    return first, palettes


def _serve_agent_card() -> ThreadingHTTPServer:
    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            body = json.dumps({"name": "fixture-agent"}).encode()
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


def _nonblank_fg(terminal: NativeTerminal) -> set[str]:
    screen = terminal.screen
    return {
        screen.buffer[y][x].fg
        for y in range(screen.lines)
        for x in range(screen.columns)
        if screen.buffer[y][x].data != " "
    }


def _theme_run(server: ThreadingHTTPServer, *, no_color: bool) -> set[str]:
    terminal = NativeTerminal(server, [], task_id=None, no_color=no_color)
    try:
        terminal.wait_until(lambda: "Compose one task" in terminal.shown())
        fgs = _nonblank_fg(terminal)
        terminal.send(b"\x1b")  # Review status; the composer closes.
        terminal.wait_until(
            lambda: "Task streaming and approvals remain unavailable"
            in terminal.shown()
        )
        fgs |= _nonblank_fg(terminal)
        terminal.quit()
        return fgs
    finally:
        terminal.close()


def test_native_entry_theme_fallback_and_no_color(monkeypatch):
    first, palettes = _startup_themes_and_palettes()
    assert palettes, "no palettes parsed from presentation.rs"
    server = _serve_agent_card()
    try:
        # A known startup theme is honored by the real entry point.
        monkeypatch.setenv("AGENT_THEME", "nord")
        fgs = _theme_run(server, no_color=False)
        assert palettes["nord"] in fgs, f"nord palette missing from {sorted(fgs)}"
        assert palettes["_default"] not in fgs, "nord run must not fall back"

        # An unknown startup name falls back to the first registered theme.
        monkeypatch.setenv("AGENT_THEME", "definitely-not-a-theme")
        fgs = _theme_run(server, no_color=False)
        assert palettes["_default"] in fgs, (
            f"unknown theme must fall back to {first}: {sorted(fgs)}"
        )
        assert palettes["nord"] not in fgs, "unknown theme must not use nord"

        # NO_COLOR disables theme colors for the compiled entry point.
        monkeypatch.setenv("AGENT_THEME", "nord")
        fgs = _theme_run(server, no_color=True)
        assert fgs <= {"default"}, f"NO_COLOR must leave no theme colors: {sorted(fgs)}"
    finally:
        server.server_close()
