import 'package:flutter/material.dart';
import 'package:forui/forui.dart';

import '../app/controller.dart';
import '../app/options.dart';
import 'progress.dart';
import 'scheme_page.dart';
import 'widgets.dart';

class ExtensionPage extends StatelessWidget {
  const ExtensionPage({super.key, required this.controller});
  final AppController controller;
  @override
  Widget build(BuildContext context) {
    final c = controller;
    return AppPageBody(
      controller: c,
      page: AppPage.extension,
      children: [
        Section(
          title: AppStrings.customDirectory,
          icon: FLucideIcons.folderOpen,
          children: [
            const Text(AppStrings.customDirectoryDescription),
            VersionPicker(controller: c),
            ActionRow(
              children: [
                ActionButton(
                  c.selectedDirectory == null
                      ? AppStrings.selectDirectory
                      : AppStrings.reselectDirectory,
                  icon: FLucideIcons.folderOpen,
                  onPress: c.busy ? null : c.pickCustomDirectory,
                ),
                if (c.selectedDirectory != null) ...[
                  ActionButton(
                    c.isOpeningCustomDirectory
                        ? AppStrings.opening
                        : AppStrings.openDirectory,
                    icon: FLucideIcons.externalLink,
                    onPress: c.isOpeningCustomDirectory
                        ? null
                        : c.openCustomDirectory,
                  ),
                  if (c.downloadUrl?.isNotEmpty == true)
                    ActionButton(
                      c.isInstallingCustom
                          ? AppStrings.installing
                          : AppStrings.installNow,
                      primary: true,
                      icon: FLucideIcons.download,
                      onPress: c.canInstallCustom
                          ? c.installCustomDirectory
                          : null,
                    ),
                ],
              ],
            ),
            if (c.selectedDirectory case final directory?) ...[
              StatusMessage(
                directory,
                icon: FLucideIcons.folder,
                color: c.isAndroid ? successColor(context) : null,
              ),
              if (c.customSchemas case final schemas?) ...[
                if (schemas.schemas.isEmpty)
                  const StatusMessage(AppStrings.noDefaultCustom),
                if (schemas.schemas.isNotEmpty)
                  ActionRow(
                    children: [
                      const Text(AppStrings.detectedSchemas),
                      for (final schema in schemas.schemas)
                        FBadge(child: Text(schema)),
                    ],
                  ),
              ],
              CustomFileList(controller: c),
            ],
            if (c.isInstallingCustom)
              InstallProgressView(active: true, progress: c.customProgress),
            if (c.customError case final error?) ErrorMessage(error),
            if (c.customResult case final result?) ...[
              StatusMessage(
                AppStrings.customInstallDone,
                icon: FLucideIcons.circleCheck,
                color: successColor(context),
              ),
              if (c.customVerifyFailureCount > 0)
                ErrorMessage(
                  AppStrings.verifyFailures(c.customVerifyFailureCount),
                ),
              FAccordion(
                key: const PageStorageKey('customInstallLogs'),
                children: [
                  FAccordionItem(
                    title: Text(AppStrings.installLogCount(result.logs.length)),
                    child: LogLines(
                      lines: result.logs,
                      storageKey: 'customLogs',
                      height: 192,
                      colorCoded: true,
                    ),
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
            ActionButton(
              AppStrings.refresh,
              icon: FLucideIcons.refreshCw,
              onPress: c.busy || c.filesLoading
                  ? null
                  : c.refreshCustomDirectory,
            ),
          ],
        ),
        const SizedBox(height: 10),
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
                return FTile(
                  prefix: Icon(
                    file.isDir ? FLucideIcons.folder : FLucideIcons.fileText,
                  ),
                  title: SelectableText(file.name),
                );
              },
            ),
          ),
      ],
    );
  }
}
