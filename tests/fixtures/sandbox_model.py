"""A loopback OpenAI-compatible chat model for the live sandbox.

``python sandbox_model.py <port>`` answers ``POST /v1/chat/completions`` with a
fixed assistant reply that echoes the last ``proof-<hex>`` nonce in the prompt,
plain or streamed, and appends each answered nonce to ``SANDBOX_MODEL_RECEIPTS``.
"""

import http.server
import json
import os
import re
import sys
import time
from pathlib import Path

MODEL = "sandbox-model"
REPLY = "Sandbox reply for the terminal client."


def completion(reply: str) -> dict:
    return {
        "id": "chatcmpl-sandbox",
        "object": "chat.completion",
        "created": int(time.time()),
        "model": MODEL,
        "choices": [
            {
                "index": 0,
                "message": {"role": "assistant", "content": reply},
                "finish_reason": "stop",
            }
        ],
        "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
    }


def chunks(reply: str) -> list[dict]:
    base = {
        "id": "chatcmpl-sandbox",
        "object": "chat.completion.chunk",
        "created": int(time.time()),
        "model": MODEL,
    }
    return [
        {
            **base,
            "choices": [
                {
                    "index": 0,
                    "delta": {"role": "assistant", "content": reply},
                    "finish_reason": None,
                }
            ],
        },
        {**base, "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]},
    ]


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_args: object) -> None:
        return

    def do_GET(self) -> None:  # noqa: N802 - http.server API
        body = json.dumps(
            {"object": "list", "data": [{"id": MODEL, "object": "model"}]}
        )
        self.send(200, "application/json", body.encode())

    def do_POST(self) -> None:  # noqa: N802 - http.server API
        length = int(self.headers.get("Content-Length") or 0)
        request = json.loads(self.rfile.read(length) or b"{}")
        nonces = re.findall(
            r"proof-[0-9a-f]{32}", json.dumps(request.get("messages", []))
        )
        reply = REPLY + (" " + nonces[-1] if nonces else "")
        if receipt_path := os.getenv("SANDBOX_MODEL_RECEIPTS"):
            with Path(receipt_path).open("a") as receipts:
                receipts.write(
                    json.dumps(
                        {"nonce": nonces[-1] if nonces else None, "reply": reply}
                    )
                    + "\n"
                )
        if not request.get("stream"):
            self.send(200, "application/json", json.dumps(completion(reply)).encode())
            return
        frames = "".join(f"data: {json.dumps(chunk)}\n\n" for chunk in chunks(reply))
        self.send(200, "text/event-stream", (frames + "data: [DONE]\n\n").encode())

    def send(self, status: int, content_type: str, body: bytes) -> None:
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


if __name__ == "__main__":
    http.server.ThreadingHTTPServer(
        ("127.0.0.1", int(sys.argv[1])), Handler
    ).serve_forever()
