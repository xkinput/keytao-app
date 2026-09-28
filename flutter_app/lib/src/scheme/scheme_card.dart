import 'package:flutter/material.dart';

import '../app/controller.dart';
import '../app/options.dart';
import '../app/widgets.dart';
import 'operation_progress.dart';
import 'version_picker.dart';

class SchemeCard extends StatelessWidget {
  const SchemeCard({super.key, required this.controller});
  final AppController controller;

  @override
  Widget build(BuildContext context) {
    final c = controller;
    final local = c.local;
    return SectionCard(
      title: AppStrings.schemeTitle,
      icon: Icons.download_outlined,
      children: [
        VersionPicker(controller: c),
        const SizedBox(height: 12),
        SchemeSelection(controller: c),
        ValueRow(AppStrings.directory, c.defaultDir),
        if (c.localStatusLine != null)
          StatusMessage(
            [
              c.localStatusLine!,
              if (c.localSchemaIds != null) c.localSchemaIds!,
            ].join(' '),
            icon: local!.deployed
                ? Icons.check_circle_outline
                : local.installed
                ? Icons.warning_amber
                : Icons.info_outline,
            color: local.deployed
                ? successColor(context)
                : local.installed
                ? warningColor(context)
                : null,
          ),
        ErrorMessage(c.installError),
        InstallProgressView(
          active: c.isInstalling,
          progress: c.installProgress,
        ),
        DeploySteps(controller: c),
        const SizedBox(height: 12),
        Wrap(
          spacing: 8,
          runSpacing: 8,
          children: [
            FilledButton.icon(
              onPressed: c.canInstall ? c.install : null,
              icon: c.checkingStorage || c.isInstalling || c.isDeploying
                  ? const SizedBox.square(
                      dimension: 18,
                      child: CircularProgressIndicator(strokeWidth: 2),
                    )
                  : const Icon(Icons.download_outlined),
              label: Text(c.installButtonLabel),
            ),
            Tooltip(
              message: c.deployTooltip,
              child: FilledButton.tonalIcon(
                onPressed: c.canDeploy ? c.deploy : null,
                icon: const Icon(Icons.play_arrow_outlined),
                label: const Text(AppStrings.deploy),
              ),
            ),
            OutlinedButton.icon(
              onPressed: c.busy ? null : c.refreshState,
              icon: const Icon(Icons.refresh),
              label: const Text(AppStrings.checkLocal),
            ),
            OutlinedButton.icon(
              onPressed: c.isAndroid || c.busy ? null : c.openDefaultDirectory,
              icon: const Icon(Icons.folder_open),
              label: const Text(AppStrings.openDirectory),
            ),
            if (c.operationLogCount > 0) OperationLogButton(controller: c),
          ],
        ),
        const SizedBox(height: 20),
        const TextField(
          minLines: 3,
          maxLines: 5,
          decoration: InputDecoration(hintText: AppStrings.testInput),
        ),
      ],
    );
  }
}

class AddonCard extends StatelessWidget {
  const AddonCard({super.key, required this.controller});
  final AppController controller;

  @override
  Widget build(BuildContext context) {
    final c = controller;
    return SectionCard(
      title: AppStrings.addonTitle,
      icon: Icons.menu_book_outlined,
      children: [
        Text(
          AppStrings.easyEnglish,
          style: Theme.of(context).textTheme.titleMedium,
        ),
        const SizedBox(height: 4),
        Text(c.addonStatusText),
        const SizedBox(height: 12),
        Wrap(
          spacing: 8,
          runSpacing: 8,
          children: [
            if (c.showAddonInstall)
              Tooltip(
                message: c.addonTooltip,
                child: FilledButton.tonalIcon(
                  onPressed: c.canInstallAddon ? c.installAddon : null,
                  icon: const Icon(Icons.download_outlined),
                  label: Text(c.addonButtonLabel),
                ),
              ),
            if (c.addon?.installed == true)
              OutlinedButton(
                onPressed: c.canUninstallAddon ? c.uninstallAddon : null,
                child: const Text(AppStrings.uninstall),
              ),
          ],
        ),
        InstallProgressView(
          active: c.isManagingAddon,
          progress: c.addonProgress,
        ),
        ErrorMessage(c.addonError),
        const Divider(),
        Text(
          AppStrings.wanxiang,
          style: Theme.of(context).textTheme.titleMedium,
        ),
        const SizedBox(height: 4),
        Text(c.wanxiangStatusText),
        const SizedBox(height: 12),
        Wrap(
          spacing: 8,
          runSpacing: 8,
          children: [
            Tooltip(
              message: c.wanxiangTooltip,
              child: FilledButton.tonalIcon(
                onPressed: c.canInstallWanxiang
                    ? () => c.manageWanxiang(true)
                    : null,
                icon: const Icon(Icons.download_outlined),
                label: Text(c.wanxiangButtonLabel),
              ),
            ),
            if (c.wanxiang?.installed == true)
              OutlinedButton(
                onPressed: c.canUninstallWanxiang
                    ? () => c.manageWanxiang(false)
                    : null,
                child: const Text(AppStrings.uninstall),
              ),
          ],
        ),
        const SizedBox(height: 12),
        const Text(AppStrings.wanxiangDescription),
        InstallProgressView(
          active: c.isManagingWanxiang,
          progress: c.wanxiangProgress,
        ),
        ErrorMessage(c.wanxiangError),
      ],
    );
  }
}
