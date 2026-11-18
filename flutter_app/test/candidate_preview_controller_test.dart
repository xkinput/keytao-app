import 'dart:async';
import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:keytao/src/app/controller.dart';
import 'package:keytao/src/rust/api/types.dart';
import 'package:keytao/src/rust/frb_generated.dart';

class _PreviewCore implements RustLibApi {
  final requests = <({int? size, Completer<String> result})>[];

  @override
  Future<String> crateApiCorePreviewCandidates({int? size}) {
    final result = Completer<String>();
    requests.add((size: size, result: result));
    return result.future;
  }

  @override
  dynamic noSuchMethod(Invocation invocation) =>
      throw StateError('Unexpected bridge call: ${invocation.memberName}');
}

String _preview(String text, int size) => jsonEncode({
  'page_size': size,
  'candidates': [
    {'text': text, 'comment': '~real'},
  ],
});

void main() {
  late _PreviewCore core;
  late AppController controller;

  setUp(() {
    core = _PreviewCore();
    RustLib.initMock(api: core);
    controller = AppController(
      info: const BridgeInfo(
        platform: BridgePlatform.macOs,
        appVersion: 'test',
        userRoot: '/isolated/candidate-preview',
      ),
    );
  });

  tearDown(() {
    controller.dispose();
    RustLib.dispose();
  });

  test(
    'preview cache survives loading, failures and invalid results',
    () async {
      final first = controller.refreshCandidatePreview();
      core.requests.single.result.complete(_preview('真实词', 12));
      await first;
      expect(controller.candidatePreviewCandidates, [
        (text: '真实词', comment: '~real'),
      ]);
      expect(controller.candidatePreviewPageSize, 12);

      final failed = controller.refreshCandidatePreview();
      expect(controller.candidatePreviewCandidates.single.text, '真实词');
      core.requests.last.result.completeError(StateError('Unavailable'));
      await failed;
      expect(controller.candidatePreviewCandidates.single.text, '真实词');

      final invalid = controller.refreshCandidatePreview();
      core.requests.last.result.complete('{"page_size":0,"candidates":[]}');
      await invalid;
      expect(controller.candidatePreviewCandidates.single.text, '真实词');
      expect(controller.candidatePreviewPageSize, 12);
    },
  );

  test('preview refresh serializes calls and discards stale results', () async {
    final first = controller.refreshCandidatePreview();
    controller.candidatePageSize = 3;
    final latest = controller.refreshCandidatePreview();
    controller.candidatePageSize = 5;
    controller.refreshCandidatePreview();
    expect(core.requests, hasLength(1));
    expect(core.requests.first.size, isNull);

    core.requests.first.result.complete(_preview('过期词', 6));
    await Future<void>.delayed(Duration.zero);
    expect(controller.candidatePreviewCandidates, isEmpty);
    expect(core.requests, hasLength(2));
    expect(core.requests.last.size, 5);
    core.requests.last.result.complete(_preview('新词', 5));
    await Future.wait([first, latest]);
    expect(controller.candidatePreviewCandidates.single.text, '新词');
    expect(controller.candidatePreviewPageSize, 5);
  });
}
