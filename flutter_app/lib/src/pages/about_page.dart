import 'package:flutter/material.dart';

import '../app/bootstrap.dart';
import '../app/controller.dart';
import '../app/widgets.dart';

class AboutPage extends StatelessWidget {
  const AboutPage({super.key, required this.controller});
  final AppController controller;
  @override
  Widget build(BuildContext context) {
    final c = controller;
    return SingleChildScrollView(
      padding: const EdgeInsets.all(16),
      child: ContentWidth(
        child: SectionCard(
          title: '关于 KeyTao',
          children: [
            ValueRow('KeyTao', c.versions?.appVersion ?? c.info.appVersion),
            const ValueRow('Flutter', flutterVersion),
            ValueRow('librime', c.versions?.librimeVersion ?? '—'),
            ValueRow('OpenCC', c.versions?.openccVersion ?? '—'),
            const Divider(),
            ValueRow('平台', c.isAndroid ? 'Android' : 'macOS'),
            ValueRow('KeyTao 目录', c.versions?.dataDir ?? c.info.userRoot),
            const SizedBox(height: 24),
            OutlinedButton.icon(
              onPressed: c.busy ? null : () => c.open(repositoryUrl),
              icon: const Icon(Icons.open_in_new_rounded),
              label: const Text('GitHub'),
            ),
          ],
        ),
      ),
    );
  }
}
