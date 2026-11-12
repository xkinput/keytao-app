import 'package:flutter/material.dart';
import 'package:forui/forui.dart';

import '../app/controller.dart';
import '../app/options.dart';
import '../rust/api/types.dart';
import 'color_picker.dart';
import 'user_words.dart';
import 'widgets.dart';

Future<void> runCloudAccountAction(
  BuildContext context,
  AppController controller,
  Future<bool> Function() action,
) async {
  if (controller.accountUser == null || !context.mounted) {
    return;
  }
  final succeeded = await action();
  if (!succeeded) {
    await controller.reloadAccountSession();
  }
}

Future<bool?> showCloudPackageTransfer(
  BuildContext context,
  AppController controller, {
  required String schemeKey,
  String? packageId,
  int totalBytes = 0,
}) {
  FocusManager.instance.primaryFocus?.unfocus();
  return showFDialog<bool>(
    context: context,
    barrierDismissible: false,
    builder: (context, _, animation) => _CloudPackageTransferDialog(
      controller: controller,
      schemeKey: schemeKey,
      packageId: packageId,
      totalBytes: totalBytes,
      animation: animation,
    ),
  );
}

class _CloudPackageTransferDialog extends StatefulWidget {
  const _CloudPackageTransferDialog({
    required this.controller,
    required this.schemeKey,
    required this.packageId,
    required this.totalBytes,
    required this.animation,
  });

  final AppController controller;
  final String schemeKey;
  final String? packageId;
  final int totalBytes;
  final Animation<double> animation;

  @override
  State<_CloudPackageTransferDialog> createState() =>
      _CloudPackageTransferDialogState();
}

class _CloudPackageTransferDialogState
    extends State<_CloudPackageTransferDialog> {
  Map<String, dynamic>? _preview;
  String? _error;
  bool _loading = true;
  bool _transferring = false;
  bool _succeeded = false;

  bool get _upload => widget.packageId == null;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) async {
      if (!mounted) return;
      if (_upload) {
        try {
          final preview = await widget.controller.previewCloudPackage(
            widget.schemeKey,
          );
          if (!mounted) return;
          setState(() {
            _loading = false;
            _preview = preview;
            if (preview == null) {
              _error = widget.controller.cloudTransferError ?? '上传失败';
            }
          });
        } catch (failure) {
          if (mounted) {
            setState(() {
              _loading = false;
              _error = errorText(failure);
            });
          }
        }
      } else {
        await _transfer();
      }
    });
  }

  Future<void> _transfer() async {
    if (_transferring) return;
    setState(() {
      _loading = false;
      _transferring = true;
    });
    try {
      final c = widget.controller;
      final succeeded = _upload
          ? await c.uploadCloudPackage(
              widget.schemeKey,
              expectedSha256: _preview!['sha256'] as String,
            )
          : await c.downloadCloudPackage(widget.packageId!);
      if (!mounted) return;
      setState(() {
        _transferring = false;
        _succeeded = succeeded;
        if (!succeeded) {
          _error = c.cloudTransferError ?? (_upload ? '上传失败' : '下载失败');
        }
      });
      if (succeeded) {
        await Future<void>.delayed(const Duration(milliseconds: 700));
        if (mounted) Navigator.pop(context, true);
      }
    } catch (failure) {
      if (mounted) {
        setState(() {
          _transferring = false;
          _error = errorText(failure);
        });
      }
    }
  }

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: widget.controller,
    builder: (context, _) {
      final progress = widget.controller.cloudTransferProgress;
      final matching =
          progress?.direction == (_upload ? 'upload' : 'download') &&
          progress?.schemeKey == widget.schemeKey;
      final total = matching
          ? progress!.totalBytes
          : BigInt.from(_preview?['archive_size'] as num? ?? widget.totalBytes);
      final sent = _succeeded
          ? total
          : matching
          ? progress!.bytesTransferred
          : BigInt.zero;
      final fraction = total > BigInt.zero
          ? (sent.toDouble() / total.toDouble()).clamp(0.0, 1.0)
          : 0.0;
      final canClose = !_loading && !_transferring && !_succeeded;
      return PopScope(
        canPop: canClose,
        child: AppDialog(
          title: _upload ? '上传方案' : '下载方案',
          animation: widget.animation,
          actions: [
            if (canClose && _error != null)
              ActionButton('关闭', onPress: () => Navigator.pop(context, false))
            else if (canClose) ...[
              ActionButton('取消', onPress: () => Navigator.pop(context, false)),
              ActionButton('确认上传', primary: true, onPress: _transfer),
            ],
          ],
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              if (_loading) const FProgress(),
              if (_error case final error?) ErrorMessage(error),
              if (_succeeded)
                StatusMessage(
                  _upload ? '上传成功' : '下载成功',
                  icon: FLucideIcons.circleCheck,
                  color: successColor(context),
                ),
              if (_transferring || _succeeded) ...[
                FDeterminateProgress(value: fraction),
                const SizedBox(height: 8),
                Semantics(
                  liveRegion: true,
                  child: Text(
                    '${formatLogBytes(sent)} / ${formatLogBytes(total)} · '
                    '${(fraction * 100).floor()}%',
                  ),
                ),
              ],
              if (canClose && _error == null && _preview != null) ...[
                Text(
                  '${(_preview!['files'] as List).length} 个文件 · '
                  '${(total.toDouble() / (1024 * 1024)).toStringAsFixed(2)} MB',
                ),
                const SizedBox(height: 8),
                ConstrainedBox(
                  constraints: const BoxConstraints(maxHeight: 280),
                  child: SingleChildScrollView(
                    child: SelectableText(
                      (_preview!['files'] as List).cast<String>().join('\n'),
                    ),
                  ),
                ),
              ],
            ],
          ),
        ),
      );
    },
  );
}

String cloudSyncDate(String? value) {
  if (value == null) return '—';
  return DateTime.tryParse(value)?.toLocal().toString().split('.').first ??
      value;
}

class SettingsCard extends StatelessWidget {
  const SettingsCard({super.key, required this.controller});
  final AppController controller;
  @override
  Widget build(BuildContext context) =>
      controller.isMobile ? _mobile(context) : _desktop(context);

  Widget _desktop(BuildContext context) {
    final c = controller;
    final s = c.uiSettings;
    return Section(
      title: AppStrings.imeSettings,
      icon: FLucideIcons.palette,
      children: [
        if (s == null)
          SettingRow(
            AppStrings.checking,
            child: ActionButton(
              AppStrings.recheck,
              onPress: c.busy ? null : c.refreshState,
            ),
          )
        else ...[
          ChoiceSetting<UiColorSchemeDto>(
            label: '配色方案',
            value: s.colorScheme,
            options: colorSchemes,
            icons: const {
              UiColorSchemeDto.auto: FLucideIcons.monitor,
              UiColorSchemeDto.light: FLucideIcons.sun,
              UiColorSchemeDto.dark: FLucideIcons.moon,
            },
            onChanged: c.busy ? null : (value) => c.saveUi(colorScheme: value),
          ),
          ChoiceSetting<PanelOrientationDto>(
            label: '候选排列',
            value: s.orientation,
            options: orientations,
            icons: const {
              PanelOrientationDto.horizontal: FLucideIcons.columns3,
              PanelOrientationDto.vertical: FLucideIcons.rows3,
            },
            onChanged: c.busy ? null : (value) => c.saveUi(orientation: value),
          ),
          ChoiceSetting<EnglishModeDto>(
            label: AppStrings.englishMode,
            value: c.desktopEnglishMode,
            options: c.englishModes,
            disabled: c.disabledEnglishModes,
            hint: c.englishModeHint,
            onChanged: c.busy ? null : c.saveDesktopEnglish,
          ),
          if (c.showsEmbeddedComposition)
            SwitchSetting(
              AppStrings.embedded,
              value: s.embeddedComposition,
              stateLabel: s.embeddedComposition
                  ? AppStrings.on
                  : AppStrings.off,
              hint: AppStrings.embeddedDescription,
              onChanged: c.busy ? null : c.saveEmbedded,
            ),
          SliderSetting(
            label: AppStrings.candidateFontSize,
            value: s.fontSize,
            min: candidateFontMin,
            max: candidateFontMax,
            divisions: candidateFontDivisions,
            icon: FLucideIcons.type,
            onChanged: c.busy ? null : (value) => c.saveUi(fontSize: value),
          ),
          AccentPicker(controller: c),
          if (s.themePath case final path?)
            Row(
              children: [
                Expanded(child: SelectableText('${AppStrings.theme}$path')),
                const SizedBox(width: 8),
                FTooltip(
                  tipBuilder: (_, _) => const Text(AppStrings.openTheme),
                  child: FButton.icon(
                    onPress: c.canOpenTheme ? c.openTheme : null,
                    variant: FButtonVariant.ghost,
                    child: const Icon(
                      FLucideIcons.folderOpen,
                      semanticLabel: AppStrings.openTheme,
                    ),
                  ),
                ),
              ],
            ),
        ],
        if (c.imeUiError case final error?) ErrorMessage(error),
      ],
    );
  }

  Widget _mobile(BuildContext context) {
    final c = controller;
    final s = c.androidSettings;
    final enabled = !c.busy && s != null;
    void save(String field, Object value) => c.saveAndroid({field: value});
    Widget slider(
      String label,
      String field,
      num value,
      double min,
      double max,
      int divisions,
      String unit, {
      bool available = true,
      String? semanticLabel,
    }) => SliderSetting(
      label: label,
      value: value.toDouble(),
      min: min,
      max: max,
      divisions: divisions,
      unit: unit,
      semanticLabel: semanticLabel,
      onChanged: enabled && available ? (value) => save(field, value) : null,
    );
    Widget toggle(String label, String field, bool value, {String? hint}) =>
        SwitchSetting(
          label,
          value: value,
          hint: hint,
          onChanged: enabled ? (value) => save(field, value) : null,
        );
    Widget choice(
      String label,
      String field,
      String value,
      Map<String, String> options,
    ) => ChoiceSetting<String>(
      label: label,
      value: value,
      options: options,
      currentLabel: options[value],
      onChanged: enabled ? (value) => save(field, value) : null,
    );
    return Section(
      title: AppStrings.mobileKeyboard,
      icon: FLucideIcons.keyboard,
      children: [
        const Text(AppStrings.gestures),
        slider(
          AppStrings.longPressDelay,
          'longPressDelayMs',
          s?.longPressDelayMs ?? 300,
          100,
          700,
          60,
          'ms',
        ),
        ChoiceSetting<String>(
          label: AppStrings.englishMode,
          value: s?.englishMode ?? 'ascii',
          currentLabel: c.englishModeLabel,
          hint: c.englishModeHint,
          options: c.androidEnglishModes,
          disabled: c.disabledAndroidEnglishModes,
          onChanged: enabled ? (value) => save('englishMode', value) : null,
        ),
        slider(
          AppStrings.keyboardHeight,
          'keyboardHeightScale',
          s?.keyboardHeightScale ?? 100,
          85,
          130,
          9,
          '%',
        ),
        choice(
          AppStrings.deleteSpeed,
          'deleteSpeed',
          s?.deleteSpeed ?? 'standard',
          const {
            'slow': AppStrings.slow,
            'standard': AppStrings.standard,
            'fast': AppStrings.fast,
          },
        ),
        choice(
          AppStrings.backspaceGesture,
          'backspaceGestureMode',
          s?.backspaceGestureMode ?? 'immediate',
          const {
            'immediate': AppStrings.immediateDelete,
            'selectThenDelete': AppStrings.selectThenDelete,
          },
        ),
        toggle(
          AppStrings.flickKeys,
          'flickKeysEnabled',
          s?.flickKeysEnabled ?? true,
          hint: AppStrings.flickKeysHint,
        ),
        toggle(
          AppStrings.doubleSpacePeriod,
          'doubleSpacePeriodEnabled',
          s?.doubleSpacePeriodEnabled ?? true,
          hint: AppStrings.doubleSpaceHint,
        ),
        slider(
          AppStrings.swipeThreshold,
          'swipeThresholdDp',
          s?.swipeThresholdDp ?? 34,
          12,
          96,
          84,
          'dp',
        ),
        const Text(AppStrings.layout),
        toggle(
          AppStrings.floatingPortrait,
          'floatingPortraitEnabled',
          s?.floatingPortraitEnabled ?? false,
        ),
        slider(
          AppStrings.scale,
          'floatingPortraitScale',
          s?.floatingPortraitScale ?? 88,
          70,
          100,
          30,
          '%',
          available: s?.floatingPortraitEnabled ?? false,
          semanticLabel: AppStrings.floatingScale(AppStrings.floatingPortrait),
        ),
        toggle(
          AppStrings.floatingLandscape,
          'floatingLandscapeEnabled',
          s?.floatingLandscapeEnabled ?? true,
        ),
        slider(
          AppStrings.scale,
          'floatingLandscapeScale',
          s?.floatingLandscapeScale ?? 72,
          45,
          100,
          55,
          '%',
          available: s?.floatingLandscapeEnabled ?? true,
          semanticLabel: AppStrings.floatingScale(AppStrings.floatingLandscape),
        ),
        choice(
          AppStrings.enterKey,
          'enterKeyBehavior',
          s?.enterKeyBehavior ?? 'system',
          const {
            'system': AppStrings.smartEnter,
            'newline': AppStrings.newline,
          },
        ),
        if (s?.configPath?.isNotEmpty == true)
          ValueRow(AppStrings.config, s!.configPath!),
        ActionRow(
          children: [
            ActionButton(
              AppStrings.resetMobileKeyboard,
              onPress: enabled ? c.resetAndroidSettings : null,
            ),
          ],
        ),
      ],
    );
  }
}

class AccountCard extends StatelessWidget {
  const AccountCard({super.key, required this.controller});
  final AppController controller;
  @override
  Widget build(BuildContext context) {
    final c = controller;
    return Section(
      title: '数据管理',
      icon: FLucideIcons.database,
      children: [
        ActionRow(
          children: [
            ActionButton(
              '自造词',
              icon: FLucideIcons.bookOpen,
              onPress:
                  c.busy ||
                      (c.installedPackages.isEmpty &&
                          c.local?.installed != true)
                  ? null
                  : () => openUserWords(context, c),
            ),
          ],
        ),
        if (c.accountUser != null) ...[
          if (c.accountMessage case final message?) StatusMessage(message),
          if (c.accountError case final error?) ErrorMessage(error),
          ActionRow(
            children: [
              ActionButton(
                '上传配置',
                onPress: c.busy
                    ? null
                    : () => runCloudAccountAction(
                        context,
                        c,
                        c.uploadSchemeProfile,
                      ),
              ),
              ActionButton(
                '从云端下载',
                onPress: c.busy
                    ? null
                    : () => runCloudAccountAction(
                        context,
                        c,
                        c.fetchCloudProfile,
                      ),
              ),
            ],
          ),
          SwitchSetting(
            '跟随账号同步',
            value: c.followAccountSync,
            onChanged: c.busy ? null : c.setFollowAccountSync,
          ),
          ValueRow('上次同步', cloudSyncDate(c.lastSchemeSyncAt)),
          SettingRow(
            '云端方案',
            child: FButton.icon(
              variant: FButtonVariant.ghost,
              onPress: c.busy
                  ? null
                  : () => runCloudAccountAction(
                      context,
                      c,
                      c.refreshCloudPackages,
                    ),
              child: const Icon(FLucideIcons.refreshCw, size: 16),
            ),
          ),
          for (final package in c.cloudPackages)
            Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Text('${package['name']}'),
                Text(
                  '${package['version'] ?? '—'} · '
                  '${formatLogBytes(BigInt.from((package['size'] as num?)?.toInt() ?? 0))} · '
                  '${cloudSyncDate(package['createAt'] as String?)}',
                  style: context.theme.typography.body.xs,
                ),
                const SizedBox(height: 8),
                ActionRow(
                  children: [
                    ActionButton(
                      '下载',
                      onPress: c.busy
                          ? null
                          : () => showCloudPackageTransfer(
                              context,
                              c,
                              schemeKey: package['schemeKey'] as String,
                              packageId: package['id'] as String,
                              totalBytes:
                                  (package['size'] as num?)?.toInt() ?? 0,
                            ),
                    ),
                    ActionButton(
                      '删除',
                      onPress: c.busy
                          ? null
                          : () => runCloudAccountAction(
                              context,
                              c,
                              () =>
                                  c.deleteCloudPackage(package['id'] as String),
                            ),
                    ),
                  ],
                ),
              ],
            ),
          if (c.schemeSyncBusy) const FProgress(),
          if (c.schemeSyncError case final error?) ErrorMessage(error),
        ],
      ],
    );
  }
}
