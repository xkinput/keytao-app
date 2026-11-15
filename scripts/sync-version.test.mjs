import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import { spawnSync } from "node:child_process"
import test from "node:test"
import { fileURLToPath } from "node:url"

const scriptPath = fileURLToPath(new URL("./sync-version.mjs", import.meta.url))

function fixture(t, cargoToml) {
  const directory = fs.mkdtempSync(path.join(path.dirname(scriptPath), ".sync-version-test-"))
  t.after(() => fs.rmSync(directory, { recursive: true, force: true }))
  fs.mkdirSync(path.join(directory, "flutter_app"))
  fs.writeFileSync(path.join(directory, "Cargo.toml"), cargoToml)
  fs.writeFileSync(path.join(directory, "flutter_app/pubspec.yaml"), "version: 1.0.0+1000000\n")
  return directory
}

test("syncs a workspace version after other package keys", (t) => {
  const directory = fixture(t, '[workspace.package]\nlicense = "MIT"\nversion = "1.0.0"\n\n[workspace.dependencies]\nserde = "1"\n')
  const result = spawnSync(process.execPath, [scriptPath, "--set", "2.4.6-rc.1"], {
    cwd: directory,
    encoding: "utf8",
  })
  assert.equal(result.status, 0, result.stderr)
  assert.match(result.stdout, /Synced version 2\.4\.6-rc\.1/)
  assert.match(fs.readFileSync(path.join(directory, "Cargo.toml"), "utf8"), /^version = "2\.4\.6-rc\.1"$/m)
  assert.equal(fs.readFileSync(path.join(directory, "flutter_app/pubspec.yaml"), "utf8"), "version: 2.4.6-rc.1+2004006\n")
})

test("does not read a version from the next TOML table", (t) => {
  const cargoToml = '[workspace.package]\nlicense = "MIT"\n\n[workspace.dependencies]\nversion = "9.9.9"\n'
  const directory = fixture(t, cargoToml)
  const result = spawnSync(process.execPath, [scriptPath], { cwd: directory, encoding: "utf8" })
  assert.notEqual(result.status, 0)
  assert.match(result.stderr, /missing \[workspace\.package\]\.version/)
  assert.equal(fs.readFileSync(path.join(directory, "Cargo.toml"), "utf8"), cargoToml)
  assert.equal(fs.readFileSync(path.join(directory, "flutter_app/pubspec.yaml"), "utf8"), "version: 1.0.0+1000000\n")
})
