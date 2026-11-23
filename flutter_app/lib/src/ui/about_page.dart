import 'package:flutter/widgets.dart';
import 'package:forui/forui.dart';

import '../app/controller.dart';
import '../app/options.dart';
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
    await showFDialog<void>(
      context: context,
      builder: (context, _, animation) => AppDialog(
        title: AppStrings.checkAppUpdate(locale),
        animation: animation,
        actions: [
          ActionButton(AppStrings.close, onPress: () => Navigator.pop(context)),
        ],
        child: ListenableBuilder(
          listenable: controller,
          builder: (_, _) => AppUpdateBanner(controller: controller),
        ),
      ),
    );
  }

  Widget _repositoryButton(String name, String url) {
    final label = '打开 $name 的 GitHub 首页';
    return FTooltip(
      tipBuilder: (_, _) => Text(label),
      child: FButton.icon(
        variant: FButtonVariant.ghost,
        onPress: controller.busy ? null : () => controller.open(url),
        child: Icon(FLucideIcons.externalLink, size: 16, semanticLabel: label),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final c = controller;
    final locale = Localizations.localeOf(context);
    final checkUpdateLabel = AppStrings.checkAppUpdate(locale);
    return AppPageBody(
      controller: c,
      page: AppPage.about,
      children: [
        if (!c.isAndroid)
          Padding(
            padding: const EdgeInsets.symmetric(vertical: 15),
            child: Column(
              children: [
                Image.asset(logoAsset, width: 60, height: 60),
                const SizedBox(height: 12),
                Text(
                  'KeyTao 输入法',
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
        Section(
          title: AppStrings.about,
          icon: FLucideIcons.info,
          children: [
            if (c.isAndroid)
              Center(
                child: Image.asset(
                  logoAsset,
                  width: 64,
                  height: 64,
                  semanticLabel: 'KeyTao',
                ),
              ),
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
              AppStrings.librimeLuaVersion,
              c.versions?.librimeLuaVersion ?? '—',
            ),
            ValueRow(
              AppStrings.openccVersion,
              c.versions?.openccVersion ?? '—',
            ),
            ValueRow(AppStrings.platform, c.aboutValues['平台']!),
          ],
        ),
      ],
    );
  }
}
