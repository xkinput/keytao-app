import 'dart:io';

import 'package:code_assets/code_assets.dart';
import 'package:flutter_rust_bridge_hooks/flutter_rust_bridge_hooks.dart';

// The bridge links keytao-app-core -> keytao-core -> librime. Hooks run with a
// scrubbed environment, so the vendored Rime SDK is passed to Cargo explicitly,
// and Android/Windows shared libraries are bundled as code assets. The macOS Runner
// build phase bundles librime and its plugins directly in Contents/Frameworks.
void main(List<String> args) async {
  await build(args, (input, output) async {
    final vendor = Directory.fromUri(input.packageRoot.resolve('../vendor/librime/'));
    final code = input.config.code;
    // librime-sys runs bindgen against the NDK sysroot; the NDK root is five levels
    // above the clang Flutter configured (toolchains/llvm/prebuilt/<host>/bin/clang).
    final ndkRoot = code.targetOS == OS.android
        ? code.cCompiler?.compiler.resolve('../../../../../').toFilePath()
        : null;
    final rime = _rimeFor(code.targetOS, code.targetArchitecture, vendor.path, ndkRoot);

    await FlutterRustBridgeNativeAssetsBuilder(
      cratePath: '../crates/keytao-app-bridge',
      extraCargoEnvironmentVariables: rime.cargoEnvironment,
    ).run(input: input, output: output);

    for (final library in rime.bundledLibraries) {
      final file = Uri.file(library).pathSegments.last;
      output.assets.code.add(
        CodeAsset(
          package: input.packageName,
          name: file.substring(0, file.indexOf('.')),
          linkMode: DynamicLoadingBundled(),
          file: await _copyForBundle(library, input),
        ),
      );
    }
  });
}

class _Rime {
  const _Rime(this.cargoEnvironment, [this.bundledLibraries = const []]);

  final Map<String, String> cargoEnvironment;
  final List<String> bundledLibraries;
}

_Rime _rimeFor(OS os, Architecture arch, String vendor, String? ndkRoot) {
  Map<String, String> sdk(String root) => {
        'RIME_INCLUDE_DIR': '$root/include',
        'RIME_LIB_DIR': '$root/lib',
        'BINDGEN_EXTRA_CLANG_ARGS': '-I$root/include',
      };

  switch (os) {
    case OS.macOS:
      final root = '${vendor}macos-universal';
      return _Rime(sdk(root));
    case OS.iOS:
      return _Rime({'KEYTAO_IOS_RIME_ROOT': '${vendor}ios'});
    case OS.android:
      final abi = switch (arch) {
        Architecture.arm64 => 'arm64-v8a',
        Architecture.arm => 'armeabi-v7a',
        Architecture.x64 => 'x86_64',
        Architecture.ia32 => 'x86',
        final other => throw UnsupportedError('librime has no Android $other build'),
      };
      final lib = '${vendor}android/$abi/lib';
      // librime.so needs the Fcitx5 support libraries and the shared C++ runtime.
      return _Rime({
        'KEYTAO_ANDROID_RIME_ROOT': '${vendor}android',
        'ANDROID_NDK_HOME': ?ndkRoot,
      }, [
        for (final name in ['librime.so', 'libFcitx5Core.so', 'libFcitx5Config.so', 'libFcitx5Utils.so', 'libc++_shared.so'])
          '$lib/$name',
      ]);
    case OS.windows:
      final windowsArch = switch (arch) {
        Architecture.x64 => 'x64',
        Architecture.arm64 => 'arm64',
        Architecture.ia32 => 'x86',
        final other => throw UnsupportedError('librime has no Windows $other build'),
      };
      final root = Directory.fromUri(Uri.directory(vendor).resolve('windows-$windowsArch'));
      final libName = arch == Architecture.arm64 ? 'rime-arm64' : 'rime';
      final bin = Directory.fromUri(root.uri.resolve('bin/'));
      final libraries = <String, String>{
        for (final file in bin.listSync().whereType<File>())
          if (file.path.toLowerCase().endsWith('.dll'))
            file.uri.pathSegments.last.toLowerCase(): file.path,
      };
      if (!libraries.containsKey('$libName.dll')) {
        throw StateError('Missing $libName.dll in ${bin.path}');
      }
      // Tauri ships bin/*.dll plus the matching MSVC redistributables staged by
      // build-windows-ime.ps1. Use the arch-specific runtime, never "current",
      // which may belong to a different architecture in a multi-arch build.
      final runtime = Uri.directory(vendor).resolve('../../target/keytao-windows-ime-runtime/$windowsArch/');
      for (final name in ['vcruntime140.dll', 'msvcp140.dll', if (arch != Architecture.ia32) 'vcruntime140_1.dll']) {
        if (libraries.containsKey(name)) continue;
        final file = File.fromUri(runtime.resolve(name));
        if (!file.existsSync()) {
          throw StateError('Missing $name; run scripts/build-windows-ime.ps1 -Arch $windowsArch first');
        }
        libraries[name] = file.path;
      }
      return _Rime({
        ...sdk(root.path),
        'KEYTAO_RIME_LIB_NAME': libName,
        'KEYTAO_RIME_DLL_NAME': '$libName.dll',
      }, libraries.values.toList()..sort());
    case OS.linux:
      // librime-dev in Dockerfile.linux-builder supplies these system paths.
      // container-build.sh packages librime, its dependencies and plugins in
      // runtime/lib; leave that closure to the Linux packager, not CodeAssets.
      return _Rime(sdk('/usr'));
    default:
      return const _Rime({});
  }
}

// Always hand Flutter a private copy: installing a code asset can delete or
// rewrite the file it was given, which once removed the vendored librime.1.dylib.
Future<Uri> _copyForBundle(String library, BuildInput input) async {
  final copy = input.outputDirectory.resolve(Uri.file(library).pathSegments.last);
  await File(library).copy(copy.toFilePath());
  return copy;
}
