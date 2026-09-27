#!/usr/bin/env python3
"""Recompute the Phase 0 surface inventory (Tauri coupling) from the keytao-app sources. Run from the repo root."""
import collections, glob, json, re, sys

cmds, emits, api = {}, [], collections.Counter()
for f in sorted(glob.glob("src-tauri/src/**/*.rs", recursive=True)):
    t = open(f).read()
    for m in re.finditer(r"#\[tauri::command[^\]]*\]\s*(?:#\[[^\]]*\]\s*)*(pub(?:\([^)]*\))?\s+)?(async\s+)?fn\s+(\w+)\s*(<[^>]*>)?\s*\(([^)]*)\)", t, re.S):
        cmds[m.group(3)] = {"file": f, "line": t[: m.start()].count("\n") + 1, "app_handle": "AppHandle" in m.group(5)}
    emits += [f"{f}:{i + 1}" for i, l in enumerate(t.split("\n")) if re.search(r"\.emit(_to)?\(", l)]
    api.update(re.findall(r"\b(resource_dir|app_cache_dir|app_data_dir|package_info|run_mobile_plugin|async_runtime|global_shortcut|get_webview_window)\b", t))
lib = open("src-tauri/src/lib.rs").read()
registered = set()
for g in re.finditer(r"generate_handler!\[(.*?)\]\s*\)", lib, re.S):
    body = re.sub(r"//[^\n]*|#\[[^\]]*\]", "", g.group(1))
    registered |= {n.split("::")[-1] for n in re.findall(r"[\w:]+", body)}
called = collections.Counter()
listens = set()
for f in glob.glob("src/**/*.ts*", recursive=True):
    t = open(f).read()
    # Any quoted occurrence counts, so conditional invokes like invoke(a ? "x" : "y") are not missed.
    literals = set(re.findall(r"[\"'`](\w+)[\"'`]", t))
    called.update(name for name in registered if name in literals)
    listens |= set(re.findall(r"listen(?:<[^>]*>)?\(\s*[\"']([\w:\-/]+)[\"']", t))
out = {
    "defined": len(cmds), "registered": len(registered), "called": len(called),
    "never_called": sorted(registered - set(called)),
    "app_handle_commands": sum(v["app_handle"] for v in cmds.values()),
    "emit_sites": len(emits), "listened_events": sorted(listens), "tauri_api": dict(api),
}
json.dump(out, sys.stdout, ensure_ascii=False, indent=1)
print()
