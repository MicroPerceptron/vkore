#!/usr/bin/env python3
"""Bounded llama-server protocol stub used by hostlinkd integration tests."""

import argparse
import json
import os
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


parser = argparse.ArgumentParser(add_help=False)
parser.add_argument("-np", type=int, default=3)
parser.add_argument("--port", type=int, required=True)
args, _ = parser.parse_known_args()


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *_):
        pass

    def send_json(self, value, status=200):
        body = json.dumps(value, separators=(",", ":")).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path == "/health":
            self.send_json({"status": "ok"})
        elif self.path == "/slots":
            self.send_json([{"id": slot, "is_processing": False} for slot in range(args.np)])
        else:
            self.send_json({"error": "unknown"}, 404)

    def do_POST(self):
        length = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(length)
        if self.path.startswith("/slots/") and self.path.endswith("?action=erase"):
            self.send_json({"success": True, "n_erased": 1})
            return
        if self.path != "/completion":
            self.send_json({"error": "unknown"}, 404)
            return
        try:
            request = json.loads(body)
        except Exception:
            self.send_json({"error": "json"}, 400)
            return
        prompt = request.get("prompt", "")
        if "MODEL_FAIL" in prompt:
            self.send_json({"error": "model failed"}, 500)
            return
        if "CRASH_SERVER" in prompt:
            os._exit(73)
        if "DISCONNECT_SERVER" in prompt:
            self.close_connection = True
            return

        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Transfer-Encoding", "chunked")
        self.end_headers()

        if "MALFORMED_SERVER" in prompt:
            events = [b"data: {not-json}\n\n"]
        else:
            slot = int(request.get("id_slot", -1))
            marker = prompt.rsplit("User: ", 1)[-1].split("\nAssistant:", 1)[0]
            if '"authenticated_task_intent"' in marker:
                planning = json.loads(marker)
                intent = planning["authenticated_task_intent"].lower()
                catalog = planning["offer_catalog"]
                objects = [
                    offer
                    for offer in catalog
                    if offer["kind"] in ("object", "view")
                    and offer["label"].lower() in intent
                ]
                executables = [
                    offer
                    for offer in catalog
                    if offer["kind"] == "executable" and offer["label"] == "cat"
                ]
                ceiling = planning["budget_ceiling"]
                text = json.dumps(
                    {
                        "action": "create_agent",
                        "label": "agent/catalog-selection",
                        "selectors": [executables[0]["index"], objects[0]["index"]],
                        "budget": {"tool_calls": min(1, ceiling["tool_calls"])},
                    },
                    separators=(",", ":"),
                )
            elif marker.startswith("{"):
                message = json.loads(marker)
                if message.get("kind") == "user_text":
                    if "MALFORMED_COMPLETION" in message.get("text", ""):
                        text = "{this is not worker json"
                    else:
                        requested = next(
                            (
                                word.rstrip(".,")
                                for word in message["text"].split()
                                if word.startswith("/")
                            ),
                            None,
                        )
                        if requested is None:
                            text = json.dumps(
                                {"action": "respond", "text": "Hello! How can I help?"},
                                separators=(",", ":"),
                            )
                        else:
                            text = json.dumps(
                                {
                                    "action": "tool_call",
                                    "command": "cat",
                                    "arguments": [{"kind": "path", "value": requested}],
                                },
                                separators=(",", ":"),
                            )
                elif message.get("kind") == "tool_result":
                    text = json.dumps(
                        {
                            "action": "respond",
                            "text": "The file contains: " + message.get("text", ""),
                        },
                        separators=(",", ":"),
                    )
                else:
                    text = '{"action":"refused","reason":"unknown input kind"}'
            else:
                text = f"slot={slot}:{marker}"
            events = []
            for token in (text[: max(1, len(text) // 2)], text[max(1, len(text) // 2) :]):
                if token:
                    events.append(
                        b"data: "
                        + json.dumps(
                            {"content": token, "tokens": [slot], "stop": False},
                            separators=(",", ":"),
                        ).encode()
                        + b"\n\n"
                    )
            events.append(b"data: {\"content\":\"\",\"tokens\":[],\"stop\":true}\n\n")
            events.append(b"data: [DONE]\n\n")
        for event in events:
            try:
                self.wfile.write(f"{len(event):x}\r\n".encode())
                self.wfile.write(event)
                self.wfile.write(b"\r\n")
                self.wfile.flush()
                if "SLOW_SERVER" in prompt:
                    time.sleep(0.2)
            except (BrokenPipeError, ConnectionResetError):
                return
        self.wfile.write(b"0\r\n\r\n")
        self.wfile.flush()


ThreadingHTTPServer(("127.0.0.1", args.port), Handler).serve_forever()
