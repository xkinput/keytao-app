# Third-party notices

## Unicode CLDR Chinese emoji annotations

`crates/keytao-theme/resources/emoji-keywords-zh.json` is derived from the
Chinese (`zh`) annotations and derived annotations in Unicode CLDR 49.0.0,
restricted to the emoji offered in `crates/keytao-theme/default-keyboard.yaml`.
Keywords include the CLDR text-to-speech names; no additional keywords are added.

Source: [unicode-org/cldr-json, revision 7d2b9cb11cec08f4d333ad73209deb3df0711c3b](https://github.com/unicode-org/cldr-json/tree/7d2b9cb11cec08f4d333ad73209deb3df0711c3b).
Source files: `cldr-json/cldr-annotations-full/annotations/zh/annotations.json`
and `cldr-json/cldr-annotations-derived-full/annotationsDerived/zh/annotations.json`.
Regenerate with `python3 scripts/generate-emoji-keywords.py`; verify with `--check`.

Copyright © 2015-2024 Unicode, Inc. Licensed under the Unicode License v3
(`Unicode-3.0`). The complete upstream copyright and permission notice is in
[`LICENSE-UNICODE.txt`](crates/keytao-theme/resources/LICENSE-UNICODE.txt), which is
bundled with the index in the Android assets and iOS keyboard extension.
