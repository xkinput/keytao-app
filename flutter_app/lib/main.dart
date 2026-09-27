import 'dart:async';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:keytao/src/rust/api/core.dart';
import 'package:keytao/src/rust/api/types.dart';
import 'package:keytao/src/rust/frb_generated.dart';

void main() {
  WidgetsFlutterBinding.ensureInitialized();
  runApp(const MaterialApp(home: BridgeSmokePage()));
}

BridgePlatform hostPlatform() => switch (Platform.operatingSystem) {
  'macos' => BridgePlatform.macOs,
  'windows' => BridgePlatform.windows,
  'linux' => BridgePlatform.linux,
  'android' => BridgePlatform.android,
  'ios' => BridgePlatform.ios,
  _ => throw UnsupportedError(
    'Unsupported platform: ${Platform.operatingSystem}',
  ),
};

class BridgeSmokePage extends StatefulWidget {
  const BridgeSmokePage({super.key});

  @override
  State<BridgeSmokePage> createState() => _BridgeSmokePageState();
}

class _BridgeSmokePageState extends State<BridgeSmokePage> {
  StreamSubscription<BridgeEvent>? _events;
  BridgeInfo? _info;
  OnboardingDto? _onboarding;
  LocalSchemaDto? _schema;
  bool _ready = false;
  bool _installing = false;
  String _progress = '正在初始化...';
  String? _result;
  String? _error;

  @override
  void initState() {
    super.initState();
    unawaited(_initialize());
  }

  Future<void> _initialize() async {
    try {
      await RustLib.init();
      final root = await Directory.systemTemp.createTemp(
        'keytao-bridge-smoke-',
      );
      final info = await initCore(
        config: BridgeConfig(
          dataDir: '${root.path}/state',
          cacheDir: '${root.path}/cache',
          resourceDir: '${root.path}/resources',
          appVersion: '0.1.0+1',
          platform: hostPlatform(),
          userRootOverride: '${root.path}/rime',
        ),
      );
      if (!mounted) return;
      final ready = Completer<void>();
      _events = coreEvents().listen(
        (event) {
          if (event.kind == BridgeEventKind.ready && !ready.isCompleted) {
            ready.complete();
          }
          if (!mounted) return;
          final install = event.installProgress;
          final message = install != null
              ? '${install.percent}% ${install.message}'
              : event.deployProgress ?? event.windowsImeStatus?.message;
          if (message != null) setState(() => _progress = message);
        },
        onError: (Object error) {
          if (!ready.isCompleted) ready.completeError(error);
          if (mounted) setState(() => _error = '$error');
        },
      );
      await ready.future.timeout(const Duration(seconds: 10));
      final onboardingState = await onboarding();
      final schema = await checkLocalSchema();
      if (!mounted) return;
      setState(() {
        _info = info;
        _onboarding = onboardingState;
        _schema = schema;
        _ready = true;
        _progress = '已就绪；方案将安装到临时目录';
      });
      debugPrint('keytao-bridge: ready platform=${info.platform} onboarding=${onboardingState.completed} schemaInstalled=${schema.installed}');
    } catch (error) {
      debugPrint('keytao-bridge: init failed: $error');
      if (mounted) setState(() => _error = '$error');
    }
  }

  Future<void> _install() async {
    setState(() {
      _installing = true;
      _error = null;
      _result = null;
    });
    try {
      final result = await installLatestScheme(scheme: 'keytao');
      final schema = await checkLocalSchema();
      if (!mounted) return;
      final failed = result.verify.where((entry) => !entry.ok).toList();
      setState(() {
        _schema = schema;
        _result = failed.isEmpty
            ? '安装完成，${result.verify.length} 项校验通过'
            : '安装已结束，${failed.length} 项校验失败：\n'
                  '${failed.map((entry) => '${entry.path}: ${entry.note}').join('\n')}';
      });
    } catch (error) {
      if (mounted) setState(() => _error = '$error');
    } finally {
      if (mounted) setState(() => _installing = false);
    }
  }

  @override
  void dispose() {
    unawaited(_events?.cancel());
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final info = _info;
    final schema = _schema;
    return Scaffold(
      appBar: AppBar(title: const Text('KeyTao 核心冒烟')),
      body: SingleChildScrollView(
        padding: const EdgeInsets.all(20),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            if (info != null) ...[
              Text('平台：${info.platform.name} · 版本：${info.appVersion}'),
              Text('引导：${_onboarding!.completed ? '已完成' : '未完成'}'),
            ],
            if (schema != null) ...[
              Text('本地方案：${schema.installed ? '已安装' : '未安装'}'),
              Text('部署：${schema.deployed ? '已部署' : '未部署'}'),
              Text('方案版本：${schema.version ?? '—'}'),
              Text(
                '方案列表：${schema.schemas.isEmpty ? '—' : schema.schemas.join(', ')}',
              ),
            ],
            const SizedBox(height: 16),
            FilledButton(
              onPressed: _ready && !_installing ? _install : null,
              child: const Text('安装方案'),
            ),
            const SizedBox(height: 12),
            Text(_progress),
            if (_result != null) Text(_result!),
            if (_error != null) SelectableText('错误：$_error'),
          ],
        ),
      ),
    );
  }
}
