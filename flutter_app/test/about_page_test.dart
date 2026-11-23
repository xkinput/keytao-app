import 'dart:async';

import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:forui/forui.dart';
import 'package:keytao/src/app/controller.dart';
import 'package:keytao/src/rust/api/core.dart' show LibjdStatusDto;
import 'package:keytao/src/rust/api/types.dart';
import 'package:keytao/src/rust/frb_generated.dart';
import 'package:keytao/src/ui/about_page.dart';
import 'package:keytao/src/ui/theme.dart';
import 'package:keytao/src/ui/widgets.dart';

class _AboutCore implements RustLibApi {
  final update = Completer<AppUpdateDto>();

  @override
  Future<AppUpdateDto> crateApiCoreCheckAppUpdate() => update.future;

  @override
  dynamic noSuchMethod(Invocation invocation) =>
      throw StateError('Unexpected bridge call: ${invocation.memberName}');
}

void main() {
  testWidgets('About update row checks and reports the current version', (
    tester,
  ) async {
    final core = _AboutCore();
    RustLib.initMock(api: core);
    addTearDown(RustLib.dispose);
    final controller =
        AppController(
            info: const BridgeInfo(
              platform: BridgePlatform.android,
              appVersion: '1',
              userRoot: '/isolated/keytao',
            ),
          )
          ..libjd = const LibjdStatusDto(
            available: false,
            installedVersion: 'v0.14.0',
          );
    addTearDown(controller.dispose);
    await tester.pumpWidget(
      KeyTaoTheme(
        platform: controller.info.platform,
        home: FScaffold(
          child: ListenableBuilder(
            listenable: controller,
            builder: (_, _) => AboutPage(controller: controller),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('libjd-engine 版本'), findsOneWidget);
    expect(find.text('v0.14.0'), findsOneWidget);
    expect(find.widgetWithText(SettingRow, '检查更新'), findsOneWidget);
    await tester.tap(find.text('检查更新'));
    await tester.pump();
    expect(controller.checkingAppUpdate, isTrue);
    expect(find.byType(FCircularProgress), findsOneWidget);
    core.update.complete(
      const AppUpdateDto(
        currentVersion: '1',
        latestVersion: '1',
        hasUpdate: false,
        releaseUrl: 'https://example.com/releases/1',
      ),
    );
    await tester.pumpAndSettle();
    expect(controller.checkingAppUpdate, isFalse);
    expect(find.text('已是最新版本'), findsOneWidget);
    expect(find.byType(FCircularProgress), findsNothing);
    await tester.pumpWidget(const SizedBox.shrink());
  });
}
