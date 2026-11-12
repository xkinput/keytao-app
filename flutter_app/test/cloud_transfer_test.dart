import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:forui/forui.dart';
import 'package:keytao/src/rust/api/types.dart';
import 'package:keytao/src/ui/settings.dart';
import 'package:keytao/src/ui/theme.dart';
import 'package:keytao/src/ui/widgets.dart';

import 'view_fixture.dart';

class _TransferController extends ViewController {
  _TransferController() : super(BridgePlatform.android);

  final completion = Completer<bool>();
  String? expectedSha256;
  String? downloadedId;
  Object? previewFailure;

  @override
  Future<Map<String, dynamic>?> previewCloudPackage(String key) async {
    if (previewFailure case final failure?) throw failure;
    return {
      'sha256': 'confirmed-digest',
      'archive_size': 100,
      'files': ['keytao.schema.yaml', 'keytao.dict.yaml'],
    };
  }

  @override
  Future<bool> uploadCloudPackage(String key, {String? expectedSha256}) {
    this.expectedSha256 = expectedSha256;
    return completion.future;
  }

  @override
  Future<bool> downloadCloudPackage(String id) {
    downloadedId = id;
    return completion.future;
  }

  void emitProgress(String direction, int bytes, {String key = 'keytao'}) {
    cloudTransferProgress = CloudTransferProgressDto(
      direction: direction,
      schemeKey: key,
      bytesTransferred: BigInt.from(bytes),
      totalBytes: BigInt.from(100),
    );
    notifyListeners();
  }
}

Future<void> _open(
  WidgetTester tester,
  _TransferController controller, {
  bool download = false,
}) async {
  await tester.pumpWidget(
    KeyTaoTheme(
      platform: BridgePlatform.android,
      home: Builder(
        builder: (context) => FScaffold(
          child: ActionButton(
            '开始',
            onPress: () => showCloudPackageTransfer(
              context,
              controller,
              schemeKey: 'keytao',
              packageId: download ? '21' : null,
              totalBytes: 100,
            ),
          ),
        ),
      ),
    ),
  );
  await tester.tap(find.text('开始'));
  await tester.pump();
  await tester.pump(const Duration(milliseconds: 300));
}

void main() {
  testWidgets('upload keeps one locked modal with byte progress and success', (
    tester,
  ) async {
    final c = _TransferController();
    addTearDown(c.dispose);
    await _open(tester, c);
    expect(find.text('确认上传'), findsOneWidget);
    expect(find.textContaining('2 个文件'), findsOneWidget);
    final dialog = tester.element(find.byType(AppDialog));
    await tester.tap(find.text('确认上传'));
    await tester.pump();
    expect(c.expectedSha256, 'confirmed-digest');
    expect(tester.element(find.byType(AppDialog)), same(dialog));
    expect(find.text('取消'), findsNothing);
    expect(find.text('关闭'), findsNothing);
    c.emitProgress('upload', 50);
    await tester.pump();
    expect(find.text('50 B / 100 B · 50%'), findsOneWidget);
    await tester.tapAt(const Offset(5, 5));
    await tester.pump();
    await tester.binding.handlePopRoute();
    await tester.pump();
    expect(find.byType(AppDialog), findsOneWidget);
    c.completion.complete(true);
    await tester.pump();
    expect(find.text('上传成功'), findsOneWidget);
    expect(find.text('100 B / 100 B · 100%'), findsOneWidget);
    await tester.pump(const Duration(milliseconds: 701));
    await tester.pumpAndSettle();
    expect(find.byType(AppDialog), findsNothing);
  });

  testWidgets('upload errors remain in the modal until closed', (tester) async {
    final c = _TransferController();
    addTearDown(c.dispose);
    await _open(tester, c);
    await tester.tap(find.text('确认上传'));
    await tester.pump();
    c.cloudTransferError = '方案包上传失败: HTTP 500';
    c.completion.complete(false);
    await tester.pumpAndSettle();
    expect(find.text('方案包上传失败: HTTP 500'), findsOneWidget);
    expect(find.text('关闭'), findsOneWidget);
    expect(find.text('上传成功'), findsNothing);
    await tester.tap(find.text('关闭'));
    await tester.pumpAndSettle();
    expect(find.byType(AppDialog), findsNothing);
  });

  testWidgets('download displays byte progress and active scheme error', (
    tester,
  ) async {
    final c = _TransferController();
    addTearDown(c.dispose);
    await _open(tester, c, download: true);
    expect(c.downloadedId, '21');
    expect(find.text('确认上传'), findsNothing);
    c.emitProgress('download', 25);
    await tester.pump();
    expect(find.text('25 B / 100 B · 25%'), findsOneWidget);
    await tester.binding.handlePopRoute();
    await tester.pump();
    expect(find.byType(AppDialog), findsOneWidget);
    c.cloudTransferError = '请先切换到其他方案';
    c.completion.complete(false);
    await tester.pumpAndSettle();
    expect(find.text('请先切换到其他方案'), findsOneWidget);
    expect(find.text('关闭'), findsOneWidget);
    await tester.tap(find.text('关闭'));
    await tester.pumpAndSettle();
  });

  testWidgets('download shows success before closing', (tester) async {
    final c = _TransferController();
    addTearDown(c.dispose);
    await _open(tester, c, download: true);
    c.completion.complete(true);
    await tester.pump();
    expect(find.text('下载成功'), findsOneWidget);
    await tester.pump(const Duration(milliseconds: 701));
    await tester.pumpAndSettle();
    expect(find.byType(AppDialog), findsNothing);
  });

  testWidgets('preview failures can be closed without starting an upload', (
    tester,
  ) async {
    final c = _TransferController()..previewFailure = StateError('预览失败');
    addTearDown(c.dispose);
    await _open(tester, c);
    expect(find.textContaining('预览失败'), findsOneWidget);
    expect(find.text('确认上传'), findsNothing);
    expect(c.expectedSha256, isNull);
    await tester.tap(find.text('关闭'));
    await tester.pumpAndSettle();
    expect(find.byType(AppDialog), findsNothing);
  });
}
