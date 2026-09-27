import 'dart:async';
import 'dart:convert';
import 'dart:ffi';
import 'dart:io';

import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:keytao/main.dart' show hostPlatform;
import 'package:keytao/src/rust/api/core.dart';
import 'package:keytao/src/rust/api/types.dart';
import 'package:keytao/src/rust/frb_generated.dart';

void main() {
  test(
    'native bridge uses an empty isolated root and streams an offline action',
    () async {
      final root = await Directory.systemTemp.createTemp('keytao-bridge-test-');
      addTearDown(() => root.delete(recursive: true));
      await RustLib.init(externalLibrary: _testBridgeLibrary());
      addTearDown(RustLib.dispose);
      final config = BridgeConfig(
        dataDir: '${root.path}/state',
        cacheDir: '${root.path}/cache',
        resourceDir: '${root.path}/resources',
        appVersion: 'bridge-host-test',
        platform: hostPlatform(),
        userRootOverride: '${root.path}/rime',
      );
      await expectLater(onboarding(), throwsA(anything));
      final info = await initCore(config: config);
      expect(info.appVersion, 'bridge-host-test');
      expect(info.platform, hostPlatform());
      expect(info.userRoot, config.userRootOverride);
      expect((await onboarding()).completed, isFalse);
      final local = await checkLocalSchema();
      expect(local.installed, isFalse);
      expect(local.deployed, isFalse);
      expect(local.version, isNull);
      expect(local.schemas, isEmpty);

      // Ready is a subscription handshake, not the CoreEvent being tested.
      final ready = Completer<void>();
      final progress = Completer<BridgeEvent>();
      final subscription = coreEvents().listen((event) {
        if (event.kind == BridgeEventKind.ready && !ready.isCompleted) {
          ready.complete();
        }
        if (event.kind == BridgeEventKind.installProgress &&
            !progress.isCompleted) {
          progress.complete(event);
        }
      });
      // FRB 2.13 can leave cancellation pending while Rust holds an idle sink.
      addTearDown(() {
        unawaited(subscription.cancel());
      });
      await ready.future.timeout(const Duration(seconds: 10));
      // Core rejects this scheme key before constructing or sending an HTTP request.
      await expectLater(
        installLatestScheme(scheme: 'invalid-offline-test-scheme'),
        throwsA(predicate((error) => error.toString().contains('不支持的方案'))),
      );
      final event = await progress.future.timeout(const Duration(seconds: 10));
      expect(event.installProgress!.stage, 'fetching');
      expect(event.installProgress!.percent, 0);
      expect(event.installProgress!.message, isNotEmpty);
      expect(event.deployProgress, isNull);
      expect(event.windowsImeStatus, isNull);
      expect(await Directory(config.userRootOverride!).exists(), isFalse);
      expect(await Directory(config.cacheDir).exists(), isFalse);

      await completeOnboarding();
      expect((await onboarding()).completed, isTrue);
      final persisted = jsonDecode(
        await File('${config.dataDir}/app-state.json').readAsString(),
      ) as Map<String, dynamic>;
      expect(persisted['onboarding_completed'], isTrue);
      await expectLater(
        initCore(config: config),
        throwsA(
          predicate(
            (error) => error.toString().contains('already initialized'),
          ),
        ),
      );
    },
  );
}

// flutter test places native assets under build/native_assets/<os>/ as plain
// libraries, where FRB's default loader (built for app bundles) does not look.
ExternalLibrary _testBridgeLibrary() {
  final dir = '${Directory.current.path}/build/native_assets';
  if (Platform.isMacOS) {
    // The app bundle ships librime from the Runner build phase, not native
    // assets; preload the vendored copy so @rpath/librime.1.dylib resolves.
    DynamicLibrary.open('${Directory.current.path}/../vendor/librime/macos-universal/lib/librime.1.dylib');
    return ExternalLibrary.open('$dir/macos/libkeytao_app_bridge.dylib');
  }
  if (Platform.isLinux) return ExternalLibrary.open('$dir/linux/libkeytao_app_bridge.so');
  if (Platform.isWindows) return ExternalLibrary.open('$dir/windows/keytao_app_bridge.dll');
  throw UnsupportedError('No host test library for ${Platform.operatingSystem}');
}
