import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:forui/forui.dart';
import 'package:keytao/src/rust/api/types.dart';
import 'package:keytao/src/ui/candidate_panel_preview.dart';
import 'package:keytao/src/ui/color_picker.dart';
import 'package:keytao/src/ui/mobile_keyboard_preview.dart';
import 'package:keytao/src/ui/theme.dart';
import 'package:keytao/src/ui/theme_editor.dart';

import 'view_fixture.dart';

class _ThemeEditorController extends ViewController {
  _ThemeEditorController(this.accent) : super(BridgePlatform.macOs);

  final Object? accent;

  @override
  Future<Map<String, dynamic>> getThemeEditor() async {
    final snapshot = await super.getThemeEditor();
    snapshot['source'] = {
      'version': 3,
      'meta': {'id': 'keytao.default', 'name': 'Default'},
    };
    for (final variant in ['resolvedLight', 'resolvedDark']) {
      snapshot[variant]['ui']['accentColor'] = accent;
    }
    return snapshot;
  }
}

void main() {
  for (final accent in <Object?>[null, 'invalid']) {
    testWidgets('unset or invalid accent stays unset: $accent', (tester) async {
      final controller = _ThemeEditorController(accent);
      addTearDown(controller.dispose);
      await tester.pumpWidget(
        KeyTaoTheme(
          platform: BridgePlatform.macOs,
          home: FScaffold(
            child: SingleChildScrollView(
              child: ThemeEditorSection(controller: controller),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();

      expect(tester.takeException(), isNull);
      expect(tester.widget<AccentPicker>(find.byType(AccentPicker)).value, '');
      expect(find.byType(CandidatePanelPreview), findsOneWidget);
      expect(find.byType(MobileKeyboardPreview), findsNothing);
      await tester.pumpWidget(const SizedBox.shrink());
    });
  }
}
