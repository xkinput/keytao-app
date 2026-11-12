import 'package:flutter_test/flutter_test.dart';
import 'package:keytao/src/app/controller.dart';
import 'package:keytao/src/rust/api/types.dart';
import 'package:keytao/src/rust/frb_generated.dart';

const a = UserDictionaryDeletionDto(word: 'a', code: 'aa');
const b = UserDictionaryDeletionDto(word: 'b', code: 'bb');
const c = UserDictionaryDeletionDto(word: 'c', code: 'cc');
const d = UserDictionaryDeletionDto(word: 'd', code: 'dd');

UserDictionarySyncDto pending({
  List<UserDictionaryDeletionDto> local = const [a, b, c, d],
  List<UserDictionaryDeletionDto> server = const [],
}) => UserDictionarySyncDto(
  fileName: 'user.dict.yaml',
  count: BigInt.zero,
  message: '',
  pending: true,
  localDeletions: local,
  serverDeletions: server,
);

class FakeDictionaryCore implements RustLibApi {
  AccountUserDto? account = const AccountUserDto(id: 1);
  UserDictionarySyncDto result = pending();
  final syncCalls = <Invocation>[];

  @override
  dynamic noSuchMethod(Invocation invocation) {
    if (invocation.memberName == #crateApiCoreAccountSyncUserDictionary) {
      syncCalls.add(invocation);
      return Future<UserDictionarySyncDto>.value(result);
    }
    if (invocation.memberName == #crateApiCoreAccountSession) {
      return Future<AccountUserDto?>.value(account);
    }
    throw StateError('Unexpected bridge call: ${invocation.memberName}');
  }
}

void main() {
  late FakeDictionaryCore core;
  late AppController controller;

  setUp(() {
    core = FakeDictionaryCore();
    RustLib.initMock(api: core);
    controller = AppController(
      info: const BridgeInfo(
        platform: BridgePlatform.macOs,
        appVersion: 'test',
        userRoot: '/isolated/keytao',
      ),
    )..accountUser = core.account;
  });

  tearDown(() {
    controller.dispose();
    RustLib.dispose();
  });

  testWidgets('cancelled deletion set is not prompted by automatic sync', (
    tester,
  ) async {
    await controller.syncUserDictionary();
    controller.dismissDictionarySyncConfirmation();
    core.result = pending(local: [d, c, b, a, a]);
    controller.scheduleUserDictionarySync();
    await tester.pump(const Duration(seconds: 1));
    await tester.pump();

    expect(core.syncCalls, hasLength(2));
    expect(controller.pendingDictionarySync, isNull);
  });

  test('manual sync shows a previously cancelled deletion set', () async {
    await controller.syncUserDictionary();
    controller.dismissDictionarySyncConfirmation();

    await controller.syncUserDictionary();

    expect(controller.pendingDictionarySync, same(core.result));
  });

  test('automatic sync shows a changed directional deletion set', () async {
    await controller.syncUserDictionary();
    controller.dismissDictionarySyncConfirmation();
    core.result = pending(local: [b, c, d], server: [a]);

    await controller.syncUserDictionary(automatic: true);

    expect(controller.pendingDictionarySync, same(core.result));
  });

  test('automatic sync distinguishes codes for the same word', () async {
    await controller.syncUserDictionary();
    controller.dismissDictionarySyncConfirmation();
    core.result = pending(
      local: [
        const UserDictionaryDeletionDto(word: 'a', code: 'ab'),
        b,
        c,
        d,
      ],
    );

    await controller.syncUserDictionary(automatic: true);

    expect(controller.pendingDictionarySync, same(core.result));
  });

  test('automatic sync remembers multiple cancelled deletion sets', () async {
    final first = core.result;
    await controller.syncUserDictionary();
    controller.dismissDictionarySyncConfirmation();
    core.result = pending(local: [], server: [a, b, c, d]);
    await controller.syncUserDictionary(automatic: true);
    controller.dismissDictionarySyncConfirmation();
    core.result = first;

    await controller.syncUserDictionary(automatic: true);

    expect(controller.pendingDictionarySync, isNull);
  });

  test('cancelled deletion sets are scoped to the account', () async {
    await controller.syncUserDictionary();
    controller.dismissDictionarySyncConfirmation();
    core.account = const AccountUserDto(id: 2);
    await controller.reloadAccountSession();

    await controller.syncUserDictionary(automatic: true);

    expect(controller.pendingDictionarySync, same(core.result));
    controller.dismissDictionarySyncConfirmation();
    core.account = const AccountUserDto(id: 1);
    await controller.reloadAccountSession();
    await controller.syncUserDictionary(automatic: true);
    expect(controller.pendingDictionarySync, isNull);
  });

  test('confirmation forwards the exact shown directional deletions', () async {
    core.result = pending(local: [a, b, c, d], server: [a]);
    await controller.syncUserDictionary();
    final shown = controller.pendingDictionarySync!;

    await controller.syncUserDictionary(confirmedDeletions: shown);

    expect(core.syncCalls.first.namedArguments[#confirmedDeletions], isNull);
    final confirmed =
        core.syncCalls.last.namedArguments[#confirmedDeletions]
            as UserDictionaryDeletionConfirmationDto;
    expect(confirmed.localDeletions, [a, b, c, d]);
    expect(confirmed.serverDeletions, [a]);
  });

  test('confirmation rejects a replaced pending snapshot', () async {
    await controller.syncUserDictionary();
    final shown = controller.pendingDictionarySync!;
    core.result = pending(local: [], server: [a, b, c, d]);
    await controller.syncUserDictionary();

    expect(
      await controller.syncUserDictionary(confirmedDeletions: shown),
      isFalse,
    );
    expect(core.syncCalls, hasLength(2));
  });
}
