import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:keytao/src/app/controller.dart';
import 'package:keytao/src/rust/api/types.dart';
import 'package:keytao/src/rust/frb_generated.dart';
import 'package:keytao/src/ui/theme.dart';
import 'package:keytao/src/ui/user_words.dart';

const _stock = UserWordDto(id: 0, word: '测试词', code: 'csci');
const _marker = UserWordDto(id: 0, word: '测试词', code: 'csci', remove: true);

class _RemoveCore implements RustLibApi {
  final changes = <Invocation>[];
  final queries = <String>[];
  bool removed = false;
  Completer<List<UserWordDto>>? searchGate;
  int cacheReleases = 0;

  @override
  dynamic noSuchMethod(Invocation invocation) {
    switch (invocation.memberName) {
      case #crateApiCoreListUserWords:
        return Future.value([_stock, if (removed) _marker]);
      case #crateApiCoreCompareUserWords:
        return Future.value([
          const UserWordComparisonDto(
            entry: _stock,
            status: 'none',
            words: [],
            codes: [],
          ),
          if (removed)
            const UserWordComparisonDto(
              entry: _marker,
              status: 'removed',
              words: [],
              codes: [],
            ),
        ]);
      case #crateApiCoreSearchStockWords:
        queries.add(invocation.namedArguments[#query] as String);
        return searchGate?.future ?? Future.value([_stock]);
      case #crateApiCoreReleaseUserWordsStockCache:
        cacheReleases++;
        return Future<void>.value();
      case #crateApiCoreSetUserWordRemoved:
        changes.add(invocation);
        removed = invocation.namedArguments[#remove] as bool;
        return Future<void>.value();
      default:
        throw StateError('Unexpected bridge call: ${invocation.memberName}');
    }
  }
}

void main() {
  late _RemoveCore core;
  late AppController controller;

  setUp(() {
    core = _RemoveCore();
    RustLib.initMock(api: core);
    controller = AppController(
      info: const BridgeInfo(
        platform: BridgePlatform.android,
        appVersion: 'test',
        userRoot: '/isolated/keytao',
      ),
    );
  });

  tearDown(() {
    controller.dispose();
    RustLib.dispose();
  });

  Future<void> openPage(WidgetTester tester, String scheme) async {
    await tester.pumpWidget(
      KeyTaoTheme(
        platform: controller.info.platform,
        home: UserWordsPage(controller: controller, schemeKey: scheme),
      ),
    );
    await tester.pumpAndSettle();
  }

  testWidgets(
    'stock search removes and restores without deleting a personal word',
    (tester) async {
      await openPage(tester, 'keytao-dz');
      await tester.enterText(find.byType(EditableText), 'csci');
      await tester.pumpAndSettle();
      await tester.pump(const Duration(milliseconds: 180));
      await tester.pumpAndSettle();
      expect(core.queries, ['csci']);
      expect(find.text(_stock.word), findsNWidgets(2));

      await tester.tap(find.text('移除'));
      await tester.pumpAndSettle();
      await tester.pump(const Duration(milliseconds: 180));
      await tester.pumpAndSettle();

      expect(core.changes.single.namedArguments, {
        #schemeKey: 'keytao-dz',
        #word: _stock.word,
        #code: _stock.code,
        #remove: true,
      });
      expect(controller.userWordsRevision, 1);
      expect(find.text('已移除'), findsOneWidget);
      expect(find.text(_stock.word), findsNWidgets(2));
      expect(find.bySemanticsLabel('删除 ${_stock.word}'), findsOneWidget);

      await tester.tap(find.text('恢复'));
      await tester.pumpAndSettle();
      await tester.pump(const Duration(milliseconds: 180));
      await tester.pumpAndSettle();

      expect(core.changes.last.namedArguments[#remove], isFalse);
      expect(controller.userWordsRevision, 2);
      expect(find.text('已移除'), findsNothing);
      expect(find.text('移除'), findsOneWidget);
    },
  );

  testWidgets('closing waits for stock search before releasing cache', (
    tester,
  ) async {
    final searchGate = core.searchGate = Completer<List<UserWordDto>>();
    await openPage(tester, 'keytao');
    await tester.enterText(find.byType(EditableText), 'csci');
    await tester.pump(const Duration(milliseconds: 180));
    expect(core.queries, ['csci']);

    await tester.pumpWidget(const SizedBox.shrink());
    expect(core.cacheReleases, 0);

    searchGate.complete([_stock]);
    await tester.pump();
    expect(core.cacheReleases, 1);
  });

  testWidgets('xmjd search does not request stock removals', (tester) async {
    await openPage(tester, 'xmjd');
    await tester.enterText(find.byType(EditableText), '测试');
    await tester.pumpAndSettle();
    await tester.pump(const Duration(milliseconds: 180));
    await tester.pumpAndSettle();

    expect(core.queries, isEmpty);
    expect(find.text('移除'), findsNothing);
  });

  testWidgets('pending personal deletion keeps the same-code marker visible', (
    tester,
  ) async {
    core.removed = true;
    await openPage(tester, 'keytao');
    await tester.tap(find.bySemanticsLabel('删除 ${_stock.word}'));
    await tester.pumpAndSettle();

    expect(find.text(_stock.word), findsOneWidget);
    expect(find.text('已移除'), findsOneWidget);

    await tester.tap(find.text('撤销'));
    await tester.pumpAndSettle();
    expect(find.text(_stock.word), findsNWidgets(2));
    expect(core.changes, isEmpty);
  });
}
