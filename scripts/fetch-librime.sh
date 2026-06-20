#!/usr/bin/env bash
# Fetch librime dependencies used by KeyTao builds.
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="${LIBRIME_VERSION:-latest}"
DESTINATION_ROOT="$PROJECT_DIR/vendor/librime"
PLATFORM_DESTINATION=""
WINDOWS_ARCH="x64"
WINDOWS_TOOLSET="msvc"
USER_AGENT="keytao-librime-fetch"
PLATFORMS=()

usage() {
    cat <<EOF
Usage: $0 [options]

Options:
  --platform PLATFORM       Fetch one platform. Can be repeated or comma-separated.
                            Values: macos, windows, linux, android, ios, all.
  --all                     Same as --platform all.
  --version VERSION         librime GitHub release tag, or latest.
  --destination-root PATH   Root destination. Defaults to vendor/librime.
  --destination PATH        Override destination for a single selected platform.
  --windows-arch ARCH       Windows arch: x64 or x86. Defaults to x64.
  --windows-toolset NAME    Windows toolset: msvc or clang. Defaults to msvc.
  -h, --help                Show this help.
EOF
}

while [ $# -gt 0 ]; do
    case "$1" in
        --platform)
            IFS=',' read -r -a parts <<< "${2:?missing value for --platform}"
            PLATFORMS+=("${parts[@]}")
            shift 2
            ;;
        --all)
            PLATFORMS+=(all)
            shift
            ;;
        --version)
            VERSION="${2:?missing value for --version}"
            shift 2
            ;;
        --destination-root)
            DESTINATION_ROOT="${2:?missing value for --destination-root}"
            shift 2
            ;;
        --destination)
            PLATFORM_DESTINATION="${2:?missing value for --destination}"
            shift 2
            ;;
        --windows-arch)
            WINDOWS_ARCH="${2:?missing value for --windows-arch}"
            shift 2
            ;;
        --windows-toolset)
            WINDOWS_TOOLSET="${2:?missing value for --windows-toolset}"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "Unknown argument: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

if [ "${#PLATFORMS[@]}" -eq 0 ]; then
    case "$(uname -s)" in
        Darwin) PLATFORMS=(macos) ;;
        Linux) PLATFORMS=(linux) ;;
        MINGW*|MSYS*|CYGWIN*) PLATFORMS=(windows) ;;
        *) echo "ERROR: cannot infer platform; pass --platform" >&2; exit 2 ;;
    esac
fi

if [ -n "$PLATFORM_DESTINATION" ] && [ "${#PLATFORMS[@]}" -ne 1 ]; then
    echo "ERROR: --destination can only be used with one --platform" >&2
    exit 2
fi

case "$WINDOWS_ARCH" in
    x64|x86) ;;
    arm|arm64|aarch64)
        echo "ERROR: rime/librime does not publish Windows ARM64 SDK release assets." >&2
        echo "       Official Windows SDK fetch supports x64 and x86 only; ARM64 needs an experimental source-built librime pipeline." >&2
        exit 2
        ;;
    *) echo "ERROR: --windows-arch must be x64 or x86" >&2; exit 2 ;;
esac
case "$WINDOWS_TOOLSET" in
    msvc|clang) ;;
    *) echo "ERROR: --windows-toolset must be msvc or clang" >&2; exit 2 ;;
esac

if ! command -v python3 >/dev/null 2>&1; then
    echo "ERROR: python3 is required to parse GitHub release metadata." >&2
    exit 1
fi

mkdir -p "$DESTINATION_ROOT"
DESTINATION_ROOT="$(cd "$DESTINATION_ROOT" && pwd)"
CACHE_ROOT="$PROJECT_DIR/.cache/librime"
RELEASE_JSON=""

normalize_platform() {
    case "$1" in
        mac|macos|darwin|osx) echo "macos" ;;
        win|windows) echo "windows" ;;
        linux) echo "linux" ;;
        android) echo "android" ;;
        ios|iphoneos) echo "ios" ;;
        all) echo "all" ;;
        *) echo "ERROR: unsupported platform '$1'" >&2; exit 2 ;;
    esac
}

expand_platforms() {
    local normalized=()
    local item
    for item in "${PLATFORMS[@]}"; do
        item="$(normalize_platform "$item")"
        if [ "$item" = "all" ]; then
            normalized+=(macos windows linux android ios)
        else
            normalized+=("$item")
        fi
    done
    printf '%s\n' "${normalized[@]}" | awk '!seen[$0]++'
}

github_api() {
    curl -fsSL -H "User-Agent: $USER_AGENT" "$1"
}

ensure_release_json() {
    if [ -n "$RELEASE_JSON" ]; then
        return
    fi
    RELEASE_JSON="$(mktemp "${TMPDIR:-/tmp}/keytao-librime-release.XXXXXX")"
    if [ "$VERSION" = "latest" ]; then
        github_api "https://api.github.com/repos/rime/librime/releases/latest" > "$RELEASE_JSON"
    else
        github_api "https://api.github.com/repos/rime/librime/releases/tags/$VERSION" > "$RELEASE_JSON"
    fi
}

release_tag() {
    ensure_release_json
    python3 - "$RELEASE_JSON" <<'PY'
import json
import sys

with open(sys.argv[1], "r", encoding="utf-8") as handle:
    print(json.load(handle).get("tag_name", ""))
PY
}

read_release_asset() {
    local kind="$1"
    local asset_substring="$2"
    ensure_release_json
    python3 - "$RELEASE_JSON" "$kind" "$asset_substring" <<'PY'
import json
import sys

release_path, kind, asset_substring = sys.argv[1], sys.argv[2], sys.argv[3]
with open(release_path, "r", encoding="utf-8") as handle:
    release = json.load(handle)

def is_match(asset):
    name = asset.get("name", "")
    if asset_substring not in name:
        return False
    if kind == "main":
        return name.startswith("rime-") and not name.startswith("rime-deps-")
    if kind == "deps":
        return name.startswith("rime-deps-")
    return False

asset = next((item for item in release.get("assets", []) if is_match(item)), None)
if not asset:
    windows_assets = [item.get("name", "") for item in release.get("assets", []) if "Windows" in item.get("name", "")]
    suffix = ""
    if windows_assets:
        suffix = "; available Windows assets: " + ", ".join(windows_assets)
    raise SystemExit(
        f"missing {kind} asset containing {asset_substring!r} in {release.get('tag_name', '<unknown>')}{suffix}"
    )

print("\t".join([
    release.get("tag_name", ""),
    asset.get("name", ""),
    asset.get("browser_download_url", ""),
    asset.get("digest", ""),
]))
PY
}

download_asset() {
    local url="$1"
    local file="$2"
    local digest="$3"

    if [ ! -f "$file" ]; then
        echo "Downloading $(basename "$file")"
        curl -fL -H "User-Agent: $USER_AGENT" -o "$file" "$url"
    else
        echo "Using cached $(basename "$file")"
    fi

    if [[ "$digest" == sha256:* ]]; then
        local expected="${digest#sha256:}"
        local actual
        actual="$(shasum -a 256 "$file" | awk '{ print $1 }')"
        if [ "$expected" != "$actual" ]; then
            echo "ERROR: sha256 mismatch for $file" >&2
            echo "  expected: $expected" >&2
            echo "  actual:   $actual" >&2
            exit 1
        fi
    fi
}

fetch_base_rime_data() {
    local destination="$1"
    mkdir -p "$destination"
    echo "Fetching base rime-data into $destination"
    local file
    for file in default.yaml key_bindings.yaml punctuation.yaml symbols.yaml; do
        curl -fsSL -H "User-Agent: $USER_AGENT" \
            "https://raw.githubusercontent.com/rime/rime-prelude/master/$file" \
            -o "$destination/$file"
    done
    curl -fsSL -H "User-Agent: $USER_AGENT" \
        "https://raw.githubusercontent.com/rime/rime-essay/master/essay.txt" \
        -o "$destination/essay.txt"
}

find_seven_zip() {
    local name
    for name in 7zz 7z 7za; do
        if command -v "$name" >/dev/null 2>&1; then
            command -v "$name"
            return 0
        fi
    done
    return 1
}

absolute_destination() {
    local destination="$1"
    mkdir -p "$(dirname "$destination")"
    printf '%s/%s\n' "$(cd "$(dirname "$destination")" && pwd)" "$(basename "$destination")"
}

copy_flat_files() {
    local source_dir="$1"
    local pattern="$2"
    local destination="$3"
    mkdir -p "$destination"
    while IFS= read -r -d '' file; do
        cp -f "$file" "$destination/$(basename "$file")"
    done < <(find "$source_dir" -type f -name "$pattern" -print0)
}

write_metadata() {
    local destination="$1"
    local platform="$2"
    local tag="$3"
    cat > "$destination/librime-release.txt" <<EOF
platform=$platform
version=$tag
source=https://github.com/rime/librime/releases/tag/$tag
EOF
}

fetch_macos() {
    local destination="${PLATFORM_DESTINATION:-$DESTINATION_ROOT/macos-universal}"
    destination="$(absolute_destination "$destination")"

    IFS=$'\t' read -r release main_name main_url main_digest < <(read_release_asset main "macOS-universal.tar.bz2")
    IFS=$'\t' read -r _ deps_name deps_url deps_digest < <(read_release_asset deps "macOS-universal.tar.bz2")

    local cache_dir="$CACHE_ROOT/$release/macos-universal"
    local extract_dir="$cache_dir/extract"
    local main_archive="$cache_dir/$main_name"
    local deps_archive="$cache_dir/$deps_name"
    mkdir -p "$cache_dir"

    download_asset "$main_url" "$main_archive" "$main_digest"
    download_asset "$deps_url" "$deps_archive" "$deps_digest"

    rm -rf "$extract_dir"
    mkdir -p "$extract_dir/main" "$extract_dir/deps"
    tar -xjf "$main_archive" -C "$extract_dir/main"
    tar -xjf "$deps_archive" -C "$extract_dir/deps"

    local main_root="$extract_dir/main/dist"
    local deps_root="$extract_dir/deps"
    if [ ! -f "$main_root/include/rime_api.h" ]; then
        echo "ERROR: extracted macOS SDK does not contain include/rime_api.h" >&2
        exit 1
    fi
    if [ ! -f "$main_root/lib/librime.1.dylib" ] && [ ! -L "$main_root/lib/librime.1.dylib" ]; then
        echo "ERROR: extracted macOS SDK does not contain lib/librime.1.dylib" >&2
        exit 1
    fi

    rm -rf "$destination"
    mkdir -p "$destination"
    ditto "$main_root/include" "$destination/include"
    ditto "$main_root/lib" "$destination/lib"
    if [ -f "$main_root/version-info.txt" ]; then
        cp "$main_root/version-info.txt" "$destination/version-info.txt"
    fi

    mkdir -p "$destination/rime-data"
    if [ -d "$deps_root/share/opencc" ]; then
        ditto "$deps_root/share/opencc" "$destination/rime-data/opencc"
    fi
    fetch_base_rime_data "$destination/rime-data"
    write_metadata "$destination" "macos-universal" "$release"

    cat > "$destination/env.sh" <<EOF
export RIME_PREFIX="$destination"
export RIME_INCLUDE_DIR="$destination/include"
export RIME_LIB_DIR="$destination/lib"
export KEYTAO_RIME_SHARED_DATA_DIR="$destination/rime-data"
export RIME_SHARED_DATA_DIR="$destination/rime-data"
export BINDGEN_EXTRA_CLANG_ARGS="\${BINDGEN_EXTRA_CLANG_ARGS:-} -I$destination/include"
EOF

    echo ""
    echo "librime macOS SDK is ready:"
    echo "  $destination"
    echo "  version: $release"
}

fetch_windows() {
    local destination="${PLATFORM_DESTINATION:-$DESTINATION_ROOT/windows-$WINDOWS_ARCH}"
    destination="$(absolute_destination "$destination")"
    local asset_suffix="Windows-$WINDOWS_TOOLSET-$WINDOWS_ARCH.7z"
    local seven_zip
    seven_zip="$(find_seven_zip || true)"
    if [ -z "$seven_zip" ]; then
        echo "ERROR: fetching Windows librime requires 7z/7zz/7za." >&2
        exit 1
    fi

    IFS=$'\t' read -r release main_name main_url main_digest < <(read_release_asset main "$asset_suffix")
    IFS=$'\t' read -r _ deps_name deps_url deps_digest < <(read_release_asset deps "$asset_suffix")

    local cache_dir="$CACHE_ROOT/$release/windows-$WINDOWS_TOOLSET-$WINDOWS_ARCH"
    local extract_dir="$cache_dir/extract"
    local main_archive="$cache_dir/$main_name"
    local deps_archive="$cache_dir/$deps_name"
    mkdir -p "$cache_dir"

    download_asset "$main_url" "$main_archive" "$main_digest"
    download_asset "$deps_url" "$deps_archive" "$deps_digest"

    rm -rf "$extract_dir"
    mkdir -p "$extract_dir/main" "$extract_dir/deps"
    "$seven_zip" x "-o$extract_dir/main" -y "$main_archive" >/dev/null
    "$seven_zip" x "-o$extract_dir/deps" -y "$deps_archive" >/dev/null

    local header
    local lib
    header="$(find "$extract_dir" -type f -name rime_api.h | head -1)"
    lib="$(find "$extract_dir" -type f -name rime.lib | head -1)"
    if [ -z "$header" ]; then
        echo "ERROR: extracted Windows SDK does not contain rime_api.h" >&2
        exit 1
    fi
    if [ -z "$lib" ]; then
        echo "ERROR: extracted Windows SDK does not contain rime.lib" >&2
        exit 1
    fi

    rm -rf "$destination"
    mkdir -p "$destination/include" "$destination/lib" "$destination/bin" "$destination/rime-data"
    cp -R "$(dirname "$header")/." "$destination/include"
    copy_flat_files "$(dirname "$lib")" "*.lib" "$destination/lib"
    copy_flat_files "$extract_dir" "*.dll" "$destination/bin"

    local data_marker
    data_marker="$(find "$extract_dir" -type f -name default.yaml | grep -E 'rime-data|share' | head -1 || true)"
    if [ -n "$data_marker" ]; then
        cp -R "$(dirname "$data_marker")/." "$destination/rime-data"
    fi
    fetch_base_rime_data "$destination/rime-data"
    write_metadata "$destination" "windows-$WINDOWS_TOOLSET-$WINDOWS_ARCH" "$release"

    cat > "$destination/env.sh" <<EOF
export RIME_PREFIX="$destination"
export RIME_INCLUDE_DIR="$destination/include"
export RIME_LIB_DIR="$destination/lib"
export PATH="$destination/bin:\$PATH"
EOF

    local ps_include="$destination/include"
    local ps_lib="$destination/lib"
    local ps_bin="$destination/bin"
    if command -v cygpath >/dev/null 2>&1; then
        ps_include="$(cygpath -w "$ps_include")"
        ps_lib="$(cygpath -w "$ps_lib")"
        ps_bin="$(cygpath -w "$ps_bin")"
    fi
    cat > "$destination/env.ps1" <<EOF
\$env:RIME_INCLUDE_DIR = "$ps_include"
\$env:RIME_LIB_DIR = "$ps_lib"
\$env:Path = "$ps_bin;\$env:Path"
EOF

    echo ""
    echo "librime Windows SDK is ready:"
    echo "  $destination"
    echo "  version: $release"
}

fetch_linux() {
    local arch
    arch="$(uname -m 2>/dev/null || echo unknown)"
    local destination="${PLATFORM_DESTINATION:-$DESTINATION_ROOT/linux-$arch}"
    destination="$(absolute_destination "$destination")"
    rm -rf "$destination"
    mkdir -p "$destination/rime-data"
    fetch_base_rime_data "$destination/rime-data"

    local include_dir=""
    local lib_dir=""
    local pc_name
    for pc_name in rime librime; do
        if pkg-config --exists "$pc_name" 2>/dev/null; then
            include_dir="$(pkg-config --variable=includedir "$pc_name" 2>/dev/null || true)"
            lib_dir="$(pkg-config --variable=libdir "$pc_name" 2>/dev/null || true)"
            break
        fi
    done

    cat > "$destination/env.sh" <<EOF
export KEYTAO_RIME_SHARED_DATA_DIR="$destination/rime-data"
export RIME_SHARED_DATA_DIR="$destination/rime-data"
EOF
    if [ -n "$include_dir" ]; then
        cat >> "$destination/env.sh" <<EOF
export RIME_INCLUDE_DIR="$include_dir"
export BINDGEN_EXTRA_CLANG_ARGS="\${BINDGEN_EXTRA_CLANG_ARGS:-} -I$include_dir"
EOF
    fi
    if [ -n "$lib_dir" ]; then
        cat >> "$destination/env.sh" <<EOF
export RIME_LIB_DIR="$lib_dir"
EOF
    fi

    echo ""
    echo "librime Linux data is ready:"
    echo "  $destination"
    if [ -z "$include_dir" ] || [ -z "$lib_dir" ]; then
        echo "  note: no official Linux SDK asset is published by rime/librime; install your distro's librime-dev/librime-plugin packages for native libraries."
    fi
}

fetch_mobile_data() {
    local platform="$1"
    local destination="${PLATFORM_DESTINATION:-$DESTINATION_ROOT/$platform}"
    destination="$(absolute_destination "$destination")"
    rm -rf "$destination"
    mkdir -p "$destination/rime-data"
    fetch_base_rime_data "$destination/rime-data"
    cat > "$destination/env.sh" <<EOF
export KEYTAO_RIME_SHARED_DATA_DIR="$destination/rime-data"
export RIME_SHARED_DATA_DIR="$destination/rime-data"
EOF
    echo ""
    echo "librime $platform data is ready:"
    echo "  $destination"
    echo "  note: the current mobile app path does not link a native librime SDK."
}

trap 'if [ -n "${RELEASE_JSON:-}" ]; then rm -f "$RELEASE_JSON"; fi' EXIT

while IFS= read -r platform; do
    case "$platform" in
        macos) fetch_macos ;;
        windows) fetch_windows ;;
        linux) fetch_linux ;;
        android|ios) fetch_mobile_data "$platform" ;;
    esac
done < <(expand_platforms)
