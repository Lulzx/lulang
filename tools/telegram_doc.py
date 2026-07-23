#!/usr/bin/env python3
"""Fetch and parse Telegram's Bot API HTML into generator-friendly JSON."""

from __future__ import annotations

import argparse
import html
import json
import re
import urllib.request
from html.parser import HTMLParser
from pathlib import Path

BOT_API_URL = "https://core.telegram.org/bots/api"


class _Text(HTMLParser):
    def __init__(self) -> None:
        super().__init__()
        self.parts: list[str] = []

    def handle_data(self, data: str) -> None:
        self.parts.append(data)

    def value(self) -> str:
        return re.sub(r"\s+", " ", "".join(self.parts)).strip()


def text(fragment: str) -> str:
    parser = _Text()
    parser.feed(fragment)
    return html.unescape(parser.value())


def first(fragment: str, tag: str) -> str:
    match = re.search(rf"<{tag}\b[^>]*>(.*?)</{tag}>", fragment, re.I | re.S)
    return text(match.group(1)) if match else ""


def rows(fragment: str) -> tuple[list[str], list[list[str]]]:
    table = re.search(r"<table\b[^>]*>(.*?)</table>", fragment, re.I | re.S)
    if not table:
        return [], []
    header = [text(cell) for cell in re.findall(r"<th\b[^>]*>(.*?)</th>", table.group(1), re.I | re.S)]
    body: list[list[str]] = []
    for row in re.findall(r"<tr\b[^>]*>(.*?)</tr>", table.group(1), re.I | re.S):
        cells = [text(cell) for cell in re.findall(r"<td\b[^>]*>(.*?)</td>", row, re.I | re.S)]
        if cells:
            body.append(cells)
    return header, body


def result_type(description: str) -> str:
    patterns = [
        r"Returns an (Array of [A-Za-z0-9_]+)",
        r"returns an (Array of [A-Za-z0-9_]+)",
        r"On success, an? ([A-Za-z0-9_]+) object is returned",
        r"On success, an? (Array of [A-Za-z0-9_]+)(?: objects?)?(?: (?:of|that) .*?)? is returned",
        r"On success, (?:the )?sent ([A-Za-z0-9_]+) is returned",
        r"On success, (?:the )?([A-Za-z0-9_]+) is returned",
        r"On success, (?:the )?.*? ([A-Z][A-Za-z0-9_]+) is returned",
        r"Returns an? ([A-Za-z0-9_]+) object",
        r"returns an? ([A-Za-z0-9_]+) object",
        r"(?:in (?:the )?form of|as) an? ([A-Za-z0-9_]+) object",
        r"Returns ([A-Za-z0-9_]+) on success",
        r"returns ([A-Za-z0-9_]+) on success",
        r"Returns (?:the )?.*?([A-Z][A-Za-z0-9_]+) on success",
        r"Returns the ([A-Za-z0-9_]+) of ",
        r"Returns .*? as an? ([A-Za-z0-9_]+) object",
        r"Returns .*? as ([A-Za-z0-9_]+) object",
        r"Returns .*? as ([A-Za-z0-9_]+) on success",
    ]
    found: list[str] = []
    for pattern in patterns:
        found.extend(re.findall(pattern, description))
        if found:
            break
    if found:
        normalized = ["Boolean" if value == "True" else value for value in found]
        return " or ".join(dict.fromkeys(normalized))
    if "True" in description or "true" in description:
        return "Boolean"
    raise ValueError(f"cannot determine method result type: {description}")


def parse(source: str) -> dict[str, object]:
    content_match = re.search(
        r'<div id="dev_page_content"[^>]*>(.*?)(?:<div class="dev_page_bottom"|</body>)',
        source,
        re.I | re.S,
    )
    content = content_match.group(1) if content_match else source
    headings = list(re.finditer(r"<h4\b[^>]*>(.*?)</h4>", content, re.I | re.S))
    types: list[dict[str, object]] = []
    unions: list[dict[str, object]] = []
    methods: list[dict[str, object]] = []

    for index, heading in enumerate(headings):
        name = text(heading.group(1))
        if not name or " " in name:
            continue
        end = headings[index + 1].start() if index + 1 < len(headings) else len(content)
        block = content[heading.end() : end]
        description = first(block, "p")
        header, body = rows(block)

        if header[:3] == ["Field", "Type", "Description"]:
            fields = []
            for cells in body:
                if len(cells) != 3:
                    raise ValueError(f"{name}: expected 3 field cells, got {cells}")
                field_name, field_type, field_description = cells
                fields.append(
                    {
                        "name": field_name,
                        "type": field_type,
                        "description": field_description,
                        "optional": field_description.startswith("Optional"),
                    }
                )
            types.append({"name": name, "description": description, "fields": fields})
        elif header[:4] == ["Parameter", "Type", "Required", "Description"]:
            parameters = []
            for cells in body:
                if len(cells) != 4:
                    raise ValueError(f"{name}: expected 4 parameter cells, got {cells}")
                parameter_name, parameter_type, required, parameter_description = cells
                parameters.append(
                    {
                        "name": parameter_name,
                        "type": parameter_type,
                        "description": parameter_description,
                        "required": required == "Yes",
                    }
                )
            methods.append(
                {
                    "name": name,
                    "description": description,
                    "parameters": parameters,
                    "result_type": result_type(description),
                }
            )
        elif name[0].islower():
            methods.append(
                {
                    "name": name,
                    "description": description,
                    "parameters": [],
                    "result_type": result_type(description),
                }
            )
        elif re.search(r"<ul\b", block, re.I):
            variants = [
                text(item)
                for item in re.findall(r"<li\b[^>]*>(.*?)</li>", block, re.I | re.S)
            ]
            variants = [value for value in variants if value and " " not in value]
            if variants:
                unions.append({"name": name, "description": description, "types": variants})
        else:
            types.append({"name": name, "description": description, "fields": []})

    if len(types) < 100 or len(methods) < 100:
        raise ValueError(
            f"document parse looks incomplete: {len(types)} types, "
            f"{len(unions)} unions, {len(methods)} methods"
        )
    version_match = re.search(r"<strong>Bot API ([0-9.]+)</strong>", content, re.I)
    release_match = re.search(
        r'<h4\b[^>]*>.*?</a>([A-Z][a-z]+ \d{1,2}, \d{4})</h4>\s*'
        r"<p><strong>Bot API [0-9.]+</strong>",
        content,
        re.I | re.S,
    )
    return {
        "source": BOT_API_URL,
        "version": version_match.group(1) if version_match else "unknown",
        "released": release_match.group(1) if release_match else "unknown",
        "methods": methods,
        "types": types,
        "union_types": unions,
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("output", type=Path)
    parser.add_argument("--html", type=Path, help="parse a saved page instead of fetching")
    args = parser.parse_args()
    if args.html:
        source = args.html.read_text(encoding="utf-8")
    else:
        request = urllib.request.Request(BOT_API_URL, headers={"User-Agent": "lutelegram-docgen/0.1"})
        with urllib.request.urlopen(request) as response:
            source = response.read().decode("utf-8")
    document = parse(source)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(document, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(
        f"wrote {len(document['types'])} types, {len(document['union_types'])} unions, "
        f"and {len(document['methods'])} methods to {args.output}"
    )


if __name__ == "__main__":
    main()
