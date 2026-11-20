#!/usr/bin/env python3
"""Tiny deterministic stdio LSP server for meld-lsp tests.

It answers only initialize, completion and shutdown. The completion item's
edit range echoes the request position so tests can prove that a virtual-
document position was mapped back to the original .meld coordinates.
"""
import json
import sys


def read_message():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        if line in (b"\r\n", b"\n"):
            break
        if line.lower().startswith(b"content-length:"):
            length = int(line.split(b":", 1)[1].strip())
    if length is None:
        return None
    return json.loads(sys.stdin.buffer.read(length).decode("utf-8"))


def write_message(msg):
    body = json.dumps(msg).encode("utf-8")
    sys.stdout.buffer.write(
        b"Content-Length: " + str(len(body)).encode("ascii") + b"\r\n\r\n" + body
    )
    sys.stdout.buffer.flush()


def main():
    while True:
        msg = read_message()
        if msg is None:
            return
        method = msg.get("method")
        if "id" not in msg:
            if method == "exit":
                return
            continue
        msg_id = msg["id"]
        if method == "initialize":
            result = {"capabilities": {"completionProvider": {}}}
        elif method == "shutdown":
            result = None
        elif method == "textDocument/completion":
            params = msg.get("params") or {}
            pos = params.get("position") or {"line": 0, "character": 0}
            uri = ((params.get("textDocument") or {}).get("uri")) or ""
            result = [
                {
                    "label": "mockFromVirtualDoc",
                    "detail": "uri=" + str(uri),
                    "textEdit": {
                        "range": {
                            "start": dict(pos),
                            "end": dict(pos),
                        },
                        "newText": "__mocked__",
                    },
                }
            ]
        else:
            result = None
        write_message({"jsonrpc": "2.0", "id": msg_id, "result": result})
        if method == "shutdown":
            # The client sends `exit` immediately after this response.
            continue


if __name__ == "__main__":
    main()
