import 'package:flutter/material.dart';
import 'package:forui/forui.dart';

import '../app/controller.dart';
import '../app/strings.dart';
import '../rust/api/types.dart';
import 'widgets.dart';

void showRimeSyncDialog(BuildContext context, AppController controller) {
  controller.refreshRimeSyncSettings();
  showFDialog<void>(
    context: context,
    builder: (_, _, animation) => ListenableBuilder(
      listenable: controller,
      builder: (_, _) =>
          _RimeSyncDialog(controller: controller, animation: animation),
    ),
  );
}

class _RimeSyncDialog extends StatefulWidget {
  const _RimeSyncDialog({required this.controller, required this.animation});
  final AppController controller;
  final Animation<double> animation;

  @override
  State<_RimeSyncDialog> createState() => _RimeSyncDialogState();
}

class _RimeSyncDialogState extends State<_RimeSyncDialog> {
  static final _generatedId = RegExp(
    r'^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$',
  );
  final _installationId = TextEditingController();
  final _syncDir = TextEditingController();
  RimeSyncSettingsDto? _settings;

  String _displaySyncDir(RimeSyncSettingsDto? settings) => settings == null
      ? ''
      : settings.syncDir.isEmpty
      ? settings.defaultSyncDir
      : settings.syncDir;

  void _readSettings({bool resetIdDraft = false, bool resetPathDraft = false}) {
    final settings = widget.controller.rimeSyncSettings;
    if (resetIdDraft || settings?.installationId != _settings?.installationId) {
      _installationId.text = settings?.installationId ?? '';
    }
    final syncDir = _displaySyncDir(settings);
    if (resetPathDraft || syncDir != _displaySyncDir(_settings)) {
      _syncDir.text = syncDir;
    }
    _settings = settings;
  }

  @override
  void initState() {
    super.initState();
    _readSettings();
  }

  @override
  void didUpdateWidget(covariant _RimeSyncDialog oldWidget) {
    super.didUpdateWidget(oldWidget);
    _readSettings();
  }

  @override
  void dispose() {
    _installationId.dispose();
    _syncDir.dispose();
    super.dispose();
  }

  bool get _hasChanges =>
      _installationId.text != _settings?.installationId ||
      _syncDir.text != _displaySyncDir(_settings);

  Future<bool> _apply(
    Future<bool> Function() action, {
    bool resetIdDraft = true,
  }) async {
    final previous = widget.controller.rimeSyncSettings;
    if (!await action()) return false;
    if (mounted && !identical(previous, widget.controller.rimeSyncSettings)) {
      setState(
        () => _readSettings(resetIdDraft: resetIdDraft, resetPathDraft: true),
      );
    }
    return true;
  }

  Future<bool> _save() => _apply(
    () => widget.controller.saveRimeSyncSettings(
      _installationId.text,
      _syncDir.text,
    ),
  );

  Future<bool> _saveChanges() async => !_hasChanges || await _save();

  void _submit(String _) {
    if (_hasChanges) _save();
  }

  Future<void> _open(Locale locale) async {
    if (await _saveChanges()) {
      await widget.controller.openRimeSyncDirectory(locale: locale);
    }
  }

  Future<void> _sync() async {
    if (await _saveChanges()) await widget.controller.syncRimeUserData();
  }

  @override
  Widget build(BuildContext context) {
    final c = widget.controller;
    final locale = Localizations.localeOf(context);
    final settings = c.rimeSyncSettings;
    final enabled = !c.busy && !c.rimeSyncLoading && settings != null;
    final canSave = enabled && _hasChanges;
    final defaultTag = FBadge(
      variant: FBadgeVariant.outline,
      child: Text(AppStrings.syncDefault(locale)),
    );
    final lastSyncAt = settings?.lastSyncAt.toInt() ?? 0;
    final lastSyncError = settings?.lastSyncError;
    return AppDialog(
      title: AppStrings.rimeSync(locale),
      animation: widget.animation,
      actions: [
        ActionButton(AppStrings.close, onPress: () => Navigator.pop(context)),
      ],
      child: SizedBox(
        width: 560,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          spacing: 12,
          children: [
            SettingRow(
              AppStrings.syncInstallationId(locale),
              stacked: true,
              value:
                  _installationId.text == settings?.installationId &&
                      _generatedId.hasMatch(_installationId.text)
                  ? defaultTag
                  : null,
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
                          onChange: (_) => setState(() {}),
                        ),
                        enabled: enabled,
                        onSubmit: enabled ? _submit : null,
                      ),
                    ),
                  ),
                  ActionButton(
                    AppStrings.syncReset(locale),
                    onPress: enabled ? c.regenerateRimeInstallationId : null,
                  ),
                ],
              ),
            ),
            SettingRow(
              AppStrings.syncDirectory(locale),
              minimumControlWidth: 400,
              value:
                  settings?.syncDir == '' &&
                      _syncDir.text == settings?.defaultSyncDir
                  ? defaultTag
                  : null,
              child: Row(
                spacing: 8,
                children: [
                  Expanded(
                    child: Semantics(
                      label: AppStrings.syncDirectory(locale),
                      child: FTextField(
                        key: const ValueKey('rime-sync-directory'),
                        control: FTextFieldControl.managed(
                          controller: _syncDir,
                          onChange: (_) => setState(() {}),
                        ),
                        enabled: enabled,
                        onSubmit: enabled ? _submit : null,
                      ),
                    ),
                  ),
                  ActionButton(
                    AppStrings.syncChoose(locale),
                    onPress: enabled
                        ? () => _apply(
                            () => c.pickRimeSyncDirectory(
                              settings.installationId,
                            ),
                            resetIdDraft: false,
                          )
                        : null,
                  ),
                  ActionButton(
                    AppStrings.syncReset(locale),
                    onPress: enabled
                        ? () => _apply(
                            () => c.resetRimeSyncDirectory(
                              settings.installationId,
                            ),
                            resetIdDraft: false,
                          )
                        : null,
                  ),
                  ActionButton(
                    AppStrings.syncOpen(locale),
                    onPress: enabled && settings.packageSyncDir.isNotEmpty
                        ? () => _open(locale)
                        : null,
                  ),
                ],
              ),
            ),
            ActionRow(
              children: [
                ActionButton(
                  AppStrings.syncSave(locale),
                  onPress: canSave ? _save : null,
                ),
                ActionButton(
                  c.rimeSyncing
                      ? AppStrings.syncing(locale)
                      : AppStrings.syncNow(locale),
                  key: const ValueKey('rime-sync-now'),
                  primary: true,
                  icon: FLucideIcons.refreshCw,
                  onPress: enabled ? _sync : null,
                ),
              ],
            ),
            if (c.rimeSyncLoading || c.rimeSyncing) const FProgress(),
            if (!c.rimeSyncing &&
                lastSyncError != null &&
                lastSyncError != c.rimeSyncError)
              ErrorMessage(lastSyncError),
            if (!c.rimeSyncing && lastSyncError == null && lastSyncAt != 0)
              Text(
                AppStrings.syncLastAt(
                  locale,
                  DateTime.fromMillisecondsSinceEpoch(lastSyncAt),
                ),
              ),
            if (c.rimeSyncError case final error?) ErrorMessage(error),
          ],
        ),
      ),
    );
  }
}
