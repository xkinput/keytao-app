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
      fullWidth: true,
      spacing: 0,
      children: [
        if (!c.isMobile)
          HairlineGridCell(child: _RimeSyncSection(controller: c)),
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
        ),
        HairlineGridSection(
          children: [
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
        ),
      ],
    );
  }
}

class _RimeSyncSection extends StatefulWidget {
  const _RimeSyncSection({required this.controller});
  final AppController controller;

  @override
  State<_RimeSyncSection> createState() => _RimeSyncSectionState();
}

class _RimeSyncSectionState extends State<_RimeSyncSection> {
  final _installationId = TextEditingController();
  final _syncDir = TextEditingController();
  RimeSyncSettingsDto? _settings;

  void _readSettings() {
    final settings = widget.controller.rimeSyncSettings;
    if (identical(settings, _settings)) return;
    _settings = settings;
    _installationId.text = settings?.installationId ?? '';
    _syncDir.text = settings?.syncDir ?? '';
  }

  @override
  void initState() {
    super.initState();
    _readSettings();
  }

  @override
  void didUpdateWidget(covariant _RimeSyncSection oldWidget) {
    super.didUpdateWidget(oldWidget);
    _readSettings();
  }

  @override
  void dispose() {
    _installationId.dispose();
    _syncDir.dispose();
    super.dispose();
  }

  Future<bool> _save() => widget.controller.saveRimeSyncSettings(
    _installationId.text,
    _syncDir.text,
  );

  Future<bool> _saveChanges() async {
    final settings = widget.controller.rimeSyncSettings;
    if (_installationId.text == settings?.installationId &&
        _syncDir.text == (settings?.syncDir ?? '')) {
      return true;
    }
    return _save();
  }

  Future<void> _open() async {
    if (await _saveChanges()) await widget.controller.openRimeSyncDirectory();
  }

  Future<void> _sync() async {
    if (await _saveChanges()) await widget.controller.syncRimeUserData();
  }

  @override
  Widget build(BuildContext context) {
    final c = widget.controller;
    final locale = Localizations.localeOf(context);
    final enabled = !c.busy && !c.rimeSyncLoading && c.rimeSyncSettings != null;
    return Section(
      title: AppStrings.rimeSync(locale),
      icon: FLucideIcons.refreshCw,
      children: [
        SettingRow(
          AppStrings.syncInstallationId(locale),
          child: Row(
            spacing: 8,
            children: [
              Expanded(
                child: Semantics(
                  label: AppStrings.syncInstallationId(locale),
                  child: FTextField(
                    key: const ValueKey('rime-sync-installation-id'),
                    control: FTextFieldControl.managed(
                      controller: _installationId,
                    ),
                    enabled: enabled,
                    onSubmit: (_) => _save(),
                  ),
                ),
              ),
              ActionButton(
                AppStrings.syncSave(locale),
                onPress: enabled ? _save : null,
              ),
            ],
          ),
        ),
        SettingRow(
          AppStrings.syncDirectory(locale),
          minimumControlWidth: 400,
          child: Row(
            spacing: 8,
            children: [
              Expanded(
                child: Semantics(
                  label: AppStrings.syncDirectory(locale),
                  child: FTextField(
                    key: const ValueKey('rime-sync-directory'),
                    control: FTextFieldControl.managed(controller: _syncDir),
                    enabled: enabled,
                    onSubmit: (_) => _save(),
                  ),
                ),
              ),
              ActionButton(
                AppStrings.syncChoose(locale),
                onPress: enabled
                    ? () => c.pickRimeSyncDirectory(_installationId.text)
                    : null,
              ),
              ActionButton(
                AppStrings.syncReset(locale),
                onPress: enabled
                    ? () => c.resetRimeSyncDirectory(_installationId.text)
                    : null,
              ),
              ActionButton(
                AppStrings.syncOpen(locale),
                onPress: enabled ? _open : null,
              ),
            ],
          ),
        ),
        ActionRow(
          children: [
            ActionButton(
              c.rimeSyncing
                  ? AppStrings.syncing(locale)
                  : switch (c.rimeSyncSucceeded) {
                      true => AppStrings.syncDone(locale),
                      false => AppStrings.syncFailed(locale),
                      null => AppStrings.syncNow(locale),
                    },
              key: const ValueKey('rime-sync-now'),
              primary: true,
              icon: FLucideIcons.refreshCw,
              onPress: enabled ? _sync : null,
            ),
          ],
        ),
        if (c.rimeSyncLoading) const FProgress(),
        if (c.rimeSyncError case final error?) ErrorMessage(error),
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
