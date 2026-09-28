import 'package:flutter/material.dart';

import '../app/controller.dart';
import '../app/options.dart';
import '../app/widgets.dart';
import '../rust/api/types.dart';

class DebugPage extends StatelessWidget {
  const DebugPage({super.key, required this.controller});
  final AppController controller;

  @override
  Widget build(BuildContext context) {
    final c = controller;
    final settings = c.logSettings;
    final enabled = c.canEditLogs && !c.logsLoading;
    return AppPageBody(
      controller: c,
      page: AppPage.debug,
      children: [
        SectionCard(
          title: AppStrings.runtimeLogs,
          icon: Icons.receipt_long_outlined,
          children: [
            SwitchListTile(
              contentPadding: EdgeInsets.zero,
              title: Text(
                settings == null
                    ? AppStrings.readingLogSettings
                    : settings.enabled
                    ? AppStrings.logsEnabled
                    : AppStrings.logsDisabled,
              ),
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
            Wrap(
              spacing: 8,
              runSpacing: 8,
              children: [
                FilledButton.tonalIcon(
                  onPressed: c.busy || c.logsLoading ? null : c.refreshLogs,
                  icon: const Icon(Icons.refresh),
                  label: const Text(AppStrings.refresh),
                ),
                OutlinedButton.icon(
                  onPressed: c.busy || c.logsLoading ? null : c.shareLogs,
                  icon: const Icon(Icons.share_outlined),
                  label: const Text(AppStrings.share),
                ),
                OutlinedButton.icon(
                  onPressed: c.busy || c.logsLoading ? null : c.clearLogs,
                  icon: const Icon(Icons.delete_outline),
                  label: const Text(AppStrings.clear),
                ),
              ],
            ),
            if (c.logsLoading) ...[
              const SizedBox(height: 12),
              const LinearProgressIndicator(),
            ],
            ErrorMessage(c.debugError),
            if (c.debugNotice != null) StatusMessage(c.debugNotice!),
            if (c.sharedLogResultText != null)
              StatusMessage(c.sharedLogResultText!),
            if (c.logStatsText != null) ...[
              const SizedBox(height: 16),
              Text(c.logStatsText!),
              for (final line in c.logFileLines)
                Padding(
                  padding: const EdgeInsets.only(top: 4),
                  child: SelectableText(line),
                ),
            ],
            const SizedBox(height: 20),
            DebugLogPanel(
              title: AppStrings.structuredRuntimeLogs,
              log: c.runtimeLog,
              storageKey: 'runtimeLogs',
              height: 240,
            ),
            const SizedBox(height: 20),
            DebugLogPanel(
              title: AppStrings.imeLogs,
              log: c.systemLogs?.ime,
              storageKey: 'imeLogs',
            ),
            const SizedBox(height: 20),
            DebugLogPanel(
              title: AppStrings.appLogs,
              log: c.systemLogs?.app,
              storageKey: 'appLogs',
            ),
          ],
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
  Widget build(BuildContext context) => Column(
    crossAxisAlignment: CrossAxisAlignment.stretch,
    children: [
      Text(title, style: Theme.of(context).textTheme.titleSmall),
      if (log?.truncated == true)
        Text(
          AppStrings.recentLines(log!.lines.length),
          style: Theme.of(context).textTheme.bodySmall,
        ),
      const SizedBox(height: 8),
      LogLines(
        lines: log?.lines ?? const [],
        storageKey: storageKey,
        height: height,
      ),
    ],
  );
}
