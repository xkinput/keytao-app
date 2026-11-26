import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:forui/forui.dart';
import 'package:keytao/src/rust/api/types.dart';
import 'package:keytao/src/ui/input_page.dart';
import 'package:keytao/src/ui/mobile_keyboard_preview.dart';
import 'package:keytao/src/ui/shell.dart';
import 'package:keytao/src/ui/theme.dart';

import 'view_fixture.dart';

Widget _testApp(Widget child) => KeyTaoTheme(
  platform: BridgePlatform.android,
  home: FScaffold(child: child),
);

String _sampleLayoutJson({
  required String id,
  required String name,
  bool hasMnemonics = false,
  bool withRowSpan = false,
}) => jsonEncode({
  'id': id,
  'name': name,
  'hasMnemonics': hasMnemonics,
  'numberRow': withRowSpan ? 'embedded' : 'auto',
  'keyGroups': {
    'row1': {
      'light': {'background': '#F5DEB3', 'foreground': '#263442'},
      'dark': {'background': '#4A3F2B', 'foreground': '#F5E9D3'},
    },
  },
  'height': 266,
  'candidateBarHeight': 52,
  'horizontalGap': 4,
  'verticalGap': 5,
  'outerInset': 5,
  'maxKeyHeight': 54,
  'rows': withRowSpan
      ? [
          [
            {'label': '1', 'value': '1'},
            {'label': '2', 'value': '2'},
          ],
          [
            {
              'label': '，',
              'rowSpan': 2,
              'stack': [
                {'label': '，'},
                {'label': '。'},
              ],
            },
            {
              'label': 'b',
              'group': 'row1',
              'slots': {
                'topLeft': 'Q',
                'topCenter': 'ˉ',
                'bottomCenter': 'a,an,ang 一',
              },
            },
          ],
          [
            {
              'label': 'd',
              'slots': {'topLeft': 'A', 'bottomCenter': 'ia,ian,iang'},
            },
          ],
        ]
      : [
          [
            {
              'label': 'q',
              'hint': '1',
              if (hasMnemonics)
                'mnemonic': {
                  'top': 'zh',
                  'bottomLines': ['iu ua'],
                  'tone': 'outer',
                },
            },
            {'label': 'w', 'hint': '2'},
          ],
          [
            {
              'label': '数',
              'hint': '符',
              'action': {'type': 'keyboardMode', 'value': 'numbers'},
            },
            {
              'label': '空格',
              'weight': 3.5,
              'action': {'type': 'space'},
            },
          ],
        ],
});

class _KeyboardTestController extends ViewController {
  _KeyboardTestController() : super(BridgePlatform.android);

  final List<String> appliedIds = [];

  @override
  Future<bool> applyKeyboardProfile(String id) async {
    appliedIds.add(id);
    keyboards = [
      for (final kb in keyboards)
        KeyboardSummaryDto(
          id: kb.id,
          name: kb.name,
          description: kb.description,
          builtIn: kb.builtIn,
          current: kb.id == id,
          hasMnemonics: kb.hasMnemonics,
          recommendedSchemas: kb.recommendedSchemas,
          previewLayoutJson: kb.previewLayoutJson,
        ),
    ];
    if (recommendedKeyboard != null) {
      final rec = recommendedKeyboard!;
      recommendedKeyboard = KeyboardSummaryDto(
        id: rec.id,
        name: rec.name,
        description: rec.description,
        builtIn: rec.builtIn,
        current: rec.id == id,
        hasMnemonics: rec.hasMnemonics,
        recommendedSchemas: rec.recommendedSchemas,
        previewLayoutJson: rec.previewLayoutJson,
      );
    }
    pendingRecommendedKeyboard = null;
    notifyListeners();
    return true;
  }
}

List<KeyboardSummaryDto> _sixKeyboards({String currentId = 'keytao.qwerty26'}) =>
    [
      KeyboardSummaryDto(
        id: 'keytao.qwerty26',
        name: '标准 26 键',
        description: '默认',
        builtIn: true,
        current: currentId == 'keytao.qwerty26',
        hasMnemonics: false,
        recommendedSchemas: const ['rime_mint'],
        previewLayoutJson: _sampleLayoutJson(
          id: 'keytao.qwerty26',
          name: '标准 26 键',
        ),
      ),
      KeyboardSummaryDto(
        id: 'keytao.keytao30',
        name: '键道6 键盘',
        description: '30键',
        builtIn: true,
        current: currentId == 'keytao.keytao30',
        hasMnemonics: true,
        recommendedSchemas: const ['keytao'],
        previewLayoutJson: _sampleLayoutJson(
          id: 'keytao.keytao30',
          name: '键道6 键盘',
          hasMnemonics: true,
        ),
      ),
      KeyboardSummaryDto(
        id: 'keytao.keytao26',
        name: '键道6 助记键盘',
        description: '26键助记',
        builtIn: true,
        current: currentId == 'keytao.keytao26',
        hasMnemonics: true,
        recommendedSchemas: const [],
        previewLayoutJson: _sampleLayoutJson(
          id: 'keytao.keytao26',
          name: '键道6 助记键盘',
          hasMnemonics: true,
        ),
      ),
      KeyboardSummaryDto(
        id: 'keytao.keydo30',
        name: '键道·我流',
        description: '我流',
        builtIn: true,
        current: currentId == 'keytao.keydo30',
        hasMnemonics: true,
        recommendedSchemas: const ['keydo'],
        previewLayoutJson: _sampleLayoutJson(
          id: 'keytao.keydo30',
          name: '键道·我流',
          hasMnemonics: true,
        ),
      ),
      KeyboardSummaryDto(
        id: 'keytao.lssp_3x5',
        name: '李氏三拼 3×5',
        description: '3x5',
        builtIn: true,
        current: currentId == 'keytao.lssp_3x5',
        hasMnemonics: false,
        recommendedSchemas: const ['triple_pinyin_lssp'],
        previewLayoutJson: _sampleLayoutJson(
          id: 'keytao.lssp_3x5',
          name: '李氏三拼 3×5',
          withRowSpan: true,
        ),
      ),
      KeyboardSummaryDto(
        id: 'keytao.lssp_4x4',
        name: '李氏三拼 4×4',
        description: '4x4',
        builtIn: true,
        current: currentId == 'keytao.lssp_4x4',
        hasMnemonics: false,
        recommendedSchemas: const [],
        previewLayoutJson: _sampleLayoutJson(
          id: 'keytao.lssp_4x4',
          name: '李氏三拼 4×4',
          withRowSpan: true,
        ),
      ),
    ];

void main() {
  testWidgets(
    'keyboard library shows 2-column preview cards, recommendation banner, and gates mnemonic switch',
    (tester) async {
      await tester.binding.setSurfaceSize(const Size(420, 900));
      addTearDown(() => tester.binding.setSurfaceSize(null));

      final c = _KeyboardTestController();
      c.keyboards = _sixKeyboards(currentId: 'keytao.qwerty26');
      c.recommendedKeyboard = c.keyboards[1]; // keytao30

      await tester.pumpWidget(
        _testApp(
          SizedBox(
            height: 850,
            child: SingleChildScrollView(
              child: ListenableBuilder(
                listenable: c,
                builder: (_, _) => InputPage(controller: c),
              ),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();

      // Recommendation banner visible.
      expect(
        find.byKey(const ValueKey('recommended-keyboard-banner')),
        findsOneWidget,
      );
      expect(find.text('推荐当前方案使用「键道6 键盘」'), findsOneWidget);

      // 2-column grid with all 6 keyboards and previews.
      expect(
        find.byKey(const ValueKey('keyboard-profile-grid')),
        findsOneWidget,
      );
      for (final kb in c.keyboards) {
        expect(find.byKey(ValueKey('keyboard-card-${kb.id}')), findsOneWidget);
        expect(find.text(kb.name), findsAtLeastNWidgets(1));
      }
      final leftCard = tester.getRect(
        find.byKey(const ValueKey('keyboard-card-keytao.qwerty26')),
      );
      final rightCard = tester.getRect(
        find.byKey(const ValueKey('keyboard-card-keytao.keytao30')),
      );
      expect(
        (leftCard.top - rightCard.top).abs(),
        lessThan(1.0),
        reason: 'First two keyboard cards should sit side by side in 2 columns',
      );
      expect(rightCard.left, greaterThan(leftCard.right));

      // Active keyboard is qwerty26 (no mnemonics) -> mnemonic switch hidden!
      expect(find.text('开启键盘助记'), findsNothing);

      // Switch to recommended keyboard (keytao30, hasMnemonics: true).
      await tester.tap(
        find.descendant(
          of: find.byKey(const ValueKey('recommended-keyboard-banner')),
          matching: find.text('切换'),
        ),
      );
      await tester.pumpAndSettle();
      expect(c.appliedIds, ['keytao.keytao30']);

      // Now mnemonic switch MUST appear!
      expect(find.text('开启键盘助记'), findsOneWidget);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'mobile shell prompts to switch to recommended keyboard with live preview',
    (tester) async {
      await tester.binding.setSurfaceSize(const Size(420, 860));
      addTearDown(() => tester.binding.setSurfaceSize(null));

      final c = _KeyboardTestController();
      c.keyboards = _sixKeyboards(currentId: 'keytao.qwerty26');
      c.pendingRecommendedKeyboard = c.keyboards[3]; // keydo30

      await tester.pumpWidget(_testApp(AppShell(controller: c)));
      await tester.pumpAndSettle();

      expect(
        find.byKey(const ValueKey('recommended-keyboard-dialog')),
        findsOneWidget,
      );
      expect(find.text('切换推荐键盘'), findsOneWidget);
      expect(find.textContaining('键道·我流'), findsAtLeastNWidgets(1));
      expect(
        find.descendant(
          of: find.byKey(const ValueKey('recommended-keyboard-dialog')),
          matching: find.byType(MobileKeyboardPreview),
        ),
        findsOneWidget,
      );

      await tester.tap(find.text('切换为 键道·我流'));
      await tester.pumpAndSettle();
      expect(c.appliedIds, ['keytao.keydo30']);
      expect(c.pendingRecommendedKeyboard, isNull);
      expect(tester.takeException(), isNull);
    },
  );
}
