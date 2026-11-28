import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:keytao/src/app/uninstaller.dart';

void main() {
  test('clears only KeyTao-specific personal data', () async {
    final home = await Directory.systemTemp.createTemp('keytao-uninstaller-');
    addTearDown(() => home.delete(recursive: true));

    final profile = Directory('${home.path}/Library/keytao');
    final support = Directory(
      '${home.path}/Library/Application Support/ink.rea.keytao-app',
    );
    final cache = Directory('${home.path}/Library/Caches/ink.rea.keytao-app');
    final preferences = File(
      '${home.path}/Library/Preferences/ink.rea.keytao-app.plist',
    );
    final sharedRime = Directory('${home.path}/Library/shared-rime-data');
    await profile.create(recursive: true);
    await support.create(recursive: true);
    await cache.create(recursive: true);
    await preferences.parent.create(recursive: true);
    await sharedRime.create(recursive: true);
    await preferences.writeAsString('preferences');

    await KeyTaoUninstaller.clearUserDataUnderHome(home.path);

    expect(await profile.exists(), isFalse);
    expect(await support.exists(), isFalse);
    expect(await cache.exists(), isFalse);
    expect(await preferences.exists(), isFalse);
    expect(await sharedRime.exists(), isTrue);
  });

  test('rejects a root directory as the user home', () async {
    await expectLater(
      KeyTaoUninstaller.clearUserDataUnderHome('/'),
      throwsA(isA<StateError>()),
    );
  });

  test('deletes a profile symlink without deleting its target', () async {
    final home = await Directory.systemTemp.createTemp('keytao-uninstaller-');
    addTearDown(() => home.delete(recursive: true));
    final outside = await Directory.systemTemp.createTemp('keytao-rime-');
    addTearDown(() => outside.delete(recursive: true));
    final marker = File('${outside.path}/keep.txt');
    await marker.writeAsString('keep');
    final profileLink = Link('${home.path}/Library/keytao');
    await profileLink.parent.create(recursive: true);
    await profileLink.create(outside.path);

    await KeyTaoUninstaller.clearUserDataUnderHome(home.path);

    expect(
      await FileSystemEntity.type(profileLink.path, followLinks: false),
      FileSystemEntityType.notFound,
    );
    expect(await marker.exists(), isTrue);
  });
}
