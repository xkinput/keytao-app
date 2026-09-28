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
    return SingleChildScrollView(
      padding: const EdgeInsets.all(16),
      child: ContentWidth(
        child: SectionCard(
          title: '关于 KeyTao',
          children: [
            for (final entry in c.aboutValues.entries) ...[
              if (entry.key == '平台') const Divider(),
              ValueRow(entry.key, entry.value),
            ],
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
