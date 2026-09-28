import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

import '../platform/android_host.dart';
import '../rust/api/core.dart' as core;
import '../rust/api/types.dart';
import '../settings/android_settings.dart';
import 'options.dart'
    show AppPage, defaultDownloadSource, schemeAssets, schemes;
import 'strings.dart';

enum OperationKind { install, deploy, addon, wanxiang, customDirectory }

enum LogLineKind { normal, error, warning, deploy, merged }

enum DeployStepState { running, ok, failed }

enum ImeBadgeState {
  checking,
  componentMissing,
  schemeMissing,
  pendingDeploy,
  ready,
}

class DeployStep {
  const DeployStep(this.message, this.state);
  final String message;
  final DeployStepState state;
}

LogLineKind operationLogKind(String line) {
  if (line.contains('[DEPLOY ERROR]') || line.contains('[ERROR]')) {
    return LogLineKind.error;
  }
  if (line.contains('[WARN]')) return LogLineKind.warning;
  if (line.contains('[DEPLOY]')) return LogLineKind.deploy;
  if (line.contains('[MERGED]') || line.contains('[RENAMED]')) {
    return LogLineKind.merged;
  }
  return LogLineKind.normal;
}

String formatLogBytes(BigInt bytes) {
  if (bytes < BigInt.from(1024)) return '$bytes B';
  if (bytes < BigInt.from(1024 * 1024)) {
    return '${(bytes.toDouble() / 1024).toStringAsFixed(1)} KB';
  }
  return '${(bytes.toDouble() / (1024 * 1024)).toStringAsFixed(1)} MB';
}

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
    ? error.message ?? AppStrings.systemFailure
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
  bool _startupStarted = false;
  bool _logsWhenIdle = false;
  int _releaseRequest = 0;
  String? _reportedMigrationDeployError;
  AppPage activePage = AppPage.input;
  OperationKind? operation;
  bool checkingStorage = false;
  bool releaseLoading = false;
  bool logsLoading = false;
  bool filesLoading = false;
  bool onboardingCompleted = false;
  AppUpdateDto? appUpdate;
  String? releaseError;
  String? installError;
  String? addonError;
  String? wanxiangError;
  String? macosImeError;
  String? debugError;
  String? debugNotice;
  InstallProgressDto? installProgress;
  InstallProgressDto? addonProgress;
  InstallProgressDto? wanxiangProgress;
  InstallProgressDto? customProgress;
  final List<DeployStep> deploySteps = [];
  WanxiangStatusDto? wanxiang;
  DebugLogsDto? systemLogs;
  String? selectedDirectory;
  String? selectedTreeUri;
  List<FileItemDto> customFiles = [];
  LocalSchemasDto? customSchemas;
  LocalSchemaDto? customLocal;
  InstallResultDto? customResult;
  String? customError;
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
  String source = defaultDownloadSource;
  ReleaseInfoDto? latestRelease;
  SchemeReleaseDto? schemeRelease;
  StreamSubscription<BridgeEvent>? _events;
  StreamSubscription<InstallProgressDto>? _androidProgress;

  bool get canInstall =>
      !busy && !releaseLoading && downloadUrl?.isNotEmpty == true;
  bool get isInstalling => operation == OperationKind.install;
  bool get isDeploying => operation == OperationKind.deploy;
  bool get isManagingAddon => operation == OperationKind.addon;
  bool get isManagingWanxiang => operation == OperationKind.wanxiang;
  bool get isInstallingCustom => operation == OperationKind.customDirectory;
  bool get canInstallAddon => !busy && local?.installed == true;
  bool get canInstallWanxiang => canInstallAddon;
  bool get canUninstallAddon => !busy && addon?.installed == true;
  bool get canUninstallWanxiang => !busy && wanxiang?.installed == true;
  bool get showAddonInstall => addon?.deployed != true;
  bool get canInstallCustom => canInstall && selectedDirectory != null;
  int get operationLogCount => operationLogs.length;
  int get customVerifyFailureCount =>
      customResult?.verify.where((v) => !v.ok).length ?? 0;
  String get installButtonLabel => checkingStorage
      ? AppStrings.checkingPermission
      : isInstalling
      ? AppStrings.installing
      : isDeploying
      ? AppStrings.deploying
      : local?.installed == true
      ? AppStrings.updateScheme
      : AppStrings.installScheme;
  String get deployTooltip => local?.installed == true
      ? AppStrings.deployCurrent
      : AppStrings.installFirst;
  String get addonTooltip => local?.installed == true
      ? AppStrings.installEnglishTooltip
      : AppStrings.installKeytaoFirst;
  String get wanxiangTooltip => local?.installed == true
      ? AppStrings.installWanxiangTooltip
      : AppStrings.installMainFirst;
  String get addonButtonLabel =>
      addon?.installed == true ? AppStrings.redeploy : AppStrings.install;
  String get wanxiangButtonLabel =>
      wanxiang?.installed == true ? AppStrings.redeploy : AppStrings.install;
  String get addonStatusText =>
      _addonStatus(addon?.installed, addon?.deployed, addon?.version);
  String get wanxiangStatusText =>
      _addonStatus(wanxiang?.installed, wanxiang?.deployed, wanxiang?.version);
  String _addonStatus(bool? installed, bool? deployed, String? version) =>
      installed == null
      ? AppStrings.checking
      : !installed
      ? AppStrings.notInstalled
      : deployed != true
      ? AppStrings.installedNotDeployed
      : AppStrings.deployedVersion(version ?? '');
  String? get localStatusLine => local == null
      ? null
      : AppStrings.localStatus(
          installed: local!.installed,
          deployed: local!.deployed,
          version: local!.version,
          label: schemes[schemeKeyFromSchemas(local!.schemas)],
        );
  String? get localSchemaIds =>
      local?.installed == true && local!.schemas.isNotEmpty
      ? '(${local!.schemas.join(', ')})'
      : null;
  String get selectedSchemeAsset => scheme == 'keytao'
      ? schemeAssets[scheme]!
      : schemeRelease?.assetName ?? schemeAssets[scheme] ?? '';
  String get schemeSelectionText =>
      '${AppStrings.selection}${schemes[scheme] ?? scheme}'
      '${releaseVersion == null ? '' : ' $releaseVersion'} · $selectedSchemeAsset';
  Map<String, String> get sourceVersions => {
    if (latestRelease?.github case final release?) 'github': release.version,
    if (latestRelease?.gitee case final release?) 'gitee': release.version,
  };
  String? get changelogBody =>
      scheme == 'keytao' && latestRelease?.body.isNotEmpty == true
      ? latestRelease!.body
      : null;
  String? get changelogTitle => changelogBody == null
      ? null
      : AppStrings.changelogTitle(
          latestRelease!.name.isEmpty
              ? latestRelease!.version
              : latestRelease!.name,
        );
  ImeBadgeState get macosImeBadge => macosIme == null
      ? ImeBadgeState.checking
      : !macosIme!.installed
      ? ImeBadgeState.componentMissing
      : local?.installed != true
      ? ImeBadgeState.schemeMissing
      : local?.deployed != true
      ? ImeBadgeState.pendingDeploy
      : ImeBadgeState.ready;
  String get macosImeBadgeText => switch (macosImeBadge) {
    ImeBadgeState.checking => AppStrings.checking,
    ImeBadgeState.componentMissing => AppStrings.componentMissing,
    ImeBadgeState.schemeMissing => AppStrings.schemeMissing,
    ImeBadgeState.pendingDeploy => AppStrings.pendingDeploy,
    ImeBadgeState.ready => AppStrings.usable,
  };
  String get themeBadge =>
      switch (uiSettings?.colorScheme ?? UiColorSchemeDto.auto) {
        UiColorSchemeDto.auto => AppStrings.followSystem,
        UiColorSchemeDto.dark => AppStrings.fixedDark,
        UiColorSchemeDto.light => AppStrings.fixedLight,
      };
  String get defaultDir => info.userRoot;
  String get dictionaryManagerUrl => Uri(
    scheme: 'rime-dict',
    host: 'open',
    queryParameters: {'dir': defaultDir},
  ).toString();
  bool get canOpenDictionaryManager =>
      !isAndroid && !busy && defaultDir.isNotEmpty;
  String get englishModeLabel =>
      (isAndroid
          ? androidSettings?.englishMode == 'schema'
          : desktopEnglishMode == EnglishModeDto.schema)
      ? AppStrings.englishSchema
      : AppStrings.englishAscii;
  String? get englishModeHint => englishReady ? null : AppStrings.englishHint;
  String? get migrationError => storage?['migrationError'] as String?;
  String get storageActionLabel => storage?['granted'] == true
      ? AppStrings.retryMigration
      : AppStrings.openStoragePermission;
  bool get canOpenStorageSettings =>
      !busy && storage?['canOpenSettings'] != false;
  String get permissionDescription =>
      AppStrings.permissionDescription(storage?['path'] as String?);
  String? get imeMessage => ime?['message'] as String?;
  String? get logDirectory => logSettings?.logDir;
  int get logFileCount => logSettings?.fileCount ?? 0;
  String get logTotalSize =>
      formatLogBytes(logSettings?.totalBytes ?? BigInt.zero);
  String? get logStatsText => logSettings == null
      ? null
      : AppStrings.logStats(logFileCount, logTotalSize);
  List<String> get logFileLines => [
    for (final file in logSettings?.files ?? <RuntimeLogFileInfoDto>[])
      '${file.name} · ${formatLogBytes(file.size)}',
  ];
  String? get sharedLogResultText =>
      sharedLogPath == null ? null : AppStrings.savedLog(sharedLogPath!);

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
    if (_events != null) return;
    if (isAndroid) {
      _androidProgress = android.installProgress.listen(
        _receiveInstallProgress,
      );
    }
    final ready = Completer<void>();
    _events = core.coreEvents().listen(
      (event) {
        if (event.kind == BridgeEventKind.ready && !ready.isCompleted) {
          ready.complete();
        }
        if (event.installProgress case final value?) {
          _receiveInstallProgress(value);
        }
        if (event.deployProgress case final value?) {
          progress = value;
          _deployStep(value, DeployStepState.running);
          addOperationLogs(['[DEPLOY] $value']);
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

  void _receiveInstallProgress(InstallProgressDto value) {
    installProgress = value;
    progress = value.message;
    installFraction = value.percent.clamp(0, 100) / 100;
    if (isInstallingCustom) customProgress = value;
    if (isManagingAddon) addonProgress = value;
    if (isManagingWanxiang) wanxiangProgress = value;
    addOperationLogs([value.message]);
  }

  void addOperationLogs(Iterable<String> lines) {
    final now = DateTime.now().toLocal();
    String part(int value) => value.toString().padLeft(2, '0');
    final time = '${part(now.hour)}:${part(now.minute)}:${part(now.second)}';
    for (final line in lines) {
      for (final part in line.split('\n')) {
        operationLogs.add('[$time] $part');
      }
    }
    _changed();
  }

  void clearOperationLogs() {
    operationLogs.clear();
    _changed();
  }

  void _deployStep(String message, DeployStepState state) {
    for (var i = 0; i < deploySteps.length; i++) {
      if (deploySteps[i].state == DeployStepState.running) {
        deploySteps[i] = DeployStep(
          deploySteps[i].message,
          state == DeployStepState.failed
              ? DeployStepState.failed
              : DeployStepState.ok,
        );
      }
    }
    deploySteps.add(DeployStep(message, state));
    _changed();
  }

  void selectPage(AppPage page) {
    if (activePage == page) return;
    activePage = page;
    if (info.platform == BridgePlatform.macOs) unawaited(setWindowTitle());
    if (page == AppPage.debug) {
      if (busy) {
        _logsWhenIdle = true;
      } else {
        unawaited(refreshLogs());
      }
    }
    _changed();
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
      } else if (_logsWhenIdle && !_disposed) {
        _logsWhenIdle = false;
        if (activePage == AppPage.debug) unawaited(refreshLogs());
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
        if (_startupStarted) unawaited(refreshRelease());
      }
    }
    try {
      addon = await core.addonSchemaStatus(id: 'easy_en');
      addonError = null;
    } catch (failure) {
      addonError = errorText(failure);
    }
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
          await _readStorage();
        }),
        read(() async {
          ime = await android.imeStatus();
        }),
      ]);
    } else {
      macosIme = null;
      await read(() async {
        try {
          macosIme = await core.macosImeStatus();
          macosImeError = null;
        } catch (failure) {
          macosImeError = errorText(failure);
          rethrow;
        }
      });
    }
    local = null;
    addon = null;
    await Future.wait([
      read(_readLocal),
      read(() async {
        try {
          wanxiang = await core.wanxiangStatus();
          wanxiangError = null;
        } catch (failure) {
          wanxiangError = errorText(failure);
        }
      }),
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
    if (isAndroid && setupReady && !onboardingCompleted) {
      await core.completeOnboarding();
      onboardingCompleted = true;
    }
  }

  Future<bool> refreshState() async {
    final result = await run(_readState);
    if (!_startupStarted && !_disposed) {
      _startupStarted = true;
      unawaited(_checkAppUpdate());
      unawaited(refreshRelease());
    }
    return result;
  }

  Future<void> _checkAppUpdate() async {
    try {
      final update = await core.checkAppUpdate();
      if (!_disposed) appUpdate = update.hasUpdate ? update : null;
    } catch (_) {
      // Startup update failures are intentionally silent, as in App.tsx.
    }
    _changed();
  }

  void onResume() {
    if (busy) {
      _refreshWhenIdle = true;
    } else {
      unawaited(refreshState());
    }
  }

  void selectScheme(String value) {
    if (busy || !schemes.containsKey(value)) return;
    _manuallySelectedScheme = true;
    if (value == scheme) return;
    scheme = value;
    schemeRelease = null;
    latestRelease = null;
    error = null;
    _changed();
    unawaited(refreshRelease());
  }

  void selectSource(String value) {
    if (busy || value == source || (value != 'github' && value != 'gitee')) {
      return;
    }
    source = value;
    unawaited(refreshRelease());
  }

  Future<bool> refreshRelease() async {
    if (_disposed) return false;
    final request = ++_releaseRequest;
    final selected = scheme;
    releaseLoading = true;
    releaseError = null;
    latestRelease = null;
    schemeRelease = null;
    _changed();
    try {
      if (selected == 'keytao') {
        final release = await core.fetchLatestRelease();
        if (request == _releaseRequest && !_disposed) latestRelease = release;
      } else {
        final release = await core.fetchSchemeRelease(scheme: selected);
        if (request == _releaseRequest && !_disposed) schemeRelease = release;
      }
      return request == _releaseRequest && !_disposed;
    } catch (failure) {
      if (request == _releaseRequest && !_disposed) {
        releaseError = '${AppStrings.releaseFailure}${errorText(failure)}';
      }
      return false;
    } finally {
      if (request == _releaseRequest && !_disposed) {
        releaseLoading = false;
        _changed();
      }
    }
  }

  Future<void> _readStorage() async {
    storage = await android.storagePermissionStatus();
    final failure = storage?['deployError'] as String?;
    if (failure != null &&
        failure.isNotEmpty &&
        failure != _reportedMigrationDeployError &&
        !isDeploying) {
      _deployStep(failure, DeployStepState.failed);
      addOperationLogs(['[DEPLOY ERROR] $failure']);
      _reportedMigrationDeployError = failure;
    } else if (failure == null || failure.isEmpty) {
      _reportedMigrationDeployError = null;
    }
  }

  Future<void> _requireStorage({bool redirect = false}) async {
    checkingStorage = true;
    _changed();
    try {
      await _readStorage();
      if (!storageReady) {
        final message = storage?['message'] as String?;
        final failure = message == null || message.isEmpty
            ? AppStrings.storagePermission
            : message;
        if (redirect) {
          installError = failure;
          addOperationLogs(['[ANDROID PERMISSION] $failure']);
          try {
            await android.openStoragePermissionSettings();
          } catch (settingsFailure) {
            addOperationLogs([
              '[ANDROID PERMISSION] ${errorText(settingsFailure)}',
            ]);
          }
        }
        throw StateError(failure);
      }
      if (!migrationReady) {
        throw StateError(storage!['migrationError'] as String);
      }
    } finally {
      checkingStorage = false;
      _changed();
    }
  }

  void _startOperation(OperationKind kind, String message) {
    operation = kind;
    verification = [];
    installProgress = null;
    installFraction = null;
    progress = message;
    _changed();
  }

  Future<bool> install() {
    if (!canInstall) return Future.value(false);
    final url = downloadUrl!;
    return run(() async {
      _startOperation(OperationKind.install, AppStrings.installing);
      installError = null;
      deploySteps.clear();
      try {
        if (isAndroid) await _requireStorage(redirect: true);
        final InstallResultDto result;
        if (isAndroid) {
          final zipPath = await core.prepareAndroidInstall(url: url);
          try {
            final extracted = await android.smartExtractZipToPrivate(zipPath);
            result = await core.finishAndroidInstall(
              resultJson: jsonEncode(extracted),
            );
          } finally {
            await core.removeDownloadedArchive(path: zipPath);
          }
        } else {
          result = await core.installSchemeFromUrl(url: url);
        }
        _recordInstallResult(result);
        verification = result.verify;
        await _readLocal();
        progress = AppStrings.installDone;
      } catch (failure) {
        progress = AppStrings.installFailed;
        installError = errorText(failure);
        addOperationLogs(['[ERROR] $installError']);
        rethrow;
      } finally {
        operation = null;
      }
    });
  }

  void _recordInstallResult(InstallResultDto result) {
    addOperationLogs(result.logs);
    addOperationLogs(
      result.verify
          .where((entry) => !entry.ok)
          .map((entry) => '[VERIFY FAIL] ${entry.path}: ${entry.note}'),
    );
  }

  Future<bool> deploy() => run(() async {
    _startOperation(OperationKind.deploy, AppStrings.deployingLibrime);
    deploySteps.clear();
    try {
      if (local?.installed != true) {
        throw StateError(AppStrings.noSchemeForDeploy);
      }
      _deployStep(AppStrings.deployingLibrime, DeployStepState.running);
      final String message;
      if (isAndroid) {
        await _requireStorage();
        final result = await android.deployImeData();
        if (result['deployed'] != true) {
          throw StateError(AppStrings.deployIncomplete);
        }
        message = result['message'] as String? ?? AppStrings.deployDone;
      } else {
        final result = await core.deployDefault();
        if (!result.success) throw StateError(result.message);
        message = result.message;
      }
      await _readState();
      if (local?.deployed != true) {
        throw StateError(AppStrings.deployCheckFailed);
      }
      _deployStep(message, DeployStepState.ok);
      addOperationLogs(['[DEPLOY] $message']);
      progress = message;
    } catch (failure) {
      progress = AppStrings.deployFailed;
      _deployStep(errorText(failure), DeployStepState.failed);
      addOperationLogs(['[DEPLOY ERROR] ${errorText(failure)}']);
      rethrow;
    } finally {
      operation = null;
    }
  });

  // The bridge holds its install lock while invoking this callback. Never
  // re-enter a bridge install/deploy call here, including during rollback.
  Future<String> _androidDeploy() async {
    try {
      final result = await android.deployImeData();
      if (result['deployed'] == true) return '';
      final message = result['message'] as String?;
      return message == null || message.isEmpty
          ? AppStrings.deployIncomplete
          : message;
    } catch (failure) {
      return errorText(failure);
    }
  }

  Future<bool> installAddon() => _manageAddon(true);
  Future<bool> uninstallAddon() => _manageAddon(false);
  Future<bool> _manageAddon(bool installed) => run(() async {
    _startOperation(
      OperationKind.addon,
      installed ? AppStrings.installing : AppStrings.uninstall,
    );
    addonError = null;
    addonProgress = null;
    try {
      if (installed && local?.installed != true) {
        throw StateError(AppStrings.installKeytaoFirst);
      }
      if (isAndroid) {
        await _requireStorage();
        if (installed) await android.copyAddonSchemaAssets('easy_en');
      }
      addon = installed
          ? await core.addonSchemaInstall(
              id: 'easy_en',
              androidDeploy: _androidDeploy,
            )
          : await core.addonSchemaUninstall(
              id: 'easy_en',
              androidDeploy: _androidDeploy,
            );
      addOperationLogs([
        installed
            ? AppStrings.easyEnglishInstalled(addon!.version)
            : AppStrings.easyEnglishRemoved,
      ]);
      await _readLocal();
      if (!installed) {
        if (isAndroid) {
          androidSettings = await core.getAndroidImeInputSettings();
        } else {
          desktopEnglishMode = await core.getDesktopEnglishMode();
        }
      }
    } catch (failure) {
      addonError = errorText(failure);
      addOperationLogs(['[ADDON ERROR] $addonError']);
      try {
        addon = await core.addonSchemaStatus(id: 'easy_en');
      } catch (_) {}
      rethrow;
    } finally {
      operation = null;
    }
  });

  Future<bool> manageWanxiang(bool installed) => run(() async {
    _startOperation(
      OperationKind.wanxiang,
      installed ? AppStrings.installing : AppStrings.uninstall,
    );
    wanxiangError = null;
    wanxiangProgress = null;
    try {
      if (installed && local?.installed != true) {
        throw StateError(AppStrings.installMainFirst);
      }
      if (isAndroid) await _requireStorage();
      wanxiang = await core.manageWanxiang(
        installed: installed,
        androidDeploy: _androidDeploy,
      );
      await _readLocal();
      addOperationLogs([
        installed ? AppStrings.wanxiangInstalled : AppStrings.wanxiangRemoved,
      ]);
    } catch (failure) {
      wanxiangError = errorText(failure);
      try {
        wanxiang = await core.wanxiangStatus();
      } catch (_) {}
      try {
        await _readLocal();
      } catch (_) {}
      rethrow;
    } finally {
      operation = null;
    }
  });

  Future<bool> pickCustomDirectory() => run(() async {
    customError = null;
    try {
      if (isAndroid) await _requireStorage(redirect: true);
      final selected = isAndroid
          ? await android.pickDirectory()
          : await const MethodChannel('ink.rea.keytao/window')
                .invokeMethod<String>('pickDirectory');
      if (selected == null || selected.isEmpty) return;
      selectedTreeUri = isAndroid ? selected : null;
      selectedDirectory = isAndroid ? _safDisplayPath(selected) : selected;
      customResult = null;
      customSchemas = null;
      customLocal = null;
      customFiles = [];
      await _readCustomDirectory();
    } catch (failure) {
      customError = errorText(failure);
      rethrow;
    }
  });

  String _safDisplayPath(String uri) {
    try {
      final tree = Uri.decodeComponent(uri.split('/tree/')[1]);
      return '/${tree.replaceFirst('primary:', 'sdcard/')}';
    } catch (_) {
      return uri;
    }
  }

  Future<void> _readCustomDirectory() async {
    if (selectedDirectory == null) return;
    filesLoading = true;
    _changed();
    final failures = <String>[];
    Future<void> read(Future<void> Function() action) async {
      try {
        await action();
      } catch (failure) {
        failures.add(errorText(failure));
      }
    }

    try {
      if (isAndroid) {
        await Future.wait([
          read(() async {
            final result = await android.listFiles(selectedTreeUri!);
            customFiles = [
              for (final item in result['files'] as List)
                FileItemDto(
                  name: item['name'] as String,
                  isDir: item['isDir'] as bool,
                ),
            ];
          }),
          read(() async {
            final result = await android.readLocalSchemas(selectedTreeUri!);
            final schemas = List<String>.from(result['schemas'] as List);
            customLocal = LocalSchemaDto(
              installed: result['installed'] == true,
              deployed: result['deployed'] == true,
              version: result['version'] as String?,
              schemas: schemas,
            );
          }),
        ]);
        customSchemas = customLocal == null
            ? null
            : LocalSchemasDto(
                hasDefaultCustom: customFiles.any(
                  (file) =>
                      !file.isDir &&
                      (file.name == 'default.custom.yaml' ||
                          file.name == 'default-custom.yaml'),
                ),
                schemas: customLocal!.schemas,
              );
      } else {
        await Future.wait([
          read(() async {
            customFiles = await core.listDir(dir: selectedDirectory!);
          }),
          read(() async {
            customSchemas = await core.readLocalSchemas(
              dir: selectedDirectory!,
            );
          }),
        ]);
      }
      if (failures.isNotEmpty) throw StateError(failures.join('；'));
    } finally {
      filesLoading = false;
      _changed();
    }
  }

  Future<bool> refreshCustomDirectory() => run(() async {
    customError = null;
    try {
      await _readCustomDirectory();
    } catch (failure) {
      customError = errorText(failure);
      rethrow;
    }
  });

  Future<bool> installCustomDirectory() {
    if (!canInstallCustom) return Future.value(false);
    final url = downloadUrl!;
    return run(() async {
      _startOperation(OperationKind.customDirectory, AppStrings.installing);
      customResult = null;
      customProgress = null;
      customError = null;
      try {
        if (isAndroid) {
          await _requireStorage();
          final archive = await core.downloadSchemeArchive(url: url);
          try {
            final result = await android.smartExtractZip(
              archive,
              selectedTreeUri!,
            );
            customResult = await core.finishAndroidInstall(
              resultJson: jsonEncode(result),
            );
          } finally {
            await core.removeDownloadedArchive(path: archive);
          }
        } else {
          customResult = await core.installSchemeToDir(
            url: url,
            dir: selectedDirectory!,
          );
        }
        _recordInstallResult(customResult!);
        progress = AppStrings.customInstallDone;
        await _readCustomDirectory();
      } catch (failure) {
        customError = errorText(failure);
        addOperationLogs(['[ERROR] $customError']);
        rethrow;
      } finally {
        operation = null;
      }
    });
  }

  Future<bool> finishOnboarding() => run(() async {
    await _readState();
    if (!setupReady) throw StateError(AppStrings.finishSetupFirst);
    await core.completeOnboarding();
    onboardingCompleted = true;
  });

  Future<bool> saveAndroid(Map<String, Object> patch) => run(() async {
    await _requireStorage();
    final current = await core.getAndroidImeInputSettings();
    if (patch['englishMode'] == 'schema') {
      await _readLocal();
      if (!englishReady) throw StateError(AppStrings.englishNotReady);
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
  }) {
    final normalizedAccent = accentColor?.trim().toUpperCase();
    if (normalizedAccent != null &&
        !RegExp(r'^#[0-9A-F]{6}$').hasMatch(normalizedAccent)) {
      return Future.value(false);
    }
    return run(() async {
      final current = await core.getImeUiSettings();
      uiSettings = await core.setImeUiSettings(
        colorScheme: colorScheme ?? current.colorScheme,
        orientation: orientation ?? current.orientation,
        accentColor: normalizedAccent ?? current.accentColor,
        fontSize: (fontSize ?? current.fontSize).clamp(10, 36),
      );
    });
  }

  Future<bool> saveEmbedded(bool value) => run(() async {
    uiSettings = await core.setImeEmbeddedComposition(embedded: value);
  });
  Future<bool> saveDesktopEnglish(EnglishModeDto value) => run(() async {
    await _readLocal();
    if (value == EnglishModeDto.schema && !englishReady) {
      throw StateError(AppStrings.englishNotReady);
    }
    desktopEnglishMode = await core.setDesktopEnglishMode(mode: value);
  });

  Future<bool> open(String value) => run(() => _open(value));
  Future<void> _open(String value) async {
    if (isAndroid) {
      await android.openUrl(value);
    } else {
      final result = await Process.run('open', [value]);
      if (result.exitCode != 0) {
        throw StateError('${AppStrings.cannotOpen}${result.stderr}');
      }
    }
  }

  Future<bool> openDictionaryManager() => open(dictionaryManagerUrl);
  Future<bool> openDefaultDirectory() => open(defaultDir);
  Future<bool> openCustomDirectory() => selectedDirectory == null
      ? Future.value(false)
      : open(selectedTreeUri ?? selectedDirectory!);
  Future<bool> openTheme() => uiSettings?.themePath == null
      ? Future.value(false)
      : open(uiSettings!.themePath!);
  Future<bool> openAppUpdate() =>
      appUpdate == null ? Future.value(false) : open(appUpdate!.releaseUrl);
  // A title change must not toggle `busy`; a missing macOS channel is ignored.
  Future<void> setWindowTitle([String? title]) async {
    try {
      await const MethodChannel('ink.rea.keytao/window')
          .invokeMethod<void>('setTitle', title ?? activePage.title);
    } on Object catch (_) {}
  }
  Future<bool> openStoragePermissionSettings() => run(() async {
    await android.openStoragePermissionSettings();
    await _readStorage();
  });
  Future<bool> openInputMethodSettings() =>
      run(android.openInputMethodSettings);
  Future<bool> showInputMethodPicker() => run(android.showInputMethodPicker);
  Future<bool> resetAndroidSettings() => saveAndroid(androidDefaults);

  Future<void> _readLogs() async {
    logsLoading = true;
    debugError = null;
    debugNotice = null;
    _changed();
    Future<String?> read(String prefix, Future<void> Function() action) async {
      try {
        await action();
        return null;
      } catch (failure) {
        return AppStrings.failure(prefix, errorText(failure));
      }
    }

    try {
      final errors = await Future.wait([
        read(AppStrings.readLogSettingsFailure, () async {
          logSettings = await core.getRuntimeLogSettings();
        }),
        read(AppStrings.readRuntimeLogFailure, () async {
          // Omit maxLines to keep Core's 20,000-line limit.
          runtimeLog = await core.readRuntimeLog();
        }),
        read(AppStrings.readSystemLogsFailure, () async {
          systemLogs = await core.readDebugLogs();
        }),
      ]);
      final messages = errors.whereType<String>().toList();
      debugError = messages.isEmpty ? null : messages.join('；');
    } finally {
      logsLoading = false;
      _changed();
    }
  }

  Future<bool> refreshLogs() => run(_readLogs);

  Future<bool> _logAction(
    String failurePrefix,
    Future<void> Function() action, {
    String? notice,
  }) => run(() async {
    debugError = null;
    debugNotice = null;
    try {
      await action();
      if (debugError == null) debugNotice = notice;
    } catch (failure) {
      debugError = AppStrings.failure(failurePrefix, errorText(failure));
      throw StateError(debugError!);
    }
  });

  Future<bool> saveLogSettings(bool enabled, RuntimeLogLevelDto level) =>
      _logAction(AppStrings.updateLogSettingsFailure, () async {
        await core.setRuntimeLogSettings(enabled: enabled, level: level);
        logSettings = await core.getRuntimeLogSettings();
      });
  Future<bool> clearLogs() => _logAction(AppStrings.clearLogsFailure, () async {
    await core.clearRuntimeLog();
    await _readLogs();
  }, notice: AppStrings.logsCleared);
  Future<void> _shareLogs() async {
    sharedLogPath = null;
    if (!isAndroid) {
      // Desktop's old share_runtime_log action opens the fixed log directory.
      await _open('$defaultDir/log');
      return;
    }
    final result = await android.shareRuntimeLog();
    sharedLogPath = result?['path'] as String?;
    if (result?['shareError'] case final String failure
        when failure.isNotEmpty) {
      throw StateError(failure);
    }
  }

  Future<bool> shareLogs() => _logAction(AppStrings.shareFailure, _shareLogs);
  Future<bool> openLogDirectory() =>
      _logAction(AppStrings.openLogDirectoryFailure, () async {
        if (logDirectory == null) return;
        try {
          await _open(logDirectory!);
        } catch (_) {
          await _shareLogs();
        }
      });
  Future<bool> copyLogDirectory() =>
      _logAction(AppStrings.copyPathFailure, () async {
        if (logDirectory == null) return;
        await Clipboard.setData(ClipboardData(text: logDirectory!));
      }, notice: AppStrings.logPathCopied);

  @override
  void dispose() {
    _disposed = true;
    unawaited(_events?.cancel());
    unawaited(_androidProgress?.cancel());
    super.dispose();
  }
}
