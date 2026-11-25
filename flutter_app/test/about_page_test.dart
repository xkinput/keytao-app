import 'dart:async';

import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:forui/forui.dart';
import 'package:keytao/src/app/controller.dart';
import 'package:keytao/src/platform/android_host.dart';
import 'package:keytao/src/rust/api/types.dart';
import 'package:keytao/src/rust/frb_generated.dart';
import 'package:keytao/src/ui/about_page.dart';
import 'package:keytao/src/ui/theme.dart';
import 'package:keytao/src/ui/widgets.dart';

class _AboutCore implements RustLibApi {
  var update = Completer<AppUpdateDto>();
  final download = Completer<bool>();
  final events = StreamController<BridgeEvent>();
  final downloads = <String>[];
  final installerCalls = <({String version, bool restart})>[];
  final checkedChannels = <bool>[];
  bool _prereleases = false;

  @override
  Future<AppUpdateDto> crateApiCoreCheckAppUpdate() {
    checkedChannels.add(_prereleases);
    return update.future;
  }

  @override
  Future<void> crateApiCoreSetAppUpdatePrereleases({
    required bool enabled,
  }) async {
    _prereleases = enabled;
  }

  @override
  Stream<BridgeEvent> crateApiCoreCoreEvents() async* {
    yield const BridgeEvent(kind: BridgeEventKind.ready);
    yield* events.stream;
  }

  @override
  Future<void> crateApiCoreHandleAppArgs({required List<String> args}) async {}

  @override
  Future<bool> crateApiCoreDownloadAppUpdate({required String version}) {
    downloads.add(version);
    return download.future;
  }

  @override
  Future<String> crateApiCoreOpenAppUpdatePackage({
    required String version,
    required bool restart,
  }) async {
    installerCalls.add((version: version, restart: restart));
    if (restart) throw StateError('安装请求已取消');
    return r'C:\verified\update.exe';
  }

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

class _PlayHost extends AndroidHost {
  final openedUrls = <String>[];

  @override
  Future<String> appUpdateSource() async => 'play';

  @override
  Future<void> openUrl(String url) async => openedUrls.add(url);
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

    core.update = Completer<AppUpdateDto>()
      ..complete(
        const AppUpdateDto(
          currentVersion: '1',
          latestVersion: '2.0.0-alpha.1',
          hasUpdate: true,
          releaseUrl: 'https://example.com/releases/2.0.0-alpha.1',
          installMode: 'package',
        ),
      );
    final channel = find.byWidgetPredicate((widget) => widget is FSelect<bool>);
    await tester.ensureVisible(channel);
    expect(find.text('正式版'), findsOneWidget);
    await tester.tap(channel);
    await tester.pumpAndSettle();
    await tester.tap(find.text('预发布版'));
    await tester.pumpAndSettle();
    expect(core.checkedChannels, [false, true]);
    expect(controller.appUpdatePrereleases, isTrue);
    expect(
      find.descendant(
        of: find.byType(AppDialog),
        matching: find.text('v2.0.0-alpha.1'),
      ),
      findsOneWidget,
    );
    await tester.pumpWidget(const SizedBox.shrink());
  });

  testWidgets('the update dialog closes and the page shows download progress', (
    tester,
  ) async {
    final core = _AboutCore();
    RustLib.initMock(api: core);
    addTearDown(RustLib.dispose);
    addTearDown(core.events.close);
    final controller = AppController(
      info: const BridgeInfo(
        platform: BridgePlatform.windows,
        appVersion: '1',
        userRoot: '/isolated/keytao',
      ),
    );
    addTearDown(controller.dispose);
    await controller.connectEvents();
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
    final deploy = Completer<void>();
    final deployment = controller.run(() => deploy.future);
    await tester.pump();
    final check = find.byKey(const ValueKey('check-app-update'));
    await tester.ensureVisible(check);
    await tester.tap(check);
    await tester.pump();
    core.update.complete(
      const AppUpdateDto(
        currentVersion: '1',
        latestVersion: '2',
        hasUpdate: true,
        releaseUrl: 'https://example.com/releases/2',
        installMode: 'restart',
      ),
    );
    await tester.pumpAndSettle();
    final dialog = find.byType(AppDialog);
    await tester.tap(find.descendant(of: dialog, matching: find.text('下载升级包')));
    await tester.pump();
    await tester.pump(const Duration(seconds: 1));
    expect(core.downloads, ['2']);
    expect(dialog, findsNothing);
    expect(find.text('正在下载升级包…'), findsOneWidget);
    expect(find.text('重启并安装'), findsNothing);

    core.events.add(
      const BridgeEvent(
        kind: BridgeEventKind.installProgress,
        installProgress: InstallProgressDto(
          stage: 'app_update_downloading',
          percent: 40,
          message: '正在下载升级包',
        ),
      ),
    );
    await tester.pump();
    expect(find.text('正在下载升级包 40%'), findsOneWidget);
    expect(
      tester
          .widget<FDeterminateProgress>(find.byType(FDeterminateProgress))
          .value,
      .4,
    );
    expect(controller.installProgress, isNull);
    core.download.complete(true);
    await tester.pumpAndSettle();
    expect(find.byType(FDeterminateProgress), findsNothing);
    expect(find.text('重启并安装'), findsOneWidget);

    final restart = find.text('重启并安装');
    await tester.ensureVisible(restart);
    await tester.tap(restart);
    await tester.pumpAndSettle();
    expect(core.installerCalls, isEmpty);
    deploy.complete();
    expect(await deployment, isTrue);
    await tester.pumpAndSettle();

    await tester.ensureVisible(restart);
    await tester.tap(restart);
    await tester.pumpAndSettle();
    expect(find.text('安装请求已取消'), findsOneWidget);
    final open = find.text('打开升级包');
    await tester.ensureVisible(open);
    await tester.tap(open);
    await tester.pumpAndSettle();
    expect(core.installerCalls, [
      (version: '2', restart: true),
      (version: '2', restart: false),
    ]);
    await tester.pumpWidget(const SizedBox.shrink());
  });

  test('Play builds open the appropriate update link without an APK', () async {
    final core = _AboutCore();
    RustLib.initMock(api: core);
    addTearDown(RustLib.dispose);
    final android = _PlayHost();
    final controller = AppController(
      info: const BridgeInfo(
        platform: BridgePlatform.android,
        appVersion: '1',
        userRoot: '/isolated/keytao',
      ),
      android: android,
    );
    addTearDown(controller.dispose);
    core.update.complete(
      const AppUpdateDto(
        currentVersion: '1',
        latestVersion: '2',
        hasUpdate: true,
        releaseUrl: 'https://example.com/releases/2',
        installMode: 'package',
      ),
    );
    await controller.checkAppUpdateNow();

    expect(controller.appUpdateAction, AppUpdateAction.openLink);
    expect(await controller.openAppUpdate(), isTrue);

    const prereleaseUrl =
        'https://github.com/xkinput/keytao-app/releases/tag/v2.1.0-alpha.1';
    expect(await controller.setAppUpdatePrereleases(true), isTrue);
    core.update = Completer<AppUpdateDto>()
      ..complete(
        const AppUpdateDto(
          currentVersion: '1',
          latestVersion: '2.1.0-alpha.1',
          hasUpdate: true,
          releaseUrl: prereleaseUrl,
          installMode: 'package',
        ),
      );
    await controller.checkAppUpdateNow();
    expect(controller.appUpdateAction, AppUpdateAction.openLink);
    expect(await controller.openAppUpdate(), isTrue);
    expect(android.openedUrls, [
      'https://play.google.com/store/apps/details?id=ink.rea.keytao_app',
      prereleaseUrl,
    ]);
    expect(core.downloads, isEmpty);
  });
}
