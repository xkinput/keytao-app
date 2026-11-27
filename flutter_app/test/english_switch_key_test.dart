import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:forui/forui.dart';
import 'package:keytao/src/app/strings.dart';
import 'package:keytao/src/rust/api/types.dart';
import 'package:keytao/src/ui/english_switch_key_picker.dart';
import 'package:keytao/src/ui/input_page.dart';
import 'package:keytao/src/ui/theme.dart';

import 'view_fixture.dart';

class _SwitchKeyController extends ViewController {
  _SwitchKeyController(super.platform);

  final List<String> savedSwitchKeys = [];

  @override
  Future<bool> saveDesktopEnglishSwitchKey(String value) async {
    desktopEnglishSwitchKey = value;
    savedSwitchKeys.add(value);
    notifyListeners();
    return true;
  }
}

void main() {
  test('englishSwitchKeyLabel formats presets and custom combos per platform', () {
    expect(englishSwitchKeyLabel('schema', isMacOs: true), '跟随方案');
    expect(englishSwitchKeyLabel('Shift_L', isMacOs: true), '左 Shift');
    expect(englishSwitchKeyLabel('Alt_L', isMacOs: true), '左 Option');
    expect(englishSwitchKeyLabel('Alt_L', isMacOs: false), '左 Alt');
    expect(
      englishSwitchKeyLabel('Control+space', isMacOs: true),
      'Ctrl + Space',
    );
    expect(
      englishSwitchKeyLabel('Alt+grave', isMacOs: true),
      'Option + `',
    );
    expect(
      englishSwitchKeyLabel('Alt+grave', isMacOs: false),
      'Alt + `',
    );
    expect(
      englishSwitchKeyPresets(isMacOs: true).containsKey('Caps_Lock'),
      isFalse,
    );
    expect(
      englishSwitchKeyPresets(isMacOs: false).containsKey('Caps_Lock'),
      isTrue,
    );
    expect(
      formatSchemaEnglishSwitchKeys(
        ['Shift_R', 'Control_L', 'Control_R'],
        isMacOs: true,
      ),
      '右 Shift / 左右 Control',
    );
    expect(
      formatSchemaEnglishSwitchKeys(['Shift_L', 'Shift_R'], isMacOs: true),
      '左右 Shift',
    );
    expect(
      englishSwitchKeyLabel(
        'schema',
        isMacOs: true,
        schemaKeys: ['Shift_R', 'Control_L', 'Control_R'],
      ),
      '跟随方案（右 Shift / 左右 Control）',
    );
  });

  testWidgets('desktop input page shows schema switch key by default and records custom key', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(1280, 900);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);

    final controller = _SwitchKeyController(BridgePlatform.macOs)
      ..desktopSchemaEnglishSwitchKeys = const [
        'Shift_R',
        'Control_L',
        'Control_R',
      ];
    addTearDown(controller.dispose);

    await tester.pumpWidget(
      KeyTaoTheme(
        platform: controller.info.platform,
        home: FScaffold(
          child: ListenableBuilder(
            listenable: controller,
            builder: (_, _) => InputPage(controller: controller),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text(AppStrings.englishSwitchKey), findsOneWidget);
    expect(find.text('当前方案切换键：右 Shift / 左右 Control'), findsOneWidget);
    final recordButton = find.byKey(
      const ValueKey('english-switch-key-record-button'),
    );
    expect(recordButton, findsOneWidget);

    await tester.ensureVisible(recordButton);
    await tester.tap(recordButton);
    await tester.pumpAndSettle();

    expect(find.text('录制切换中英键'), findsOneWidget);

    // Tap Left Shift to capture Shift_L.
    await tester.sendKeyDownEvent(LogicalKeyboardKey.shiftLeft);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.shiftLeft);
    await tester.pump();
    expect(find.text('左 Shift'), findsWidgets);

    // Press Control+Space to capture combo.
    await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
    await tester.sendKeyDownEvent(LogicalKeyboardKey.space);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.space);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
    await tester.pump();
    expect(find.text('Ctrl + Space'), findsOneWidget);

    await tester.tap(find.text('确定'));
    await tester.pumpAndSettle();

    expect(controller.desktopEnglishSwitchKey, 'Control+space');
    expect(controller.savedSwitchKeys, ['Control+space']);
    expect(find.text('已自定义覆盖（方案默认：右 Shift / 左右 Control）'), findsOneWidget);
  });

  testWidgets('mobile input page omits desktop switch key selector', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(430, 900);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);

    final controller = _SwitchKeyController(BridgePlatform.android);
    addTearDown(controller.dispose);

    await tester.pumpWidget(
      KeyTaoTheme(
        platform: controller.info.platform,
        home: FScaffold(
          child: ListenableBuilder(
            listenable: controller,
            builder: (_, _) => InputPage(controller: controller),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text(AppStrings.englishSwitchKey), findsNothing);
    expect(
      find.byKey(const ValueKey('english-switch-key-record-button')),
      findsNothing,
    );
  });
}
