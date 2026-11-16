#!/usr/bin/env python3
"""Generate the shared Chinese emoji index from pinned Unicode CLDR data."""

import argparse
import json
from pathlib import Path
import re
import urllib.request


ROOT = Path(__file__).resolve().parents[1]
REVISION = "7d2b9cb11cec08f4d333ad73209deb3df0711c3b"  # CLDR 49.0.0
BASE_URL = f"https://raw.githubusercontent.com/unicode-org/cldr-json/{REVISION}/"
SOURCES = {
    "annotations.json": "cldr-json/cldr-annotations-full/annotations/zh/annotations.json",
    "annotationsDerived.json": "cldr-json/cldr-annotations-derived-full/annotationsDerived/zh/annotations.json",
    "LICENSE": "LICENSE",
}
RESOURCES = ROOT / "crates/keytao-theme/resources"


def offered_emoji():
    values = []
    active = False
    for line in (ROOT / "crates/keytao-theme/default-keyboard.yaml").read_text().splitlines():
        layer = re.match(r"^  ([A-Za-z0-9_]+):", line)
        if layer:
            active = layer[1].startswith("symbols_emoji_")
        if active and "value:" in line and "action:" not in line:
            # Shared emoji keys use JSON-compatible quoted YAML scalars.
            value = re.search(r'value:\s*("(?:[^"\\]|\\.)*")', line)
            if value is None:
                raise ValueError(f"Unsupported emoji key: {line}")
            values.append(json.loads(value[1]))
    if not values:
        raise ValueError("No keyboard emoji found")
    return list(dict.fromkeys(values))


def generate(sources):
    annotations = {}
    for filename, section in (("annotations.json", "annotations"),
                              ("annotationsDerived.json", "annotationsDerived")):
        annotations.update(json.loads(sources[filename])[section]["annotations"])
    result = {}
    for emoji in offered_emoji():
        # CLDR omits emoji presentation selectors in annotation keys.
        entry = annotations.get(emoji.replace("\ufe0f", ""))
        if not entry or not entry.get("tts"):
            raise ValueError(f"Missing CLDR Chinese annotation: {emoji!r}")
        result[emoji] = list(dict.fromkeys(entry["tts"] + entry.get("default", [])))
    assert all(words and all(isinstance(word, str) and word for word in words)
               for words in result.values())
    return (json.dumps(result, ensure_ascii=False, separators=(",", ":")) + "\n").encode()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="Verify generated resources without writing")
    parser.add_argument("--source-dir", type=Path, help="Read previously downloaded CLDR source files")
    args = parser.parse_args()
    sources = {}
    for filename, path in SOURCES.items():
        if args.source_dir:
            sources[filename] = (args.source_dir / filename).read_bytes()
        else:
            with urllib.request.urlopen(BASE_URL + path, timeout=30) as response:
                sources[filename] = response.read()
    if b"UNICODE LICENSE V3" not in sources["LICENSE"]:
        raise ValueError("Expected Unicode License v3")
    output = {
        "emoji-keywords-zh.json": generate(sources),
        "LICENSE-UNICODE.txt": sources["LICENSE"],
    }
    for filename, content in output.items():
        path = RESOURCES / filename
        if args.check:
            if path.read_bytes() != content:
                raise SystemExit(f"Outdated resource: {path}")
        else:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(content)
    print(f"CLDR 49.0.0: {len(json.loads(output['emoji-keywords-zh.json']))} emoji, "
          f"{len(output['emoji-keywords-zh.json'])} bytes; "
          f"{'verified' if args.check else 'generated'}")


if __name__ == "__main__":
    main()
