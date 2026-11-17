#!/usr/bin/env python3
"""Check the generated postinstall launch without running installer actions."""

import os
from pathlib import Path
import subprocess
import unittest


BUILD_SCRIPT = Path(__file__).with_name("build-macos.sh").read_text()
POSTINSTALL = BUILD_SCRIPT.split("cat > \"$PKG_SCRIPTS/postinstall\" << 'SCRIPTEOF'\n", 1)[1].split("\nSCRIPTEOF", 1)[0]
LAUNCH = POSTINSTALL[POSTINSTALL.index('if [[ -z "${CI:-}"'):]
MOCKED_LAUNCH = LAUNCH.replace("/usr/bin/stat", "mock_stat").replace("/usr/bin/id", "mock_id").replace("/bin/launchctl", "mock_launchctl")
MOCKS = """
mock_stat() { printf '%s' "$CONSOLE_UID"; }
mock_id() { printf '%s' "$CONSOLE_USER"; }
mock_launchctl() { printf '%s\\n' "$@"; }
"""


class PostinstallLaunchTest(unittest.TestCase):
    def run_launch(self, uid="501", user="alice", ci="", volume="/", cli=""):
        env = os.environ | {"CONSOLE_UID": uid, "CONSOLE_USER": user, "CI": ci,
                            "COMMAND_LINE_INSTALL": cli}
        result = subprocess.run(
            ["/bin/bash", "-c", MOCKS + MOCKED_LAUNCH, "postinstall", "pkg", "/", volume],
            env=env, capture_output=True, text=True, check=True,
        )
        self.assertEqual(result.stderr, "")
        return result.stdout.splitlines()

    def test_console_user_launch_is_unelevated(self):
        self.assertEqual(self.run_launch(), [
            "asuser", "501", "/usr/bin/sudo", "-u", "alice",
            "/usr/bin/open", "-a", "/Applications/KeyTao.app",
        ])

    def test_no_console_user_does_not_launch(self):
        for uid, user in [("0", "root"), ("", ""), ("invalid", "alice"),
                          ("501", ""), ("248", "_mbsetupuser"), ("501", "loginwindow")]:
            with self.subTest(uid=uid, user=user):
                self.assertEqual(self.run_launch(uid, user), [])

    def test_ci_or_other_install_volume_does_not_launch(self):
        self.assertEqual(self.run_launch(ci="true"), [])
        self.assertEqual(self.run_launch(volume="/Volumes/Other"), [])

    def test_command_line_or_mdm_install_does_not_launch(self):
        self.assertEqual(self.run_launch(cli="1"), [])


if __name__ == "__main__":
    unittest.main()
