#!/usr/bin/env python3
"""Small, dependency-free Language Server for lulang."""

from __future__ import annotations

import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
from typing import Any, BinaryIO
from urllib.parse import unquote, urlparse

KEYWORDS = [
    "main", "fn", "export", "extern", "type", "enum", "property", "operator",
    "let", "var", "inout", "return", "if", "else", "for", "in", "while",
    "true", "false", "and", "or", "not",
]
BUILTINS = {
    "arr": "arr(n, initial) -> [T]\n\nCreate an array with value semantics.",
    "len": "len(value) -> i64\n\nReturn an array or string length.",
    "sum": "sum(i in lo..hi) expression\n\nOrder-free numerical reduction.",
    "print": "print(values...)\n\nPrint values separated by spaces.",
    "sqrt": "sqrt(x: f64) -> f64",
    "sin": "sin(x: f64) -> f64",
    "cos": "cos(x: f64) -> f64",
}


def compiler_path() -> str:
    candidates = [
        os.environ.get("LULANG_BIN"),
        shutil.which("lu"),
        Path(__file__).resolve().parents[1] / "target" / "release" / "lu",
    ]
    for candidate in candidates:
        if candidate and Path(candidate).is_file():
            return str(candidate)
    raise RuntimeError("cannot find `lu`; set LULANG_BIN or put it on PATH")


def diagnostics(text: str) -> list[dict[str, Any]]:
    with tempfile.NamedTemporaryFile("w", suffix=".lu", delete=False) as source:
        source.write(text)
        path = source.name
    try:
        result = subprocess.run(
            [compiler_path(), "check", path], text=True, capture_output=True
        )
    finally:
        Path(path).unlink(missing_ok=True)
    if result.returncode == 0:
        return []
    message = (result.stderr or result.stdout).strip()
    return [{
        "range": {
            "start": {"line": 0, "character": 0},
            "end": {"line": max(0, text.count("\n")), "character": 0},
        },
        "severity": 1,
        "source": "lulang",
        "message": message.removeprefix("error: "),
    }]


def format_document(text: str) -> str:
    with tempfile.NamedTemporaryFile("w", suffix=".lu", delete=False) as source:
        source.write(text)
        path = source.name
    try:
        result = subprocess.run(
            [compiler_path(), "fmt", path], text=True, capture_output=True
        )
        if result.returncode:
            raise RuntimeError((result.stderr or result.stdout).strip())
        return Path(path).read_text()
    finally:
        Path(path).unlink(missing_ok=True)


DECLARATION = re.compile(
    r"(?m)^[ \t]*(?:(export|extern)(?:[ \t]+\"[^\"]*\")?[ \t]+)?"
    r"(fn|type|enum|property)[ \t]+([A-Za-z_][A-Za-z0-9_]*)"
)


def document_symbols(text: str) -> list[dict[str, Any]]:
    kinds = {"fn": 12, "property": 12, "type": 23, "enum": 10}
    symbols = []
    for match in DECLARATION.finditer(text):
        line = text.count("\n", 0, match.start())
        character = match.start() - (text.rfind("\n", 0, match.start()) + 1)
        end = character + len(match.group(0).strip())
        symbols.append({
            "name": match.group(3),
            "kind": kinds[match.group(2)],
            "range": {
                "start": {"line": line, "character": character},
                "end": {"line": line, "character": end},
            },
            "selectionRange": {
                "start": {"line": line, "character": character},
                "end": {"line": line, "character": end},
            },
        })
    return symbols


def word_at(text: str, line: int, character: int) -> str:
    lines = text.splitlines()
    if line >= len(lines):
        return ""
    row = lines[line]
    character = min(character, len(row))
    left = character
    right = character
    while left > 0 and (row[left - 1].isalnum() or row[left - 1] == "_"):
        left -= 1
    while right < len(row) and (row[right].isalnum() or row[right] == "_"):
        right += 1
    return row[left:right]


class Server:
    def __init__(self, reader: BinaryIO, writer: BinaryIO):
        self.reader = reader
        self.writer = writer
        self.documents: dict[str, str] = {}
        self.shutdown = False

    def read(self) -> dict[str, Any] | None:
        length = None
        while True:
            line = self.reader.readline()
            if not line:
                return None
            if line in {b"\r\n", b"\n"}:
                break
            name, value = line.decode().split(":", 1)
            if name.lower() == "content-length":
                length = int(value.strip())
        if length is None:
            return None
        return json.loads(self.reader.read(length))

    def send(self, payload: dict[str, Any]) -> None:
        body = json.dumps(payload, separators=(",", ":")).encode()
        self.writer.write(f"Content-Length: {len(body)}\r\n\r\n".encode() + body)
        self.writer.flush()

    def response(self, identifier: Any, result: Any) -> None:
        self.send({"jsonrpc": "2.0", "id": identifier, "result": result})

    def publish(self, uri: str) -> None:
        self.send({
            "jsonrpc": "2.0",
            "method": "textDocument/publishDiagnostics",
            "params": {"uri": uri, "diagnostics": diagnostics(self.documents[uri])},
        })

    def handle(self, message: dict[str, Any]) -> bool:
        method = message.get("method")
        params = message.get("params", {})
        identifier = message.get("id")
        if method == "initialize":
            self.response(identifier, {"capabilities": {
                "textDocumentSync": 1,
                "documentFormattingProvider": True,
                "documentSymbolProvider": True,
                "hoverProvider": True,
                "definitionProvider": True,
                "completionProvider": {"triggerCharacters": []},
            }, "serverInfo": {"name": "lulang-lsp", "version": "0.1.0"}})
        elif method == "shutdown":
            self.shutdown = True
            self.response(identifier, None)
        elif method == "exit":
            return False
        elif method == "textDocument/didOpen":
            document = params["textDocument"]
            self.documents[document["uri"]] = document["text"]
            self.publish(document["uri"])
        elif method == "textDocument/didChange":
            uri = params["textDocument"]["uri"]
            self.documents[uri] = params["contentChanges"][-1]["text"]
            self.publish(uri)
        elif method == "textDocument/didClose":
            uri = params["textDocument"]["uri"]
            self.documents.pop(uri, None)
            self.send({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics",
                       "params": {"uri": uri, "diagnostics": []}})
        elif method == "textDocument/formatting":
            uri = params["textDocument"]["uri"]
            formatted = format_document(self.documents[uri])
            self.response(identifier, [{
                "range": {"start": {"line": 0, "character": 0},
                          "end": {"line": 2**31 - 1, "character": 0}},
                "newText": formatted,
            }])
        elif method == "textDocument/documentSymbol":
            uri = params["textDocument"]["uri"]
            self.response(identifier, document_symbols(self.documents[uri]))
        elif method == "textDocument/completion":
            items = [{"label": item, "kind": 14} for item in KEYWORDS]
            items += [{"label": item, "kind": 3, "detail": BUILTINS[item]}
                      for item in BUILTINS]
            self.response(identifier, {"isIncomplete": False, "items": items})
        elif method == "textDocument/hover":
            uri = params["textDocument"]["uri"]
            position = params["position"]
            word = word_at(self.documents[uri], position["line"], position["character"])
            result = {"contents": {"kind": "markdown", "value": f"```lu\n{BUILTINS[word]}\n```"}} \
                if word in BUILTINS else None
            self.response(identifier, result)
        elif method == "textDocument/definition":
            uri = params["textDocument"]["uri"]
            position = params["position"]
            word = word_at(self.documents[uri], position["line"], position["character"])
            symbol = next((item for item in document_symbols(self.documents[uri])
                           if item["name"] == word), None)
            self.response(identifier, {"uri": uri, "range": symbol["selectionRange"]}
                          if symbol else None)
        elif identifier is not None:
            self.response(identifier, None)
        return True

    def run(self) -> None:
        while True:
            message = self.read()
            if message is None or not self.handle(message):
                return


def main() -> None:
    Server(sys.stdin.buffer, sys.stdout.buffer).run()


if __name__ == "__main__":
    main()
