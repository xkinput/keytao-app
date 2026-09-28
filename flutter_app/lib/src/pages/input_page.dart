import 'package:flutter/material.dart';

import '../app/controller.dart';
import '../app/options.dart';
import '../app/widgets.dart';
import '../settings/settings_card.dart';

class InputPage extends StatelessWidget {
  const InputPage({super.key, required this.controller});
  final AppController controller;

  @override
  Widget build(BuildContext context) => AppPageBody(
    controller: controller,
    page: AppPage.input,
    children: [
      AppBrand(version: controller.info.appVersion),
      AndroidImeCard(controller: controller),
      SettingsCard(controller: controller),
    ],
  );
}

class StorageStatus extends StatelessWidget {
  const StorageStatus({super.key, required this.controller});
  final AppController controller;

  @override
  Widget build(BuildContext context) {
    final c = controller;
    final deployError = c.storage?['deployError'] as String?;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        ValueRow(AppStrings.directory, c.defaultDir),
        if (!c.storageReady || c.migrationError?.isNotEmpty == true)
          Align(
            alignment: Alignment.centerLeft,
            child: OutlinedButton.icon(
              onPressed: c.canOpenStorageSettings
                  ? c.openStoragePermissionSettings
                  : null,
              icon: const Icon(Icons.folder_open),
              label: Text(c.storageActionLabel),
            ),
          ),
        ErrorMessage(c.migrationError),
        ErrorMessage(deployError),
      ],
    );
  }
}

class AndroidImeCard extends StatelessWidget {
  const AndroidImeCard({super.key, required this.controller});
  final AppController controller;

  @override
  Widget build(BuildContext context) {
    final c = controller;
    Widget status(String label, bool done) => ListTile(
      contentPadding: EdgeInsets.zero,
      leading: Icon(
        done ? Icons.check_circle_outline : Icons.radio_button_unchecked,
        color: done ? successColor(context) : null,
      ),
      title: Text(label),
    );
    return SectionCard(
      title: AppStrings.imeSettings,
      icon: Icons.keyboard_outlined,
      children: [
        StorageStatus(controller: c),
        const Divider(),
        status(AppStrings.enableKeytao, c.ime?['enabled'] == true),
        status(AppStrings.selectKeytao, c.ime?['selected'] == true),
        if (c.imeMessage?.isNotEmpty == true) StatusMessage(c.imeMessage!),
        Wrap(
          spacing: 8,
          runSpacing: 8,
          children: [
            OutlinedButton.icon(
              onPressed: c.busy ? null : c.openInputMethodSettings,
              icon: const Icon(Icons.settings_outlined),
              label: const Text(AppStrings.openSystemSettings),
            ),
            FilledButton.tonalIcon(
              onPressed: c.busy || c.ime?['canShowPicker'] != true
                  ? null
                  : c.showInputMethodPicker,
              icon: const Icon(Icons.keyboard_outlined),
              label: const Text(AppStrings.chooseKeytao),
            ),
            IconButton(
              tooltip: AppStrings.recheck,
              onPressed: c.busy ? null : c.refreshState,
              icon: const Icon(Icons.refresh),
            ),
          ],
        ),
      ],
    );
  }
}
