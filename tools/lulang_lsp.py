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
FUNCTION_DECLARATION = re.compile(
    r"(?m)^[ \t]*(?:export[ \t]+)?fn[ \t]+([A-Za-z_][A-Za-z0-9_]*)"
    r"[ \t]*(\([^)]*\)(?:[ \t]*:[ \t]*[^{\n]+)?)"
)
PROPERTY_DECLARATION = re.compile(
    r"(?m)^[ \t]*property[ \t]+([A-Za-z_][A-Za-z0-9_]*)"
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


def operator_declarations(text: str) -> list[dict[str, Any]]:
    declarations = []
    for line_number, line in enumerate(text.splitlines()):
        stripped = line.lstrip()
        if not stripped.startswith("operator"):
            continue
        rest = stripped[len("operator"):]
        if not rest:
            continue
        if rest[0].isspace():
            body = rest.lstrip()
            if not body or ")" not in body:
                continue
            opening = body[0]
            after_parameter = body.split(")", 1)[1].lstrip()
            if not after_parameter:
                continue
            closing = after_parameter[0]
            glyph = opening
            aliases = {opening, closing}
            display = f"{opening}…{closing}"
            character = line.find(opening, len(line) - len(stripped) + len("operator"))
        else:
            if ")" not in rest:
                continue
            after_parameter = rest.split(")", 1)[1].lstrip()
            match = re.match(r"([^\w\s(]+)", after_parameter)
            if not match:
                continue
            glyph = match.group(1)
            aliases = {glyph}
            display = glyph
            character = line.find(glyph, len(line) - len(stripped) + len("operator"))
        declarations.append({
            "glyph": glyph,
            "aliases": aliases,
            "display": display,
            "signature": stripped.split("{", 1)[0].strip(),
            "range": {
                "start": {"line": line_number, "character": character},
                "end": {"line": line_number, "character": character + len(glyph)},
            },
        })
    return declarations


def operator_at(text: str, line: int, character: int) -> dict[str, Any] | None:
    rows = text.splitlines()
    if line >= len(rows):
        return None
    row = rows[line]
    for declaration in operator_declarations(text):
        for alias in declaration["aliases"]:
            start = 0
            while True:
                start = row.find(alias, start)
                if start < 0:
                    break
                if start <= character < start + len(alias):
                    return declaration
                start += len(alias)
    return None


def property_lenses(text: str, uri: str) -> list[dict[str, Any]]:
    lenses = []
    for match in PROPERTY_DECLARATION.finditer(text):
        line = text.count("\n", 0, match.start())
        character = match.start(1) - (text.rfind("\n", 0, match.start(1)) + 1)
        source_range = {
            "start": {"line": line, "character": character},
            "end": {"line": line, "character": character + len(match.group(1))},
        }
        lenses.append({
            "range": source_range,
            "command": {
                "title": "Run property (100 cases)",
                "command": "lulang.runProperty",
                "arguments": [uri, match.group(1), source_range],
            },
        })
    return lenses


def run_property(text: str, name: str, runs: int = 100) -> tuple[bool, str]:
    with tempfile.NamedTemporaryFile("w", suffix=".lu", delete=False) as source:
        source.write(text)
        path = source.name
    try:
        result = subprocess.run(
            [
                compiler_path(), "test", "--runs", str(runs),
                "--property", name, path,
            ],
            text=True,
            capture_output=True,
        )
    finally:
        Path(path).unlink(missing_ok=True)
    output = "\n".join(
        part.strip() for part in [result.stdout, result.stderr] if part.strip()
    )
    return result.returncode == 0, output


def declaration_signature(text: str, name: str) -> str | None:
    for match in FUNCTION_DECLARATION.finditer(text):
        if match.group(1) == name:
            return f"fn {name}{match.group(2).strip()}"
    return None


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
                "codeLensProvider": {"resolveProvider": False},
                "completionProvider": {"triggerCharacters": []},
                "executeCommandProvider": {"commands": ["lulang.runProperty"]},
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
            uri = params["textDocument"]["uri"]
            items += [
                {"label": match.group(1), "kind": 3,
                 "detail": declaration_signature(self.documents[uri], match.group(1))}
                for match in FUNCTION_DECLARATION.finditer(self.documents[uri])
            ]
            items += [
                {"label": "dot → ·", "kind": 24, "insertText": "·"},
                {"label": "approx → ≈", "kind": 24, "insertText": "≈"},
                {"label": "norm → ‖…‖", "kind": 24,
                 "insertText": "‖${1:value}‖", "insertTextFormat": 2},
            ]
            self.response(identifier, {"isIncomplete": False, "items": items})
        elif method == "textDocument/hover":
            uri = params["textDocument"]["uri"]
            position = params["position"]
            word = word_at(self.documents[uri], position["line"], position["character"])
            operator = operator_at(
                self.documents[uri], position["line"], position["character"]
            )
            detail = BUILTINS.get(word) or declaration_signature(
                self.documents[uri], word
            )
            if operator:
                detail = operator["signature"]
            result = (
                {"contents": {"kind": "markdown", "value": f"```lu\n{detail}\n```"}}
                if detail else None
            )
            self.response(identifier, result)
        elif method == "textDocument/definition":
            uri = params["textDocument"]["uri"]
            position = params["position"]
            word = word_at(self.documents[uri], position["line"], position["character"])
            symbol = next((item for item in document_symbols(self.documents[uri])
                           if item["name"] == word), None)
            operator = operator_at(
                self.documents[uri], position["line"], position["character"]
            )
            target = symbol["selectionRange"] if symbol else (
                operator["range"] if operator else None
            )
            self.response(identifier, {"uri": uri, "range": target} if target else None)
        elif method == "textDocument/codeLens":
            uri = params["textDocument"]["uri"]
            self.response(identifier, property_lenses(self.documents[uri], uri))
        elif method == "workspace/executeCommand":
            if params.get("command") != "lulang.runProperty":
                self.response(identifier, None)
            else:
                uri, name, source_range = params.get("arguments", [None, None, None])
                if uri not in self.documents or not name:
                    self.response(identifier, None)
                else:
                    success, output = run_property(self.documents[uri], name)
                    if not success:
                        self.send({
                            "jsonrpc": "2.0",
                            "method": "textDocument/publishDiagnostics",
                            "params": {
                                "uri": uri,
                                "diagnostics": [{
                                    "range": source_range,
                                    "severity": 1,
                                    "source": "lulang property",
                                    "message": output,
                                }],
                            },
                        })
                    self.send({
                        "jsonrpc": "2.0",
                        "method": "window/showMessage",
                        "params": {"type": 3 if success else 1, "message": output},
                    })
                    self.response(identifier, output)
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
