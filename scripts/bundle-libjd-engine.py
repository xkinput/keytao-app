#!/usr/bin/env python3
"""Stage the fixed upstream engine-only binaries and license for packaging."""

import argparse
import hashlib
import io
from pathlib import Path
import subprocess
import tarfile
from tempfile import NamedTemporaryFile, TemporaryDirectory
from urllib.request import urlopen
import zipfile


VERSION = "v0.14.0"
REPOSITORY = "https://github.com/hronro/ime-jd"
RELEASE = f"{REPOSITORY}/releases/download/{VERSION}"
LICENSE = f"https://raw.githubusercontent.com/hronro/ime-jd/{VERSION}/LICENSE"
LICENSE_SHA256 = "3972dc9744f6499f0f9b2dbf76696f2ae7ad8af9b23dde66d6af86c9dfb36986"
SOURCE_COMMIT = "5452e0d576ddbbcfed2ba5595442ad0a1db5f68b"
SOURCE_SHA256 = "9d64498dd993f13eba7d65ab8539725494bf9e3b7e5745c40df65fedf4d8fe21"
MAX_FILE_BYTES = 4 * 1024 * 1024

# SHA-256 properties from GitHub's release asset metadata for v0.14.0.
ASSETS = {
    "macos-arm64": ("dylib.tar.xz", "92f71cc4497bc08270ca38b10a25d2bd5a78f65fc39be7f3c8c6a5b504f9066f"),
    "macos-x86_64": ("dylib.tar.xz", "965de4ec580e7bfcf56b2a6dcb1d651f252a66ecca44aefc359a7389cf03f0cb"),
    "windows-arm64": ("zip", "ae9b42c9051dadaf5ddc1c3bd2bb31bdb09423d65e82759019ac4e409cb84967"),
    "windows-x86_64": ("zip", "64a92a15a724c24b1e44dc2b94e108051febe4aa49c9dd5ec8fd71a36bb0ee49"),
    "linux-arm64": ("so.tar.xz", "ceba256d6c0284bff5e285643dcf3f1a68293878cb17ac78f53a7c8ebb57b651"),
    "linux-x86_64": ("so.tar.xz", "affb7ebddfb89fb16cbfbb04247379a0c92b65fd81c6fa7ca77febe021cd46a8"),
    "android-arm64": ("so.tar.xz", "796b4e618213a11adc364aad88afe51043298d24c3218383990c5cb42d32e619"),
    "android-x86_64": ("so.tar.xz", "0bdd44d2b1389d07f268a9fe310e67b988a1836cd2f6d41f70b59880c322e62c"),
    "ios-arm64": ("a.tar.xz", "cddfc647f540d1cfd99257c297369fd220ddfee287be78e3ddc6179bc2f14b56"),
}
CACHE = Path(__file__).resolve().parents[1] / "target" / "libjd-engine-cache"


def verified_download(url, digest):
    CACHE.mkdir(parents=True, exist_ok=True)
    cached = CACHE / digest
    if cached.is_file() and cached.stat().st_size <= MAX_FILE_BYTES:
        data = cached.read_bytes()
        if hashlib.sha256(data).hexdigest() == digest:
            return data
    with urlopen(url, timeout=45) as response:
        data = response.read(MAX_FILE_BYTES + 1)
    if len(data) > MAX_FILE_BYTES or hashlib.sha256(data).hexdigest() != digest:
        raise ValueError(f"Unexpected SHA-256 or size for {url}")
    with NamedTemporaryFile(dir=CACHE, delete=False) as staging:
        staging.write(data)
    Path(staging.name).replace(cached)
    return data


def stage_library(target, directory):
    extension, digest = ASSETS[target]
    asset = f"libjd-engine-{VERSION}-{target}.{extension}"
    data = io.BytesIO(verified_download(f"{RELEASE}/{asset}", digest))
    if extension == "zip":
        with zipfile.ZipFile(data) as archive:
            member = archive.getinfo("jd.dll")
            if member.file_size > MAX_FILE_BYTES:
                raise ValueError(f"Unexpected DLL size in {asset}")
            library = archive.read(member)
        suffix = "dll"
    else:
        suffix = extension.split(".")[0]
        with tarfile.open(fileobj=data, mode="r:xz") as archive:
            member = archive.getmember(f"libjd.{suffix}")
            if not member.isfile() or member.size > MAX_FILE_BYTES:
                raise ValueError(f"Unexpected library entry in {asset}")
            with archive.extractfile(member) as stream:
                library = stream.read()
    directory.mkdir(parents=True, exist_ok=True)
    output = directory / f"libjd-engine.{suffix}"
    output.write_bytes(library)
    print(f"Verified {asset}")
    return output


def stage_android_arm(directory):
    # Upstream publishes 64-bit Android binaries; use the same engine-only
    # build from the pinned source for KeyTao's armeabi-v7a APK.
    source = verified_download(
        f"https://codeload.github.com/hronro/ime-jd/tar.gz/{SOURCE_COMMIT}", SOURCE_SHA256
    )
    with TemporaryDirectory() as temp:
        root = Path(temp)
        with tarfile.open(fileobj=io.BytesIO(source), mode="r:gz") as archive:
            archive.extractall(root, filter="data")
        try:
            subprocess.run(
                ["zig", "build", "-Dtarget=arm-linux-androideabi", "-Dcpu=baseline",
                 "-Doptimize=fast", "-Dbuiltin_dict=false", "--prefix", root / "build"],
                cwd=root / f"ime-jd-{SOURCE_COMMIT}" / "core",
                check=True,
            )
        except FileNotFoundError as error:
            raise RuntimeError("Zig 0.17 is required to build Android armeabi-v7a libjd-engine") from error
        library = (root / "build/lib/libjd.so").read_bytes()
        if len(library) > MAX_FILE_BYTES:
            raise ValueError("Unexpected size for Android armeabi-v7a libjd-engine")
        directory.mkdir(parents=True, exist_ok=True)
        (directory / "libjd-engine.so").write_bytes(library)
        print(f"Built libjd-engine {VERSION} for Android armeabi-v7a")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("target", choices=[*ASSETS, "macos-universal", "android"])
    parser.add_argument("libraries", type=Path)
    parser.add_argument("notices", type=Path)
    args = parser.parse_args()
    license_text = verified_download(LICENSE, LICENSE_SHA256)

    if args.target == "macos-universal":
        args.libraries.mkdir(parents=True, exist_ok=True)
        with TemporaryDirectory() as temp:
            slices = [
                stage_library(f"macos-{arch}", Path(temp) / arch)
                for arch in ("arm64", "x86_64")
            ]
            output = args.libraries / "libjd-engine.dylib"
            subprocess.run(["lipo", "-create", *slices, "-output", output], check=True)
            subprocess.run(["install_name_tool", "-id", "@rpath/libjd-engine.dylib", output], check=True)
    elif args.target == "android":
        stage_android_arm(args.libraries / "armeabi-v7a")
        stage_library("android-arm64", args.libraries / "arm64-v8a")
        stage_library("android-x86_64", args.libraries / "x86_64")
    else:
        stage_library(args.target, args.libraries)

    args.notices.mkdir(parents=True, exist_ok=True)
    (args.notices / "libjd-engine-LICENSE.txt").write_bytes(license_text)
    (args.notices / "libjd-engine-SOURCE.txt").write_text(
        f"libjd-engine {VERSION} (GPL-3.0)\n"
        f"Upstream source: {REPOSITORY}/tree/{VERSION}\n"
        f"Upstream release: {REPOSITORY}/releases/tag/{VERSION}\n",
        encoding="utf-8",
    )


if __name__ == "__main__":
    main()
