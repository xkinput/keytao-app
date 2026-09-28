import 'package:flutter/widgets.dart';
import 'package:forui/forui.dart';

import '../app/controller.dart';
import '../app/options.dart';
import 'widgets.dart';

class AboutPage extends StatelessWidget {
  const AboutPage({super.key, required this.controller});
  final AppController controller;
  @override
  Widget build(BuildContext context) {
    final c = controller;
    return AppPageBody(
      controller: c,
      page: AppPage.about,
      children: [
        if (!c.isAndroid)
          Padding(
            padding: const EdgeInsets.symmetric(vertical: 20),
            child: Column(
              children: [
                Image.asset(logoAsset, width: 80, height: 80),
                const SizedBox(height: 12),
                Text(
                  AppStrings.title,
                  style: context.theme.typography.display.xl2.copyWith(
                    fontWeight: FontWeight.w600,
                  ),
                ),
                const SizedBox(height: 6),
                Text('v${c.info.appVersion}'),
                const SizedBox(height: 12),
                const Text(AppStrings.tagline, textAlign: TextAlign.center),
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
            ValueRow('Flutter', c.aboutValues['Flutter']!),
            ValueRow(
              AppStrings.librimeVersion,
              c.versions?.librimeVersion ?? '—',
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
