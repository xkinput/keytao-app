# Codex handoff (2026-11-21)

Work in progress handed from the Claude session to Codex. Read this file first, then the two design files next to it.

- `package-isolation-design.md`: per-package isolation design, the adversarial critique, the owner's decisions and the notes added after each merged batch. **This is the contract for batches B3–B6.**
- `ime-jd-research.md`: research and design for the optional libjd (hronro/ime-jd) engine. Blocked on owner answers (see below).

## Ground rules

- Reply to the owner in Chinese, concise. Code, identifiers and comments in English.
- Never push, tag or release without the owner's explicit OK. Production deploys and schema/data-layout changes need the owner's OK too.
- Never read, copy or print `app-state.json` or auth tokens. Never print `.env` values.
- Never touch `~/Library/keytao`, `~/Library/Rime` or other live IME data unless the owner asks; experiments go in a temp dir.
- Never enter passwords; the owner handles sudo, Apple ID and credentials.
- Shell:
  - Never use the `test` builtin or `[ ]`; use `[[ ]]` or `case`.
  - `ls` is aliased to eza, so use `find`.
- Flutter: use the system/nix Flutter only, always with `--no-pub`.
- Scope discipline: build only what is asked. Add no explanatory fine print to the UI. UI copy is Chinese and short.
- Tests only for decision rules, data integrity, concurrency and external contracts. Every new test must be seen failing once with the defect re-injected.
- Hot files take one batch at a time: `crates/keytao-core/src/lib.rs`, `crates/keytao-app-core/src/packages.rs`.
- Shift / Caps Lock `switch_key` behavior stays exactly as configured. The owner rejected changing it, so do not revisit.

## Acceptance commands

```sh
V=$PWD/vendor/librime/macos-universal
export RIME_INCLUDE_DIR=$V/include RIME_LIB_DIR=$V/lib DYLD_FALLBACK_LIBRARY_PATH=$V/lib
cargo test -q --no-fail-fast -p keytao-theme -p keytao-core -p keytao-core-ffi -p keytao-app-core
scripts/check-core-cross.sh
nix shell nixpkgs#zig nixpkgs#cargo-zigbuild -c cargo-zigbuild check -p keytao-windows-ime -p keytao-windows-ime-server --all-targets --target x86_64-pc-windows-gnu
cargo test -q -p keytao-windows-ime --test composition_rules --test latency_rules --test engine_host
crates/keytao-macos-ime/build.sh --debug --skip-pkg   # if Swift changed
(cd flutter_app && flutter analyze --no-pub lib test && flutter test --no-pub)
(cd flutter_app && JAVA_HOME=/nix/store/al24k44gwd1rk08p1x2p4r88ba9xw132-zulu-ca-jdk-17.0.19/Library/Java/JavaVirtualMachines/zulu-17.jdk/Contents/Home \
  flutter build apk --debug --no-pub --target-platform android-arm64 && rm -rf build)   # if Kotlin changed
```

Known noise:

- `flutter test` has 2 pre-existing `controller_parity_test.dart` failures.
- Two keytao-core runtime tests occasionally fail on temp-dir cleanup under load; they pass on rerun.
- The `cfg(windows)` TSF tests (`commit_retry_tests`, `shortcut_passthrough_tests`) only run on Windows CI. That is the "IME regressions" workflow, which runs on every push to main.

## Repository state

- `main` is 9 commits ahead of `origin/main` and **not pushed**. The last release is `v1.2.15-alpha.6`.
- Unpushed, user-visible changes:
  - Windows font list enabled, and the chosen family is used by the Windows panel.
  - Theme editor groups keep their expansion state while dragging.
  - Windows: the app process never shares librime with an in-process IME engine (fixes "librime is initialized for another user directory").
  - Scheme cards show 使用中/使用 instead of 部署; only the deploying package shows 部署中.
  - Per-side candidate panel padding.
  - Cross-system font family resolution (`crates/keytao-theme/src/font_resolution.rs`).
- Unpushed, no user-visible change:
  - Isolation B0 (`crates/keytao-core/src/runtime_dirs.rs`).
  - Isolation B1 (`crates/keytao-app-core/src/runtime_materialize.rs`, flag `KEYTAO_RUNTIME_ISOLATION` / `runtime_isolation`, default off).
- **Isolation B2 may be in progress** in `.worktrees/iso` on branch `feat/per-package-deploy`. Check `git -C .worktrees/iso status` first.
  - If the worktree exists with uncommitted changes, review them against the B2 spec below before continuing.
  - Do not start B3 until B2 is merged.
- Release procedure, run only when the owner says so:
  1. Bump `Cargo.toml` line 18 and the `flutter_app/pubspec.yaml` version (keep the `+1002015` build number).
  2. `cargo update -q --workspace --offline`.
  3. Commit `chore(release): prepare X`.
  4. Create the annotated tag `vX`.
  5. Push `main`, then the tag. A tag with `-` is a prerelease.
  6. CI pins nix on `main`; pull afterwards.

## Next work, in order

### 1. Isolation B2: per-package deploy (in progress, see above)

- With the flag on:
  - Deploy every stale package in order, with the active package first.
  - Skip a package whose receipt matches, and make no maintenance call for it.
  - Failure isolation: a failed package keeps its last good build, and other builds stay unchanged (md5).
- Structured progress `{package_key, package_name, index, total, step}` goes through the bridge to Flutter.
- Scheme cards show 部署中 / 等待部署 in order. The deploy text reads `正在部署 <name>（i/n）`, and the IME hints use the same string.
- iOS stays on full deploy but emits structured progress.

### 2. Isolation B3–B6, per `package-isolation-design.md` section 6, as amended by the critique

- **Before the flag can ever be on**, fix the two items noted at the end of the design file:
  1. Orphan listing must not treat legacy flat `build/*` as orphans while the flag is on (`package_orphans.rs:122`).
  2. `RuntimeDirs::resolve` must fail closed and log when a package's files dir is missing (`runtime_dirs.rs:37`).
- **B3, switching:**
  - The reload stamp carries the package key.
  - Every IME re-initializes librime with the new package's dirs.
  - Windows staged swap applies to all packages (critique #3).
  - The AppContainer cache is keyed per package.
  - An IME may only switch to a package that is already built (critique #9).
- **B4, migration** (needs the owner's OK before it runs on real data):
  - Run `package_activation::recover` first.
  - Pick the active package deterministically.
  - Keep the legacy flat root untouched, so rollback is possible.
  - Copy user-owned custom files into every package (owner decision D4).
  - Userdb is per package (D1). Never copy a live leveldb.
  - Never touch the user's sync dir (D5).
  - List orphans; never auto-delete them.
- **B5, UI:**
  - App home: two-level selector, package then its schema_list.
  - F4 first level inside the switcher: lists packages as display name + folder name (D2).
    - librime's Switcher cannot inject candidates. Keep the menu state in keytao-core (a new `switcher_model.rs`) and have every frontend render it.
    - Spike this first on macOS.
  - Add-ons install and switch like main packages (D3). The exception is Easy English, which is a per-package overlay.
- **B6:** remove the copy-into-shared-dir activation path and the cross-package `.keytao-package-lua` snapshots.
- Not yet covered by any batch (critique #6):
  - `keytao.user.dict.yaml` and account sync (`account.rs`).
  - `rime_sync.rs`.
  - `upload_bundle.rs` root checks.

### 3. ime-jd engine (blocked on the owner)

The owner's request is to add 切换到 ime-jd 引擎 under the 键道6 card. Clicking it downloads the matching libjd release and uses it as the input engine behind the current UI, with 顶功 and the rest unchanged. The proposed plan is in `ime-jd-research.md`.

Open questions for the owner, all unanswered:

1. libjd is GPL-3.0. Accept GPL obligations for distributed builds, or ask hronro for an exception?
2. Pin the tag and sha256 per KeyTao release (recommended), or track GitHub latest?
3. Which platforms first? Proposed: macOS, then Linux, then Windows (pipe mode only). iOS and Android Play builds are out.
4. Feed KeyTao custom words into libjd? It has a 10,000-entry cap.
5. Show the libjd dictionary version in the UI?

Hot files overlap with isolation (`lib.rs`), so engine batch J0 (`ImeBackend` trait refactor) starts after B3.

## Known follow-ups (not blocking)

- Windows (from review):
  - The first candidate render scans system fonts once per process, twice when a theme font is set (`font_resolution.rs:107`, Windows `panel.rs`).
  - An extra 25–50 ms key hold when QQ refuses a sync session.
  - A capped end retry may leave one raw letter after 5 refusals.
- macOS: a custom family with bold may render Regular (`ImeTheme.swift:379`; this predates the change).
- Font equivalence only maps the first family in a comma list (`font_resolution.rs:85`).
- Skipped-schema warning is in memory only (`controller.dart`). Conflict errors name the other package by key, not display name.
- Android `KeytaoTheme.kt:40` uses `@RestrictTo` font APIs, wrapped in `runCatching`.
- Owner to verify on real devices:
  - Windows QQ: 顶功, focus switch, left/right Shift, Shift+letter, font list.
  - Android keyboard chips/tiles.
  - Scheme conflict prompts.
  - Theme dialog: padding, fonts.
