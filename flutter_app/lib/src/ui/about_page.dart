import 'dart:io' show exit;

// ignore: deprecated_member_use
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:forui/forui.dart';

import '../app/controller.dart';
import '../app/options.dart';
import '../app/uninstaller.dart';
import '../rust/api/types.dart';
import 'debug_page.dart';
import 'theme.dart';
import 'widgets.dart';

const _licenses = [
  ('librime', 'BSD-3-Clause', ['assets/licenses/librime-LICENSE.txt']),
  (
    'libjd-engine',
    'GPL-3.0',
    [
      'assets/licenses/libjd-engine-SOURCE.txt',
      'assets/licenses/libjd-engine-LICENSE.txt',
    ],
  ),
  ('Flutter', 'BSD-3-Clause', ['assets/licenses/flutter-LICENSE.txt']),
  ('OpenCC', 'Apache-2.0', ['assets/licenses/opencc-LICENSE.txt']),
  (
    'webview_core',
    'Apache-2.0',
    ['third_party/webview_core/NOTICE', 'third_party/webview_core/LICENSE'],
  ),
];

const _librimeLicenses = [
  ('librime-lua', 'BSD-3-Clause', ['assets/licenses/librime-lua-LICENSE.txt']),
  ('Lua', 'MIT', ['assets/licenses/lua-LICENSE.txt']),
  (
    'librime-octagram',
    'GPL-3.0',
    [
      'assets/licenses/librime-octagram-SOURCE.txt',
      'assets/licenses/librime-octagram-LICENSE.txt',
    ],
  ),
  (
    'librime-predict',
    'BSD-3-Clause',
    ['assets/licenses/librime-predict-LICENSE.txt'],
  ),
  ('leveldb', 'BSD-3-Clause', ['assets/licenses/leveldb-LICENSE.txt']),
  (
    'marisa-trie',
    'BSD-2-Clause OR LGPL-2.1-or-later',
    ['assets/licenses/marisa-trie-LICENSE.txt'],
  ),
  ('yaml-cpp', 'MIT', ['assets/licenses/yaml-cpp-LICENSE.txt']),
  ('glog', 'BSD-3-Clause', ['assets/licenses/glog-LICENSE.txt']),
];

class AboutPage extends StatelessWidget {
  const AboutPage({super.key, required this.controller});
  final AppController controller;

  Future<void> _checkForUpdates(BuildContext context, Locale locale) async {
    final update = await controller.checkAppUpdateNow();
    if (!context.mounted) return;
    if (update == null) {
      if (controller.appUpdateError case final error?) {
        controller.showToast(AppStrings.appUpdateFailure(locale, error));
      }
      return;
    }
    if (!update.hasUpdate) {
      controller.showToast(AppStrings.appUpToDate(locale));
      return;
    }
    final redownload = await showFDialog<bool>(
      context: context,
      builder: (context, _, animation) => AppDialog(
        title: AppStrings.checkAppUpdate(locale),
        animation: animation,
        actions: [
          ActionButton(AppStrings.close, onPress: () => Navigator.pop(context)),
        ],
        child: ListenableBuilder(
          listenable: controller,
          builder: (_, _) => AppUpdateBanner(
            controller: controller,
            onDownload: (redownload) => Navigator.pop(context, redownload),
          ),
        ),
      ),
    );
    if (redownload != null) {
      await controller.openAppUpdate(redownload: redownload);
    }
  }

  Future<void> _uninstall(BuildContext context) async {
    var clearUserData = false;
    final shouldContinue = await showFDialog<bool>(
      context: context,
      builder: (context, _, animation) => StatefulBuilder(
        builder: (context, setState) => AppDialog(
          title: '卸载 KeyTao',
          animation: animation,
          actions: [
            ActionButton('取消', onPress: () => Navigator.pop(context)),
            ActionButton(
              '继续',
              icon: FLucideIcons.trash2,
              primary: true,
              onPress: () => Navigator.pop(context, true),
            ),
          ],
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              const Text('将从 Mac 移除 KeyTao 主应用和系统输入法组件。'),
              const SizedBox(height: 12),
              Row(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  FCheckbox(
                    key: const ValueKey('uninstall-clear-user-data'),
                    value: clearUserData,
                    semanticsLabel: '同时清除本机 KeyTao 个人资料',
                    onChange: (value) => setState(() {
                      clearUserData = value;
                    }),
                  ),
                  const SizedBox(width: 10),
                  const Expanded(
                    child: Padding(
                      padding: EdgeInsets.only(top: 4),
                      child: Text(
                        '同时清除本机方案、词库、学习记录、应用设置和缓存。'
                        '自定义 Rime 数据目录不会删除。',
                      ),
                    ),
                  ),
                ],
              ),
            ],
          ),
        ),
      ),
    );
    if (shouldContinue != true || !context.mounted) return;

    if (clearUserData) {
      var confirmed = false;
      final clearConfirmed = await showFDialog<bool>(
        context: context,
        barrierDismissible: false,
        builder: (context, _, animation) => StatefulBuilder(
          builder: (context, setState) => AppDialog(
            title: '永久删除 KeyTao 个人资料？',
            animation: animation,
            actions: [
              ActionButton('返回', onPress: () => Navigator.pop(context)),
              ActionButton(
                '永久删除并卸载',
                icon: FLucideIcons.trash2,
                primary: true,
                onPress: confirmed ? () => Navigator.pop(context, true) : null,
              ),
            ],
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                const Text(
                  '将删除 ~/Library/keytao、KeyTao 应用状态、缓存和偏好设置。'
                  '此操作无法撤销。',
                ),
                const SizedBox(height: 12),
                Row(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    FCheckbox(
                      key: const ValueKey('uninstall-confirm-clear-data'),
                      value: confirmed,
                      semanticsLabel: '确认永久删除 KeyTao 个人资料',
                      onChange: (value) => setState(() {
                        confirmed = value;
                      }),
                    ),
                    const SizedBox(width: 10),
                    const Expanded(
                      child: Padding(
                        padding: EdgeInsets.only(top: 4),
                        child: Text('我了解这些本机数据将被永久删除'),
                      ),
                    ),
                  ],
                ),
              ],
            ),
          ),
        ),
      );
      if (clearConfirmed != true || !context.mounted) return;
    }

    try {
      await KeyTaoUninstaller.uninstall(clearUserData: clearUserData);
      exit(0);
    } on KeyTaoUninstallCancelled {
      if (context.mounted) controller.showToast('已取消卸载，KeyTao 未移除');
    } catch (error) {
      if (context.mounted) {
        controller.showToast(
          '卸载未完成：${error.toString().replaceFirst('Bad state: ', '')}',
        );
      }
    }
  }

  Widget _repositoryButton(String name, String url) {
    final label = '打开 $name 的 GitHub 首页';
    return FTooltip(
      tipBuilder: (_, _) => Text(label),
      child: FButton.icon(
        variant: FButtonVariant.ghost,
        onPress: () => controller.open(url),
        child: Icon(FLucideIcons.externalLink, size: 16, semanticLabel: label),
      ),
    );
  }

  Widget _licenseRow(String name, VoidCallback onPress, {String? license}) =>
      FButton.raw(
        variant: FButtonVariant.ghost,
        onPress: onPress,
        child: SettingRow(
          name,
          keepInline: true,
          child: Row(
            mainAxisSize: MainAxisSize.min,
            spacing: 8,
            children: [
              if (license != null) Text(license),
              const Icon(FLucideIcons.chevronRight, size: 16),
            ],
          ),
        ),
      );

  Future<String> _loadLicenseAssets(List<String> assets) async =>
      (await Future.wait(assets.map(rootBundle.loadString))).join('\n\n');

  void _showLicense(BuildContext context, String name, List<String> assets) {
    final text = Future.wait([
      _loadLicenseAssets(assets),
      if (name == 'librime')
        for (final (component, license, files) in _librimeLicenses)
          _loadLicenseAssets(files)
              .then((body) => '$component · $license\n$body'),
    ]).then((parts) => parts.join('\n\n'));
    showFDialog<void>(
      context: context,
      builder: (context, _, animation) => AppDialog(
        title: name,
        animation: animation,
        actions: [
          ActionButton(AppStrings.close, onPress: () => Navigator.pop(context)),
        ],
        child: FutureBuilder<String>(
          future: text,
          builder: (_, snapshot) => snapshot.hasError
              ? ErrorMessage(errorText(snapshot.error!))
              : snapshot.hasData
              ? SelectableText(snapshot.data!)
              : const FCircularProgress(),
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final c = controller;
    final locale = Localizations.localeOf(context);
    final checkUpdateLabel = AppStrings.checkAppUpdate(locale);
    final chinese = locale.languageCode.startsWith('zh');
    final updateChannelLabel = chinese ? '更新频道' : 'Update channel';
    return AppPageBody(
      controller: c,
      page: AppPage.about,
      fullWidth: true,
      spacing: 0,
      children: [
        Padding(
          padding: const EdgeInsets.symmetric(vertical: 15),
          child: Column(
            children: [
              Image.asset(logoAsset, width: 60, height: 60),
              const SizedBox(height: 12),
              Text(
                'KeyTao 输入法',
                textAlign: TextAlign.center,
                style: context.theme.typography.display.xl2.copyWith(
                  fontSize: MediaQuery.sizeOf(context).width >= 720 ? 34 : 28,
                  color: AppPalette.of(context).ink,
                  fontWeight: FontWeight.w700,
                  height: 1.1,
                ),
              ),
              const SizedBox(height: 8),
              Text(
                'v${c.info.appVersion}',
                style: context.theme.typography.body.sm.copyWith(
                  color: AppPalette.of(context).muted,
                ),
              ),
              const SizedBox(height: 12),
              Text(
                'KeyTao 是支持 librime 与 libjd 的跨平台输入法',
                textAlign: TextAlign.center,
                style: context.theme.typography.body.sm.copyWith(
                  color: AppPalette.of(context).muted,
                ),
              ),
            ],
          ),
        ),
        HairlineGridSection(
          children: [
            HairlineGridCell(
              child: Section(
                title: AppStrings.about,
                icon: FLucideIcons.info,
                children: [
                  ValueRow(
                    AppStrings.keytaoVersion,
                    c.versions?.appVersion ?? c.info.appVersion,
                    trailing: Row(
                      mainAxisSize: MainAxisSize.min,
                      children: [
                        FTooltip(
                          tipBuilder: (_, _) => Text(checkUpdateLabel),
                          child: FButton.icon(
                            key: const ValueKey('check-app-update'),
                            variant: FButtonVariant.ghost,
                            onPress: c.checkingAppUpdate || c.appUpdateBusy
                                ? null
                                : () => _checkForUpdates(context, locale),
                            child: c.checkingAppUpdate
                                ? Semantics(
                                    label: checkUpdateLabel,
                                    child: const SizedBox.square(
                                      dimension: 16,
                                      child: FCircularProgress(),
                                    ),
                                  )
                                : Icon(
                                    FLucideIcons.refreshCw,
                                    size: 16,
                                    semanticLabel: checkUpdateLabel,
                                  ),
                          ),
                        ),
                        _repositoryButton('KeyTao', repositoryUrl),
                      ],
                    ),
                  ),
                  SettingRow(
                    updateChannelLabel,
                    keepInline: true,
                    child: SizedBox(
                      width: 160,
                      child: Semantics(
                        label: updateChannelLabel,
                        enabled: c.canChangeAppUpdateChannel,
                        child: FSelect<bool>(
                          size: FTextFieldSizeVariant.sm,
                          enabled: c.canChangeAppUpdateChannel,
                          control: FSelectControl.lifted(
                            value: c.appUpdatePrereleases,
                            onChange: (enabled) async {
                              if (enabled != null &&
                                  await c.setAppUpdatePrereleases(enabled) &&
                                  context.mounted) {
                                await _checkForUpdates(context, locale);
                              }
                            },
                          ),
                          items: {
                            (chinese ? '正式版' : 'Stable'): false,
                            (chinese ? '预发布版' : 'Pre-release'): true,
                          },
                        ),
                      ),
                    ),
                  ),
                  ValueRow('Flutter', c.aboutValues['Flutter']!),
                  ValueRow(
                    AppStrings.librimeVersion,
                    c.versions?.librimeVersion ?? '—',
                    trailing: _repositoryButton(
                      'librime',
                      'https://github.com/rime/librime',
                    ),
                  ),
                  if (c.libjd?.installedVersion case final version?)
                    ValueRow(
                      'libjd-engine 版本',
                      version,
                      trailing: _repositoryButton(
                        'libjd-engine',
                        AppController.libjdRepositoryUrl,
                      ),
                    ),
                  ValueRow(
                    AppStrings.openccVersion,
                    c.versions?.openccVersion ?? '—',
                  ),
                  ValueRow(AppStrings.platform, c.aboutValues['平台']!),
                ],
              ),
            ),
            HairlineGridCell(
              child: Section(
                title: '开源许可',
                children: [
                  for (final (name, license, assets) in _licenses)
                    _licenseRow(
                      name,
                      () => _showLicense(context, name, assets),
                      license: license,
                    ),
                  _licenseRow(
                    '全部开源许可',
                    () => showLicensePage(
                      context: context,
                      applicationName: 'KeyTao',
                      applicationVersion: c.info.appVersion,
                    ),
                  ),
                ],
              ),
            ),
            if (c.info.platform == BridgePlatform.macOs)
              HairlineGridCell(
                child: Section(
                  title: '应用卸载',
                  icon: FLucideIcons.trash2,
                  children: [
                    SettingRow(
                      '卸载 KeyTao',
                      description: '从 Mac 移除主应用和系统输入法组件。',
                      child: ActionButton(
                        '卸载应用',
                        key: const ValueKey('uninstall-keytao'),
                        icon: FLucideIcons.trash2,
                        onPress: () => _uninstall(context),
                        tooltip: '卸载 KeyTao',
                      ),
                    ),
                  ],
                ),
              ),
            ...debugLogCells(c),
          ],
        ),
      ],
    );
  }
}
