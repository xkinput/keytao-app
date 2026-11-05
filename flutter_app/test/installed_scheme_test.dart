import 'package:flutter_test/flutter_test.dart';
import 'package:keytao/src/app/controller.dart';

void main() {
  test('xmjd schemas retain the recorded non-top package', () {
    expect(installedSchemeKey(['xmjd6'], 'xmjd-feiding'), 'xmjd-feiding');
  });

  test('xmjd schemas default to the top package without a record', () {
    expect(installedSchemeKey(['xmjd6'], null), 'xmjd');
  });

  test('xmjd schemas default to the top package for an unknown record', () {
    expect(installedSchemeKey(['xmjd6'], 'unknown'), 'xmjd');
  });

  test('keytao schemas ignore a stale non-top package record', () {
    expect(installedSchemeKey(['keytao'], 'xmjd-feiding'), 'keytao');
  });

  test('empty schemas do not restore a recorded package', () {
    expect(installedSchemeKey([], 'xmjd-feiding'), isNull);
  });
}
