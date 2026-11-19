import 'package:flutter/widgets.dart';
import 'package:forui/forui.dart';

import '../app/controller.dart';
import '../app/options.dart';
import 'theme.dart';
import 'widgets.dart';

class AboutPage extends StatelessWidget {
  const AboutPage({super.key, required this.controller});
  final AppController controller;
  @override
  Widget build(BuildContext context) {
    final c = controller;
    final locale = Localizations.localeOf(context);
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
                  AppStrings.title,
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
                  AppStrings.tagline,
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
            ),
            FTappable(
              onPress: c.checkingAppUpdate
                  ? null
                  : () async {
                      final update = await c.checkAppUpdateNow();
                      if (!context.mounted ||
                          update == null ||
                          !update.hasUpdate) {
                        return;
                      }
                      await showFDialog<void>(
                        context: context,
                        builder: (context, _, animation) => AppDialog(
                          title: AppStrings.checkAppUpdate(locale),
                          animation: animation,
                          actions: [
                            ActionButton(
                              AppStrings.downloadAppUpdate(locale),
                              primary: true,
                              onPress: () {
                                Navigator.pop(context);
                                c.openAppUpdate();
                              },
                            ),
                          ],
                          child: Text('v${update.latestVersion}'),
                        ),
                      );
                    },
              child: ConstrainedBox(
                constraints: const BoxConstraints(minHeight: 44),
                child: SettingRow(
                  AppStrings.checkAppUpdate(locale),
                  keepInline: true,
                  description: c.checkingAppUpdate
                      ? null
                      : c.appUpdateError != null
                      ? AppStrings.appUpdateFailure(locale, c.appUpdateError!)
                      : c.appUpdate?.hasUpdate == false
                      ? AppStrings.appUpToDate(locale)
                      : null,
                  child: Row(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      if (c.appUpdate?.hasUpdate == true) ...[
                        FBadge(child: Text('v${c.appUpdate!.latestVersion}')),
                        const SizedBox(width: 8),
                      ],
                      if (c.checkingAppUpdate)
                        const SizedBox.square(
                          dimension: 18,
                          child: FCircularProgress(),
                        )
                      else
                        const Icon(FLucideIcons.chevronRight, size: 16),
                    ],
                  ),
                ),
              ),
            ),
            ValueRow('Flutter', c.aboutValues['Flutter']!),
            ValueRow(
              AppStrings.librimeVersion,
              c.versions?.librimeVersion ?? '—',
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
            ValueRow(
              AppStrings.keytaoDirectory,
              c.versions?.dataDir ?? c.defaultDir,
            ),
            if (c.isAndroid)
              ActionRow(
                children: [
                  ActionButton(
                    'GitHub',
                    icon: FLucideIcons.externalLink,
                    onPress: c.busy ? null : () => c.open(repositoryUrl),
                  ),
                ],
              )
            else
              SettingRow(
                'GitHub',
                child: ActionButton(
                  Uri.parse(repositoryUrl).path.substring(1),
                  onPress: c.busy ? null : () => c.open(repositoryUrl),
                ),
              ),
          ],
        ),
      ],
    );
  }
}
