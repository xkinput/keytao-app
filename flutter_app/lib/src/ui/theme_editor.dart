import 'dart:async';
import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:forui/forui.dart';

import '../app/controller.dart';
import '../app/options.dart';
import '../rust/api/types.dart';
import 'candidate_panel_preview.dart';
import 'color_picker.dart';
import 'system_font_picker.dart';
import 'theme.dart';
import 'theme_library.dart';
import 'widgets.dart';

Future<void> showThemeEditorDialog(
  BuildContext context, {
  required AppController controller,
}) => showFDialog<void>(
  context: context,
  builder: (context, _, animation) {
    final content = Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      spacing: 8,
      children: [
        ThemeLibrarySection(controller: controller),
        Expanded(child: ThemeEditorSection(controller: controller)),
      ],
    );
    final close = ActionButton('关闭', onPress: () => Navigator.pop(context));
    if (controller.isMobile) {
      return Scaffold(
        backgroundColor: AppPalette.of(context).surface,
        body: SafeArea(
          child: Padding(
            padding: const EdgeInsets.all(16),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              spacing: 12,
              children: [
                Row(
                  children: [
                    Expanded(
                      child: Text(
                        '编辑主题',
                        style: context.theme.typography.display.lg,
                      ),
                    ),
                    close,
                  ],
                ),
                Expanded(child: content),
              ],
            ),
          ),
        ),
      );
    }
    return AppDialog(
      title: '编辑主题',
      animation: animation,
      constraints: const BoxConstraints(maxWidth: 1200, maxHeight: 900),
      scrollable: false,
      actions: [close],
      child: SizedBox(
        width: 1120,
        height: (MediaQuery.sizeOf(context).height - 200).clamp(240, 720),
        child: content,
      ),
    );
  },
);

class ThemeEditorSection extends StatefulWidget {
  const ThemeEditorSection({super.key, required this.controller});
  final AppController controller;

  @override
  State<ThemeEditorSection> createState() => _ThemeEditorSectionState();
}

class _ThemeEditorSectionState extends State<ThemeEditorSection> {
  Map<String, dynamic>? _snapshot;
  Map<String, dynamic>? _draft;
  final Map<String, Map<String, dynamic>> _previews = {};
  (String, Object?)? _previewField;
  ImeUiSettingsDto? _settings;
  bool _storageReady = false;
  bool _loading = false;
  bool _working = false;
  bool _pending = true;
  late bool _mobile;
  String _target = 'light';
  String? _error;
  final Map<String, String> _textDrafts = {};
  final Map<String, bool> _expandedGroups = {};
  final _controlKeys = {false: GlobalKey(), true: GlobalKey()};
  Future<List<SystemFontFamilyDto>>? _fontListing;

  AppController get _controller => widget.controller;
  SystemFontCatalog get _fonts => SystemFontCatalog.forController(_controller);
  bool get _ready => !_controller.isAndroid || _controller.storageReady;
  bool get _enabled => _ready && !_loading && !_working && !_controller.busy;
  Map<String, dynamic> get _source =>
      _snapshot!['source'] as Map<String, dynamic>;
  Map<String, dynamic> get _resolved => _draft ?? _previews[_target]!;

  Set<String> get _disabledTargets {
    final light = _snapshot!['resolvedLight']['ui']['effectiveColorScheme'];
    final dark = _snapshot!['resolvedDark']['ui']['effectiveColorScheme'];
    return light == dark ? {light == 'dark' ? 'light' : 'dark'} : {};
  }

  void _accept(Map<String, dynamic> snapshot) {
    _snapshot = snapshot;
    for (final target in ['light', 'dark']) {
      _previews[target] = _controller.resolveThemeEditorPreview(
        source: _source,
        dark: target == 'dark',
      );
    }
    if (_disabledTargets.contains(_target)) {
      _target = _target == 'dark' ? 'light' : 'dark';
    }
    _draft = null;
    _textDrafts.clear();
    _loadFontWeights();
  }

  void _loadFontWeights() {
    final family = _resolved['font']['effectiveFamily'] as String?;
    if (family == null || family.isEmpty || _fontListing != null) return;
    _fontListing = _fonts.list();
    unawaited(
      _fontListing!.then((_) {
        if (mounted) setState(() {});
      }, onError: (Object _) => _fontListing = null),
    );
  }

  @override
  void initState() {
    super.initState();
    _mobile = _controller.isMobile;
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
    _preview(path, value);
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
    if (_draft != null && _previewField?.$1 == path) {
      final value = _previewField?.$2;
      if (value != null) return value;
    }
    final raw =
        _snapshot![_target == 'dark' ? 'resolvedDark' : 'resolvedLight'];
    final resolvedPath = path.startsWith('pc.') ? path.substring(3) : path;
    if (path.startsWith('common.')) {
      return switch (path) {
        'common.accentColor' =>
          _at(raw, 'ui.accentColor') is Map
              ? _at(_resolved, 'ui.accentColor')
              : _at(raw, 'ui.accentColor'),
        'common.backgroundColor' => _at(_resolved, 'panel.background'),
        'common.cornerRadius' =>
          _at(_source, '$_target.$path') ??
              _at(_source, path) ??
              _at(_resolved, 'panel.cornerRadius'),
        _ => _at(raw, path.replaceFirst('common.', '')),
      };
    }
    final rawValue = _at(raw, resolvedPath);
    final value = rawValue is Map ? _at(_resolved, resolvedPath) : rawValue;
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
      _mobile != _controller.isMobile ||
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
      Map<String, dynamic>? base = source;
      for (final part in path.split('.').take(parts.length - 2)) {
        base = base?[part] as Map<String, dynamic>?;
      }
      base?.remove(parts.last);
    } else {
      node[parts.last] = value;
    }
    try {
      final preview = _controller.resolveThemeEditorPreview(
        source: source,
        dark: _target == 'dark',
      );
      setState(() {
        _draft = preview;
        _previewField = (path, value);
      });
      _loadFontWeights();
    } catch (failure) {
      setState(() => _error = errorText(failure));
    }
  }

  Widget _row(String label, Widget child, {Widget? trailing}) => ConstrainedBox(
    constraints: BoxConstraints(
      minHeight: context.platformVariant.touch ? 44 : 36,
    ),
    child: Row(
      spacing: 8,
      children: [
        SizedBox(width: 88, child: Text(label)),
        Expanded(child: child),
        ?trailing,
      ],
    ),
  );

  Widget _reset(String label, VoidCallback? onPress) => FTooltip(
    tipBuilder: (_, _) => const Text('恢复默认'),
    child: FButton.icon(
      variant: FButtonVariant.ghost,
      onPress: onPress,
      child: Icon(
        FLucideIcons.rotateCcw,
        size: 14,
        semanticLabel: '$label：恢复默认',
      ),
    ),
  );

  Widget _field(
    String label,
    String path,
    Widget child, {
    bool swatch = false,
    bool compact = false,
  }) {
    final supported = _supported(path);
    final reset = _reset(
      label,
      _enabled &&
              supported &&
              (_at(_source, '$_target.$path') != null ||
                  _at(_source, path) != null)
          ? () => _save(path, null)
          : null,
    );
    final control = Semantics(
      label: label,
      enabled: _enabled && supported,
      child: child,
    );
    final row = compact
        ? Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Row(
                children: [
                  Expanded(child: Text(label)),
                  reset,
                ],
              ),
              control,
            ],
          )
        : swatch
        ? Row(
            spacing: 4,
            children: [
              Expanded(child: Text(label)),
              control,
              reset,
            ],
          )
        : _row(label, control, trailing: reset);
    return KeyedSubtree(
      key: ValueKey(('theme-control', path)),
      child: supported
          ? row
          : FTooltip(tipBuilder: (_, _) => const Text('本平台不支持'), child: row),
    );
  }

  Widget _slider(
    String label,
    String path,
    double min,
    double max,
    int divisions, {
    bool scale = false,
    bool compact = false,
  }) {
    final value = ((_value(path) as num).toDouble() * (scale ? 100 : 1)).clamp(
      min,
      max,
    );
    double number(double fraction) =>
        min + (fraction * divisions).round() / divisions * (max - min);
    String display(double value) =>
        '${value == value.roundToDouble() ? value.toInt() : value.toStringAsFixed(1)}${scale ? '%' : ''}';
    return _field(
      label,
      path,
      Row(
        spacing: 8,
        children: [
          Expanded(
            child: FSlider(
              enabled: _enabled && _supported(path),
              // A disabled thumb can leave a queued tooltip layout; use the value tooltip.
              tooltipControls: const FSliderTooltipControls.disabled(),
              control: FSliderControl.liftedContinuous(
                value: FSliderValue(max: (value - min) / (max - min)),
                stepPercentage: 1 / divisions,
                onChange: (value) {
                  final draft = number(value.max);
                  _preview(path, scale ? draft / 100 : draft);
                },
              ),
              onEnd: _enabled && _supported(path)
                  ? (value) {
                      final next = number(value.max);
                      _save(path, scale ? next / 100 : next);
                    }
                  : null,
              semanticValueFormatterCallback: (value) =>
                  '$label ${display(number(value))}',
            ),
          ),
          FTooltip(
            tipBuilder: (_, _) => Text('$label ${display(value)}'),
            child: SizedBox(
              width: compact ? 20 : 40,
              child: Text(
                display(value),
                textAlign: TextAlign.right,
                style: context.theme.typography.body.xs,
              ),
            ),
          ),
        ],
      ),
      compact: compact,
    );
  }

  Widget _select<T>(
    String label,
    T value,
    Map<T, String> options,
    ValueChanged<T>? onChanged,
  ) => Semantics(
    label: label,
    child: FSelect<T>(
      size: FTextFieldSizeVariant.sm,
      enabled: onChanged != null,
      control: FSelectControl.lifted(
        value: value,
        onChange: (value) {
          if (value != null) onChanged?.call(value);
        },
      ),
      items: {for (final entry in options.entries) entry.value: entry.key},
    ),
  );

  Widget _choice(String label, String path, Map<String, String> options) =>
      _field(
        label,
        path,
        _select(
          label,
          _value(path) as String,
          options,
          _enabled && _supported(path) ? (value) => _save(path, value) : null,
        ),
      );

  Widget _toggle(String label, String path) => _field(
    label,
    path,
    Align(
      alignment: Alignment.centerRight,
      child: FSwitch(
        semanticsLabel: label,
        value: _value(path) as bool,
        enabled: _enabled && _supported(path),
        onChange: (value) => _save(path, value),
      ),
    ),
  );

  Widget _text(String label, String path) {
    final id = '$_target.$path';
    void save(String value) => _save(path, value);
    return _field(
      label,
      path,
      FTextField(
        key: ValueKey(('theme-field', _target, path, _snapshot)),
        size: FTextFieldSizeVariant.sm,
        enabled: _enabled && _supported(path),
        hint: label,
        control: FTextFieldControl.managed(
          initial: TextEditingValue(text: _value(path) as String? ?? ''),
          onChange: (value) {
            _textDrafts[id] = value.text;
            _preview(path, value.text);
          },
        ),
        onSubmit: save,
        suffixBuilder: (context, style, states) => FButton.icon(
          variant: FButtonVariant.ghost,
          onPress: _enabled && _supported(path)
              ? () => save(_textDrafts[id] ?? _value(path) as String? ?? '')
              : null,
          child: Icon(FLucideIcons.check, size: 14, semanticLabel: '保存$label'),
        ),
      ),
    );
  }

  Future<void> _pickFont() async {
    final family = await showThemeFontPicker(
      context,
      controller: _controller,
      selected: _value('common.font.family') as String? ?? '',
    );
    if (!mounted) return;
    setState(() {});
    if (family == null || !_enabled) return;
    final weight = _value('common.font.weight') as String;
    _preview('common.font.family', family);
    final supported = _fonts.supportedWeights(
      _resolved['font']['effectiveFamily'] as String? ?? '',
    );
    final nextWeight = supported == null
        ? weight
        : closestThemeFontWeight(weight, supported);
    await _perform(
      () => _controller.updateThemeEditor(
        target: _target,
        path: 'common.font',
        value: {'family': family, 'weight': nextWeight},
      ),
    );
  }

  Widget _fontFamily() {
    final family = _value('common.font.family') as String? ?? '';
    final name = _fonts.families
        ?.where((font) => font.family == family)
        .firstOrNull
        ?.displayName;
    return _field(
      '字体族',
      'common.font.family',
      FButton(
        variant: FButtonVariant.outline,
        onPress: _enabled && _supported('common.font.family')
            ? _pickFont
            : null,
        child: Text(
          family.isEmpty ? '默认' : name ?? family,
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
        ),
      ),
    );
  }

  Widget _fontWeight() {
    final selected = _value('common.font.weight') as String;
    final supported = _fonts.supportedWeights(
      _resolved['font']['effectiveFamily'] as String? ?? '',
    );
    return _field(
      '字重',
      'common.font.weight',
      Wrap(
        spacing: 4,
        runSpacing: 4,
        children: [
          for (final weight in themeFontWeights.entries)
            Semantics(
              label: '字重：${weight.value}',
              selected:
                  selected == weight.key &&
                  (supported == null || supported.contains(weight.key)),
              child: ActionButton(
                weight.value,
                primary:
                    selected == weight.key &&
                    (supported == null || supported.contains(weight.key)),
                tooltip: weight.value,
                onPress:
                    _enabled &&
                        _supported('common.font.weight') &&
                        (supported == null || supported.contains(weight.key))
                    ? () => _save('common.font.weight', weight.key)
                    : null,
              ),
            ),
        ],
      ),
    );
  }

  String _colorValue(String path) {
    final color = _value(path);
    if (color is String) {
      return _draft != null && _previewField?.$1 == path ? color : '';
    }
    if (color is! Map<String, dynamic>) return '';
    return '#${['red', 'green', 'blue', 'alpha'].map((part) => (color[part] as int).toRadixString(16).padLeft(2, '0')).join().toUpperCase()}';
  }

  Widget _color(String label, String path) {
    final value = _colorValue(path);
    return _field(
      label,
      path,
      FTooltip(
        tipBuilder: (_, _) => Text('$label $value'),
        child: FButton(
          variant: FButtonVariant.ghost,
          mainAxisSize: MainAxisSize.min,
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
          child: Semantics(
            label: '选择$label',
            value: value,
            child: Row(
              mainAxisSize: MainAxisSize.min,
              spacing: 6,
              children: [
                ThemeColorSwatch(value: value),
                Text(value.isEmpty ? '默认' : value),
              ],
            ),
          ),
        ),
      ),
      swatch: true,
    );
  }

  Widget _colors(Map<String, String> fields) => _compactRows([
    for (final entry in fields.entries) _color(entry.value, entry.key),
  ]);

  Widget _compactRows(List<Widget> children) => LayoutBuilder(
    builder: (context, constraints) {
      final columns = !_controller.isMobile && constraints.maxWidth >= 640
          ? 2
          : 1;
      final width = (constraints.maxWidth - (columns - 1) * 12) / columns;
      return Wrap(
        spacing: 12,
        runSpacing: 2,
        children: [
          for (final child in children) SizedBox(width: width, child: child),
        ],
      );
    },
  );

  Widget _group(String id, String title, List<Widget> children) => FAccordion(
    key: ValueKey(('theme-group', id)),
    control: FAccordionControl.lifted(
      expanded: (_) => _expandedGroups[id] ?? false,
      onChange: (_, expanded) => setState(() => _expandedGroups[id] = expanded),
    ),
    style: FAccordionStyleDelta.delta(
      titlePadding: EdgeInsetsGeometryDelta.value(
        EdgeInsets.symmetric(vertical: context.platformVariant.touch ? 12 : 8),
      ),
      childPadding: const EdgeInsetsGeometryDelta.value(
        EdgeInsets.only(bottom: 4),
      ),
    ),
    children: [
      FAccordionItem(
        title: Text(title, style: context.theme.typography.body.sm),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          spacing: 2,
          children: children,
        ),
      ),
    ],
  );

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
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      spacing: 4,
      children: [
        _row(
          '外观',
          _select<UiColorSchemeDto>(
            '外观',
            UiColorSchemeDto.values.byName(_snapshot!['colorScheme'] as String),
            const {
              UiColorSchemeDto.auto: '自动',
              UiColorSchemeDto.light: '浅色',
              UiColorSchemeDto.dark: '深色',
            },
            _enabled
                ? (value) => _perform(
                    () => _controller.setThemeEditorColorScheme(value),
                  )
                : null,
          ),
          trailing: _reset(
            '外观',
            _enabled
                ? () => _perform(
                    () => _controller.setThemeEditorColorScheme(null),
                  )
                : null,
          ),
        ),
        _row(
          '编辑目标',
          _select<String>(
            '编辑目标',
            _target,
            {
              for (final entry in const {'light': '浅色', 'dark': '深色'}.entries)
                if (!_disabledTargets.contains(entry.key))
                  entry.key: entry.value,
            },
            _enabled
                ? (value) => setState(() {
                    _target = value;
                    _draft = null;
                    _loadFontWeights();
                  })
                : null,
          ),
        ),
        if (_working) const FProgress(),
        Expanded(
          child: FTabs(
            expands: true,
            contentPhysics: const NeverScrollableScrollPhysics(),
            style: const FTabsStyleDelta.delta(spacing: 6, minHeight: 32),
            control: FTabControl.lifted(
              index: _mobile ? 1 : 0,
              onChange: (index) => setState(() {
                _mobile = index == 1;
                _draft = null;
              }),
            ),
            children: [
              FTabEntry(label: const Text('PC'), child: _tabContent(false)),
              FTabEntry(label: const Text('移动端'), child: _tabContent(true)),
            ],
          ),
        ),
        if (_error case final error?) ErrorMessage(error),
      ],
    );
  }

  Widget _tabContent(bool mobile) => LayoutBuilder(
    builder: (context, constraints) {
      final controls = SingleChildScrollView(
        key: PageStorageKey(('theme-controls', mobile)),
        padding: const EdgeInsets.only(right: 4, bottom: 8),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          spacing: 2,
          children: [
            _colors(const {
              'common.accentColor': '强调色',
              'common.backgroundColor': '背景色',
            }),
            _slider('圆角', 'common.cornerRadius', 0, 28, 28),
            _group(mobile ? 'mobile.font' : 'pc.font', '字体', [
              _fontFamily(),
              _fontWeight(),
              if (!mobile)
                _compactRows([
                  _slider(
                    AppStrings.candidateFontSize,
                    'pc.font.size',
                    10,
                    36,
                    26,
                  ),
                  for (final entry in const {
                    'labelSize': '序号字号',
                    'commentSize': '注释字号',
                    'preeditSize': '编码字号',
                  }.entries)
                    _slider(entry.value, 'pc.font.${entry.key}', 9, 28, 19),
                ]),
            ]),
            if (!mobile) ...[
              _choice('方向', 'pc.panel.orientation', const {
                'horizontal': '横排',
                'vertical': '竖排',
              }),
              _group('pc.panel', '面板', [
                _colors(const {
                  'pc.panel.background': '面板背景',
                  'pc.panel.borderColor': '面板边框',
                }),
                _compactRows([
                  _slider('边框宽度', 'pc.panel.borderWidth', 0, 4, 8),
                  _slider('面板圆角', 'pc.panel.cornerRadius', 0, 28, 28),
                ]),
                Semantics(
                  label: '面板内边距',
                  child: Row(
                    spacing: 12,
                    children: [
                      for (final side in const {
                        'Top': '上',
                        'Right': '右',
                        'Bottom': '下',
                        'Left': '左',
                      }.entries)
                        Expanded(
                          child: _slider(
                            side.value,
                            'pc.panel.padding${side.key}',
                            0,
                            side.key == 'Top' || side.key == 'Bottom' ? 28 : 32,
                            side.key == 'Top' || side.key == 'Bottom' ? 28 : 32,
                            compact: true,
                          ),
                        ),
                    ],
                  ),
                ),
                _compactRows([
                  _slider('候选间距', 'pc.panel.gap', 0, 24, 24),
                  _slider('编码间距', 'pc.panel.preeditGap', 0, 48, 48),
                  _toggle('面板阴影', 'pc.panel.shadow'),
                ]),
              ]),
              _group('pc.candidate', '候选', [
                _colors({
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
                    'pc.candidate.${entry.key}': entry.value,
                }),
                _compactRows([
                  _slider('边框宽度', 'pc.candidate.borderWidth', 0, 3, 6),
                  _slider('高亮圆角', 'pc.candidate.cornerRadius', 0, 24, 24),
                ]),
                _text('序号后缀', 'pc.candidate.labelSuffix'),
              ]),
              _group('pc.modeHint', '模式提示', [
                _colors(const {
                  'pc.modeHint.background': '背景',
                  'pc.modeHint.foreground': '字色',
                  'pc.modeHint.borderColor': '边框',
                }),
                _compactRows([
                  _slider('边框宽度', 'pc.modeHint.borderWidth', 0, 4, 8),
                  _slider('字号', 'pc.modeHint.fontSize', 12, 42, 30),
                  _slider('宽度', 'pc.modeHint.width', 36, 180, 144),
                  _slider('高度', 'pc.modeHint.height', 28, 140, 112),
                  _slider('圆角', 'pc.modeHint.cornerRadius', 0, 32, 32),
                  _toggle('阴影', 'pc.modeHint.shadow'),
                ]),
              ]),
            ] else ...[
              _colors(const {
                'mobile.keyboard.background': '键盘背景',
                'mobile.keyboard.key.background': '按键底色',
                'mobile.keyboard.key.foreground': '按键字色',
                'mobile.keyboard.key.hintForeground': '提示字色',
              }),
              _compactRows([
                _slider('按键圆角', 'mobile.keyboard.key.cornerRadius', 0, 24, 24),
                _slider(
                  '按键字号',
                  'mobile.keyboard.key.fontScale',
                  80,
                  130,
                  50,
                  scale: true,
                ),
              ]),
              _group('mobile.keyTypes', '功能键', [
                _colors({
                  for (final entry in const {
                    'keyTypes.function': '功能键',
                    'keyTypes.space': '空格',
                    'keyTypes.enter': '回车',
                    'keyClasses.accent': '强调键',
                  }.entries)
                    for (final color in const {
                      'background': '底色',
                      'foreground': '字色',
                      'borderColor': '边框',
                    }.entries)
                      'mobile.keyboard.${entry.key}.${color.key}':
                          '${entry.value}${color.value}',
                }),
                _compactRows([
                  for (final entry in const {
                    'keyTypes.function': '功能键',
                    'keyTypes.space': '空格',
                    'keyTypes.enter': '回车',
                    'keyClasses.accent': '强调键',
                  }.entries)
                    _slider(
                      '${entry.value}边框宽度',
                      'mobile.keyboard.${entry.key}.borderWidth',
                      0,
                      2,
                      4,
                    ),
                ]),
              ]),
              _group('mobile.key', '按键细节', [
                _colors(const {
                  'mobile.keyboard.key.borderColor': '边框',
                  'mobile.keyboard.key.shadowColor': '阴影',
                }),
                _compactRows([
                  _slider('边框宽度', 'mobile.keyboard.key.borderWidth', 0, 2, 4),
                  _slider('阴影偏移', 'mobile.keyboard.key.shadowOffsetY', 0, 4, 8),
                  _slider(
                    '提示字号',
                    'mobile.keyboard.key.hintFontScale',
                    80,
                    130,
                    50,
                    scale: true,
                  ),
                ]),
              ]),
              _group('mobile.candidateBar', '候选栏', [
                _colors({
                  for (final entry in const {
                    'background': '栏背景',
                    'itemBackground': '候选底色',
                    'foreground': '候选字色',
                    'selectedBackground': '高亮底色',
                    'selectedForeground': '高亮字色',
                    'borderColor': '候选边框',
                    'selectedBorderColor': '高亮边框',
                    'labelColor': '序号字色',
                    'selectedLabelColor': '高亮序号',
                    'commentColor': '注释字色',
                    'selectedCommentColor': '高亮注释',
                  }.entries)
                    'mobile.candidateBar.${entry.key}': entry.value,
                }),
                _compactRows([
                  _slider('边框宽度', 'mobile.candidateBar.borderWidth', 0, 2, 4),
                  _slider(
                    '高亮边框宽度',
                    'mobile.candidateBar.selectedBorderWidth',
                    0,
                    3,
                    6,
                  ),
                  _slider('圆角', 'mobile.candidateBar.cornerRadius', 0, 24, 24),
                  _slider('横向内边距', 'mobile.candidateBar.paddingX', 0, 28, 28),
                  _slider('文字间距', 'mobile.candidateBar.inlineGap', 0, 18, 18),
                  _slider(
                    '字号缩放',
                    'mobile.candidateBar.fontScale',
                    80,
                    130,
                    50,
                    scale: true,
                  ),
                  _toggle('显示注释', 'mobile.candidateBar.showComment'),
                ]),
              ]),
              _group('mobile.toolbar', '工具栏', [
                _colors({
                  for (final entry in const {
                    'background': '栏背景',
                    'foreground': '图标字色',
                    'selectedForeground': '选中字色',
                    'buttonBackground': '按钮底色',
                    'buttonBorderColor': '按钮边框',
                    'activeBackground': '强调底色',
                    'selectedBackground': '选中底色',
                    'pressedBackground': '按下底色',
                    'activeBorderColor': '强调边框',
                  }.entries)
                    'mobile.keyboard.toolbar.${entry.key}': entry.value,
                }),
                _compactRows([
                  _slider(
                    '边框宽度',
                    'mobile.keyboard.toolbar.buttonBorderWidth',
                    0,
                    2,
                    4,
                  ),
                  _slider(
                    '按钮圆角',
                    'mobile.keyboard.toolbar.buttonCornerRadius',
                    0,
                    24,
                    24,
                  ),
                ]),
              ]),
            ],
          ],
        ),
      );
      final preview = FittedBox(
        fit: BoxFit.scaleDown,
        alignment: Alignment.topCenter,
        child: SizedBox(
          width: 360,
          child: mobile
              ? CandidatePanelPreview(
                  editorPreview: true,
                  theme: _resolved,
                  fontCatalog: _fonts,
                  keyboard: true,
                  showPreedit: false,
                  keyboardSchemeName:
                      _controller
                          .installedPackageFor(_controller.activeSchemeKey)
                          ?.displayName ??
                      schemeDisplayName(_controller.activeSchemeKey),
                  candidates: _controller.candidatePreviewCandidates,
                  keyboardLogo: SchemaLogo(controller: _controller, size: 28),
                )
              : Column(
                  mainAxisSize: MainAxisSize.min,
                  crossAxisAlignment: CrossAxisAlignment.start,
                  spacing: 12,
                  children: [
                    CandidatePanelPreview(
                      editorPreview: true,
                      theme: _resolved,
                      fontCatalog: _fonts,
                      showPreedit: true,
                      candidateCount: 3,
                      candidates: _controller.candidatePreviewCandidates,
                    ),
                    _modeHintPreview(),
                  ],
                ),
        ),
      );
      final height = (MediaQuery.sizeOf(context).height * .30)
          .clamp(0, constraints.maxHeight * .55)
          .toDouble();
      final stableControls = KeyedSubtree(
        key: _controlKeys[mobile],
        child: controls,
      );
      return constraints.maxWidth >= 900
          ? Row(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              spacing: 16,
              children: [
                Expanded(child: stableControls),
                SizedBox(width: 360, child: preview),
              ],
            )
          : Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              spacing: 8,
              children: [
                SizedBox(height: height, child: preview),
                Expanded(child: stableControls),
              ],
            );
    },
  );

  Widget _modeHintPreview() {
    final hint = _resolved['modeHint'] as Map<String, dynamic>;
    Color color(Map<String, dynamic> section, String name) {
      final value = section[name] as Map<String, dynamic>;
      return Color.fromARGB(
        value['alpha'] as int,
        value['red'] as int,
        value['green'] as int,
        value['blue'] as int,
      );
    }

    final shadow = hint['shadowStyle'] as Map<String, dynamic>?;
    final family = _resolved['font']['effectiveFamily'] as String?;
    Widget preview(String? fontFamily) => Semantics(
      label: '模式提示预览',
      child: Container(
        width: (hint['width'] as num).toDouble(),
        height: (hint['height'] as num).toDouble(),
        alignment: Alignment.center,
        decoration: BoxDecoration(
          color: color(hint, 'background'),
          borderRadius: BorderRadius.circular(
            (hint['cornerRadius'] as num).toDouble(),
          ),
          border: (hint['borderWidth'] as num) > 0
              ? Border.all(
                  color: color(hint, 'borderColor'),
                  width: (hint['borderWidth'] as num).toDouble(),
                )
              : null,
          boxShadow: shadow == null
              ? null
              : [
                  BoxShadow(
                    color: color(shadow, 'color'),
                    blurRadius: (shadow['blurRadius'] as num).toDouble(),
                    offset: Offset(0, (shadow['offsetY'] as num).toDouble()),
                  ),
                ],
        ),
        child: Text(
          hint['chineseText'] as String,
          textScaler: TextScaler.noScaling,
          style: TextStyle(
            fontFamily: fontFamily,
            color: color(hint, 'foreground'),
            fontSize: (hint['fontSize'] as num).toDouble(),
          ),
        ),
      ),
    );
    if (family == null || family.isEmpty) return preview(null);
    return FutureBuilder<String>(
      key: ValueKey(('mode-hint-font', family)),
      future: _fonts.load(family),
      builder: (context, snapshot) => preview(snapshot.data ?? family),
    );
  }
}
