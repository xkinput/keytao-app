#!/usr/bin/env bash
# Run after both official Linux deb assets have been published. Requires Nix and Python 3.
set -euo pipefail

if [[ $# -ne 1 || ! $1 =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]]; then
  echo "Usage: $0 <version-without-v>" >&2
  exit 2
fi

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
python3 - "$PROJECT_DIR/flake.nix" "$1" <<'PY'
import json
import pathlib
import re
import subprocess
import sys

path = pathlib.Path(sys.argv[1])
version = sys.argv[2]
source = path.read_text()
updates = {r'(releaseVersion = ")[^"]+(";)': version}
for system, arch in [('x86_64-linux', 'x64'), ('aarch64-linux', 'arm64')]:
    url = f'https://github.com/xkinput/keytao-app/releases/download/v{version}/keytao-app-{version}-linux-{arch}.deb'
    result = subprocess.check_output([
        'nix', '--extra-experimental-features', 'nix-command',
        'store', 'prefetch-file', '--json', '--hash-type', 'sha256', url,
    ], text=True)
    digest = json.loads(result)['hash']
    if not re.fullmatch(r'sha256-[A-Za-z0-9+/]{43}=', digest):
        raise SystemExit(f'Invalid SHA-256 from prefetch-file for {system}')
    updates[rf'({system} = ")sha256-[^"]+(";)'] = digest

# Do not change the pin until both downloads and all replacements have succeeded.
for pattern, value in updates.items():
    source, count = re.subn(pattern, lambda match: match[1] + value + match[2], source)
    if count != 1:
        raise SystemExit(f'Expected exactly one release pin matching {pattern}')
path.write_text(source)
print(f'Updated flake.nix to official release {version}; review the diff and run the Nix pre-run.')
PY
