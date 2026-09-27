import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

import '../platform/android_host.dart';
import '../rust/api/core.dart' as core;
import '../rust/api/types.dart';
import '../settings/android_settings.dart';

const schemes = {
  'keytao': '键道6',
  'xmjd': '星猫键道',
  'txjx': '天行键',
  'keydo': '键道·我流',
};
const repositoryUrl = 'https://github.com/xkinput/keytao-app';

String? schemeKeyFromSchemas(List<String> schemas) {
  for (final schema in schemas) {
    final name = schema.trim().toLowerCase();
    for (final entry in const {
      'xmjd6': 'xmjd',
      'txjx': 'txjx',
      'keydo': 'keydo',
      'keytao': 'keytao',
    }.entries) {
      if (name.startsWith(entry.key)) return entry.value;
    }
  }
  return null;
}

String errorText(Object error) => error is PlatformException
    ? error.message ?? '系统操作失败'
    : error is StateError
    ? error.message
    : '$error';

class AppController extends ChangeNotifier {
  AppController({required this.info, this.android = const AndroidHost()});
  final BridgeInfo info;
  final AndroidHost android;
  bool get isAndroid => info.platform == BridgePlatform.android;
  bool busy = false;
  bool _disposed = false;
  bool _refreshWhenIdle = false;
  bool _manuallySelectedScheme = false;
  String? error;
  String progress = '';
  double? installFraction;
  final List<String> operationLogs = [];
  List<VerifyEntryDto> verification = [];
  LocalSchemaDto? local;
  AddonSchemaStatusDto? addon;
  Map<String, dynamic>? storage;
  Map<String, dynamic>? ime;
  MacosImeStatusDto? macosIme;
  AndroidImeInputSettingsDto? androidSettings;
  ImeUiSettingsDto? uiSettings;
  EnglishModeDto desktopEnglishMode = EnglishModeDto.ascii;
  ComponentVersionsDto? versions;
  RuntimeLogSettingsDto? logSettings;
  DebugLogFileDto? runtimeLog;
  String? sharedLogPath;
  String scheme = 'keytao';
  String source = 'github';
  ReleaseInfoDto? latestRelease;
  SchemeReleaseDto? schemeRelease;
  StreamSubscription<BridgeEvent>? _events;

  bool get storageReady =>
      storage?['granted'] == true && storage?['writable'] == true;
  bool get migrationReady =>
      storageReady &&
      (storage?['migrationError'] as String?)?.isNotEmpty != true;
  bool get englishReady =>
      addon?.englishAvailable == true ||
      (addon?.installed == true && addon?.deployed == true) ||
      (local?.schemas.contains('english') ?? false);
  bool get setupReady {
    if (local?.installed != true || local?.deployed != true) return false;
    if (isAndroid) {
      return migrationReady &&
          ime?['enabled'] == true &&
          ime?['selected'] == true;
    }
    return macosIme?.installed == true;
  }

  PlatformReleaseDto? get selectedRelease =>
      source == 'github' ? latestRelease?.github : latestRelease?.gitee;
  String? get releaseVersion =>
      scheme == 'keytao' ? selectedRelease?.version : schemeRelease?.version;
  String? get downloadUrl => scheme == 'keytao'
      ? (isAndroid
            ? selectedRelease?.downloadUrls.android
            : selectedRelease?.downloadUrls.macos)
      : schemeRelease?.downloadUrl;

  void _changed() {
    if (!_disposed) notifyListeners();
  }

  Future<void> connectEvents() async {
    final ready = Completer<void>();
    _events = core.coreEvents().listen(
      (event) {
        if (event.kind == BridgeEventKind.ready && !ready.isCompleted) {
          ready.complete();
        }
        if (event.installProgress case final value?) {
          progress = value.message;
          installFraction = value.percent.clamp(0, 100) / 100;
          operationLogs.add(value.message);
        }
        if (event.deployProgress case final value?) {
          progress = value;
          operationLogs.add(value);
        }
        _changed();
      },
      onError: (Object failure) {
        if (!ready.isCompleted) ready.completeError(failure);
        error = errorText(failure);
        _changed();
      },
    );
    await ready.future.timeout(const Duration(seconds: 10));
  }

  Future<bool> run(Future<void> Function() action) async {
    if (busy || _disposed) return false;
    busy = true;
    error = null;
    _changed();
    try {
      await action();
      return true;
    } catch (failure) {
      error = errorText(failure);
      return false;
    } finally {
      busy = false;
      _changed();
      if (_refreshWhenIdle && !_disposed) {
        _refreshWhenIdle = false;
        unawaited(refreshState());
      }
    }
  }

  Future<void> _readLocal() async {
    local = await core.checkLocalSchema();
    if (!_manuallySelectedScheme) {
      final detected = schemeKeyFromSchemas(local!.schemas);
      if (detected != null && detected != scheme) {
        scheme = detected;
        schemeRelease = null;
        latestRelease = null;
      }
    }
    addon = await core.addonSchemaStatus(id: 'easy_en');
  }

  Future<void> _readState() async {
    final errors = <String>[];
    Future<void> read(Future<void> Function() action) async {
      try {
        await action();
      } catch (failure) {
        errors.add(errorText(failure));
      }
    }

    if (isAndroid) {
      storage = null;
      ime = null;
      await Future.wait([
        read(() async {
          storage = await android.storagePermissionStatus();
        }),
        read(() async {
          ime = await android.imeStatus();
        }),
      ]);
    } else {
      macosIme = null;
      await read(() async {
        macosIme = await core.macosImeStatus();
      });
    }
    local = null;
    addon = null;
    await Future.wait([
      read(_readLocal),
      read(() async {
        versions = await core.getComponentVersions();
      }),
      if (isAndroid && storageReady)
        read(() async {
          androidSettings = await core.getAndroidImeInputSettings();
        }),
      if (!isAndroid) ...[
        read(() async {
          uiSettings = await core.getImeUiSettings();
        }),
        read(() async {
          desktopEnglishMode = await core.getDesktopEnglishMode();
        }),
      ],
    ]);
    if (isAndroid && !storageReady) androidSettings = null;
    if (errors.isNotEmpty) error = errors.toSet().join('\n');
  }

  Future<bool> refreshState() => run(_readState);

  void onResume() {
    if (busy) {
      _refreshWhenIdle = true;
    } else {
      unawaited(refreshState());
    }
  }

  void selectScheme(String value) {
    if (busy) return;
    _manuallySelectedScheme = true;
    if (value == scheme) return;
    scheme = value;
    schemeRelease = null;
    latestRelease = null;
    error = null;
    _changed();
  }

  void selectSource(String value) {
    if (busy) return;
    source = value;
    _changed();
  }

  Future<void> _fetchRelease() async {
    if (scheme == 'keytao') {
      latestRelease = await core.fetchLatestRelease();
    } else {
      schemeRelease = await core.fetchSchemeRelease(scheme: scheme);
    }
  }

  Future<bool> refreshRelease() => run(_fetchRelease);

  Future<void> _requireStorage() async {
    storage = await android.storagePermissionStatus();
    if (!storageReady) {
      final message = storage?['message'] as String?;
      throw StateError(
        message == null || message.isEmpty ? '请检查 KeyTao 目录的文件访问权限' : message,
      );
    }
    if (!migrationReady) throw StateError(storage!['migrationError'] as String);
  }

  void _startOperation(String message) {
    operationLogs.clear();
    verification = [];
    installFraction = null;
    progress = message;
    _changed();
  }

  Future<bool> install() => run(() async {
    _startOperation('正在安装方案');
    try {
      if (isAndroid) await _requireStorage();
      if (downloadUrl == null) await _fetchRelease();
      final url = downloadUrl;
      if (url == null || url.isEmpty) throw StateError('当前来源没有可用的方案，请切换来源或刷新');
      final InstallResultDto result;
      if (isAndroid) {
        final zipPath = await core.prepareAndroidInstall(url: url);
        final extracted = await android.smartExtractZipToPrivate(zipPath);
        result = await core.finishAndroidInstall(
          resultJson: jsonEncode(extracted),
        );
      } else {
        result = await core.installSchemeFromUrl(url: url);
      }
      operationLogs.addAll(result.logs);
      verification = result.verify;
      await _readLocal();
      if (verification.any((entry) => !entry.ok)) {
        throw StateError('方案校验失败，请查看操作日志');
      }
      progress = '安装完成';
    } catch (failure) {
      progress = '安装失败';
      operationLogs.add(errorText(failure));
      rethrow;
    }
  });

  Future<bool> deploy() => run(() async {
    _startOperation('正在部署');
    try {
      if (isAndroid) {
        await _requireStorage();
        final result = await android.deployImeData();
        operationLogs.add('${result['path']}\n${result['schemaName'] ?? ''}');
        if (result['deployed'] != true) throw StateError('部署未完成');
      } else {
        final result = await core.deployDefault();
        operationLogs.add(result.message);
        if (!result.success) throw StateError(result.message);
      }
      await _readState();
      if (local?.deployed != true) throw StateError('部署后检查未通过，请查看操作日志');
      progress = '部署完成';
    } catch (failure) {
      progress = '部署失败';
      operationLogs.add(errorText(failure));
      rethrow;
    }
  });

  Future<bool> finishOnboarding() => run(() async {
    await _readState();
    if (!setupReady) throw StateError('请先完成输入法设置');
    await core.completeOnboarding();
  });

  Future<bool> saveAndroid(Map<String, Object> patch) => run(() async {
    await _requireStorage();
    final current = await core.getAndroidImeInputSettings();
    if (patch['englishMode'] == 'schema') {
      await _readLocal();
      if (!englishReady) throw StateError('英文方案未就绪');
    }
    androidSettings = await core.setAndroidImeInputSettings(
      settings: patchAndroidSettings(current, patch),
    );
  });

  Future<bool> saveUi({
    UiColorSchemeDto? colorScheme,
    PanelOrientationDto? orientation,
    String? accentColor,
    double? fontSize,
  }) => run(() async {
    final current = await core.getImeUiSettings();
    uiSettings = await core.setImeUiSettings(
      colorScheme: colorScheme ?? current.colorScheme,
      orientation: orientation ?? current.orientation,
      accentColor: accentColor ?? current.accentColor,
      fontSize: (fontSize ?? current.fontSize).clamp(10, 36),
    );
  });
  Future<bool> saveEmbedded(bool value) => run(() async {
    uiSettings = await core.setImeEmbeddedComposition(embedded: value);
  });
  Future<bool> saveDesktopEnglish(EnglishModeDto value) => run(() async {
    await _readLocal();
    if (value == EnglishModeDto.schema && !englishReady) {
      throw StateError('英文方案未就绪');
    }
    desktopEnglishMode = await core.setDesktopEnglishMode(mode: value);
  });

  Future<bool> open(String value) => run(() async {
    if (isAndroid) {
      await android.openUrl(value);
    } else {
      final result = await Process.run('open', [value]);
      if (result.exitCode != 0) throw StateError('无法打开：${result.stderr}');
    }
  });

  Future<void> _readLogs() async {
    logSettings = await core.getRuntimeLogSettings();
    // Match DebugTab.tsx: omit maxLines to use the core's 20,000-line limit.
    runtimeLog = await core.readRuntimeLog();
  }

  Future<bool> refreshLogs() => run(_readLogs);
  Future<bool> saveLogSettings(bool enabled, RuntimeLogLevelDto level) =>
      run(() async {
        await core.setRuntimeLogSettings(enabled: enabled, level: level);
        await _readLogs();
      });
  Future<bool> clearLogs() => run(() async {
    await core.clearRuntimeLog();
    await _readLogs();
  });
  Future<bool> shareLogs() => run(() async {
    sharedLogPath = null;
    final result = await android.shareRuntimeLog();
    sharedLogPath = result?['path'] as String?;
    if (result?['shareError'] case final String failure
        when failure.isNotEmpty) {
      throw StateError(failure);
    }
  });

  @override
  void dispose() {
    _disposed = true;
    unawaited(_events?.cancel());
    super.dispose();
  }
}
