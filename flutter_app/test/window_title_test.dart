import 'dart:ui';

import 'package:flutter_test/flutter_test.dart';
import 'package:keytao/src/app/controller.dart';

void main() {
  test(
    'window title follows the system language and preserves the page name',
    () {
      for (final locale in const [
        Locale('zh'),
        Locale('zh', 'CN'),
        Locale.fromSubtags(
          languageCode: 'zh',
          scriptCode: 'Hant',
          countryCode: 'TW',
        ),
      ]) {
        expect(formatWindowTitle('输入法', locale), '输入法 · KeyTao 输入法');
      }
      for (final locale in const [
        Locale('en', 'US'),
        Locale('ja'),
        Locale('de'),
      ]) {
        expect(formatWindowTitle('方案', locale), '方案 · KeyTao Input Method');
      }
    },
  );
}
