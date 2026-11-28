import 'dart:io';

import '../platform/android_host.dart';
import '../platform/ios_host.dart';
import '../rust/api/core.dart' as core;
import '../rust/api/timing.dart';
import '../rust/api/types.dart';
import '../rust/frb_generated.dart';
import 'controller.dart';
import 'operation_timing.dart';

const _desktopCommands = {
  '--deploy',
  '--reload',
  '--disable-engine',
  '--enable-engine',
};

Future<int?> runDesktopCommand(List<String> args) async {
  if (Platform.isAndroid ||
      Platform.isIOS ||
      !args.any(_desktopCommands.contains)) {
    return null;
  }
  if (args.length != 1) {
    stderr.writeln(
      'Usage: keytao-app --deploy | --reload | --disable-engine | --enable-engine',
    );
    return 64;
  }
  try {
    await RustLib.init();
    await core.initCore(config: await bridgeConfig(hostPlatform()));
    if (args.single == '--deploy' || args.single == '--reload') {
      final result = await core.deployDefault();
      if (!result.success) throw StateError(result.message);
      stdout.writeln(result.message);
      if (result.warning case final warning?) stderr.writeln(warning);
    } else {
      final enabled = args.single == '--enable-engine';
      await core.setInputEngineEnabled(enabled: enabled);
      stdout.writeln(enabled ? 'Input engine enabled.' : 'Input engine disabled.');
    }
    return 0;
  } catch (failure) {
    stderr.writeln(errorText(failure));
    return 1;
  }
}

// iOS build names must be numeric, so its build passes the full version here.
const appVersion = String.fromEnvironment(
  'KEYTAO_VERSION',
  defaultValue: String.fromEnvironment('FLUTTER_BUILD_NAME'),
);
const flutterVersion = String.fromEnvironment(
  'FLUTTER_VERSION',
  defaultValue: '3.47.5',
);

BridgePlatform hostPlatform() => switch (Platform.operatingSystem) {
  'macos' => BridgePlatform.macOs,
  'windows' => BridgePlatform.windows,
  'linux' => BridgePlatform.linux,
  'android' => BridgePlatform.android,
  'ios' => BridgePlatform.ios,
  _ => throw UnsupportedError(
    'Unsupported platform: ${Platform.operatingSystem}',
  ),
};

Future<({AppController controller, OnboardingDto onboarding})> bootstrap({
  List<String> args = const [],
  void Function()? onMigratingPackages,
}) async {
  final stopwatch = Stopwatch()..start();
  await RustLib.init();
  final rustLibInitUs = stopwatch.elapsedMicroseconds;
  const android = AndroidHost();
  final platform = hostPlatform();
  stopwatch.reset();
  final config = await bridgeConfig(platform);
  final bridgeConfigUs = stopwatch.elapsedMicroseconds;
  stopwatch.reset();
  final info = await core.initCore(config: config);
  if (platform != BridgePlatform.android && platform != BridgePlatform.ios) {
    recordBootstrapTimings(
      rustLibInitUs: rustLibInitUs,
      bridgeConfigUs: bridgeConfigUs,
      initCoreUs: stopwatch.elapsedMicroseconds,
    );
    enableAppOperationTiming();
  }
  if (platform == BridgePlatform.android) await android.adoptAppLogger();
  final controller = AppController(info: info, android: android);
  try {
    await measureAppOperation(
      AppOperationName.connectEvents,
      () => controller.connectEvents(args: args),
    );
    await measureAppOperation(
      AppOperationName.startPackageMigration,
      () => controller.startPackageMigration(onPending: onMigratingPackages),
    );
    final onboarding = await measureAppOperation(
      AppOperationName.onboarding,
      core.onboarding,
    );
    await measureAppOperation(
      AppOperationName.refreshState,
      controller.refreshState,
    );
    return (controller: controller, onboarding: onboarding);
  } catch (_) {
    controller.dispose();
    rethrow;
  }
}

Future<BridgeConfig> bridgeConfig(
  BridgePlatform platform, {
  Map<String, String>? environment,
  String? resolvedExecutable,
}) async {
  final env = environment ?? Platform.environment;
  final executable = resolvedExecutable ?? Platform.resolvedExecutable;
  final BridgeConfig config;
  if (platform == BridgePlatform.android) {
    final paths = await const AndroidHost().paths();
    config = BridgeConfig(
      dataDir: paths['dataDir'] as String,
      cacheDir: paths['cacheDir'] as String,
      resourceDir: paths['dataDir'] as String,
      userRootOverride: paths['keytaoRoot'] as String,
      appVersion: appVersion,
      platform: platform,
    );
  } else if (platform == BridgePlatform.macOs) {
    final home = env['HOME'];
    if (home == null || !home.startsWith('/')) throw StateError('无法读取用户目录');
    final data = Directory(
      '$home/Library/Application Support/ink.rea.keytao-app',
    );
    final cache = Directory('$home/Library/Caches/ink.rea.keytao-app');
    await data.create(recursive: true);
    await cache.create(recursive: true);
    config = BridgeConfig(
      dataDir: data.path,
      cacheDir: cache.path,
      resourceDir: File(executable).parent.uri
          .resolve('../Resources')
          .toFilePath(),
      appVersion: appVersion,
      platform: platform,
    );
  } else if (platform == BridgePlatform.ios) {
    final paths = await const IosHost().paths();
    config = BridgeConfig(
      dataDir: paths['dataDir'] as String,
      cacheDir: paths['cacheDir'] as String,
      resourceDir: paths['dataDir'] as String,
      userRootOverride: paths['userRoot'] as String,
      appVersion: appVersion,
      platform: platform,
    );
  } else if (platform == BridgePlatform.windows) {
    final local = env['LOCALAPPDATA'];
    if (local == null || local.isEmpty) {
      throw StateError('LOCALAPPDATA is unavailable');
    }
    // Tauri app_local_data_dir and app_cache_dir both use LocalAppData.
    final data = '$local\\ink.rea.keytao-app';
    config = BridgeConfig(
      dataDir: data,
      cacheDir: data,
      resourceDir: executable.substring(
        0,
        executable.lastIndexOf(RegExp(r'[/\\]')),
      ),
      appVersion: appVersion,
      platform: platform,
    );
  } else {
    String xdg(String key, String fallback) {
      final value = env[key];
      if (value != null && value.startsWith('/')) return value;
      final home = env['HOME'];
      if (home == null || !home.startsWith('/')) {
        throw StateError('HOME is unavailable');
      }
      return '$home/$fallback';
    }

    config = BridgeConfig(
      dataDir: '${xdg('XDG_DATA_HOME', '.local/share')}/ink.rea.keytao-app',
      cacheDir: '${xdg('XDG_CACHE_HOME', '.cache')}/ink.rea.keytao-app',
      // Dart resolves executable symlinks through the OS; a wrapper execs this
      // binary too, so resources stay beside /usr/lib/KeyTao/keytao-app.
      resourceDir: File(executable).parent.path,
      appVersion: appVersion,
      platform: platform,
    );
  }
  return config;
}
