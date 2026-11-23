#!/usr/bin/env python3
"""Check both installer launch paths and compile the real installer offline."""

import pathlib
import subprocess
import sys
import tempfile


repo = pathlib.Path(__file__).resolve().parents[3]
windows = repo / "packaging/windows"
source = (windows / "keytao.nsi").read_text(encoding="utf-8")
launch = (windows / "unelevated-launch.nsh").read_text(encoding="utf-8")
assert '!define MUI_FINISHPAGE_RUN_FUNCTION LaunchKeyTaoAsUser' in source
assert 'MUI_FINISHPAGE_RUN_NOTCHECKED' not in source
assert 'LangString KeyTaoRunText ${LANG_SIMPCHINESE} "运行 KeyTao"' in source
assert '${If} ${Silent}\n  ${AndIf} $RestartAfterUpgrade != 1\n    Return' in launch
init = source.split('Function .onInit\n', 1)[1].split('FunctionEnd', 1)[0]
assert init.index('${If} ${Silent}') < init.index('"/KEYTAO_RESTART"')
assert init.index('"MainBinaryName"') < init.index('StrCpy $RestartAfterUpgrade 1')
success = source.split('Function .onInstSuccess\n', 1)[1].split('FunctionEnd', 1)[0]
assert '${If} $RestartAfterUpgrade == 1' in success
assert success.index('${If} $0 == "registered"') < success.index('Call LaunchKeyTaoAsUser')
assert 'GetTokenInformation(p r2, i 20' in launch
assert 'StrCmp $R1 0 done\n  StrCmp $0 0 0 done' in launch
assert 'CreateEnvironmentBlock(*p .r8, p r3, i 0)' in launch
assert 'i 0x400, p r8' in launch
assert 'DestroyEnvironmentBlock(p r8)' in launch
assert launch.count('CreateProcessWithTokenW(') == 1
assert 'ExecShell' not in launch and 'explorer.exe' not in launch
assert 'rR' not in launch
assert '!if ${NSIS_PTR_SIZE} != 4' in launch

with tempfile.TemporaryDirectory(prefix="keytao-unelevated-nsis-") as directory:
    work = pathlib.Path(directory)
    bundle = work / "bundle"
    bundle.mkdir()
    (bundle / "keytao-app.exe").write_text("non-executable fixture", encoding="utf-8")
    helper = work / "setup-helper.exe"
    helper.write_text("non-executable fixture", encoding="utf-8")
    install = work / "install.nsh"
    install.write_text('!insertmacro InstallPayloadFile "keytao-app.exe"\n', encoding="utf-8")
    uninstall = work / "uninstall.nsh"
    uninstall.write_text('Delete "$INSTDIR\\keytao-app.exe"\n', encoding="utf-8")
    values = {
        "VERSION": "1.0.0",
        "VERSION_NUMERIC": "1.0.0.0",
        "REPO_ROOT": repo,
        "BUNDLE_DIR": bundle,
        "INSTALL_FILES": install,
        "UNINSTALL_FILES": uninstall,
        "SETUP_HELPER": helper,
        "SETUP_HELPER_ARM64": helper,
        "OUTPUT_FILE": work / "keytao-launch-fixture.exe",
    }
    subprocess.run(
        [sys.argv[1] if len(sys.argv) > 1 else "makensis", "-INPUTCHARSET", "UTF8", "-V2"]
        + [f"-D{key}={value}" for key, value in values.items()]
        + [str(windows / "keytao.nsi")],
        cwd=windows,
        check=True,
    )
    assert (work / "keytao-launch-fixture.exe").is_file()
print("PASS: checked finish checkbox, opt-in upgrade, fail-closed launch and real NSIS compilation")
