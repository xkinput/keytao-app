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

## Downloadable Snow Jiandao and LSSP Triple Pinyin

Snow Jiandao (`snow_jiandao`) is downloaded from
[rimeinn/rime-snow-pinyin](https://github.com/rimeinn/rime-snow-pinyin), licensed
under GPL-3.0. KeyTao installs its required Snow schemas, dictionaries and
`lua/snow` modules, namespaces radical lookup as `snow_radical_pinyin`, renames
the upstream user-pack seed as `snow_pinyin.user_pack` to keep it separate from
protected user dictionaries, and
registers only Snow Jiandao. Other layouts and host settings are omitted.

LSSP Triple Pinyin (`triple_pinyin_lssp`) is downloaded from
[arsenali/rime-triple-pinyin-lssp](https://github.com/arsenali/rime-triple-pinyin-lssp),
licensed under LGPL-3.0. KeyTao installs the primary layout and namespaced Lua
modules, omitting host settings, the optional language-model patch, zianyin table
and Pinyin reverse lookup. Its fourteen dictionary tables are package-owned
copies in `lssp-dicts` from the compatible
[Wanxiang v17.0.0 release](https://github.com/amzxyz/rime-wanxiang/releases/tag/v17.0.0),
licensed under [CC BY 4.0](https://github.com/amzxyz/rime-wanxiang/blob/v17.0.0/LICENSE).
Its stroke lookup is a package-owned copy from
[rime/rime-stroke](https://github.com/rime/rime-stroke), licensed under LGPL-3.0,
renamed to `lssp_stroke`. Punctuation uses the runtime's default preset, and the
vocabulary reference uses the runtime's bundled `essay` data.

These packages are downloaded at runtime and are not bundled with KeyTao. Their
installed notices retain source and license attribution. Uninstall preserves
user customizations and learned dictionaries, including Snow's user database.

## libjd-engine

KeyTao bundles the engine library from [hronro/ime-jd](https://github.com/hronro/ime-jd),
pinned to `v0.14.0` and licensed under GPL-3.0, on macOS, Windows, Linux, and
Android, and statically links it on iOS. Each release package includes the full
license text in `libjd-engine-LICENSE.txt` and the corresponding source location
in `libjd-engine-SOURCE.txt`.
