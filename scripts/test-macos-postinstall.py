#!/usr/bin/env python3
"""Check postinstall actions in the console session without running the installer."""

import os
from pathlib import Path
import subprocess
import unittest


BUILD_SCRIPT = Path(__file__).with_name("build-macos.sh").read_text()
POSTINSTALL = BUILD_SCRIPT.split("cat > \"$PKG_SCRIPTS/postinstall\" << 'SCRIPTEOF'\n", 1)[1].split("\nSCRIPTEOF", 1)[0]
CONSOLE = POSTINSTALL[POSTINSTALL.index('if [[ -z "${CI:-}"'):]
MOCKED_CONSOLE = CONSOLE.replace("/usr/bin/stat", "mock_stat").replace("/usr/bin/id", "mock_id").replace("/bin/launchctl", "mock_launchctl")
MOCKS = """
mock_stat() { printf '%s' "$CONSOLE_UID"; }
mock_id() { printf '%s' "$CONSOLE_USER"; }
mock_launchctl() { printf '%s\\n' "$@"; }
"""


class PostinstallConsoleTest(unittest.TestCase):
    registration = [
        "asuser", "501", "/usr/bin/sudo", "-u", "alice",
        "/Library/Input Methods/KeyTao.app/Contents/MacOS/KeyTaoIME",
        "--register-input-source",
    ]

    def run_console(self, uid="501", user="alice", ci="", volume="/", cli=""):
        env = os.environ | {"CONSOLE_UID": uid, "CONSOLE_USER": user, "CI": ci,
                            "COMMAND_LINE_INSTALL": cli}
        result = subprocess.run(
            ["/bin/bash", "-c", MOCKS + MOCKED_CONSOLE, "postinstall", "pkg", "/", volume],
            env=env, capture_output=True, text=True, check=True,
        )
        self.assertEqual(result.stderr, "")
        return result.stdout.splitlines()

    def test_console_user_registers_before_launching_app(self):
        self.assertEqual(self.run_console(), self.registration + [
            "asuser", "501", "/usr/bin/sudo", "-u", "alice",
            "/usr/bin/open", "-a", "/Applications/KeyTao.app",
        ])

    def test_no_console_user_skips_session_actions(self):
        for uid, user in [("0", "root"), ("", ""), ("invalid", "alice"),
                          ("501", ""), ("248", "_mbsetupuser"), ("501", "loginwindow")]:
            with self.subTest(uid=uid, user=user):
                self.assertEqual(self.run_console(uid, user), [])

    def test_ci_or_other_install_volume_skips_session_actions(self):
        self.assertEqual(self.run_console(ci="true"), [])
        self.assertEqual(self.run_console(volume="/Volumes/Other"), [])

    def test_command_line_or_mdm_install_registers_without_launching_app(self):
        self.assertEqual(self.run_console(cli="1"), self.registration)


if __name__ == "__main__":
    unittest.main()
