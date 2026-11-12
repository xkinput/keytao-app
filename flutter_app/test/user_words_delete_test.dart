import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:keytao/src/app/controller.dart';
import 'package:keytao/src/rust/api/types.dart';
import 'package:keytao/src/rust/frb_generated.dart';
import 'package:keytao/src/ui/theme.dart';
import 'package:keytao/src/ui/user_words.dart';

const _entry = UserWordDto(id: 0, word: '测试词', code: 'csci');

class _DeleteCore implements RustLibApi {
  final deleted = <Invocation>[];
  final actions = <String>[];
  bool hasEntry = true;
  Completer<void>? deleteGate;
  Object? deleteFailure;

  @override
  dynamic noSuchMethod(Invocation invocation) {
    switch (invocation.memberName) {
      case #crateApiCoreListUserWords:
        return Future.value(hasEntry ? [_entry] : <UserWordDto>[]);
      case #crateApiCoreCompareUserWords:
        return Future.value([
          if (hasEntry)
            const UserWordComparisonDto(
              entry: _entry,
              status: 'none',
              words: [],
              codes: [],
            ),
        ]);
      case #crateApiCoreDeleteUserWord:
        actions.add('delete');
        deleted.add(invocation);
        if (deleteFailure case final failure?) {
          return Future<void>.error(failure);
        }
        hasEntry = false;
        return deleteGate?.future ?? Future<void>.value();
      case #crateApiCoreAccountPendingWords:
        return Future.value('[]');
      default:
        throw StateError('Unexpected bridge call: ${invocation.memberName}');
    }
  }
}

class _UserWordsController extends AppController {
  _UserWordsController(this.actions)
    : super(
        info: const BridgeInfo(
          platform: BridgePlatform.android,
          appVersion: 'test',
          userRoot: '/isolated/keytao',
        ),
      );

  final List<String> actions;

  @override
  void scheduleUserDictionarySync() {}

  @override
  Future<bool> syncUserDictionary({
    UserDictionarySyncDto? confirmedDeletions,
    bool automatic = false,
  }) async {
    actions.add('sync');
    return true;
  }
}

void main() {
  late _DeleteCore core;
  late AppController controller;

  setUp(() {
    core = _DeleteCore();
    RustLib.initMock(api: core);
    controller = _UserWordsController(core.actions)
      ..installedPackages = const [
        InstalledPackageDto(
          schemeKey: 'local-a',
          active: false,
          hasPackageZip: true,
        ),
        InstalledPackageDto(
          schemeKey: 'local-b',
          active: false,
          hasPackageZip: true,
        ),
      ];
  });

  tearDown(() {
    controller.dispose();
    RustLib.dispose();
  });

  Future<void> openPage(
    WidgetTester tester, {
    String schemeKey = 'local-a',
  }) async {
    await tester.pumpWidget(
      KeyTaoTheme(
        platform: controller.info.platform,
        home: UserWordsPage(controller: controller, schemeKey: schemeKey),
      ),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.bySemanticsLabel('删除 ${_entry.word}'));
    await tester.pumpAndSettle();
    expect(core.deleted, isEmpty);
  }

  testWidgets('undo snackbar expires and commits deletion once', (
    tester,
  ) async {
    await openPage(tester);
    final snackbar = tester.widget<SnackBar>(find.byType(SnackBar));
    expect(snackbar.persist, isFalse);
    expect(snackbar.duration, const Duration(seconds: 4));

    await tester.pump(const Duration(seconds: 4));
    await tester.pumpAndSettle();

    expect(core.deleted, hasLength(1));
    expect(core.deleted.single.namedArguments, {
      #schemeKey: 'local-a',
      #id: _entry.id,
      #expectedWord: _entry.word,
      #expectedCode: _entry.code,
    });
    expect(controller.busy, isFalse);
  });

  testWidgets('undo keeps the word without persisting a deletion', (
    tester,
  ) async {
    await openPage(tester);
    await tester.tap(find.text('撤销'));
    await tester.pumpAndSettle();
    await tester.pump(const Duration(seconds: 5));

    expect(core.deleted, isEmpty);
    expect(controller.busy, isFalse);
    expect(find.text(_entry.word), findsOneWidget);
  });

  testWidgets('leaving the page commits before the undo timeout', (
    tester,
  ) async {
    await openPage(tester);
    await tester.pumpWidget(const SizedBox.shrink());
    await tester.pumpAndSettle();
    await tester.pump(const Duration(seconds: 5));

    expect(core.deleted, hasLength(1));
    expect(controller.busy, isFalse);
    expect(tester.takeException(), isNull);
  });

  testWidgets(
    'switching schemes commits the pending word to its original scheme',
    (tester) async {
      await openPage(tester);
      await tester.tap(find.text('local-b'));
      await tester.pumpAndSettle();

      expect(core.deleted, hasLength(1));
      expect(core.deleted.single.namedArguments[#schemeKey], 'local-a');
      expect(find.text('撤销'), findsNothing);
      expect(controller.busy, isFalse);
    },
  );

  testWidgets('adding a word first commits the pending deletion', (
    tester,
  ) async {
    await openPage(tester);
    await tester.tap(find.text('添加'));
    await tester.pumpAndSettle();

    expect(core.deleted, hasLength(1));
    expect(find.text('撤销'), findsNothing);
    expect(find.text('添加自造词'), findsOneWidget);
  });

  testWidgets('sync waits for the pending deletion to be committed', (
    tester,
  ) async {
    controller.accountUser = const AccountUserDto(id: 1);
    await openPage(tester, schemeKey: 'keytao');
    core.actions.clear();
    await tester.tap(find.text('同步'));
    await tester.pumpAndSettle();

    expect(core.actions, ['delete', 'sync']);
    expect(core.deleted, hasLength(1));
    expect(controller.busy, isFalse);
  });

  testWidgets('leaving during an action flush does not write twice', (
    tester,
  ) async {
    await openPage(tester);
    core.deleteGate = Completer<void>();
    await tester.tap(find.text('local-b'));
    await tester.pump();
    expect(core.deleted, hasLength(1));

    await tester.pumpWidget(const SizedBox.shrink());
    core.deleteGate!.complete();
    await tester.pumpAndSettle();

    expect(core.deleted, hasLength(1));
    expect(controller.busy, isFalse);
    expect(tester.takeException(), isNull);
  });

  testWidgets('failed deletion restores the word and stops the next action', (
    tester,
  ) async {
    await openPage(tester);
    core.deleteFailure = StateError('删除失败');
    await tester.tap(find.text('添加'));
    await tester.pumpAndSettle();

    expect(core.deleted, hasLength(1));
    expect(find.text(_entry.word), findsOneWidget);
    expect(find.textContaining('删除失败'), findsOneWidget);
    expect(find.text('添加自造词'), findsNothing);
    expect(controller.busy, isFalse);
  });

  testWidgets('a quick delete after undo can still be flushed on leaving', (
    tester,
  ) async {
    await openPage(tester);
    await tester.tap(find.text('撤销'));
    await tester.pump(const Duration(milliseconds: 1));
    await tester.tap(find.bySemanticsLabel('删除 ${_entry.word}'));
    await tester.pump(const Duration(milliseconds: 1));
    await tester.pumpWidget(const SizedBox.shrink());
    await tester.pumpAndSettle();

    expect(core.deleted, hasLength(1));
    expect(controller.busy, isFalse);
    expect(tester.takeException(), isNull);
  });

  testWidgets('a previous snackbar cannot commit or undo a newer deletion', (
    tester,
  ) async {
    await openPage(tester);
    final oldUndo = tester.widget<SnackBar>(find.byType(SnackBar)).action!;
    await tester.tap(find.text('撤销'));
    await tester.pump(const Duration(milliseconds: 1));
    await tester.tap(find.bySemanticsLabel('删除 ${_entry.word}'));
    await tester.pumpAndSettle();

    expect(core.deleted, isEmpty);
    oldUndo.onPressed();
    await tester.pump();
    expect(find.text(_entry.word), findsNothing);
    await tester.tap(find.text('撤销'));
    await tester.pumpAndSettle();
    await tester.pump(const Duration(seconds: 5));

    expect(core.deleted, isEmpty);
    expect(find.text(_entry.word), findsOneWidget);
    expect(controller.busy, isFalse);
  });
}
