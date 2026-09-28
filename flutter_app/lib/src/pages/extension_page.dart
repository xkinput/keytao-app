import 'package:flutter/material.dart';

import '../app/controller.dart';
import '../app/options.dart';
import '../app/widgets.dart';
import '../scheme/operation_progress.dart';
import '../scheme/version_picker.dart';

class ExtensionPage extends StatelessWidget {
  const ExtensionPage({super.key, required this.controller});
  final AppController controller;

  @override
  Widget build(BuildContext context) {
    final c = controller;
    final schemas = c.customSchemas;
    return AppPageBody(
      controller: c,
      page: AppPage.extension,
      children: [
        SectionCard(
          title: AppStrings.customDirectory,
          icon: Icons.folder_open,
          children: [
            const Text(AppStrings.customDirectoryDescription),
            const SizedBox(height: 16),
            VersionPicker(controller: c),
            const SizedBox(height: 12),
            Wrap(
              spacing: 8,
              runSpacing: 8,
              children: [
                OutlinedButton.icon(
                  onPressed: c.busy ? null : c.pickCustomDirectory,
                  icon: const Icon(Icons.folder_open),
                  label: Text(
                    c.selectedDirectory == null
                        ? AppStrings.selectDirectory
                        : AppStrings.reselectDirectory,
                  ),
                ),
                if (c.selectedDirectory != null) ...[
                  OutlinedButton.icon(
                    onPressed: c.busy ? null : c.openCustomDirectory,
                    icon: const Icon(Icons.open_in_new),
                    label: const Text(AppStrings.openDirectory),
                  ),
                  FilledButton.icon(
                    onPressed: c.canInstallCustom
                        ? c.installCustomDirectory
                        : null,
                    icon: const Icon(Icons.download_outlined),
                    label: Text(
                      c.isInstallingCustom
                          ? AppStrings.installing
                          : AppStrings.installNow,
                    ),
                  ),
                ],
              ],
            ),
            if (c.selectedDirectory != null) ...[
              const SizedBox(height: 12),
              StatusMessage(
                c.selectedDirectory!,
                icon: Icons.check_circle_outline,
                color: successColor(context),
              ),
              if (schemas != null) ...[
                if (!schemas.hasDefaultCustom)
                  const StatusMessage(AppStrings.noDefaultCustom),
                if (schemas.schemas.isNotEmpty) ...[
                  const SizedBox(height: 8),
                  const Text(AppStrings.detectedSchemas),
                  Wrap(
                    spacing: 8,
                    runSpacing: 4,
                    children: [
                      for (final schema in schemas.schemas)
                        Chip(label: Text(schema)),
                    ],
                  ),
                ],
              ],
              CustomFileList(controller: c),
            ],
            InstallProgressView(
              active: c.isInstallingCustom,
              progress: c.customProgress,
            ),
            ErrorMessage(c.customError),
            if (c.customResult case final result?) ...[
              StatusMessage(
                AppStrings.customInstallDone,
                icon: Icons.check_circle_outline,
                color: successColor(context),
              ),
              if (c.customVerifyFailureCount > 0)
                ErrorMessage(
                  AppStrings.verifyFailures(c.customVerifyFailureCount),
                ),
              ExpansionTile(
                key: const PageStorageKey('customInstallLogs'),
                tilePadding: EdgeInsets.zero,
                title: Text(AppStrings.installLogCount(result.logs.length)),
                children: [
                  LogLines(
                    lines: result.logs,
                    storageKey: 'customLogs',
                    height: 192,
                    colorCoded: true,
                  ),
                ],
              ),
            ],
          ],
        ),
      ],
    );
  }
}

class CustomFileList extends StatelessWidget {
  const CustomFileList({super.key, required this.controller});
  final AppController controller;

  @override
  Widget build(BuildContext context) {
    final c = controller;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Row(
          children: [
            Expanded(child: Text(AppStrings.itemCount(c.customFiles.length))),
            TextButton.icon(
              onPressed: c.busy || c.filesLoading
                  ? null
                  : c.refreshCustomDirectory,
              icon: const Icon(Icons.refresh),
              label: const Text(AppStrings.refresh),
            ),
          ],
        ),
        if (c.filesLoading)
          const StatusMessage(AppStrings.reading)
        else if (c.customFiles.isEmpty)
          const StatusMessage(AppStrings.directoryEmpty)
        else
          ConstrainedBox(
            constraints: const BoxConstraints(maxHeight: 240),
            child: ListView.builder(
              key: const PageStorageKey('customFiles'),
              primary: false,
              shrinkWrap: true,
              itemCount: c.customFiles.length,
              itemBuilder: (context, index) {
                final file = c.customFiles[index];
                return ListTile(
                  dense: true,
                  contentPadding: EdgeInsets.zero,
                  leading: Icon(
                    file.isDir
                        ? Icons.folder_outlined
                        : Icons.description_outlined,
                  ),
                  title: Text(file.name),
                );
              },
            ),
          ),
      ],
    );
  }
}
