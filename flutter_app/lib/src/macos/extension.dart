import 'package:flutter/cupertino.dart';
import 'package:flutter/material.dart' show SelectableText;
import 'package:macos_ui/macos_ui.dart';

import '../app/controller.dart';
import '../app/options.dart';
import 'scheme.dart';
import 'widgets.dart';

class MacosExtensionPage extends StatelessWidget {
  const MacosExtensionPage({
    super.key,
    required this.controller,
    this.scrollController,
  });
  final AppController controller;
  final ScrollController? scrollController;

  @override
  Widget build(BuildContext context) {
    final c = controller;
    return MacosForm(
      scrollController: scrollController,
      children: [
        MacosUpdateBanner(controller: c),
        MacosGroup(
          title: AppStrings.customDirectory,
          icon: CupertinoIcons.folder_open,
          children: [
            const MacosMessage(AppStrings.customDirectoryDescription),
            MacosVersionPicker(controller: c),
            MacosActionRow(
              children: [
                PushButton(
                  controlSize: ControlSize.regular,
                  secondary: true,
                  onPressed: c.busy ? null : c.pickCustomDirectory,
                  child: Text(
                    c.selectedDirectory == null
                        ? AppStrings.selectDirectory
                        : AppStrings.reselectDirectory,
                  ),
                ),
                if (c.selectedDirectory != null) ...[
                  PushButton(
                    controlSize: ControlSize.regular,
                    secondary: true,
                    onPressed: c.busy ? null : c.openCustomDirectory,
                    child: const Text(AppStrings.openDirectory),
                  ),
                  PushButton(
                    controlSize: ControlSize.regular,
                    onPressed: c.canInstallCustom
                        ? c.installCustomDirectory
                        : null,
                    child: Text(
                      c.isInstallingCustom
                          ? AppStrings.installing
                          : AppStrings.installNow,
                    ),
                  ),
                ],
              ],
            ),
            if (c.selectedDirectory case final directory?) ...[
              MacosMessage(directory, icon: CupertinoIcons.folder),
              if (c.customSchemas case final schemas?) ...[
                if (!schemas.hasDefaultCustom)
                  const MacosMessage(
                    AppStrings.noDefaultCustom,
                    icon: CupertinoIcons.info_circle,
                  ),
                if (schemas.schemas.isNotEmpty)
                  Padding(
                    padding: const EdgeInsets.all(12),
                    child: Wrap(
                      spacing: 6,
                      runSpacing: 6,
                      crossAxisAlignment: WrapCrossAlignment.center,
                      children: [
                        const Text(AppStrings.detectedSchemas),
                        for (final schema in schemas.schemas)
                          MacosBadge(schema),
                      ],
                    ),
                  ),
              ],
              MacosDirectoryFiles(controller: c),
            ],
            if (c.isInstallingCustom)
              MacosInstallProgress(active: true, progress: c.customProgress),
            if (c.customError case final error?)
              MacosMessage(
                error,
                color: MacosColors.systemRedColor,
                icon: CupertinoIcons.exclamationmark_triangle,
              ),
            if (c.customResult case final result?) ...[
              const MacosMessage(
                AppStrings.customInstallDone,
                color: MacosColors.systemGreenColor,
                icon: CupertinoIcons.checkmark_circle,
              ),
              if (c.customVerifyFailureCount > 0)
                MacosMessage(
                  AppStrings.verifyFailures(c.customVerifyFailureCount),
                  color: MacosColors.systemRedColor,
                ),
              MacosInstallLogs(lines: result.logs),
            ],
          ],
        ),
      ],
    );
  }
}

class MacosDirectoryFiles extends StatelessWidget {
  const MacosDirectoryFiles({super.key, required this.controller});
  final AppController controller;

  @override
  Widget build(BuildContext context) {
    final c = controller;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        MacosSettingRow(
          AppStrings.itemCount(c.customFiles.length),
          child: PushButton(
            controlSize: ControlSize.regular,
            secondary: true,
            onPressed: c.busy || c.filesLoading
                ? null
                : c.refreshCustomDirectory,
            child: const Text(AppStrings.refresh),
          ),
        ),
        if (c.filesLoading)
          const MacosMessage(AppStrings.reading)
        else if (c.customFiles.isEmpty)
          const MacosMessage(AppStrings.directoryEmpty)
        else
          SizedBox(
            height: 192,
            child: ListView.builder(
              primary: false,
              itemCount: c.customFiles.length,
              itemBuilder: (context, index) {
                final file = c.customFiles[index];
                return Padding(
                  padding: const EdgeInsets.symmetric(
                    horizontal: 12,
                    vertical: 5,
                  ),
                  child: Row(
                    children: [
                      MacosIcon(
                        file.isDir ? CupertinoIcons.folder : CupertinoIcons.doc,
                        size: 16,
                      ),
                      const SizedBox(width: 8),
                      Expanded(child: SelectableText(file.name)),
                    ],
                  ),
                );
              },
            ),
          ),
      ],
    );
  }
}

class MacosInstallLogs extends StatefulWidget {
  const MacosInstallLogs({super.key, required this.lines});
  final List<String> lines;

  @override
  State<MacosInstallLogs> createState() => _MacosInstallLogsState();
}

class _MacosInstallLogsState extends State<MacosInstallLogs> {
  bool _expanded = false;

  @override
  Widget build(BuildContext context) => Column(
    children: [
      MacosSettingRow(
        AppStrings.installLogCount(widget.lines.length),
        child: MacosDisclosureButton(
          semanticLabel: AppStrings.installLogCount(widget.lines.length),
          isPressed: _expanded,
          onPressed: () => setState(() => _expanded = !_expanded),
        ),
      ),
      if (_expanded)
        SizedBox(height: 192, child: MacosLogView(lines: widget.lines)),
    ],
  );
}
