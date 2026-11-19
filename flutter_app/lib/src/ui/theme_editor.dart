import 'dart:async';
import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:forui/forui.dart';

import '../app/controller.dart';
import '../app/options.dart';
import '../rust/api/types.dart';
import 'candidate_panel_preview.dart';
import 'color_picker.dart';
import 'mobile_keyboard_preview.dart';
import 'theme.dart';
import 'widgets.dart';

class ThemeEditorSection extends StatefulWidget {
  const ThemeEditorSection({super.key, required this.controller});
  final AppController controller;

  @override
  State<ThemeEditorSection> createState() => _ThemeEditorSectionState();
}

class _ThemeEditorSectionState extends State<ThemeEditorSection> {
  Map<String, dynamic>? _snapshot;
  Map<String, dynamic>? _draft;
  ImeUiSettingsDto? _settings;
  bool _storageReady = false;
  bool _loading = false;
  bool _working = false;
  bool _pending = true;
  bool _advanced = false;
  String _target = 'light';
  String? _error;
  final Map<String, String> _textDrafts = {};

  AppController get _controller => widget.controller;
  bool get _ready => !_controller.isAndroid || _controller.storageReady;
  bool get _enabled => _ready && !_loading && !_working && !_controller.busy;
  Map<String, dynamic> get _source =>
      _snapshot!['source'] as Map<String, dynamic>;
  Map<String, dynamic> get _resolved =>
      _draft ??
      _snapshot![_target == 'dark' ? 'resolvedDark' : 'resolvedLight']
          as Map<String, dynamic>;

  Set<String> get _disabledTargets {
    final light = _snapshot!['resolvedLight']['ui']['effectiveColorScheme'];
    final dark = _snapshot!['resolvedDark']['ui']['effectiveColorScheme'];
    return light == dark ? {light == 'dark' ? 'light' : 'dark'} : {};
  }

  void _accept(Map<String, dynamic> snapshot) {
    _snapshot = snapshot;
    if (_disabledTargets.contains(_target)) {
      _target = _target == 'dark' ? 'light' : 'dark';
    }
    _draft = null;
    _textDrafts.clear();
  }

  @override
  void initState() {
    super.initState();
    _settings = _controller.uiSettings;
    _storageReady = _ready;
    _target = _settings?.effectiveColorScheme == EffectiveColorSchemeDto.dark
        ? 'dark'
        : 'light';
    _controller.addListener(_changed);
    WidgetsBinding.instance.addPostFrameCallback((_) => _load());
  }

  @override
  void dispose() {
    _controller.removeListener(_changed);
    super.dispose();
  }

  void _changed() {
    if (!identical(_settings, _controller.uiSettings) ||
        _storageReady != _ready) {
      _settings = _controller.uiSettings;
      _storageReady = _ready;
      _pending = true;
    }
    if (_pending && !_working && !_loading && !_controller.busy) {
      WidgetsBinding.instance.addPostFrameCallback((_) => _load());
    }
    if (mounted) setState(() {});
  }

  Future<void> _load() async {
    if (!mounted || !_ready || _loading || _working || _controller.busy) return;
    _pending = false;
    setState(() => _loading = true);
    try {
      final snapshot = await _controller.getThemeEditor();
      if (mounted) {
        setState(() {
          _accept(snapshot);
          _error = null;
        });
      }
    } catch (failure) {
      if (mounted) setState(() => _error = errorText(failure));
    } finally {
      if (mounted) {
        setState(() => _loading = false);
        if (_pending) unawaited(_load());
      }
    }
  }

  Future<void> _save(String path, Object? value) async {
    if (!_enabled) return;
    await _perform(
      () => _controller.updateThemeEditor(
        target: _target,
        path: path,
        value: value,
      ),
    );
  }

  Future<void> _perform(Future<Map<String, dynamic>> Function() action) async {
    setState(() {
      _working = true;
      _error = null;
    });
    try {
      final snapshot = await action();
      if (mounted) {
        setState(() {
          _accept(snapshot);
        });
      }
    } catch (failure) {
      if (mounted) setState(() => _error = errorText(failure));
    } finally {
      if (mounted) {
        setState(() {
          _working = false;
          _draft = null;
        });
        if (_pending) unawaited(_load());
      }
    }
  }

  Object? _at(Object? value, String path) {
    for (final part in path.split('.')) {
      value = value is Map ? value[part] : null;
    }
    return value;
  }

  Object? _value(String path) {
    final resolvedPath = path.startsWith('pc.') ? path.substring(3) : path;
    if (path.startsWith('common.')) {
      return switch (path) {
        'common.accentColor' => _at(_resolved, 'ui.accentColor'),
        'common.backgroundColor' => _at(_resolved, 'panel.background'),
        'common.cornerRadius' =>
          _at(_source, '$_target.$path') ??
              _at(_source, path) ??
              _at(_resolved, 'panel.cornerRadius'),
        _ => _at(_resolved, path.replaceFirst('common.', '')),
      };
    }
    final value = _at(_resolved, resolvedPath);
    if (value != null || !path.startsWith('mobile.keyboard.key')) return value;
    final field = path.split('.').last;
    if (path.startsWith('mobile.keyboard.keyTypes.space.') ||
        path.startsWith('mobile.keyboard.keyTypes.enter.')) {
      return _at(_resolved, 'mobile.keyboard.keyTypes.function.$field') ??
          _at(_resolved, 'mobile.keyboard.key.$field');
    }
    return _at(_resolved, 'mobile.keyboard.key.$field');
  }

  bool _supported(String path) =>
      (_snapshot!['capabilities']['supportedFields'] as List).contains(path);

  void _preview(String path, Object? value) {
    final source = jsonDecode(jsonEncode(_source)) as Map<String, dynamic>;
    var node = source;
    final parts = [_target, ...path.split('.')];
    for (final part in parts.take(parts.length - 1)) {
      node = (node[part] ??= <String, dynamic>{}) as Map<String, dynamic>;
    }
    if (value == null) {
      node.remove(parts.last);
    } else {
      node[parts.last] = value;
    }
    try {
      final preview = _controller.resolveThemeEditorPreview(
        source: source,
        dark: _target == 'dark',
      );
      setState(() => _draft = preview);
    } catch (failure) {
      setState(() => _error = errorText(failure));
    }
  }

  Widget _field(String label, String path, Widget child) {
    final supported = _supported(path);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      spacing: 4,
      children: [
        Row(
          spacing: 8,
          children: [
            Expanded(child: child),
            FTooltip(
              tipBuilder: (_, _) => Text('$label：恢复默认'),
              child: FButton.icon(
                variant: FButtonVariant.ghost,
                onPress:
                    _enabled &&
                        supported &&
                        (_at(_source, '$_target.$path') != null ||
                            _at(_source, path) != null)
                    ? () => _save(path, null)
                    : null,
                child: Icon(
                  FLucideIcons.rotateCcw,
                  size: 16,
                  semanticLabel: '$label：恢复默认',
                ),
              ),
            ),
          ],
        ),
        if (!supported)
          Align(
            alignment: Alignment.centerRight,
            child: Text(
              '本平台不支持',
              style: context.theme.typography.body.xs.copyWith(
                color: AppPalette.of(context).muted,
              ),
            ),
          ),
      ],
    );
  }

  Widget _slider(
    String label,
    String path,
    double min,
    double max,
    int divisions, {
    bool scale = false,
  }) => _field(
    label,
    path,
    SliderSetting(
      label: label,
      value: (_value(path) as num).toDouble() * (scale ? 100 : 1),
      min: min,
      max: max,
      divisions: divisions,
      unit: scale ? '%' : '',
      onPreview: (value) => _preview(path, scale ? value / 100 : value),
      onChanged: _enabled && _supported(path)
          ? (value) => _save(path, scale ? value / 100 : value)
          : null,
    ),
  );

  Widget _choice(
    String label,
    String path,
    Map<String, String> options, {
    bool scrollable = false,
  }) => _field(
    label,
    path,
    ChoiceSetting<String>(
      label: label,
      value: _value(path) as String,
      options: options,
      scrollable: scrollable,
      onChanged: _enabled && _supported(path)
          ? (value) => _save(path, value)
          : null,
    ),
  );

  Widget _toggle(String label, String path) => _field(
    label,
    path,
    SwitchSetting(
      label,
      value: _value(path) as bool,
      onChanged: _enabled && _supported(path)
          ? (value) => _save(path, value)
          : null,
    ),
  );

  Widget _text(String label, String path) {
    final id = '$_target.$path';
    void save(String value) => _save(path, value);
    return _field(
      label,
      path,
      SettingRow(
        label,
        child: FTextField(
          key: ValueKey(('theme-field', _target, path, _value(path))),
          enabled: _enabled && _supported(path),
          hint: label,
          control: FTextFieldControl.managed(
            initial: TextEditingValue(text: _value(path) as String? ?? ''),
            onChange: (value) => _textDrafts[id] = value.text,
          ),
          onSubmit: save,
          suffixBuilder: (context, style, states) => FButton.icon(
            variant: FButtonVariant.ghost,
            onPress: _enabled && _supported(path)
                ? () => save(_textDrafts[id] ?? _value(path) as String? ?? '')
                : null,
            child: Icon(
              FLucideIcons.check,
              size: 16,
              semanticLabel: '保存$label',
            ),
          ),
        ),
      ),
    );
  }

  String _colorValue(String path) {
    final color = _value(path);
    if (color is! Map<String, dynamic>) return '';
    return '#${['red', 'green', 'blue', 'alpha'].map((part) => (color[part] as int).toRadixString(16).padLeft(2, '0')).join().toUpperCase()}';
  }

  Widget _color(String label, String path) {
    final value = _colorValue(path);
    return _field(
      label,
      path,
      SettingRow(
        label,
        child: ActionButton(
          value,
          icon: FLucideIcons.pipette,
          onPress: _enabled && _supported(path)
              ? () async {
                  final color = await showThemeColorPicker(
                    context,
                    initial: value,
                    title: label,
                    onPreview: (value) => _preview(path, value),
                  );
                  if (!mounted) return;
                  if (color == null) {
                    setState(() => _draft = null);
                  } else {
                    await _save(path, color);
                  }
                }
              : null,
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    if (_snapshot == null) {
      return Column(
        children: [
          if (_loading) const FProgress(),
          if (_error case final error?) ErrorMessage(error),
        ],
      );
    }
    final colors = {'background': '底色', 'foreground': '字色'};
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      spacing: 18,
      children: [
        Section(
          title: '外观',
          children: [
            Row(
              spacing: 8,
              children: [
                Expanded(
                  child: ChoiceSetting<UiColorSchemeDto>(
                    label: '外观',
                    value: UiColorSchemeDto.values.byName(
                      _snapshot!['colorScheme'] as String,
                    ),
                    options: const {
                      UiColorSchemeDto.auto: '自动',
                      UiColorSchemeDto.light: '浅色',
                      UiColorSchemeDto.dark: '深色',
                    },
                    onChanged: _enabled
                        ? (value) => _perform(
                            () => _controller.setThemeEditorColorScheme(value),
                          )
                        : null,
                  ),
                ),
                FButton.icon(
                  variant: FButtonVariant.ghost,
                  onPress: _enabled
                      ? () => _perform(
                          () => _controller.setThemeEditorColorScheme(null),
                        )
                      : null,
                  child: const Icon(
                    FLucideIcons.rotateCcw,
                    size: 16,
                    semanticLabel: '外观：恢复默认',
                  ),
                ),
              ],
            ),
            ChoiceSetting<String>(
              label: '编辑目标',
              value: _target,
              options: const {'light': '浅色', 'dark': '深色'},
              disabled: _disabledTargets,
              onChanged: _enabled
                  ? (value) => setState(() {
                      _target = value;
                      _draft = null;
                    })
                  : null,
            ),
            if (_working) const FProgress(),
            LayoutBuilder(
              builder: (context, constraints) {
                final previews = [
                  CandidatePanelPreview(
                    theme: _resolved,
                    showPreedit:
                        !(_controller
                                .uiSettings
                                ?.effectiveEmbeddedComposition ??
                            false),
                    candidateCount:
                        _controller.candidatePageSize ??
                        _controller.candidatePreviewPageSize ??
                        6,
                    candidates: _controller.candidatePreviewCandidates,
                  ),
                  MobileKeyboardPreview(
                    theme: _resolved,
                    schemeName:
                        _controller
                            .installedPackageFor(_controller.activeSchemeKey)
                            ?.displayName ??
                        schemeDisplayName(_controller.activeSchemeKey),
                    candidates: _controller.candidatePreviewCandidates,
                    logo: SchemaLogo(controller: _controller, size: 28),
                  ),
                ];
                return constraints.maxWidth >= 760
                    ? Row(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        spacing: 24,
                        children: [
                          for (final preview in previews)
                            Expanded(child: preview),
                        ],
                      )
                    : Column(spacing: 18, children: previews);
              },
            ),
          ],
        ),
        Section(
          title: '通用',
          children: [
            _field(
              '强调色',
              'common.accentColor',
              AccentPicker(
                controller: _controller,
                value: _colorValue('common.accentColor'),
                enabled: _enabled && _supported('common.accentColor'),
                onPreview: (value) => _preview('common.accentColor', value),
                onChanged: (value) =>
                    _save('common.accentColor', value.isEmpty ? null : value),
              ),
            ),
            _field(
              '背景色',
              'common.backgroundColor',
              BackgroundPicker(
                controller: _controller,
                value: _colorValue('common.backgroundColor'),
                enabled: _enabled && _supported('common.backgroundColor'),
                onPreview: (value) => _preview(
                  'common.backgroundColor',
                  value.isEmpty ? null : value,
                ),
                onChanged: (value) => _save(
                  'common.backgroundColor',
                  value.isEmpty ? null : value,
                ),
              ),
            ),
            _slider('圆角', 'common.cornerRadius', 0, 28, 28),
            _text('字体族', 'common.font.family'),
            _choice('字重', 'common.font.weight', const {
              'ultralight': '极细',
              'thin': '纤细',
              'light': '细',
              'regular': '常规',
              'medium': '中等',
              'semibold': '半粗',
              'bold': '粗',
              'heavy': '重',
              'black': '特粗',
            }, scrollable: true),
          ],
        ),
        Section(
          title: 'PC 面板',
          children: [
            _choice('方向', 'pc.panel.orientation', const {
              'horizontal': '横排',
              'vertical': '竖排',
            }),
            _slider(AppStrings.candidateFontSize, 'pc.font.size', 10, 36, 26),
            ActionButton(
              _advanced ? '收起高级' : '高级',
              icon: _advanced
                  ? FLucideIcons.chevronUp
                  : FLucideIcons.chevronDown,
              onPress: () => setState(() => _advanced = !_advanced),
            ),
            if (_advanced) ...[
              _color('面板背景', 'pc.panel.background'),
              _color('面板边框', 'pc.panel.borderColor'),
              _slider('边框宽度', 'pc.panel.borderWidth', 0, 4, 8),
              _slider('面板圆角', 'pc.panel.cornerRadius', 0, 28, 28),
              _slider('横向内边距', 'pc.panel.paddingX', 0, 32, 32),
              _slider('纵向内边距', 'pc.panel.paddingY', 0, 28, 28),
              _slider('候选间距', 'pc.panel.gap', 0, 24, 24),
              _slider('编码间距', 'pc.panel.preeditGap', 0, 48, 48),
              _toggle('面板阴影', 'pc.panel.shadow'),
              for (final entry in const {
                'labelSize': '序号字号',
                'commentSize': '注释字号',
                'preeditSize': '编码字号',
              }.entries)
                _slider(entry.value, 'pc.font.${entry.key}', 9, 28, 19),
              for (final entry in const {
                'background': '候选底色',
                'foreground': '候选字色',
                'selectedForeground': '高亮字色',
                'selectedBackground': '高亮底色',
                'labelColor': '序号字色',
                'selectedLabelColor': '高亮序号',
                'commentColor': '注释字色',
                'selectedCommentColor': '高亮注释',
                'hoverBackground': '悬停底色',
                'borderColor': '候选边框',
                'selectedBorderColor': '高亮边框',
              }.entries)
                _color(entry.value, 'pc.candidate.${entry.key}'),
              _slider('候选边框宽度', 'pc.candidate.borderWidth', 0, 3, 6),
              _slider('高亮圆角', 'pc.candidate.cornerRadius', 0, 24, 24),
              _text('序号后缀', 'pc.candidate.labelSuffix'),
            ],
          ],
        ),
        Section(
          title: '键盘',
          children: [
            _color('键盘背景', 'mobile.keyboard.background'),
            for (final entry in const {
              'key': '按键',
              'keyTypes.function': '功能键',
              'keyTypes.space': '空格',
              'keyTypes.enter': '回车',
              'keyClasses.accent': '强调键',
            }.entries)
              for (final color in colors.entries)
                _color(
                  '${entry.value}${color.value}',
                  'mobile.keyboard.${entry.key}.${color.key}',
                ),
            _color('提示字色', 'mobile.keyboard.key.hintForeground'),
            _slider('按键圆角', 'mobile.keyboard.key.cornerRadius', 0, 24, 24),
            _color('按键边框', 'mobile.keyboard.key.borderColor'),
            _slider('按键边框宽度', 'mobile.keyboard.key.borderWidth', 0, 2, 4),
            _color('按键阴影', 'mobile.keyboard.key.shadowColor'),
            _slider('阴影偏移', 'mobile.keyboard.key.shadowOffsetY', 0, 4, 8),
            _slider(
              '按键字号',
              'mobile.keyboard.key.fontScale',
              80,
              130,
              50,
              scale: true,
            ),
            _slider(
              '提示字号',
              'mobile.keyboard.key.hintFontScale',
              80,
              130,
              50,
              scale: true,
            ),
          ],
        ),
        Section(
          title: '候选栏',
          children: [
            _slider(
              '候选字号缩放',
              'mobile.candidateBar.fontScale',
              80,
              130,
              50,
              scale: true,
            ),
            _toggle('显示注释', 'mobile.candidateBar.showComment'),
            _color('栏背景', 'mobile.candidateBar.background'),
          ],
        ),
        if (_error case final error?) ErrorMessage(error),
      ],
    );
  }
}
