#!/usr/bin/env node
import fs from "node:fs"

const workspaceCargoPath = "Cargo.toml"
const packageJsonPath = "package.json"
const tauriConfigPath = "src-tauri/tauri.conf.json"

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

function writeJsonVersion(path, version) {
  const json = JSON.parse(fs.readFileSync(path, "utf8"))
  json.version = version
  fs.writeFileSync(path, `${JSON.stringify(json, null, 2)}\n`)
}

if (explicitVersion) {
  writeWorkspaceVersion(explicitVersion)
}

const version = readWorkspaceVersion()
writeJsonVersion(packageJsonPath, version)
writeJsonVersion(tauriConfigPath, version)
console.log(`Synced version ${version}`)
