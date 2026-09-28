#!/usr/bin/env python3
"""Exercise the real CMake install rules with offline, non-executable fixtures.

This is not a Windows compiler, PowerShell parser, NSIS compiler, or TSF test.
All generated files and logs live in a unique temporary directory.
"""

import pathlib
import re
import shutil
import subprocess
import tempfile

REPO = pathlib.Path(__file__).resolve().parents[2]
CMAKE = shutil.which("cmake") or "/opt/local/bin/cmake"


def run(args, log, expect_success=True):
    result = subprocess.run(args, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    log.write_text(result.stdout)
    if (result.returncode == 0) != expect_success:
        raise AssertionError(f"Unexpected exit {result.returncode}: {args}\n{result.stdout}")
    print(f"{log.stem}: exit {result.returncode} (expected)")


def main():
    work = pathlib.Path(tempfile.mkdtemp(prefix="keytao-b4w-static-"))
    root = work / "fixture"
    windows = root / "flutter_app/windows"
    shutil.copytree(REPO / "flutter_app/windows", windows, ignore=shutil.ignore_patterns("ephemeral"))
    shutil.copy(REPO / "Cargo.toml", root / "Cargo.toml")
    engine = root / "fixture-engine"
    engine.mkdir()
    for name in ["flutter_windows.dll", "icudtl.dat", "app.so"]:
        (engine / name).write_text(f"fixture {name}")
    (engine / "flutter_assets").mkdir()
    (engine / "flutter_assets/AssetManifest.bin").write_text("fixture assets")
    (engine / "native_assets/windows").mkdir(parents=True)
    (engine / "native_assets/windows/keytao_app_bridge.dll").write_text("fixture bridge")
    (windows / "flutter/CMakeLists.txt").write_text('''
add_library(flutter INTERFACE)
add_library(flutter_wrapper_app INTERFACE)
add_custom_target(flutter_assemble)
set(FLUTTER_LIBRARY "${KEYTAO_ROOT}/fixture-engine/flutter_windows.dll" PARENT_SCOPE)
set(FLUTTER_ICU_DATA_FILE "${KEYTAO_ROOT}/fixture-engine/icudtl.dat" PARENT_SCOPE)
set(AOT_LIBRARY "${KEYTAO_ROOT}/fixture-engine/app.so" PARENT_SCOPE)
set(PROJECT_BUILD_DIR "${KEYTAO_ROOT}/fixture-engine/" PARENT_SCOPE)
''')
    (windows / "flutter/generated_plugins.cmake").write_text("")
    (root / "resources/addon-schemas/english").mkdir(parents=True)
    (root / "resources/addon-schemas/english/english.schema.yaml").write_text("fixture addon")
    staging = root / "target/keytao-windows-ime-runtime"
    for arch in ["current", "x86", "arm64x"]:
        runtime = staging / arch
        (runtime / "rime-data").mkdir(parents=True)
        for name in ["keytao_windows_ime.dll", "rime.dll", "default-theme.yaml", "rime-data/default.yaml"]:
            (runtime / name).write_text(f"fixture {arch}/{name}")
    app_runtime = root / "target/keytao-windows-app-runtime"
    app_runtime.mkdir()
    for name in ["rime.dll", "msvcp140.dll", "vcruntime140.dll", "vcruntime140_1.dll"]:
        (app_runtime / name).write_text(f"fixture {name}")
    build = work / "build"
    install = work / "install"
    configure = [CMAKE, "-S", str(windows), "-B", str(build), "-DCMAKE_BUILD_TYPE=Release", f"-DCMAKE_INSTALL_PREFIX={install}"]
    run(configure, work / "configure.log")
    version = (build / "version.h").read_text()
    expected = re.search(r'\[workspace.package\]\s*version = "([^"]+)"', (root / "Cargo.toml").read_text())[1]
    numeric = re.match(r"(\d+)\.(\d+)\.(\d+)", expected).groups()
    assert f'#define KEYTAO_VERSION "{expected}"' in version, version
    assert f"KEYTAO_VERSION_NUMBER {','.join(numeric)},0" in version, version
    # No Windows sources are compiled; the install target receives a dummy file.
    (build / "runner/keytao-app").write_text("fixture executable")
    install_command = [CMAKE, "--install", str(build), "--config", "Release"]
    run(install_command, work / "install.log")
    for file in ["keytao-app", "rime.dll", "msvcp140.dll", "vcruntime140.dll", "vcruntime140_1.dll",
                 "keytao_app_bridge.dll", "data/flutter_assets/AssetManifest.bin", "data/app.so",
                 "rime-data/default.yaml", "addon-schemas/english/english.schema.yaml"]:
        assert (install / file).is_file(), file
    for arch in ["current", "x86", "arm64x"]:
        installed = install / f"keytao-windows-ime-runtime/{arch}/rime-data/default.yaml"
        assert installed.read_bytes() == (staging / arch / "rime-data/default.yaml").read_bytes()
    # Incomplete staging must fail the real install rules, even after a good run.
    shutil.rmtree(staging / "x86")
    run(install_command, work / "missing-x86.log", expect_success=False)
    (root / "Cargo.toml").write_text('[workspace.package]\nname = "fixture"\n')
    run(configure, work / "missing-version.log", expect_success=False)
    print("PASS: workspace version, installed resources/dependencies, missing staging/version failures")
    print(f"Evidence: {work}")


if __name__ == "__main__":
    main()
