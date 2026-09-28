import 'package:flutter/material.dart';
import 'package:forui/forui.dart';

import '../app/controller.dart';
import '../app/options.dart';
import '../rust/api/types.dart';
import 'widgets.dart';

class DebugPage extends StatelessWidget {
  const DebugPage({super.key, required this.controller});
  final AppController controller;
  @override
  Widget build(BuildContext context) {
    final c = controller;
    final settings = c.logSettings;
    final enabled = c.canEditLogs && (!c.isMobile || !c.logsLoading);
    return AppPageBody(
      controller: c,
      page: AppPage.debug,
      children: [
        Section(
          title: AppStrings.runtimeLogs,
          icon: FLucideIcons.scrollText,
          children: [
            SwitchSetting(
              settings == null
                  ? AppStrings.readingLogSettings
                  : settings.enabled
                  ? AppStrings.logsEnabled
                  : AppStrings.logsDisabled,
              value: settings?.enabled ?? false,
              onChanged: enabled
                  ? (value) => c.saveLogSettings(value, settings!.level)
                  : null,
            ),
            ChoiceSetting<RuntimeLogLevelDto>(
              label: AppStrings.logLevel,
              value: settings?.level ?? RuntimeLogLevelDto.info,
              options: logLevels,
              onChanged: enabled
                  ? (value) => c.saveLogSettings(settings!.enabled, value)
                  : null,
            ),
            ActionRow(
              children: [
                ActionButton(
                  AppStrings.refresh,
                  primary: true,
                  icon: FLucideIcons.refreshCw,
                  onPress: c.busy || c.logsLoading ? null : c.refreshLogs,
                ),
                if (c.isAndroid)
                  ActionButton(
                    AppStrings.share,
                    icon: FLucideIcons.share2,
                    onPress: c.busy || c.logsLoading ? null : c.shareLogs,
                  ),
                ActionButton(
                  AppStrings.clear,
                  icon: FLucideIcons.trash2,
                  onPress: c.busy || (c.isMobile && c.logsLoading)
                      ? null
                      : c.clearLogs,
                ),
                if (!c.isMobile) ...[
                  ActionButton(
                    AppStrings.openLogDirectory,
                    icon: FLucideIcons.folderOpen,
                    onPress: c.canEditLogs ? c.openLogDirectory : null,
                  ),
                  ActionButton(
                    AppStrings.copyPath,
                    icon: FLucideIcons.copy,
                    onPress: c.canEditLogs ? c.copyLogDirectory : null,
                  ),
                ],
              ],
            ),
            if (c.logsLoading) const FProgress(),
            if (c.debugError case final error?) ErrorMessage(error),
            if (c.debugNotice case final notice?) StatusMessage(notice),
            if (c.isAndroid && c.sharedLogResultText != null)
              StatusMessage(c.sharedLogResultText!),
            if (c.logStatsText case final stats?) StatusMessage(stats),
            for (final file in c.logFileLines) SelectableText(file),
            if (!c.isMobile && c.logDirectory != null)
              StatusMessage(c.logDirectory!),
          ],
        ),
        DebugLogPanel(
          title: AppStrings.structuredRuntimeLogs,
          log: c.runtimeLog,
          storageKey: 'runtimeLogs',
          height: 240,
        ),
        DebugLogPanel(
          title: AppStrings.imeLogs,
          log: c.systemLogs?.ime,
          storageKey: 'imeLogs',
        ),
        DebugLogPanel(
          title: AppStrings.appLogs,
          log: c.systemLogs?.app,
          storageKey: 'appLogs',
        ),
      ],
    );
  }
}

class DebugLogPanel extends StatelessWidget {
  const DebugLogPanel({
    super.key,
    required this.title,
    required this.log,
    required this.storageKey,
    this.height = 192,
  });
  final String title;
  final DebugLogFileDto? log;
  final String storageKey;
  final double height;
  @override
  Widget build(BuildContext context) => Section(
    title: title,
    children: [
      if (log?.truncated == true)
        Text(
          AppStrings.recentLines(log!.lines.length),
          style: context.theme.typography.body.xs,
        ),
      LogLines(
        lines: log?.lines ?? const [],
        storageKey: storageKey,
        height: height,
      ),
    ],
  );
}
