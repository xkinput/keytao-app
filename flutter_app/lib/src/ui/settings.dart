import 'dart:async';

import 'package:flutter/material.dart';
import 'package:forui/forui.dart';

import '../app/controller.dart';
import '../app/options.dart';
import '../rust/api/types.dart';
import 'candidate_panel_preview.dart';
import 'color_picker.dart';
import 'login.dart';
import 'rime_sync_dialog.dart';
import 'system_font_picker.dart';
import 'theme.dart';
import 'theme_editor.dart';
import 'theme_library.dart';
import 'widgets.dart';

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

class SettingsCard extends StatefulWidget {
  const SettingsCard({
    super.key,
    required this.controller,
    this.hero,
    this.inputCells = const [],
  }) : appearance = false;

  const SettingsCard.skin({super.key, required this.controller})
    : appearance = true,
      hero = null,
      inputCells = const [];

  final AppController controller;
  final Widget? hero;
  final List<Widget> inputCells;
  final bool appearance;

  @override
  State<SettingsCard> createState() => _SettingsCardState();
}

class _SettingsCardState extends State<SettingsCard> {
  AppController get controller => widget.controller;
  int _themeLibraryRevision = 0;
  Timer? _previewTimer;
  ImeUiSettingsDto? _previewSource;
  UiColorSchemeDto? _previewScheme;
  PanelOrientationDto? _previewOrientation;
  String? _previewAccent;
  String? _previewBackground;
  double? _previewRadius;
  bool _resetRadius = false;
  double? _previewFontSize;

  @override
  void dispose() {
    _previewTimer?.cancel();
    super.dispose();
  }

  void _schedulePreview(VoidCallback update, {bool immediate = false}) {
    if (!mounted) return;
    _previewTimer?.cancel();
    if (immediate) {
      setState(update);
    } else {
      _previewTimer = Timer(const Duration(milliseconds: 80), () {
        setState(update);
      });
    }
  }

  void _resetPreview() {
    _previewTimer?.cancel();
    _previewScheme = null;
    _previewOrientation = null;
    _previewAccent = null;
    _previewBackground = null;
    _previewRadius = null;
    _resetRadius = false;
    _previewFontSize = null;
  }

  Future<void> _savePreview({
    UiColorSchemeDto? colorScheme,
    PanelOrientationDto? orientation,
    bool? hideCandidatePanel,
    String? accentColor,
    String? backgroundColor,
    double? fontSize,
    double? cornerRadius,
    bool resetCornerRadius = false,
  }) async {
    if (!mounted) return;
    _previewTimer?.cancel();
    setState(() {
      _previewScheme = colorScheme ?? _previewScheme;
      _previewOrientation = orientation ?? _previewOrientation;
      _previewAccent = accentColor ?? _previewAccent;
      _previewBackground = backgroundColor ?? _previewBackground;
      _previewFontSize = fontSize ?? _previewFontSize;
      _previewRadius = resetCornerRadius
          ? null
          : cornerRadius ?? _previewRadius;
      if (cornerRadius != null || resetCornerRadius) {
        _resetRadius = resetCornerRadius;
      }
    });
    final saved = await controller.saveUi(
      colorScheme: colorScheme,
      orientation: orientation,
      hideCandidatePanel: hideCandidatePanel,
      accentColor: accentColor,
      backgroundColor: backgroundColor,
      fontSize: fontSize,
      cornerRadius: cornerRadius,
      resetCornerRadius: resetCornerRadius,
    );
    if (!saved && mounted) setState(_resetPreview);
  }

  @override
  Widget build(BuildContext context) {
    final settings = controller.uiSettings;
    if (!identical(settings, _previewSource)) {
      _previewSource = settings;
      _resetPreview();
    }
    return widget.appearance ? _skin(context) : _input(context);
  }

  Widget _candidatePageSize() {
    final c = controller;
    final enabled = !c.busy && (!c.isAndroid || c.storageReady);
    return SettingRow(
      '候选个数',
      keepInline: true,
      controlMaxWidth: 160,
      child: Semantics(
        label: '候选个数',
        enabled: enabled,
        child: SizedBox(
          width: 160,
          child: FSelect<int>(
            size: context.platformVariant.touch
                ? FTextFieldSizeVariant.sm
                : FTextFieldSizeVariant.md,
            enabled: enabled,
            control: FSelectControl.lifted(
              value: c.candidatePageSize ?? 0,
              onChange: (value) =>
                  c.saveCandidatePageSize(value == 0 ? null : value),
            ),
            items: {
              '跟随方案': 0,
              for (var size = 1; size <= 10; size++) '$size': size,
            },
          ),
        ),
      ),
    );
  }

  Widget _input(BuildContext context) {
    final c = controller;
    final s = c.uiSettings;
    final preview = c.isMobile
        ? _preview(context, keyboard: true)
        : s != null && !s.hideCandidatePanel
        ? _preview(context)
        : null;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (MediaQuery.sizeOf(context).width >= 880 &&
            widget.hero != null &&
            preview != null)
          Row(
            children: [
              Expanded(child: widget.hero!),
              Expanded(child: preview),
            ],
          )
        else ...[
          ?widget.hero,
          if (c.isMobile) const SizedBox(height: 12),
          ?preview,
        ],
        HairlineGridSection(
          children: [
            if (!c.isMobile)
              if (s == null)
                HairlineGridCell(
                  child: SettingRow(
                    AppStrings.checking,
                    child: ActionButton(
                      AppStrings.recheck,
                      onPress: c.busy ? null : c.refreshState,
                    ),
                  ),
                )
              else
                _settingsCell('候选', [
                  BigTextRadioGroup<PanelOrientationDto?>(
                    label: '候选模式',
                    value: s.hideCandidatePanel ? null : s.orientation,
                    options: const {...orientations, null: '隐藏'},
                    onChanged: c.canSaveTheme
                        ? (value) => value == null
                              ? c.saveHideCandidatePanel(true)
                              : _savePreview(
                                  orientation: value,
                                  hideCandidatePanel: false,
                                )
                        : null,
                  ),
                  _candidatePageSize(),
                  if (c.showsEmbeddedComposition)
                    _separated(
                      SwitchSetting(
                        AppStrings.embedded,
                        value: s.effectiveEmbeddedComposition,
                        onChanged: c.canSaveTheme && !s.hideCandidatePanel
                            ? c.saveEmbedded
                            : null,
                      ),
                    ),
                  if (c.isWindows)
                    _separated(
                      SwitchSetting(
                        '托盘显示中英状态',
                        value: s.showTrayModeIndicator,
                        onChanged: c.canSaveTheme
                            ? c.saveShowTrayModeIndicator
                            : null,
                      ),
                    ),
                ]),
            if (c.isMobile) ..._mobile(context),
            ...widget.inputCells,
            if (c.imeUiError case final error?)
              HairlineGridCell(child: ErrorMessage(error)),
          ],
        ),
      ],
    );
  }

  Widget _skin(BuildContext context) {
    final c = controller;
    final settings = c.uiSettings;
    final showPanel = c.isMobile || settings?.hideCandidatePanel == false;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (showPanel)
          LayoutBuilder(
            builder: (context, constraints) {
              if (!c.isMobile && constraints.maxWidth >= 800) {
                return Row(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    for (final scheme in const [
                      UiColorSchemeDto.light,
                      UiColorSchemeDto.dark,
                    ])
                      Expanded(child: _preview(context, appearance: scheme)),
                  ],
                );
              }
              return _preview(context, keyboard: c.isMobile);
            },
          ),
        HairlineGridSection(
          minCellWidth: 420,
          columnCount: MediaQuery.sizeOf(context).width >= 880 ? 2 : 1,
          children: [
            if (settings == null)
              HairlineGridCell(
                child: SettingRow(
                  AppStrings.checking,
                  child: ActionButton(
                    AppStrings.recheck,
                    onPress: c.busy ? null : c.refreshState,
                  ),
                ),
              )
            else ...[
              _settingsCell('配色', [
                BigTextRadioGroup<UiColorSchemeDto>(
                  label: '配色方案',
                  value: settings.colorScheme,
                  options: colorSchemes,
                  onChanged: c.canSaveTheme
                      ? (value) => _savePreview(colorScheme: value)
                      : null,
                ),
              ]),
              HairlineGridCell(
                child: ThemeLibrarySection(
                  key: ValueKey(_themeLibraryRevision),
                  controller: c,
                  quickSwitch: true,
                  onEdit: _editTheme,
                ),
              ),
              if (showPanel) ...[
                _settingsCell('颜色', [_accentPicker(), _backgroundPicker()]),
                _settingsCell('候选窗', [
                  if (!c.isMobile)
                    SliderSetting(
                      label: AppStrings.candidateFontSize,
                      value: settings.fontSize,
                      min: candidateFontMin,
                      max: candidateFontMax,
                      divisions: candidateFontDivisions,
                      onPreview: (value) =>
                          _schedulePreview(() => _previewFontSize = value),
                      onChanged: c.canSaveTheme
                          ? (value) => _savePreview(fontSize: value)
                          : null,
                    ),
                  _cornerRadius(),
                ]),
              ],
            ],
            if (c.imeUiError case final error?)
              HairlineGridCell(child: ErrorMessage(error)),
          ],
        ),
      ],
    );
  }

  Widget _settingsCell(String label, List<Widget> children) => HairlineGridCell(
    child: Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      spacing: 8,
      children: [EyebrowLabel(label), ...children],
    ),
  );

  Widget _separated(Widget child) => Container(
    padding: const EdgeInsets.only(top: 14),
    decoration: BoxDecoration(
      border: Border(top: BorderSide(color: AppPalette.of(context).hairline)),
    ),
    child: child,
  );

  Future<void> _editTheme() async {
    await showThemeEditorDialog(context, controller: controller);
    if (mounted) setState(() => _themeLibraryRevision++);
  }

  Widget _accentPicker() => AccentPicker(
    controller: controller,
    onPreview: (value) => _schedulePreview(
      () => _previewAccent = value,
      immediate: value == controller.uiSettings?.accentColor,
    ),
    onChanged: (value) => _savePreview(accentColor: value),
  );

  Widget _backgroundPicker() => BackgroundPicker(
    controller: controller,
    onPreview: (value) => _schedulePreview(
      () => _previewBackground = value,
      immediate: value == controller.uiSettings?.backgroundColor,
    ),
    onChanged: (value) => _savePreview(backgroundColor: value),
  );

  Widget _cornerRadius() {
    final radius = controller.uiSettings!.cornerRadius;
    return SliderSetting(
      label: '圆角',
      value: radius ?? 16,
      min: 0,
      max: 28,
      divisions: 28,
      onPreview: (value) => _schedulePreview(() {
        _previewRadius = value;
        _resetRadius = false;
      }),
      onChanged: controller.canSaveTheme
          ? (value) => _savePreview(cornerRadius: value)
          : null,
      valueAction: radius == null
          ? null
          : FButton.icon(
              variant: FButtonVariant.ghost,
              size: context.platformVariant.touch
                  ? FButtonSizeVariant.md
                  : FButtonSizeVariant.xs,
              onPress: controller.canSaveTheme
                  ? () => _savePreview(resetCornerRadius: true)
                  : null,
              child: const Icon(
                FLucideIcons.rotateCcw,
                size: 14,
                semanticLabel: '圆角：恢复默认',
              ),
            ),
    );
  }

  Widget _preview(
    BuildContext context, {
    bool keyboard = false,
    UiColorSchemeDto? appearance,
  }) {
    final c = controller;
    final s = c.uiSettings;
    final accent =
        _previewAccent ??
        s?.accentColor ??
        '#${(ChromeAccent.of(context).toARGB32() & 0xFFFFFF).toRadixString(16).padLeft(6, '0')}';
    final orientation = keyboard
        ? PanelOrientationDto.horizontal
        : _previewOrientation ?? s!.orientation;
    final fontSize = _previewFontSize ?? s?.fontSize ?? 18;
    final cornerRadius = _resetRadius
        ? null
        : _previewRadius ?? s?.cornerRadius;
    final backgroundColor = _previewBackground ?? s?.backgroundColor ?? '';
    final colorScheme =
        appearance ?? _previewScheme ?? s?.colorScheme ?? UiColorSchemeDto.auto;
    final preview = CandidatePanelPreview(
      fontCatalog: SystemFontCatalog.forController(c),
      key: ValueKey((colorScheme, orientation)),
      theme: c.cachedCandidatePanelPreview(
        colorScheme: colorScheme,
        accentColor: accent,
        cornerRadius: cornerRadius,
        backgroundColor: backgroundColor,
        fontSize: fontSize,
        systemDark: MediaQuery.platformBrightnessOf(context) == Brightness.dark,
      ),
      orientation: orientation,
      showPreedit: !keyboard && !s!.effectiveEmbeddedComposition,
      candidateCount: c.candidatePageSize ?? c.candidatePreviewPageSize ?? 6,
      candidates: c.candidatePreviewCandidates,
      keyboard: keyboard,
      showKeyboardCandidates: keyboard && widget.appearance,
      editorPreview: keyboard && widget.appearance,
      keyboardSettings: keyboard ? c.androidSettings : null,
      keyboardSchemeName:
          c.currentSchemaName ??
          c.installedPackageFor(c.activeSchemeKey)?.displayName ??
          schemeDisplayName(c.activeSchemeKey),
      keyboardLogo: SchemaLogo(controller: c, size: 28),
    );
    if (keyboard) return preview;
    return CustomPaint(
      painter: _PreviewDots(AppPalette.of(context).hairline),
      child: ConstrainedBox(
        constraints: BoxConstraints(minHeight: widget.appearance ? 240 : 128),
        child: Padding(
          padding: EdgeInsets.symmetric(
            horizontal: MediaQuery.sizeOf(context).width >= 720 ? 24 : 0,
            vertical: 14,
          ),
          child: Column(
            mainAxisAlignment: MainAxisAlignment.center,
            crossAxisAlignment: widget.appearance
                ? CrossAxisAlignment.center
                : CrossAxisAlignment.end,
            children: [
              if (appearance != null) ...[
                EyebrowLabel(colorSchemes[appearance]!),
                const SizedBox(height: 8),
              ],
              AnimatedSwitcher(
                duration: MediaQuery.disableAnimationsOf(context)
                    ? Duration.zero
                    : const Duration(milliseconds: 160),
                layoutBuilder: (current, _) =>
                    current ?? const SizedBox.shrink(),
                child: preview,
              ),
              if (!widget.appearance) ...[
                const SizedBox(height: 8),
                Align(
                  alignment: Alignment.centerRight,
                  child: Text(
                    '${orientations[orientation]} · ${c.candidatePageSize == null ? '跟随方案' : '${c.candidatePageSize} 个'} · ${fontSize.round()} 号',
                    textAlign: TextAlign.right,
                    style: context.theme.typography.body.xs.copyWith(
                      color: AppPalette.of(context).muted,
                    ),
                  ),
                ),
              ],
            ],
          ),
        ),
      ),
    );
  }

  List<Widget> _mobile(BuildContext context) {
    final c = controller;
    final s = c.androidSettings;
    final enabled = !c.busy && s != null;
    final clipboardEnabled = s?.clipboardHistoryEnabled ?? true;
    final portraitLabel = c.isIos
        ? AppStrings.oneHandedPortrait
        : AppStrings.floatingPortrait;
    final landscapeLabel = c.isIos
        ? AppStrings.splitLandscape
        : AppStrings.floatingLandscape;
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
    Widget toggle(
      String label,
      String field,
      bool value, {
      String? hint,
      bool available = true,
    }) => SwitchSetting(
      label,
      value: value,
      hint: hint,
      onChanged: enabled && available ? (value) => save(field, value) : null,
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
      onChanged: enabled ? (value) => save(field, value) : null,
    );
    return [
      HairlineGridCell(
        child: Semantics(
          label: AppStrings.mobileKeyboard,
          child: Section(
            title: '候选与键盘',
            children: [
              Text(AppStrings.englishMode),
              BigTextRadioGroup<String>(
                label: AppStrings.englishMode,
                value: s?.englishMode ?? 'ascii',
                options: const {
                  'ascii': 'ASCII',
                  'schema': AppStrings.englishSchema,
                },
                semanticLabels: c.androidEnglishModes,
                disabled: c.disabledAndroidEnglishModes,
                onChanged: enabled
                    ? (value) => save('englishMode', value)
                    : null,
              ),
              if (c.englishModeHint case final hint?)
                Text(hint, style: context.theme.typography.body.xs),
              _candidatePageSize(),
              slider(
                AppStrings.longPressDelay,
                'longPressDelayMs',
                s?.longPressDelayMs ?? 300,
                100,
                700,
                60,
                'ms',
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
              toggle(
                AppStrings.symbolAutoReturn,
                'symbolAutoReturn',
                s?.symbolAutoReturn ?? false,
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
            ],
          ),
        ),
      ),
      HairlineGridCell(
        child: Section(
          title: AppStrings.clipboard,
          children: [
            toggle(
              AppStrings.clipboardHistoryEnabled,
              'clipboardHistoryEnabled',
              clipboardEnabled,
            ),
            slider(
              AppStrings.clipboardHistoryLimit,
              'clipboardHistoryLimit',
              s?.clipboardHistoryLimit ?? 50,
              10,
              200,
              190,
              '条',
              available: clipboardEnabled,
            ),
            toggle(
              AppStrings.clipboardSuggestionEnabled,
              'clipboardSuggestionEnabled',
              s?.clipboardSuggestionEnabled ?? true,
              available: clipboardEnabled,
            ),
            slider(
              AppStrings.clipboardSuggestionTimeoutSec,
              'clipboardSuggestionTimeoutSec',
              s?.clipboardSuggestionTimeoutSec ?? 30,
              5,
              120,
              115,
              '秒',
              available:
                  clipboardEnabled && (s?.clipboardSuggestionEnabled ?? true),
            ),
            toggle(
              AppStrings.clipboardReturnAfterPaste,
              'clipboardReturnAfterPaste',
              s?.clipboardReturnAfterPaste ?? true,
              available: clipboardEnabled,
            ),
            toggle(
              AppStrings.clipboardHideSensitive,
              'clipboardHideSensitive',
              s?.clipboardHideSensitive ?? true,
              available: clipboardEnabled,
            ),
          ],
        ),
      ),
      HairlineGridCell(
        child: Section(
          title: AppStrings.layout,
          children: [
            if (c.isAndroid)
              SwitchSetting(
                AppStrings.keyboardFlushBottom,
                hint: AppStrings.keyboardFlushBottomHint,
                value: c.ime?['keyboardFlushBottom'] == true,
                onChanged: c.busy || c.ime == null
                    ? null
                    : c.setKeyboardFlushBottom,
              ),
            toggle(
              portraitLabel,
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
              semanticLabel: AppStrings.floatingScale(portraitLabel),
            ),
            toggle(
              landscapeLabel,
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
              semanticLabel: AppStrings.floatingScale(landscapeLabel),
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
        ),
      ),
    ];
  }
}

class _PreviewDots extends CustomPainter {
  const _PreviewDots(this.color);
  final Color color;

  @override
  void paint(Canvas canvas, Size size) {
    final paint = Paint()..color = color;
    for (var y = 7.0; y < size.height; y += 14) {
      for (var x = 7.0; x < size.width; x += 14) {
        canvas.drawCircle(Offset(x, y), 1, paint);
      }
    }
  }

  @override
  bool shouldRepaint(_PreviewDots oldDelegate) => color != oldDelegate.color;
}

class AccountCard extends StatelessWidget {
  const AccountCard({super.key, required this.controller});
  final AppController controller;
  @override
  Widget build(BuildContext context) {
    final c = controller;
    return Semantics(
      label: '数据管理',
      child: Section(
        title: '账号与同步',
        icon: FLucideIcons.database,
        trailing: ActionButton(
          c.accountUser == null ? AppStrings.login : '我的',
          onPress: () => showAccountDialog(context, c),
        ),
        children: [
          if (!c.isMobile)
            ActionRow(
              children: [
                ActionButton(
                  AppStrings.rimeSync(Localizations.localeOf(context)),
                  key: const ValueKey('rime-sync-button'),
                  onPress: c.busy ? null : () => showRimeSyncDialog(context, c),
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
                  onPress: c.busy ? null : c.uploadSchemeProfile,
                ),
                ActionButton(
                  '从云端下载',
                  onPress: c.busy ? null : c.fetchCloudProfile,
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
              child: FTooltip(
                tipBuilder: (_, _) => const Text('刷新云端方案'),
                child: FButton.icon(
                  variant: FButtonVariant.ghost,
                  onPress: c.busy ? null : c.refreshCloudPackages,
                  child: Icon(
                    FLucideIcons.refreshCw,
                    size: context.platformVariant.touch ? 18 : 16,
                    semanticLabel: '刷新云端方案',
                  ),
                ),
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
                            : () =>
                                  c.deleteCloudPackage(package['id'] as String),
                      ),
                    ],
                  ),
                ],
              ),
            if (c.schemeSyncBusy) const FProgress(),
            if (c.schemeSyncError case final error?) ErrorMessage(error),
          ],
        ],
      ),
    );
  }
}

/// The deployed scheme's own switches, shown on the scheme page.
class SchemaSwitchesSection extends StatelessWidget {
  const SchemaSwitchesSection({super.key, required this.controller});
  final AppController controller;

  @override
  Widget build(BuildContext context) {
    final c = controller;
    if (c.usesLibjd(c.activeSchemeKey)) return const SizedBox.shrink();
    if (c.schemaSwitchError case final error?) {
      return Section(
        title: '方案开关',
        icon: FLucideIcons.slidersHorizontal,
        children: [
          ErrorMessage(error),
          ActionRow(
            children: [
              ActionButton('重试', onPress: c.busy ? null : c.refreshState),
            ],
          ),
        ],
      );
    }
    final switches = [
      for (final entry in c.schemaSwitches)
        if (entry.options.isEmpty)
          SwitchSetting(
            entry.states[1],
            value: entry.value == 1,
            onChanged: c.busy
                ? null
                : (value) => c.saveSchemaSwitch(entry, value ? 1 : 0),
          )
        else
          ChoiceSetting<int>(
            label: entry.states.join(' / '),
            value: entry.value,
            options: {
              for (var index = 0; index < entry.states.length; index++)
                index: entry.states[index],
            },
            onChanged: c.busy
                ? null
                : (value) => c.saveSchemaSwitch(entry, value),
          ),
    ];
    return LayoutBuilder(
      builder: (context, constraints) => Section(
        title: '方案开关',
        icon: FLucideIcons.slidersHorizontal,
        children:
            constraints.hasBoundedWidth &&
                (c.isMobile ||
                    (MediaQuery.sizeOf(context).width >= 720 &&
                        constraints.maxWidth >= 640))
            ? [
                for (var index = 0; index < switches.length; index += 2)
                  Table(
                    border: index + 1 < switches.length
                        ? TableBorder(
                            verticalInside: BorderSide(
                              color: AppPalette.of(context).hairline,
                            ),
                          )
                        : null,
                    children: [
                      TableRow(
                        children: [
                          HairlineGridCell(
                            padding: EdgeInsets.only(
                              right: c.isMobile ? 8 : 16,
                            ),
                            child: switches[index],
                          ),
                          if (index + 1 < switches.length)
                            HairlineGridCell(
                              padding: EdgeInsets.only(
                                left: c.isMobile ? 8 : 16,
                              ),
                              child: switches[index + 1],
                            )
                          else
                            const SizedBox.shrink(),
                        ],
                      ),
                    ],
                  ),
              ]
            : switches,
      ),
    );
  }
}
