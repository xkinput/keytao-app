import 'dart:async';

import 'package:flutter_test/flutter_test.dart';
import 'package:keytao/src/app/controller.dart';
import 'package:keytao/src/rust/api/types.dart';
import 'package:keytao/src/rust/frb_generated.dart';

import 'controller_parity_test.dart' show FakeCore, installedLocal;

class FakeLiveSyncCore extends FakeCore {
  AccountUserDto? account = const AccountUserDto(id: 1);
  Completer<void>? accountRefreshGate;
  Completer<void>? syncGate;
  List<InstalledPackageDto> packages = const [
    InstalledPackageDto(
      schemeKey: 'keytao',
      active: true,
      hasPackageZip: true,
    ),
  ];
  List<UserWordDto> words = [];
  List<UserWordDto>? syncedWords;
  String? syncFailure;
  UserDictionarySyncDto syncResult = UserDictionarySyncDto(
    fileName: 'keytao.user.dict.yaml',
    count: BigInt.zero,
    message: '',
    pending: false,
    localDeletions: const [],
    serverDeletions: const [],
  );

  @override
  Future<List<UserWordDto>> crateApiCoreListUserWords({
    required String schemeKey,
  }) async {
    expectSync(schemeKey, 'keytao');
    return List.of(words);
  }

  @override
  Future<UserDictionarySyncDto> crateApiCoreAccountSyncUserDictionary({
    UserDictionaryDeletionConfirmationDto? confirmedDeletions,
  }) async {
    calls.add('dictionarySync');
    await syncGate?.future;
    if (!syncResult.pending && syncedWords != null) {
      words = List.of(syncedWords!);
    }
    if (syncFailure != null) throw StateError(syncFailure!);
    return syncResult;
  }

  @override
  Future<AccountUserDto?> crateApiCoreAccountSession() async => account;

  @override
  Future<List<InstalledPackageDto>> crateApiCoreListInstalledPackages() async =>
      List.of(packages);

  @override
  Future<AccountUserDto?> crateApiCoreAccountRefresh() async {
    calls.add('accountRefresh');
    await accountRefreshGate?.future;
    return account;
  }

  @override
  Future<AccountUserDto> crateApiCoreAccountLogin({
    required String name,
    required String password,
  }) async => account = const AccountUserDto(id: 1);

  @override
  Future<void> crateApiCoreAccountLogout() async => account = null;
}

void main() {
  late FakeLiveSyncCore core;
  late AppController controller;

  int callCount(String name) => core.calls.where((call) => call == name).length;

  setUp(() {
    core = FakeLiveSyncCore();
    RustLib.initMock(api: core);
    controller = AppController(
      info: const BridgeInfo(
        platform: BridgePlatform.macOs,
        appVersion: 'test',
        userRoot: '/isolated/keytao',
      ),
    )
      ..local = installedLocal
      ..installedPackages = List.of(core.packages);
  });

  tearDown(() async {
    controller.dispose();
    await core.events.close();
    RustLib.dispose();
  });

  testWidgets('user-word deploy debounces changes and coalesces overlap', (
    tester,
  ) async {
    core.deployGate = Completer<void>();
    controller.markUserWordsChanged('keytao');
    await tester.pump(const Duration(milliseconds: 799));
    expect(callCount('deployDefault'), 0);
    controller.markUserWordsChanged('keytao-dz');
    await tester.pump(const Duration(milliseconds: 799));
    expect(callCount('deployDefault'), 0);
    await tester.pump(const Duration(milliseconds: 1));
    expect(callCount('deployDefault'), 1);
    expect(controller.busy, isTrue);
    expect(controller.userWordsPendingDeploy, isEmpty);

    controller.markUserWordsChanged('keytao-bj');
    controller.markUserWordsChanged('keytao');
    await tester.pump(const Duration(seconds: 1));
    controller.markUserWordsChanged('keytao-dz');
    await tester.pump(const Duration(seconds: 1));
    expect(callCount('deployDefault'), 1);

    core.deployGate!.complete();
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 800));
    expect(callCount('deployDefault'), 2);
    await tester.pump(const Duration(seconds: 2));
    expect(callCount('deployDefault'), 2);
  });

  testWidgets('failed user-word deploy surfaces the error without retrying', (
    tester,
  ) async {
    core.deployFailure = 'deploy failed';
    controller.markUserWordsChanged('keytao');
    await tester.pump(const Duration(milliseconds: 800));
    expect(callCount('deployDefault'), 1);
    expect(controller.deploySteps.last.message, 'deploy failed');
    expect(controller.deploySteps.last.state, DeployStepState.failed);
    await tester.pump(const Duration(seconds: 10));
    expect(callCount('deployDefault'), 1);
  });

  testWidgets(
    'dictionary sync deploys local changes only and keeps confirmation',
    (tester) async {
      controller.accountUser = core.account;
      core.syncedWords = [
        const UserWordDto(id: 1, word: 'custom word', code: 'abcd'),
      ];
      expect(await controller.syncUserDictionary(), isTrue);
      await tester.pump(const Duration(milliseconds: 800));
      expect(callCount('deployDefault'), 1);

      expect(await controller.syncUserDictionary(), isTrue);
      await tester.pump(const Duration(milliseconds: 800));
      expect(callCount('deployDefault'), 1);

      final completed = core.syncResult;
      core.syncResult = UserDictionarySyncDto(
        fileName: 'keytao.user.dict.yaml',
        count: BigInt.zero,
        message: '',
        pending: true,
        localDeletions: const [
          UserDictionaryDeletionDto(word: 'a', code: 'aa'),
          UserDictionaryDeletionDto(word: 'b', code: 'bb'),
          UserDictionaryDeletionDto(word: 'c', code: 'cc'),
          UserDictionaryDeletionDto(word: 'd', code: 'dd'),
        ],
        serverDeletions: const [],
      );
      core.syncedWords = [];
      expect(await controller.syncUserDictionary(), isTrue);
      await tester.pump(const Duration(milliseconds: 800));
      expect(controller.pendingDictionarySync, same(core.syncResult));
      expect(core.words, hasLength(1));
      expect(callCount('deployDefault'), 1);

      final shown = controller.pendingDictionarySync!;
      core.syncResult = completed;
      expect(
        await controller.syncUserDictionary(confirmedDeletions: shown),
        isTrue,
      );
      await tester.pump(const Duration(milliseconds: 800));
      expect(core.words, isEmpty);
      expect(callCount('deployDefault'), 2);
    },
  );

  testWidgets('startup pull waits for refresh and repeats only in foreground', (
    tester,
  ) async {
    core.accountRefreshGate = Completer<void>();
    expect(await controller.refreshState(), isTrue);
    await tester.pump(const Duration(milliseconds: 800));
    expect(callCount('accountRefresh'), 1);
    expect(callCount('dictionarySync'), 0);

    core.accountRefreshGate!.complete();
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 800));
    expect(callCount('dictionarySync'), 1);

    await tester.pump(const Duration(minutes: 5));
    await tester.pump(const Duration(milliseconds: 800));
    expect(callCount('dictionarySync'), 2);
    controller.onPause();
    await tester.pump(const Duration(minutes: 5));
    expect(callCount('dictionarySync'), 2);

    controller.onResume();
    await tester.pump(const Duration(milliseconds: 800));
    expect(callCount('dictionarySync'), 3);
    controller.onPause();
    await tester.pump(const Duration(seconds: 60));
  });

  testWidgets('resumes within 60 seconds share one background pull', (
    tester,
  ) async {
    controller.accountUser = core.account;
    core.syncGate = Completer<void>();
    controller.onResume();
    await tester.pump(const Duration(milliseconds: 800));
    expect(callCount('dictionarySync'), 1);
    expect(controller.syncingDictionary, isTrue);
    expect(controller.busy, isFalse);

    controller.busy = true;
    core.syncGate!.complete();
    await tester.pump();
    expect(controller.busy, isTrue);
    controller.busy = false;

    controller.onResume();
    await tester.pump(const Duration(milliseconds: 800));
    expect(callCount('dictionarySync'), 1);

    core.syncGate = Completer<void>();
    final manual = controller.syncUserDictionary();
    expect(controller.busy, isTrue);
    await tester.pump();
    expect(callCount('dictionarySync'), 2);
    core.syncGate!.complete();
    expect(await manual, isTrue);
    expect(controller.busy, isFalse);

    controller.onPause();
    await tester.pump(const Duration(seconds: 60));
    controller.onResume();
    await tester.pump(const Duration(milliseconds: 800));
    expect(callCount('dictionarySync'), 3);
    controller.onPause();
    await tester.pump(const Duration(seconds: 60));
  });

  testWidgets('automatic pulls require a stored keytao package', (tester) async {
    core.packages = [];
    core.local = const LocalSchemaDto(
      installed: false,
      deployed: false,
      schemas: [],
    );
    controller.installedPackages = [];
    controller.local = core.local;
    expect(controller.scheme, 'keytao');
    expect(controller.activeSchemeKey, 'keytao');

    expect(await controller.refreshState(), isTrue);
    await tester.pump(const Duration(milliseconds: 800));
    expect(callCount('dictionarySync'), 0);
    controller.onResume();
    await tester.pump(const Duration(milliseconds: 800));
    expect(callCount('dictionarySync'), 0);
    await tester.pump(const Duration(minutes: 5));
    await tester.pump(const Duration(milliseconds: 800));
    expect(callCount('dictionarySync'), 0);
    expect(await controller.login('user', 'password'), isTrue);
    await tester.pump(const Duration(milliseconds: 800));
    expect(callCount('dictionarySync'), 0);
    controller.onPause();
  });

  testWidgets('logout cancels periodic pulls and login restarts them', (
    tester,
  ) async {
    expect(await controller.refreshState(), isTrue);
    await tester.pump(const Duration(milliseconds: 800));
    expect(callCount('dictionarySync'), 1);

    await controller.logout();
    await tester.pump(const Duration(minutes: 5));
    expect(callCount('dictionarySync'), 1);
    expect(await controller.login('user', 'password'), isTrue);
    await tester.pump(const Duration(milliseconds: 800));
    expect(callCount('dictionarySync'), 2);
    await tester.pump(const Duration(minutes: 5));
    await tester.pump(const Duration(milliseconds: 800));
    expect(callCount('dictionarySync'), 3);
    controller.onPause();
    await tester.pump(const Duration(seconds: 60));
  });

  testWidgets(
    'partially failed sync deploys changes and keeps the sync error',
    (tester) async {
      controller.accountUser = core.account;
      core.syncedWords = [
        const UserWordDto(id: 1, word: 'custom word', code: 'abcd'),
      ];
      core.syncFailure = 'sync failed after local write';
      expect(await controller.syncUserDictionary(), isFalse);
      expect(controller.accountError, 'sync failed after local write');
      await tester.pump(const Duration(milliseconds: 800));
      expect(callCount('deployDefault'), 1);
      expect(controller.accountError, 'sync failed after local write');
      await tester.pump(const Duration(seconds: 10));
      expect(callCount('deployDefault'), 1);
    },
  );

  testWidgets('login schedules a dictionary pull without opening user words', (
    tester,
  ) async {
    core.account = null;
    expect(await controller.login('user', 'password'), isTrue);
    expect(callCount('dictionarySync'), 0);
    await tester.pump(const Duration(milliseconds: 800));
    expect(callCount('dictionarySync'), 1);
    controller.onPause();
    await tester.pump(const Duration(seconds: 60));
  });
}
