import 'dart:io';

import '../platform/android_host.dart';
import '../rust/api/core.dart' as core;
import '../rust/api/types.dart';
import '../rust/frb_generated.dart';
import 'controller.dart';

const appVersion = String.fromEnvironment(
  'FLUTTER_BUILD_NAME',
  defaultValue: '1.2.1-alpha.42',
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

Future<({AppController controller, OnboardingDto onboarding})>
bootstrap() async {
  await RustLib.init();
  const android = AndroidHost();
  final platform = hostPlatform();
  final BridgeConfig config;
  if (platform == BridgePlatform.android) {
    final paths = await android.paths();
    config = BridgeConfig(
      dataDir: paths['dataDir'] as String,
      cacheDir: paths['cacheDir'] as String,
      resourceDir: paths['dataDir'] as String,
      userRootOverride: paths['keytaoRoot'] as String,
      appVersion: appVersion,
      platform: platform,
    );
  } else if (platform == BridgePlatform.macOs) {
    final home = Platform.environment['HOME'];
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
      resourceDir: File(Platform.resolvedExecutable).parent.uri
          .resolve('../Resources')
          .toFilePath(),
      appVersion: appVersion,
      platform: platform,
    );
  } else {
    throw UnsupportedError('当前支持 Android 和 macOS');
  }
  final info = await core.initCore(config: config);
  if (platform == BridgePlatform.android) await android.adoptAppLogger();
  final controller = AppController(info: info, android: android);
  try {
    await controller.connectEvents();
    final onboarding = await core.onboarding();
    await controller.refreshState();
    return (controller: controller, onboarding: onboarding);
  } catch (_) {
    controller.dispose();
    rethrow;
  }
}
