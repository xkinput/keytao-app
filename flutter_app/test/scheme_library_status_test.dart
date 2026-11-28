import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:forui/forui.dart';
import 'package:keytao/src/app/controller.dart';
import 'package:keytao/src/app/options.dart';
import 'package:keytao/src/rust/api/types.dart';
import 'package:keytao/src/rust/frb_generated.dart';
import 'package:keytao/src/ui/input_page.dart';
import 'package:keytao/src/ui/scheme_page.dart';
import 'package:keytao/src/ui/theme.dart';
import 'package:keytao/src/ui/widgets.dart';

import 'view_fixture.dart';

const _saveFailure = '方案偏好保存失败';
const _installFailure = '方案下载失败：压缩包损坏';
const _download = InstallProgressDto(
  stage: 'downloading',
  percent: 42,
  message: '正在下载方案',
);

class _StatusCore implements RustLibApi {
  int cancelCalls = 0;
  int settingsWrites = 0;

  @override
  Future<void> crateApiCoreCancelSchemeDownload() async {
    cancelCalls++;
  }

  @override
  Future<String> crateApiCoreSetSchemeSyncPreferences({
    bool? followAccountSync,
    String? schemeKey,
    bool? sync_,
    String? settingsJson,
  }) async {
    settingsWrites++;
    throw StateError(_saveFailure);
  }

  @override
  dynamic noSuchMethod(Invocation invocation) =>
      throw StateError('Unexpected bridge call: ${invocation.memberName}');
}

class _StatusController extends ViewController {
  _StatusController([super.platform = BridgePlatform.macOs]);

  void update(VoidCallback change) {
    change();
    notifyListeners();
  }

  @override
  Future<void> ensureRemoteSchemeLogo(
    String key, {
    String? repoUrl,
    String? branch,
    bool? addonInstalled,
  }) async {}
}

Future<void> _pumpPage(
  WidgetTester tester,
  _StatusController controller,
) async {
  tester.view.devicePixelRatio = 1;
  tester.view.physicalSize = const Size(1200, 1000);
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  await tester.pumpWidget(
    KeyTaoTheme(
      platform: controller.info.platform,
      home: FScaffold(
        child: ListenableBuilder(
          listenable: controller,
          builder: (_, _) => controller.info.platform == BridgePlatform.macOs
              ? InputPage(controller: controller)
              : SchemePage(controller: controller),
        ),
      ),
    ),
  );
  await tester.pumpAndSettle();
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  late _StatusCore api;

  setUp(() {
    api = _StatusCore();
    RustLib.initMock(api: api);
  });

  tearDown(() => RustLib.dispose());

  testWidgets('macOS library can cancel a scheme download while busy', (
    tester,
  ) async {
    final c = _StatusController()
      ..busy = true
      ..operation = OperationKind.install
      ..installingSchemeKey = 'txjx'
      ..installProgress = _download;
    addTearDown(c.dispose);
    await _pumpPage(tester, c);

    final status = find.byKey(const ValueKey('scheme-library-status'));
    final cancel = find.widgetWithText(ActionButton, AppStrings.cancel);
    expect(find.text('远程方案'), findsOneWidget);
    expect(c.canCancelDownload, isTrue);
    expect(status, findsOneWidget);
    expect(cancel, findsOneWidget);
    expect(find.descendant(of: status, matching: cancel), findsOneWidget);
    expect(tester.widget<ActionButton>(cancel).onPress, isNotNull);
    await tester.ensureVisible(cancel);
    await tester.pumpAndSettle();
    await tester.tap(cancel);
    await tester.pumpAndSettle();
    expect(api.cancelCalls, 1);

    c.update(() {
      c.installProgress = const InstallProgressDto(
        stage: 'extracting',
        percent: 60,
        message: '正在解压方案',
      );
    });
    await tester.pumpAndSettle();
    expect(status, findsNothing);
    expect(cancel, findsNothing);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox.shrink());
  });

  testWidgets('macOS library leaves libjd download cancellation separate', (
    tester,
  ) async {
    final c = _StatusController();
    addTearDown(c.dispose);
    await _pumpPage(tester, c);
    final status = find.byKey(const ValueKey('scheme-library-status'));
    expect(status, findsNothing);

    c.update(() {
      c.busy = true;
      c.operation = OperationKind.libjd;
      c.installProgress = _download;
    });
    await tester.pumpAndSettle();
    expect(c.canCancelDownload, isTrue);
    expect(c.isManagingLibjd, isTrue);
    expect(status, findsNothing);
    expect(find.widgetWithText(ActionButton, AppStrings.cancel), findsNothing);
    expect(api.cancelCalls, 0);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox.shrink());
  });

  testWidgets(
    'macOS library shows a failed first download without a package error',
    (tester) async {
      final c = _StatusController();
      addTearDown(c.dispose);
      await _pumpPage(tester, c);
      final status = find.byKey(const ValueKey('scheme-library-status'));
      expect(status, findsNothing);
      expect(c.packageError, isNull);

      c.update(() => c.installError = _installFailure);
      await tester.pumpAndSettle();
      expect(c.packageError, isNull);
      expect(status, findsOneWidget);
      expect(find.text(_installFailure), findsOneWidget);
      expect(
        find.descendant(of: status, matching: find.text(_installFailure)),
        findsOneWidget,
      );
      expect(
        find.widgetWithText(ActionButton, AppStrings.cancel),
        findsNothing,
      );

      c.update(() => c.installError = null);
      await tester.pumpAndSettle();
      expect(find.text(_installFailure), findsNothing);
      expect(status, findsNothing);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );

  testWidgets(
    'macOS library shows hidden-scheme save errors without an account',
    (tester) async {
      final c = _StatusController();
      addTearDown(c.dispose);
      await _pumpPage(tester, c);
      expect(c.accountUser, isNull);
      expect(find.byKey(const ValueKey('scheme-library-status')), findsNothing);

      final hide = find.byKey(const ValueKey('hide-scheme-txjx'));
      await tester.ensureVisible(hide);
      await tester.pumpAndSettle();
      await tester.tap(hide);
      await tester.pumpAndSettle();
      expect(api.settingsWrites, 1);
      expect(c.schemeSyncError, _saveFailure);
      expect(find.text(_saveFailure), findsOneWidget);
      expect(
        find.descendant(
          of: find.byKey(const ValueKey('scheme-library-status')),
          matching: find.text(_saveFailure),
        ),
        findsOneWidget,
      );
      expect(
        find.widgetWithText(ActionButton, AppStrings.cancel),
        findsNothing,
      );

      c.update(() => c.schemeSyncError = null);
      await tester.pumpAndSettle();
      expect(find.text(_saveFailure), findsNothing);
      expect(find.byKey(const ValueKey('scheme-library-status')), findsNothing);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );

  for (final platform in [
    BridgePlatform.windows,
    BridgePlatform.linux,
    BridgePlatform.android,
    BridgePlatform.ios,
  ]) {
    testWidgets('${platform.name} keeps a single standalone scheme status', (
      tester,
    ) async {
      final c = _StatusController(platform)
        ..busy = true
        ..operation = OperationKind.install
        ..installingSchemeKey = 'txjx'
        ..installProgress = _download
        ..installError = _installFailure
        ..schemeSyncError = _saveFailure;
      addTearDown(c.dispose);
      await _pumpPage(tester, c);

      expect(find.byKey(const ValueKey('scheme-library-status')), findsNothing);
      expect(find.text(_saveFailure), findsOneWidget);
      expect(find.text(_installFailure), findsOneWidget);
      expect(
        find.descendant(
          of: find.byType(SchemeLibrarySections),
          matching: find.text(_saveFailure),
        ),
        findsNothing,
      );
      expect(
        find.descendant(
          of: find.byType(SchemeLibrarySections),
          matching: find.text(_installFailure),
        ),
        findsNothing,
      );
      final cancel = find.widgetWithText(ActionButton, AppStrings.cancel);
      expect(cancel, findsOneWidget);
      expect(tester.widget<ActionButton>(cancel).onPress, isNotNull);
      await tester.ensureVisible(cancel);
      await tester.pumpAndSettle();
      await tester.tap(cancel);
      await tester.pumpAndSettle();
      expect(api.cancelCalls, 1);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
    });
  }
}
