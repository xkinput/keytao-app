import 'package:flutter/material.dart';

import '../app/controller.dart';
import '../app/options.dart';
import '../app/widgets.dart';

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
        SectionCard(
          title: AppStrings.about,
          icon: Icons.info_outline,
          children: [
            Center(
              child: Image.asset(
                logoAsset,
                width: 64,
                height: 64,
                semanticLabel: 'KeyTao',
              ),
            ),
            const SizedBox(height: 20),
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
            const Divider(),
            const ValueRow(AppStrings.platform, 'Android'),
            ValueRow(
              AppStrings.keytaoDirectory,
              c.versions?.dataDir ?? c.defaultDir,
            ),
            const SizedBox(height: 20),
            OutlinedButton.icon(
              onPressed: c.busy ? null : () => c.open(repositoryUrl),
              icon: const Icon(Icons.open_in_new),
              label: const Text('GitHub'),
            ),
          ],
        ),
      ],
    );
  }
}
