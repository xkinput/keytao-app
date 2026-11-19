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
                        control: FTextFieldControl.managed(
                          controller: _syncDir,
                        ),
                        hint: c.rimeSyncSettings?.effectiveSyncDir,
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
            if (c.rimeSyncLoading || c.rimeSyncing) const FProgress(),
            if (c.rimeSyncError case final error?) ErrorMessage(error),
          ],
        ),
      ),
    );
  }
}
