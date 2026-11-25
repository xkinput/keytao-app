// ignore: deprecated_member_use
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:forui/forui.dart';

import '../app/controller.dart';
import '../app/options.dart';
import 'debug_page.dart';
import 'theme.dart';
import 'widgets.dart';

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
                title: '许可与声明',
                children: [
                  ActionButton(
                    '开源许可证',
                    onPress: () => showLicensePage(
                      context: context,
                      applicationName: 'KeyTao',
                      applicationVersion: c.info.appVersion,
                    ),
                  ),
                  ActionButton(
                    '第三方声明',
                    onPress: () => showFDialog<void>(
                      context: context,
                      builder: (context, _, animation) => AppDialog(
                        title: '第三方声明',
                        animation: animation,
                        actions: [
                          ActionButton(
                            AppStrings.close,
                            onPress: () => Navigator.pop(context),
                          ),
                        ],
                        child: FutureBuilder<String>(
                          future: rootBundle.loadString(
                            'third_party/webview_core/NOTICE',
                          ),
                          builder: (_, snapshot) => snapshot.hasError
                              ? ErrorMessage(errorText(snapshot.error!))
                              : snapshot.hasData
                              ? SelectableText(snapshot.data!)
                              : const FCircularProgress(),
                        ),
                      ),
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
