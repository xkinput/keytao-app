#!/usr/bin/env bash
# Compatibility wrapper for the unified librime fetcher.
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
exec "$PROJECT_DIR/scripts/fetch-librime.sh" --platform macos "$@"
