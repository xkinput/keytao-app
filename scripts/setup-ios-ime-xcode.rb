#!/usr/bin/env ruby
# Patch the generated Tauri Apple XcodeGen project to include the KeyTao keyboard extension.
require "fileutils"
require "pathname"
require "yaml"

PROJECT_DIR = File.expand_path("..", __dir__)
DEFAULT_PROJECT_YML = File.join(PROJECT_DIR, "src-tauri", "gen", "apple", "project.yml")
LOCAL_XCODEGEN = File.join(PROJECT_DIR, ".cache", "bin", "xcodegen")
APP_GROUP = ENV.fetch("KEYTAO_IOS_APP_GROUP", "group.ink.rea.keytao-app")
APP_BUNDLE_ID = ENV.fetch("KEYTAO_IOS_APP_BUNDLE_ID", "ink.rea.keytao-app")
KEYBOARD_TARGET = ENV.fetch("KEYTAO_IOS_KEYBOARD_TARGET", "KeyTaoKeyboard")
KEYBOARD_BUNDLE_ID = ENV.fetch("KEYTAO_IOS_KEYBOARD_BUNDLE_ID", "#{APP_BUNDLE_ID}.keyboard")
DEVELOPMENT_TEAM = ENV["KEYTAO_IOS_DEVELOPMENT_TEAM"].to_s.strip
CODE_SIGN_IDENTITY = ENV.fetch("KEYTAO_IOS_CODE_SIGN_IDENTITY", "Apple Development")
DEFAULT_TAURI_XCODE_SCRIPT_LINE = 'pnpm tauri ios xcode-script -v --platform ${PLATFORM_DISPLAY_NAME:?} --sdk-root ${SDKROOT:?} --framework-search-paths "${FRAMEWORK_SEARCH_PATHS:?}" --header-search-paths "${HEADER_SEARCH_PATHS:?}" --gcc-preprocessor-definitions "${GCC_PREPROCESSOR_DEFINITIONS:-}" --configuration ${CONFIGURATION:?} ${FORCE_COLOR} ${ARCHS:?}'

RUNTIME_SETTINGS = {
  "KEYTAO_IOS_RUNTIME_NAME[sdk=iphoneos*]" => "iphoneos-arm64",
  "KEYTAO_IOS_RUNTIME_NAME[sdk=iphonesimulator*][arch=arm64]" => "iphonesimulator-arm64",
  "KEYTAO_IOS_RUNTIME_NAME[sdk=iphonesimulator*][arch=x86_64]" => "iphonesimulator-x86_64",
  "KEYTAO_IOS_RUNTIME_DIR" => "$(SRCROOT)/../../../target/keytao-ios-runtime/$(KEYTAO_IOS_RUNTIME_NAME)",
  "KEYTAO_IOS_SWIFTPM_BUILD_DIR" => "$(SRCROOT)/../../../crates/keytao-ios-ime/build/$(CONFIGURATION)$(EFFECTIVE_PLATFORM_NAME)",
  "HEADER_SEARCH_PATHS" => "$(inherited) $(KEYTAO_IOS_RUNTIME_DIR)/include $(SRCROOT)/../../../crates/keytao-core-ffi/include",
  "SWIFT_INCLUDE_PATHS" => "$(inherited) $(KEYTAO_IOS_SWIFTPM_BUILD_DIR)",
  "LIBRARY_SEARCH_PATHS" => "$(inherited) $(KEYTAO_IOS_RUNTIME_DIR)/lib",
  "OTHER_LDFLAGS" => "$(inherited) -lrime -lc++ -lz"
}.freeze

RESOLVE_RUNTIME_DIR_SCRIPT = <<~SCRIPT.strip
  resolve_keytao_ios_runtime_dir() {
    if [ -n "${KEYTAO_IOS_RUNTIME_DIR:-}" ] &&
       [ "${KEYTAO_IOS_RUNTIME_DIR%/}" != */target/keytao-ios-runtime ] &&
       { [ -d "${KEYTAO_IOS_RUNTIME_DIR:-}/include" ] || [ -d "${KEYTAO_IOS_RUNTIME_DIR:-}/rime-data" ]; }; then
      return 0
    fi

    for library_path in ${LIBRARY_SEARCH_PATHS:-}; do
      case "${library_path}" in
        */target/keytao-ios-runtime/lib|*/target/keytao-ios-runtime//lib)
          KEYTAO_IOS_RUNTIME_DIR="${library_path%/lib}"
          KEYTAO_IOS_RUNTIME_DIR="${KEYTAO_IOS_RUNTIME_DIR%/}"
          export KEYTAO_IOS_RUNTIME_DIR
          break
          ;;
        */target/keytao-ios-runtime/*/lib)
          KEYTAO_IOS_RUNTIME_DIR="${library_path%/lib}"
          export KEYTAO_IOS_RUNTIME_DIR
          if [ -d "${KEYTAO_IOS_RUNTIME_DIR}/rime-data" ]; then
            return 0
          fi
          ;;
      esac
    done

    case "${PLATFORM_NAME:-}" in
      iphonesimulator)
        case " ${ARCHS:-} " in
          *" arm64 "*) KEYTAO_IOS_RUNTIME_NAME="iphonesimulator-arm64" ;;
          *" x86_64 "*) KEYTAO_IOS_RUNTIME_NAME="iphonesimulator-x86_64" ;;
          *) KEYTAO_IOS_RUNTIME_NAME="iphonesimulator-arm64" ;;
        esac
        ;;
      iphoneos)
        KEYTAO_IOS_RUNTIME_NAME="iphoneos-arm64"
        ;;
      *)
        KEYTAO_IOS_RUNTIME_NAME="${KEYTAO_IOS_RUNTIME_NAME:-iphoneos-arm64}"
        ;;
    esac
    case "${KEYTAO_IOS_RUNTIME_DIR:-}" in
      */target/keytao-ios-runtime)
        KEYTAO_IOS_RUNTIME_DIR="${KEYTAO_IOS_RUNTIME_DIR}/${KEYTAO_IOS_RUNTIME_NAME}"
        ;;
      *)
        KEYTAO_IOS_RUNTIME_DIR="${SRCROOT}/../../../target/keytao-ios-runtime/${KEYTAO_IOS_RUNTIME_NAME}"
        ;;
    esac
    export KEYTAO_IOS_RUNTIME_NAME KEYTAO_IOS_RUNTIME_DIR
  }

  resolve_keytao_ios_runtime_dir
SCRIPT

COPY_RIME_DATA_SCRIPT = <<~SCRIPT.strip
  set -euo pipefail
  #{RESOLVE_RUNTIME_DIR_SCRIPT}
  if [ -d "${KEYTAO_IOS_RUNTIME_DIR}/rime-data" ]; then
    destination="${TARGET_BUILD_DIR}/${UNLOCALIZED_RESOURCES_FOLDER_PATH}/rime-data"
    rm -rf "${destination}"
    mkdir -p "${TARGET_BUILD_DIR}/${UNLOCALIZED_RESOURCES_FOLDER_PATH}"
    cp -R "${KEYTAO_IOS_RUNTIME_DIR}/rime-data" "${destination}"
  fi
SCRIPT

COPY_IOS_CONFIG_SCRIPT = <<~SCRIPT.strip
  set -euo pipefail
  config_source="${SRCROOT}/../../../crates/keytao-ios-ime/Sources/KeyTaoIOSIME/Resources/keytao_ios_ime.json"
  logo_source="${SRCROOT}/../../../crates/keytao-ios-ime/Sources/KeyTaoIOSIME/Resources/keytao-logo.png"
  destination="${TARGET_BUILD_DIR}/${UNLOCALIZED_RESOURCES_FOLDER_PATH}"
  rm -rf "${destination}/Resources"
  mkdir -p "${destination}"
  if [ -f "${config_source}" ]; then
    cp "${config_source}" "${destination}/keytao_ios_ime.json"
  fi
  if [ -f "${logo_source}" ]; then
    cp "${logo_source}" "${destination}/keytao-logo.png"
  fi
SCRIPT

NORMALIZE_APP_INFO_PLIST_SCRIPT = <<~SCRIPT.strip
  set -euo pipefail
  plist="${SRCROOT}/${INFOPLIST_FILE}"
  if [ -f "${plist}" ]; then
    plutil -replace CFBundleShortVersionString -string "${MARKETING_VERSION:-1.2.1}" "${plist}"
    plutil -replace CFBundleVersion -string "${CURRENT_PROJECT_VERSION:-1}" "${plist}"
  fi
SCRIPT

NORMALIZE_KEYBOARD_INFO_PLIST_SCRIPT = <<~SCRIPT.strip
  set -euo pipefail
  plist="${SRCROOT}/#{KEYBOARD_TARGET}/Info.plist"
  if [ -f "${plist}" ]; then
    plutil -replace CFBundleShortVersionString -string "${MARKETING_VERSION:-1.2.1}" "${plist}"
    plutil -replace CFBundleVersion -string "${CURRENT_PROJECT_VERSION:-1}" "${plist}"
  fi
SCRIPT

SIGN_EMBEDDED_KEYBOARD_SCRIPT = <<~SCRIPT.strip
  set -euo pipefail
  appex="${TARGET_BUILD_DIR}/${UNLOCALIZED_RESOURCES_FOLDER_PATH}/PlugIns/#{KEYBOARD_TARGET}.appex"
  if [ ! -d "${appex}" ]; then
    exit 0
  fi

  xcent=""
  if [ -d "${PROJECT_TEMP_DIR:-}" ]; then
    xcent="$(find "${PROJECT_TEMP_DIR}" -path "*/#{KEYBOARD_TARGET}.build/#{KEYBOARD_TARGET}.appex*.xcent" -type f -print -quit 2>/dev/null || true)"
  fi

  sign_identity="${EXPANDED_CODE_SIGN_IDENTITY:-}"
  if [ -z "${sign_identity}" ] || [ "${sign_identity}" = "Sign to Run Locally" ]; then
    sign_identity="-"
  fi

  if [ "${PLATFORM_NAME:-}" = "iphonesimulator" ]; then
    sign_identity="-"
    if [ -z "${xcent}" ] && [ -f "${SRCROOT}/#{KEYBOARD_TARGET}/KeyTaoKeyboardSimulator.entitlements" ]; then
      xcent="${SRCROOT}/#{KEYBOARD_TARGET}/KeyTaoKeyboardSimulator.entitlements"
    fi
  fi

  if [ -n "${xcent}" ]; then
    /usr/bin/codesign --force --sign "${sign_identity}" --identifier "#{KEYBOARD_BUNDLE_ID}" --entitlements "${xcent}" --timestamp=none --generate-entitlement-der "${appex}"
  else
    /usr/bin/codesign --force --sign "${sign_identity}" --identifier "#{KEYBOARD_BUNDLE_ID}" --timestamp=none --generate-entitlement-der "${appex}"
  fi
  /usr/bin/codesign --verify --deep --strict "${appex}"
SCRIPT

def die(message)
  warn "ERROR: #{message}"
  exit 1
end

def note(message)
  puts "==> #{message}"
end

def append_build_words(existing, required_words)
  words = []
  existing.to_s.split.each do |word|
    words << word unless words.include?(word)
  end
  words = ["$(inherited)"] if words.empty?
  required_words.each do |word|
    words << word unless words.include?(word)
  end
  words.join(" ")
end

def upsert_named_script(target, phase, name, script)
  scripts = target[phase]
  scripts = [] unless scripts.is_a?(Array)
  existing = scripts.find { |entry| entry.is_a?(Hash) && entry["name"] == name }
  payload = {
    "name" => name,
    "script" => script,
    "basedOnDependencyAnalysis" => false
  }
  if existing
    existing.merge!(payload)
  else
    scripts << payload
  end
  target[phase] = scripts
end

def remove_named_script(target, phase, name)
  scripts = target[phase]
  return unless scripts.is_a?(Array)

  target[phase] = scripts.reject { |entry| entry.is_a?(Hash) && entry["name"] == name }
end

def ios_runtime_wrapped_script(tauri_line)
  <<~SCRIPT.strip
    #{RESOLVE_RUNTIME_DIR_SCRIPT}
    export KEYTAO_IOS_RIME_ROOT="${KEYTAO_IOS_RUNTIME_DIR:?}"
    export RIME_INCLUDE_DIR="${KEYTAO_IOS_RUNTIME_DIR:?}/include"
    export RIME_LIB_DIR="${KEYTAO_IOS_RUNTIME_DIR:?}/lib"
    export KEYTAO_RIME_SHARED_DATA_DIR="${KEYTAO_IOS_RUNTIME_DIR:?}/rime-data"
    export RIME_SHARED_DATA_DIR="${KEYTAO_IOS_RUNTIME_DIR:?}/rime-data"
    #{tauri_line}
  SCRIPT
end

def usage
  puts <<~USAGE
    Usage: scripts/setup-ios-ime-xcode.rb [--project-yml PATH] [--no-generate]

    Patches src-tauri/gen/apple/project.yml after `pnpm tauri ios init` so the
    generated Xcode project embeds the KeyTao custom keyboard extension.
  USAGE
end

project_yml = DEFAULT_PROJECT_YML
generate = true

until ARGV.empty?
  case ARGV.shift
  when "--project-yml"
    project_yml = File.expand_path(ARGV.shift || die("--project-yml requires a value"))
  when "--no-generate"
    generate = false
  when "-h", "--help"
    usage
    exit 0
  else
    die "unknown option"
  end
end

unless File.file?(project_yml)
  die "#{project_yml} does not exist. Run `pnpm tauri ios init --ci --skip-targets-install` first."
end

data = YAML.load_file(project_yml) || {}
targets = data["targets"] ||= {}
packages = data["packages"] ||= {}

app_target_name =
  ENV["KEYTAO_IOS_APP_TARGET"] ||
  targets.find { |_name, target| target.is_a?(Hash) && %w[application app].include?(target["type"].to_s) }&.first ||
  targets.find { |name, target| target.is_a?(Hash) && name.to_s.downcase.include?("keytao") && target["type"].to_s !~ /extension/ }&.first

die "cannot find the containing app target in #{project_yml}; set KEYTAO_IOS_APP_TARGET" unless app_target_name

gen_dir = File.dirname(project_yml)
app_entitlements_path =
  if app_target = targets[app_target_name]
    path = app_target.dig("entitlements", "path")
    path && !path.empty? ? File.expand_path(path, gen_dir) : File.join(gen_dir, "KeyTaoApp.generated.entitlements")
  else
    File.join(gen_dir, "KeyTaoApp.generated.entitlements")
  end
app_simulator_entitlements_path = File.join(gen_dir, "KeyTaoAppSimulator.generated.entitlements")

app_group_entitlements_plist = <<~PLIST
  <?xml version="1.0" encoding="UTF-8"?>
  <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
  <plist version="1.0">
  <dict>
    <key>com.apple.security.application-groups</key>
    <array>
      <string>#{APP_GROUP}</string>
    </array>
  </dict>
  </plist>
PLIST

simulator_entitlements_plist = <<~PLIST
  <?xml version="1.0" encoding="UTF-8"?>
  <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
  <plist version="1.0">
  <dict>
  </dict>
  </plist>
PLIST

FileUtils.mkdir_p(File.dirname(app_entitlements_path))
File.write(app_entitlements_path, app_group_entitlements_plist)
File.write(app_simulator_entitlements_path, simulator_entitlements_plist)

keyboard_source_dir = File.join(gen_dir, KEYBOARD_TARGET)
FileUtils.mkdir_p(keyboard_source_dir)
keyboard_simulator_entitlements_path = File.join(keyboard_source_dir, "KeyTaoKeyboardSimulator.entitlements")
File.write(keyboard_simulator_entitlements_path, simulator_entitlements_plist)
File.write(File.join(keyboard_source_dir, "KeyTaoKeyboardPrincipalViewController.swift"), <<~SWIFT)
  import Foundation
  import KeyTaoIOSIME

  @objc(KeyTaoKeyboardPrincipalViewController)
  public final class KeyTaoKeyboardPrincipalViewController: KeyTaoIOSIME.KeyTaoKeyboardViewController {}
SWIFT

packages["KeyTaoIOSIME"] = {
  "path" => "../../../crates/keytao-ios-ime"
}

keyboard_info_properties = {
  "CFBundleDisplayName" => "KeyTao 输入法",
  "CFBundleShortVersionString" => "$(MARKETING_VERSION)",
  "CFBundleVersion" => "$(CURRENT_PROJECT_VERSION)",
  "CFBundleIcons" => {
    "CFBundlePrimaryIcon" => {
      "CFBundleIconName" => "AppIcon",
      "CFBundleIconFiles" => [
        "AppIcon60x60"
      ]
    }
  },
  "NSExtension" => {
    "NSExtensionPointIdentifier" => "com.apple.keyboard-service",
    "NSExtensionPrincipalClass" => "KeyTaoKeyboardPrincipalViewController",
    "NSExtensionAttributes" => {
      "IsASCIICapable" => true,
      "PrimaryLanguage" => "zh-Hans",
      "RequestsOpenAccess" => true
    }
  }
}

keyboard_settings = {
  "PRODUCT_BUNDLE_IDENTIFIER" => KEYBOARD_BUNDLE_ID,
  "PRODUCT_NAME" => "KeyTaoKeyboard",
  "SKIP_INSTALL" => "YES",
  "CODE_SIGN_ENTITLEMENTS[sdk=iphoneos*]" => "../../../crates/keytao-ios-ime/Resources/KeyTaoKeyboard.entitlements",
  "CODE_SIGN_ENTITLEMENTS[sdk=iphonesimulator*]" => "#{KEYBOARD_TARGET}/KeyTaoKeyboardSimulator.entitlements",
  "CODE_SIGN_INJECT_BASE_ENTITLEMENTS[sdk=iphonesimulator*]" => "NO",
  "CODE_SIGN_IDENTITY[sdk=iphonesimulator*]" => "-",
  "CODE_SIGN_STYLE[sdk=iphonesimulator*]" => "Manual",
  "DEVELOPMENT_TEAM[sdk=iphonesimulator*]" => "",
  "SWIFT_VERSION" => "5.0",
  "APPLICATION_EXTENSION_API_ONLY" => "YES",
  "IPHONEOS_DEPLOYMENT_TARGET" => "15.0",
  "ARCHS" => "arm64",
  "VALID_ARCHS" => "arm64",
  "MARKETING_VERSION" => "1.2.1",
  "CURRENT_PROJECT_VERSION" => "1",
  "ASSETCATALOG_COMPILER_APPICON_NAME" => "AppIcon",
}.merge(RUNTIME_SETTINGS).merge(
  "OTHER_LDFLAGS" => "$(inherited) -lkeytao_core_ffi -lc++ -liconv -lz"
)
unless DEVELOPMENT_TEAM.empty?
  keyboard_settings["CODE_SIGN_STYLE"] = "Automatic"
  keyboard_settings["DEVELOPMENT_TEAM"] = DEVELOPMENT_TEAM
  keyboard_settings["CODE_SIGN_IDENTITY"] = CODE_SIGN_IDENTITY
end

targets[KEYBOARD_TARGET] = {
  "type" => "app-extension",
  "platform" => "iOS",
  "deploymentTarget" => "15.0",
  "info" => {
    "path" => "#{KEYBOARD_TARGET}/Info.plist",
    "properties" => keyboard_info_properties
  },
  "sources" => [
    {
      "path" => "Assets.xcassets"
    },
    {
      "path" => "#{KEYBOARD_TARGET}/KeyTaoKeyboardPrincipalViewController.swift"
    }
  ],
  "settings" => {
    "base" => keyboard_settings
  },
  "dependencies" => [
    {
      "package" => "KeyTaoIOSIME",
      "product" => "KeyTaoIOSIME"
    }
  ]
}

app_target = targets[app_target_name]
app_target.delete("entitlements")
app_target["settings"] ||= {}
app_target["settings"]["base"] ||= {}
app_target["info"] ||= {}
app_target["info"]["properties"] ||= {}
app_target["info"]["properties"]["CFBundleDisplayName"] = "KeyTao"
app_target["info"]["properties"]["CFBundleShortVersionString"] = "$(MARKETING_VERSION)"
app_target["info"]["properties"]["CFBundleVersion"] = "$(CURRENT_PROJECT_VERSION)"
app_target["settings"]["base"]["MARKETING_VERSION"] = "1.2.1"
app_target["settings"]["base"]["CURRENT_PROJECT_VERSION"] = "1"
app_target["settings"]["base"]["ASSETCATALOG_COMPILER_APPICON_NAME"] = "AppIcon"
unless DEVELOPMENT_TEAM.empty?
  app_target["settings"]["base"]["CODE_SIGN_STYLE"] = "Automatic"
  app_target["settings"]["base"]["DEVELOPMENT_TEAM"] = DEVELOPMENT_TEAM
  app_target["settings"]["base"]["CODE_SIGN_IDENTITY"] = CODE_SIGN_IDENTITY
end
RUNTIME_SETTINGS.each do |key, value|
  existing = app_target["settings"]["base"][key].to_s
  if key.start_with?("KEYTAO_IOS_")
    app_target["settings"]["base"][key] = value
  elsif key == "OTHER_LDFLAGS"
    app_target["settings"]["base"][key] = append_build_words(existing, %w[-lrime -lc++ -lz])
  else
    app_target["settings"]["base"][key] = append_build_words(existing, value.split - ["$(inherited)"])
  end
end
app_target["settings"]["base"].delete("CODE_SIGN_ENTITLEMENTS")
app_target["settings"]["base"]["CODE_SIGN_ENTITLEMENTS[sdk=iphoneos*]"] =
  Pathname.new(app_entitlements_path).relative_path_from(Pathname.new(gen_dir)).to_s
app_target["settings"]["base"]["CODE_SIGN_ENTITLEMENTS[sdk=iphonesimulator*]"] =
  Pathname.new(app_simulator_entitlements_path).relative_path_from(Pathname.new(gen_dir)).to_s
app_target["settings"]["base"]["CODE_SIGN_INJECT_BASE_ENTITLEMENTS[sdk=iphonesimulator*]"] = "NO"
app_target["settings"]["base"]["CODE_SIGN_IDENTITY[sdk=iphonesimulator*]"] = "-"
app_target["settings"]["base"]["CODE_SIGN_STYLE[sdk=iphonesimulator*]"] = "Manual"
app_target["settings"]["base"]["DEVELOPMENT_TEAM[sdk=iphonesimulator*]"] = ""
%w[
  LIBRARY_SEARCH_PATHS[arch=arm64]
  LIBRARY_SEARCH_PATHS[arch=x86_64]
].each do |key|
  existing = app_target["settings"]["base"][key].to_s
  next if existing.include?("$(KEYTAO_IOS_RUNTIME_DIR)/lib")

  app_target["settings"]["base"][key] = existing.empty? ? "$(inherited) $(KEYTAO_IOS_RUNTIME_DIR)/lib" : "#{existing} $(KEYTAO_IOS_RUNTIME_DIR)/lib"
end
if app_target["preBuildScripts"].is_a?(Array)
  app_target["preBuildScripts"].each do |script|
    next unless script.is_a?(Hash)

    script_body = script["script"].to_s
    tauri_line = script_body.lines.find { |line| line.include?("tauri ios xcode-script") }&.strip
    if tauri_line.nil? &&
       script["name"].to_s == "Build Rust Code" &&
       script_body.include?("Using prebuilt libapp.a")
      tauri_line = DEFAULT_TAURI_XCODE_SCRIPT_LINE
    end
    next unless tauri_line

    script["script"] = ios_runtime_wrapped_script(tauri_line)
  end
end
upsert_named_script(app_target, "postBuildScripts", "Normalize KeyTao App Info", NORMALIZE_APP_INFO_PLIST_SCRIPT)
upsert_named_script(app_target, "postBuildScripts", "Copy KeyTao Rime Data", COPY_RIME_DATA_SCRIPT)
upsert_named_script(app_target, "postBuildScripts", "Sign Embedded KeyTao Keyboard", SIGN_EMBEDDED_KEYBOARD_SCRIPT)
app_target["dependencies"] ||= []
keyboard_dependency = app_target["dependencies"].find { |dependency| dependency.is_a?(Hash) && dependency["target"] == KEYBOARD_TARGET }
unless keyboard_dependency
  keyboard_dependency = { "target" => KEYBOARD_TARGET }
  app_target["dependencies"] << keyboard_dependency
end
keyboard_dependency["embed"] = true
keyboard_dependency["codeSign"] = true

remove_named_script(targets[KEYBOARD_TARGET], "preBuildScripts", "Stage KeyTao SwiftPM Resources")
upsert_named_script(targets[KEYBOARD_TARGET], "preBuildScripts", "Normalize KeyTao Keyboard Info", NORMALIZE_KEYBOARD_INFO_PLIST_SCRIPT)
upsert_named_script(targets[KEYBOARD_TARGET], "postBuildScripts", "Copy KeyTao iOS Config", COPY_IOS_CONFIG_SCRIPT)
upsert_named_script(targets[KEYBOARD_TARGET], "postBuildScripts", "Copy KeyTao Rime Data", COPY_RIME_DATA_SCRIPT)

File.write(project_yml, YAML.dump(data))
note "Patched #{project_yml} with #{KEYBOARD_TARGET} and App Group #{APP_GROUP}"

if generate
  xcodegen = ENV["XCODEGEN"] || (File.executable?(LOCAL_XCODEGEN) ? LOCAL_XCODEGEN : "xcodegen")
  unless system(xcodegen, "--version", out: File::NULL, err: File::NULL)
    die "xcodegen is required to regenerate the Xcode project. Install it or place it at #{LOCAL_XCODEGEN}, then rerun this script."
  end
  Dir.chdir(gen_dir) do
    system(xcodegen, "generate", "--spec", "project.yml") || die("xcodegen generate failed")
  end
  note "Regenerated Xcode project in #{gen_dir}"
end
