import 'package:flutter/material.dart';
import 'package:forui/forui.dart';

import '../app/controller.dart';
import '../app/options.dart' show AppStrings, logLevels;
import '../rust/api/types.dart';
import 'widgets.dart';

List<Widget> debugLogCells(AppController c) {
  final settings = c.logSettings;
  final enabled = c.canEditLogs && settings != null;
  return [
    HairlineGridCell(
      child: Section(
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
                ? (value) => c.saveLogSettings(value, settings.level)
                : null,
          ),
          ChoiceSetting<RuntimeLogLevelDto>(
            label: AppStrings.logLevel,
            value: settings?.level ?? RuntimeLogLevelDto.info,
            options: logLevels,
            onChanged: enabled
                ? (value) => c.saveLogSettings(settings.enabled, value)
                : null,
          ),
          ActionRow(
            children: [
              ActionButton(
                AppStrings.refresh,
                primary: true,
                icon: FLucideIcons.refreshCw,
                onPress: c.canEditLogs ? c.refreshLogs : null,
              ),
              if (c.isAndroid)
                ActionButton(
                  AppStrings.share,
                  icon: FLucideIcons.share2,
                  onPress: c.canEditLogs ? c.shareLogs : null,
                ),
              ActionButton(
                AppStrings.clear,
                icon: FLucideIcons.trash2,
                onPress: c.canEditLogs ? c.clearLogs : null,
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
    ),
    DebugLogPanel(
      title: AppStrings.structuredRuntimeLogs,
      log: c.runtimeLog,
      storageKey: 'runtimeLogs',
      height: 180,
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
  ];
}

class DebugLogPanel extends StatelessWidget {
  const DebugLogPanel({
    super.key,
    required this.title,
    required this.log,
    required this.storageKey,
    this.height = 144,
  });
  final String title;
  final DebugLogFileDto? log;
  final String storageKey;
  final double height;
  @override
  Widget build(BuildContext context) => HairlineGridCell(
    child: Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      spacing: 12,
      children: [
        EyebrowLabel(title),
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
    ),
  );
}
