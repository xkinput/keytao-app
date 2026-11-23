import 'dart:async';

import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:forui/forui.dart';
import 'package:keytao/src/app/controller.dart';
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

class _AboutController extends AppController {
  _AboutController({required super.info});

  final openedUrls = <String>[];

  @override
  Future<bool> open(String value) async {
    openedUrls.add(value);
    return true;
  }
}

void main() {
  testWidgets('About version rows report updates and open their repositories', (
    tester,
  ) async {
    final core = _AboutCore();
    RustLib.initMock(api: core);
    addTearDown(RustLib.dispose);
    final controller =
        _AboutController(
            info: const BridgeInfo(
              platform: BridgePlatform.macOs,
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
    expect(find.text('KeyTao 输入法'), findsOneWidget);
    expect(find.text('KeyTao 是支持 librime 与 libjd 的跨平台输入法'), findsOneWidget);
    expect(find.text('KeyTao IM 版本'), findsOneWidget);
    expect(find.text('libjd-engine 版本'), findsOneWidget);
    expect(find.text('v0.14.0'), findsOneWidget);
    expect(find.widgetWithText(SettingRow, '检查更新'), findsNothing);
    expect(find.text('GitHub'), findsNothing);
    expect(find.text('键道目录'), findsNothing);

    for (final name in ['KeyTao', 'librime', 'libjd-engine']) {
      final link = find.byWidgetPredicate(
        (widget) =>
            widget is Icon && widget.semanticLabel == '打开 $name 的 GitHub 首页',
      );
      await tester.ensureVisible(link);
      await tester.tap(link);
      await tester.pump();
    }
    expect(controller.openedUrls, [
      'https://github.com/xkinput/keytao-app',
      'https://github.com/rime/librime',
      'https://github.com/hronro/ime-jd',
    ]);

    final updateButton = find.byKey(const ValueKey('check-app-update'));
    await tester.ensureVisible(updateButton);
    await tester.tap(updateButton);
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
    expect(controller.takePendingToast(), '已是最新版本');
    expect(find.byType(FCircularProgress), findsNothing);
    await tester.pump(const Duration(milliseconds: 300));
    await tester.pumpWidget(const SizedBox.shrink());
  });
}
