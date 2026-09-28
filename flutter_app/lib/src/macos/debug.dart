import 'package:flutter/cupertino.dart';
import 'package:macos_ui/macos_ui.dart';

import '../app/controller.dart';
import '../app/options.dart';
import '../rust/api/types.dart';
import 'widgets.dart';

class MacosDebugPage extends StatelessWidget {
  const MacosDebugPage({
    super.key,
    required this.controller,
    this.scrollController,
  });
  final AppController controller;
  final ScrollController? scrollController;

  @override
  Widget build(BuildContext context) {
    final c = controller;
    final settings = c.logSettings;
    return MacosForm(
      scrollController: scrollController,
      children: [
        MacosUpdateBanner(controller: c),
        MacosGroup(
          title: AppStrings.runtimeLogs,
          icon: CupertinoIcons.doc_text,
          children: [
            MacosSettingRow(
              settings == null
                  ? AppStrings.readingLogSettings
                  : settings.enabled
                  ? AppStrings.logsEnabled
                  : AppStrings.logsDisabled,
              child: MacosSwitch(
                size: ControlSize.regular,
                value: settings?.enabled ?? false,
                onChanged: c.canEditLogs
                    ? (value) => c.saveLogSettings(value, settings!.level)
                    : null,
              ),
            ),
            MacosChoice<RuntimeLogLevelDto>(
              label: AppStrings.logLevel,
              value: settings?.level ?? RuntimeLogLevelDto.info,
              options: logLevels,
              onChanged: c.canEditLogs
                  ? (value) => c.saveLogSettings(settings!.enabled, value)
                  : null,
            ),
            MacosActionRow(
              children: [
                if (c.logsLoading) const ProgressCircle(radius: 8),
                PushButton(
                  controlSize: ControlSize.regular,
                  secondary: true,
                  onPressed: c.busy || c.logsLoading ? null : c.refreshLogs,
                  child: const Text(AppStrings.refresh),
                ),
                PushButton(
                  controlSize: ControlSize.regular,
                  secondary: true,
                  onPressed: c.busy ? null : c.clearLogs,
                  child: const Text(AppStrings.clear),
                ),
                PushButton(
                  controlSize: ControlSize.regular,
                  secondary: true,
                  onPressed: c.canEditLogs ? c.openLogDirectory : null,
                  child: const Text(AppStrings.openLogDirectory),
                ),
                PushButton(
                  controlSize: ControlSize.regular,
                  secondary: true,
                  onPressed: c.canEditLogs ? c.copyLogDirectory : null,
                  child: const Text(AppStrings.copyPath),
                ),
              ],
            ),
            if (c.debugError case final error?)
              MacosMessage(
                error,
                color: MacosColors.systemRedColor,
                icon: CupertinoIcons.exclamationmark_triangle,
              ),
            if (c.debugNotice case final notice?) MacosMessage(notice),
            if (c.logStatsText case final stats?) MacosMessage(stats),
            for (final file in c.logFileLines) MacosMessage(file),
            if (c.logDirectory case final path?) MacosMessage(path),
          ],
        ),
        MacosDebugLogPanel(
          title: AppStrings.structuredRuntimeLogs,
          log: c.runtimeLog,
          height: 240,
        ),
        MacosDebugLogPanel(title: AppStrings.imeLogs, log: c.systemLogs?.ime),
        MacosDebugLogPanel(title: AppStrings.appLogs, log: c.systemLogs?.app),
      ],
    );
  }
}

class MacosDebugLogPanel extends StatelessWidget {
  const MacosDebugLogPanel({
    super.key,
    required this.title,
    this.log,
    this.height = 192,
  });
  final String title;
  final DebugLogFileDto? log;
  final double height;

  @override
  Widget build(BuildContext context) => MacosGroup(
    title: title,
    children: [
      if (log?.truncated == true)
        MacosMessage(AppStrings.recentLines(log!.lines.length)),
      SizedBox(
        height: height,
        child: MacosLogView(
          key: PageStorageKey(title),
          lines: log?.lines ?? const [],
          colorize: false,
        ),
      ),
    ],
  );
}
