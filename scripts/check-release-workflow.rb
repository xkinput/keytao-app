#!/usr/bin/env ruby
# Offline release contract check; uses Ruby stdlib and the existing Node runtime.
require 'yaml'
require 'json'
require 'open3'

Dir.chdir(File.expand_path('..', __dir__))
source = File.read('.github/workflows/release.yml')
workflow = YAML.safe_load(source)
jobs = workflow.fetch('jobs')
def check(condition, message)
  abort "FAIL: #{message}" unless condition
end

check(workflow.dig('on', 'push', 'tags') == ['v*'], 'Keep the v* tag trigger')
input = workflow.dig('on', 'workflow_dispatch', 'inputs', 'version')
check(input && input['default'] == '' && input['required'] == false, 'Dispatch defaults to the workspace version')
check(workflow['permissions'] == { 'contents' => 'read' }, 'Builds must have read-only repository access')
check(!source.match?(/tauri|pnpm|macos-15-intel/i), 'No legacy Tauri build legs')
builds = %w[build-desktop build-android build-ios]
check(jobs.keys.sort == (builds + %w[prepare build-nix create-release]).sort, 'Unexpected job set')
nix = jobs.fetch('build-nix')
check(nix['if'] == "github.event_name == 'workflow_dispatch'", 'Nix must run only during dispatch pre-runs')
check(nix['runs-on'] == 'ubuntu-latest' && !nix.key?('needs'), 'Nix must independently repackage the published release')
check(!nix.key?('permissions'), 'Nix must inherit read-only permissions')
check(nix['steps'].map { |s| s['uses'] }.compact == %w[actions/checkout@v4 cachix/install-nix-action@v31], 'Nix needs checkout and installer')
check(nix['steps'].map { |s| s['run'] }.compact == [
  'nix build .#packages.x86_64-linux.default --print-build-logs',
  'nix flake check --no-build'
], 'Nix pre-run must build and check without updating or publishing')
release = jobs.fetch('create-release')
check(release['if'] == "github.event_name == 'push' && startsWith(github.ref, 'refs/tags/v')", 'Release must be tag-push-only')
check(release['needs'].sort == (builds + ['prepare']).sort, 'Release must wait for every build')
check(release['permissions'] == { 'contents' => 'write' }, 'Release needs contents write access')
check(release['steps'].any? { |s| s['uses'] == 'actions/download-artifact@v4' && s.dig('with', 'merge-multiple') }, 'Release must collect all artifacts')
check(release['steps'].any? { |s| s['run'] == 'gh release upload "$GITHUB_REF_NAME" dist/release-upload/* --clobber' }, 'Release must upload the collected artifacts')

paths = source.scan(%r{scripts/[\w.-]+\.(?:sh|ps1|mjs)\b}).uniq
paths.each { |path| check(File.file?(path), "Missing referenced script: #{path}") }
version = '${{ needs.prepare.outputs.version }}'
rust = '${{ needs.prepare.outputs.rust-toolchain }}'
check(File.read('rust-toolchain.toml').include?('channel = "1.98.1"'), 'Unexpected Rust toolchain pin')
docker = File.read('scripts/Dockerfile.linux-builder')
check(docker.include?('ARG FLUTTER_VERSION=3.47.5') && docker.include?('COPY rust-toolchain.toml'), 'Linux must use pinned Flutter and workspace Rust')

builds.each do |name|
  job = jobs.fetch(name)
  check(job['needs'] == 'prepare' && !job.key?('if'), "#{name}: dispatch must run without a skipped Release dependency")
  check(!job.key?('permissions'), "#{name}: must inherit read-only permissions")
  check(job.dig('env', 'VERSION') == version && job.dig('env', 'RUSTUP_TOOLCHAIN') == rust, "#{name}: metadata must be shared")
  steps = job.fetch('steps')
  sync = steps.find { |s| s.fetch('run', '').include?('node scripts/sync-version.mjs --set "$VERSION"') }
  check(sync && !sync.key?('if') && sync['run'].include?('cargo update --workspace'), "#{name}: sync version and workspace lock for --locked builds")
  flutter = steps.find { |s| s['uses'] == 'subosito/flutter-action@v2' }
  check(flutter && flutter.dig('with', 'flutter-version') == '3.47.5', "#{name}: wrong Flutter pin")
  toolchain = steps.find { |s| s['uses'] == 'dtolnay/rust-toolchain@master' }
  check(toolchain && toolchain.dig('with', 'toolchain') == rust, "#{name}: wrong Rust toolchain")
  upload = steps.find { |s| s['uses'] == 'actions/upload-artifact@v4' }
  check(upload && !upload.key?('if') && upload.dig('with', 'if-no-files-found') == 'error', "#{name}: upload required in both modes")
  check(!steps.to_json.match?(/createRelease|gh release/), "#{name}: must never mutate Releases")
end

desktop = jobs.fetch('build-desktop')
matrix = desktop.dig('strategy', 'matrix', 'include')
expected = {
  ['linux', 'x64'] => ['ubuntu-22.04', %w[deb rpm].map { |ext| "dist/keytao-app-#{version}-linux-x64.#{ext}" }],
  ['linux', 'arm64'] => ['ubuntu-24.04-arm', %w[deb rpm].map { |ext| "dist/keytao-app-#{version}-linux-arm64.#{ext}" }],
  ['macos', 'universal'] => ['macos-15', ["target/keytao-macos-pkg/keytao-app-#{version}-macos.pkg"]],
  ['windows', 'x64'] => ['windows-latest', ["target/release/bundle/nsis/keytao-app-#{version}-windows-x64-setup.exe"]]
}
check(matrix.map { |m| [m['os_name'], m['package_arch']] }.sort == expected.keys.sort, 'Expected exactly four desktop legs')
matrix.each do |leg|
  runner, artifacts = expected.fetch([leg['os_name'], leg['package_arch']])
  check(leg['platform'] == runner && leg['artifacts'].lines.map(&:strip) == artifacts, "Wrong desktop runner/artifacts: #{leg['os_name']} #{leg['package_arch']}")
end
check(desktop.dig('strategy', 'fail-fast') == false, 'Run all desktop legs even if one fails')
check(desktop['steps'].find { |s| s['uses'] == 'actions/upload-artifact@v4' }.dig('with', 'path') == '${{ matrix.artifacts }}', 'Desktop upload must use declared paths')
{
  'linux' => %w[build-linux.sh verify-linux-bundles.sh],
  'macos' => %w[build-macos.sh verify-macos-pkg.sh],
  'windows' => %w[build-windows-flutter.ps1]
}.each do |os, scripts|
  scripts.each do |script|
    check(desktop['steps'].any? { |s| s['if'] == "matrix.os_name == '#{os}'" && s.fetch('run', '').include?("scripts/#{script}") }, "Missing #{os} entrypoint: #{script}")
  end
end
check(File.read('scripts/build-windows-flutter.ps1').include?('"verify-windows-bundle.ps1") -ReleaseDir $bundleDir -InstallerPath $installer'), 'Windows entrypoint must verify its installer')

android = jobs.fetch('build-android')
ios = jobs.fetch('build-ios')
check(android['runs-on'] == 'ubuntu-latest' && ios['runs-on'] == 'macos-15', 'Wrong mobile runners')
android_run = android['steps'].map { |s| s.fetch('run', '') }.join("\n")
check(android_run.include?('for abi in armeabi-v7a arm64-v8a x86_64; do') && !android_run.include?('sync --all'), 'Android must import only three ABIs; Gradle syncs assets')
check(android_run.include?('cd flutter_app/android') && android_run.include?('ndk;27.0.12077973'), 'Wrong Android signing directory or NDK')
check(android_run.include?('apksigner" verify --verbose --print-certs'), 'Verify every APK signature before upload')
check(android['steps'].any? { |s| s['working-directory'] == 'flutter_app' && s.fetch('run', '').include?('flutter build apk --release --split-per-abi --target-platform android-arm,android-arm64,android-x64') }, 'Wrong Flutter APK build')
mobile_paths = {
  android => %w[arm arm64 x86_64].map { |arch| "dist/release-upload/keytao-app-#{version}-android-#{arch}-release.apk" },
  ios => ["flutter_app/build/ios/ipa/keytao-app-#{version}-ios-arm64-unsigned.ipa"]
}
mobile_paths.each do |job, artifacts|
  actual = job['steps'].find { |s| s['uses'] == 'actions/upload-artifact@v4' }.dig('with', 'path')
  check(actual.lines.map(&:strip) == artifacts, 'Wrong mobile artifact names')
end
%w[build-ios-librime.sh build-flutter-ios.sh].each do |script|
  check(ios['steps'].any? { |s| s.fetch('run', '').include?("scripts/#{script}") }, "Missing iOS entrypoint: #{script}")
end

# Execute the actual metadata code with read-only local inputs, never the API.
metadata = jobs.fetch('prepare')['steps'].find { |s| s['id'] == 'metadata' }.dig('with', 'script')
release_code = release['steps'].find { |s| s['id'] == 'create' }.dig('with', 'script')
output, status = Open3.capture2e('node', '-e', <<~'JS', metadata, release_code)
  const assert = require('node:assert/strict')
  const resolve = new Function('context', 'core', 'require', process.argv[1])
  function run(eventName, ref, requested = '') {
    process.env.REQUESTED_VERSION = requested
    const output = {}
    resolve({eventName, ref}, {setOutput: (key, value) => output[key] = value}, require)
    return output
  }
  const workspace = require('node:fs').readFileSync('Cargo.toml', 'utf8').match(/^\[workspace\.package\]\s*\nversion\s*=\s*"([^"]+)"/m)[1]
  assert.equal(run('workflow_dispatch', 'refs/heads/main').version, workspace)
  assert.equal(run('workflow_dispatch', 'refs/tags/v0.0.0', ' 2.3.4-alpha.9 ').version, '2.3.4-alpha.9')
  assert.equal(run('push', 'refs/tags/v1.2.1-alpha.89').version, '1.2.1-alpha.89')
  assert.equal(run('push', 'refs/tags/v2.3.4')['rust-toolchain'], '1.98.1')
  for (const value of ['v1.2.3', '../bad', '1.2.3\nBAD=value', '1.2.3;exit 0']) {
    assert.throws(() => run('workflow_dispatch', 'refs/heads/main', value))
  }
  const create = new (Object.getPrototypeOf(async function() {}).constructor)('context', 'github', process.argv[2])
  ;(async () => {
    for (const tag of ['v1.2.1-alpha.89', 'v2.3.4']) {
      let payload
      await create({ref: `refs/tags/${tag}`, repo: {owner: 'test', repo: 'test'}}, {
        rest: {repos: {createRelease: async value => { payload = value; return {data: {id: 1}} }}}
      })
      assert.equal(payload.prerelease, tag.includes('alpha'))
      assert.equal(payload.tag_name, tag)
      assert.equal(payload.draft, false)
      assert.equal(payload.generate_release_notes, true)
    }
  })().catch(error => { console.error(error); process.exitCode = 1 })
JS
check(status.success?, "Metadata/release decision checks: #{output}")
puts "PASS: YAML, #{paths.length} script paths, 6 release build legs + Nix pre-run, 10 asset names, SDK pins, dispatch isolation, version input and alpha detection"
