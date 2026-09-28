#!/usr/bin/env node
import fs from "node:fs"

const workspaceCargoPath = "Cargo.toml"
const flutterPubspecPath = "flutter_app/pubspec.yaml"

const explicitVersion = process.argv[2] === "--set" ? process.argv[3] : null

if (process.argv[2] === "--set" && !explicitVersion) {
  throw new Error("Usage: node scripts/sync-version.mjs --set <version>")
}

function readWorkspaceVersion() {
  const cargoToml = fs.readFileSync(workspaceCargoPath, "utf8")
  const match = cargoToml.match(
    /^\[workspace\.package\]\s*?\n([\s\S]*?)(?=^\[|\s*$)/m
  )
  if (!match) {
    throw new Error("Cargo.toml is missing [workspace.package]")
  }
  const version = match[1].match(/^version\s*=\s*"([^"]+)"/m)?.[1]
  if (!version) {
    throw new Error("Cargo.toml is missing [workspace.package].version")
  }
  return version
}

function writeWorkspaceVersion(version) {
  const cargoToml = fs.readFileSync(workspaceCargoPath, "utf8")
  const versionPattern =
    /^(\[workspace\.package\]\s*?\n(?:(?!^\[)[\s\S])*?^version\s*=\s*")[^"]+(")/m
  if (!versionPattern.test(cargoToml)) {
    throw new Error("Failed to update [workspace.package].version")
  }
  const next = cargoToml.replace(versionPattern, `$1${version}$2`)
  fs.writeFileSync(workspaceCargoPath, next)
}

function writeFlutterVersion(version) {
  const match = version.match(/^(\d+)\.(\d+)\.(\d+)(?:-[0-9A-Za-z.-]+)?$/)
  if (!match) {
    throw new Error(`Unsupported Flutter app version: ${version}`)
  }
  // Preserve the Android versionCode formula; prereleases share the base code.
  const versionCode = Number(match[1]) * 1000000 + Number(match[2]) * 1000 + Number(match[3])
  const pubspec = fs.readFileSync(flutterPubspecPath, "utf8")
  if (!/^version:.*$/m.test(pubspec)) {
    throw new Error("flutter_app/pubspec.yaml is missing version")
  }
  fs.writeFileSync(
    flutterPubspecPath,
    pubspec.replace(/^version:.*$/m, `version: ${version}+${versionCode}`)
  )
}

if (explicitVersion) {
  writeWorkspaceVersion(explicitVersion)
}

const version = readWorkspaceVersion()
writeFlutterVersion(version)
console.log(`Synced version ${version}`)
