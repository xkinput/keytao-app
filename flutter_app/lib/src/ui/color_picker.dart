import 'package:flutter/material.dart';
import 'package:forui/forui.dart';

import '../app/controller.dart';
import '../app/options.dart';
import 'theme.dart';
import 'widgets.dart';

Color parseAccentColor(String hex) {
  if (hex == 'transparent') return Colors.transparent;
  final digits = hex.startsWith('#') ? hex.substring(1) : '';
  final expanded = digits.length == 3
      ? digits.split('').map((digit) => '$digit$digit').join()
      : digits;
  final value = int.tryParse(expanded, radix: 16);
  if (value == null) return const Color(0xFF3B73D9);
  return expanded.length == 8
      ? Color(((value & 0xff) << 24) | (value >> 8))
      : Color(0xFF000000 | value);
}

String _hex(Color color) {
  final value = color.toARGB32();
  final rgb = (value & 0xFFFFFF).toRadixString(16).padLeft(6, '0');
  final alpha = value >> 24;
  return '#$rgb${alpha == 255 ? '' : alpha.toRadixString(16).padLeft(2, '0')}'
      .toUpperCase();
}

class ThemeColorSwatch extends StatelessWidget {
  const ThemeColorSwatch({
    super.key,
    required this.value,
    this.size = 22,
    this.child,
  });

  final String value;
  final double size;
  final Widget? child;

  @override
  Widget build(BuildContext context) => Container(
    width: size,
    height: size,
    clipBehavior: Clip.antiAlias,
    decoration: BoxDecoration(
      border: Border.all(color: AppPalette.of(context).hairline),
      borderRadius: BorderRadius.circular(4),
    ),
    child: Stack(
      fit: StackFit.expand,
      children: [
        Column(
          children: [
            for (var row = 0; row < 4; row++)
              Expanded(
                child: Row(
                  children: [
                    for (var column = 0; column < 4; column++)
                      Expanded(
                        child: ColoredBox(
                          color: (row + column).isEven
                              ? Colors.white
                              : const Color(0xFFBDBDBD),
                        ),
                      ),
                  ],
                ),
              ),
          ],
        ),
        ColoredBox(color: parseAccentColor(value)),
        if (child case final overlay?) Center(child: overlay),
      ],
    ),
  );
}

Future<String?> showThemeColorPicker(
  BuildContext context, {
  required String initial,
  required String title,
  ValueChanged<String>? onPreview,
}) => showFDialog<String>(
  context: context,
  builder: (_, _, animation) => _ColorDialog(
    initial: initial,
    animation: animation,
    title: title,
    onPreview: onPreview,
  ),
);

class AccentPicker extends StatelessWidget {
  const AccentPicker({
    super.key,
    required this.controller,
    this.onPreview,
    this.background = false,
    this.value,
    this.onChanged,
    this.enabled = true,
  });
  final AppController controller;
  final ValueChanged<String>? onPreview;
  final bool background;
  final String? value;
  final Future<void> Function(String)? onChanged;
  final bool enabled;
  String get _selected =>
      value ??
      (background
          ? controller.uiSettings!.backgroundColor
          : controller.uiSettings!.accentColor);
  String get _chooseLabel =>
      background ? '选择背景色' : AppStrings.chooseAccentColor;

  Future<void> _save(String value) async {
    onPreview?.call(value);
    if (onChanged case final change?) {
      await change(value);
      return;
    }
    final saved = await (background
        ? controller.saveUi(backgroundColor: value)
        : controller.saveUi(accentColor: value));
    if (!saved) onPreview?.call(_selected);
  }

  Future<void> _pick(BuildContext context, String initial) async {
    final value = await showThemeColorPicker(
      context,
      initial: initial,
      title: _chooseLabel,
      onPreview: onPreview,
    );
    if (value != null) {
      await _save(value);
    } else {
      onPreview?.call(_selected);
    }
  }

  @override
  Widget build(BuildContext context) {
    final raw = _selected.toUpperCase();
    final selected = raw.length == 9 && raw.endsWith('FF')
        ? raw.substring(0, 7)
        : raw;
    var resolvedColor = '#3B73D9';
    if (background && value == null) {
      final settings = controller.uiSettings!;
      final theme = controller.resolveCandidatePanelPreview(
        colorScheme: settings.colorScheme,
        accentColor: settings.accentColor,
        cornerRadius: settings.cornerRadius,
        backgroundColor: settings.backgroundColor,
        systemDark: MediaQuery.platformBrightnessOf(context) == Brightness.dark,
        fontSize: settings.fontSize,
      );
      final mobileColor = theme['mobile']?['keyboard']?['background'];
      final color =
          (controller.isMobile ? mobileColor : null) ??
          theme['panel']['background'] as Map<String, dynamic>;
      resolvedColor = _hex(
        Color.fromARGB(
          255,
          color['red'] as int,
          color['green'] as int,
          color['blue'] as int,
        ),
      );
    }
    final touch = context.platformVariant.touch;
    final size = touch ? 44.0 : 28.0;
    const gap = 4.0;
    final presets = background ? themeBackgrounds : themeAccents;
    final current = background && selected.isEmpty ? resolvedColor : selected;
    Widget swatch(String hex, {bool custom = false, bool reset = false}) {
      final isSelected =
          !reset &&
          current.isNotEmpty &&
          (custom ? !presets.containsKey(current) : current == hex);
      final label = reset
          ? '恢复默认'
          : custom
          ? _chooseLabel
          : background
          ? '使用背景色 $hex'
          : AppStrings.useColor(hex);
      final swatchColor = parseAccentColor(hex);
      return FTooltip(
        tipBuilder: (_, _) => Text(label),
        child: Semantics(
          selected: reset ? null : isSelected,
          label: label,
          child: FTappable(
            focusedOutlineStyle: FFocusedOutlineStyleDelta.delta(
              borderRadius: BorderRadius.circular(8),
            ),
            onPress:
                !controller.canSaveTheme ||
                    !enabled ||
                    (reset && selected.isEmpty)
                ? null
                : () => custom ? _pick(context, hex) : _save(reset ? '' : hex),
            child: SizedBox(
              width: size,
              height: size,
              child: Center(
                child: reset
                    ? Icon(
                        FLucideIcons.rotateCcw,
                        size: 16,
                        color: AppPalette.of(context).ink
                            .withValues(alpha: selected.isEmpty ? .40 : .82),
                      )
                    : AnimatedContainer(
                        duration: MediaQuery.disableAnimationsOf(context)
                            ? Duration.zero
                            : const Duration(milliseconds: 150),
                        decoration: BoxDecoration(
                          borderRadius: BorderRadius.circular(4),
                          boxShadow: isSelected
                              ? [
                                  BoxShadow(
                                    color: swatchColor.withValues(alpha: .65),
                                    blurRadius: 6,
                                    offset: const Offset(0, 2),
                                  ),
                                ]
                              : const [],
                        ),
                        child: ThemeColorSwatch(
                          value: hex,
                          size: touch ? 26 : 24,
                          child: custom || isSelected
                              ? Icon(
                                  custom
                                      ? FLucideIcons.pipette
                                      : FLucideIcons.check,
                                  size: 14,
                                  color: swatchColor.computeLuminance() > .179
                                      ? Colors.black
                                      : Colors.white,
                                )
                              : null,
                        ),
                      ),
              ),
            ),
          ),
        ),
      );
    }

    return SettingRow(
      background ? '背景色' : AppStrings.accentColor,
      controlAlignment: Alignment.centerLeft,
      child: LayoutBuilder(
        builder: (context, constraints) {
          final tiles = [
            for (final hex in presets.keys) swatch(hex),
            swatch(
              (background && value == null) || current.isEmpty
                  ? resolvedColor
                  : current,
              custom: true,
            ),
            if (background) swatch(resolvedColor, reset: true),
          ];
          final count = tiles.length;
          final width = ((themeAccents.length + 1) * (size + 2 * gap) - 2 * gap)
              .clamp(0.0, constraints.maxWidth);
          final capacity = ((width + gap) / (size + gap)).floor().clamp(
            1,
            count,
          );
          final rows = (count / capacity).ceil();
          final columns = (count / rows).ceil();
          return Align(
            alignment: Alignment.centerLeft,
            child: SizedBox(
              width: width,
              child: Column(
                spacing: gap,
                children: [
                  for (var start = 0; start < count; start += columns)
                    Row(
                      mainAxisAlignment: MainAxisAlignment.spaceBetween,
                      children: tiles.skip(start).take(columns).toList(),
                    ),
                ],
              ),
            ),
          );
        },
      ),
    );
  }
}

class BackgroundPicker extends AccentPicker {
  const BackgroundPicker({
    super.key,
    required super.controller,
    super.onPreview,
    super.value,
    super.onChanged,
    super.enabled,
  }) : super(background: true);
}

class _ColorDialog extends StatefulWidget {
  const _ColorDialog({
    required this.initial,
    required this.animation,
    required this.title,
    this.onPreview,
  });
  final String initial;
  final Animation<double> animation;
  final String title;
  final ValueChanged<String>? onPreview;
  @override
  State<_ColorDialog> createState() => _ColorDialogState();
}

class _ColorDialogState extends State<_ColorDialog> {
  late HSVColor _color = HSVColor.fromColor(parseAccentColor(widget.initial));
  late final _text = TextEditingController(text: _hex(_color.toColor()));
  bool _valid = true;
  void _update(HSVColor color) => setState(() {
    _color = color;
    _text.text = _hex(color.toColor());
    _valid = true;
    widget.onPreview?.call(_text.text);
  });
  @override
  void dispose() {
    _text.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => AppDialog(
    title: widget.title,
    animation: widget.animation,
    actions: [
      ActionButton(AppStrings.cancel, onPress: () => Navigator.pop(context)),
      ActionButton(
        AppStrings.apply,
        primary: true,
        onPress: _valid
            ? () => Navigator.pop(context, _hex(_color.toColor()))
            : null,
      ),
    ],
    child: Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        LayoutBuilder(
          builder: (context, constraints) {
            void pick(Offset position) => _update(
              _color
                  .withSaturation(
                    (position.dx / constraints.maxWidth).clamp(0, 1),
                  )
                  .withValue((1 - position.dy / 140).clamp(0, 1)),
            );
            return Semantics(
              label: widget.title,
              child: GestureDetector(
                onPanDown: (details) => pick(details.localPosition),
                onPanUpdate: (details) => pick(details.localPosition),
                child: SizedBox(
                  height: 140,
                  width: double.infinity,
                  child: Stack(
                    children: [
                      Positioned.fill(
                        child: DecoratedBox(
                          decoration: BoxDecoration(
                            gradient: LinearGradient(
                              colors: [
                                Colors.white,
                                HSVColor.fromAHSV(
                                  1,
                                  _color.hue,
                                  1,
                                  1,
                                ).toColor(),
                              ],
                            ),
                          ),
                        ),
                      ),
                      const Positioned.fill(
                        child: DecoratedBox(
                          decoration: BoxDecoration(
                            gradient: LinearGradient(
                              begin: Alignment.topCenter,
                              end: Alignment.bottomCenter,
                              colors: [Colors.transparent, Colors.black],
                            ),
                          ),
                        ),
                      ),
                      Positioned(
                        left: (_color.saturation * constraints.maxWidth - 6)
                            .clamp(0, constraints.maxWidth - 12),
                        top: ((1 - _color.value) * 140 - 6).clamp(0, 128),
                        child: Container(
                          width: 12,
                          height: 12,
                          decoration: BoxDecoration(
                            shape: BoxShape.circle,
                            border: Border.all(color: Colors.white, width: 2),
                          ),
                        ),
                      ),
                    ],
                  ),
                ),
              ),
            );
          },
        ),
        const SizedBox(height: 16),
        FSlider(
          control: FSliderControl.liftedContinuous(
            value: FSliderValue(max: _color.hue / 360),
            onChange: (value) => _update(_color.withHue(value.max * 360)),
          ),
          tooltipBuilder: (_, value) => Text('${(value * 360).round()}°'),
          semanticValueFormatterCallback: (value) =>
              '${(value * 360).round()}°',
        ),
        const SizedBox(height: 16),
        FSlider(
          control: FSliderControl.liftedContinuous(
            value: FSliderValue(max: _color.alpha),
            onChange: (value) => _update(_color.withAlpha(value.max)),
          ),
          tooltipBuilder: (_, value) => Text('${(value * 100).round()}%'),
          semanticValueFormatterCallback: (value) =>
              '不透明度 ${(value * 100).round()}%',
        ),
        const SizedBox(height: 16),
        FTextField(
          label: const Text('HEX'),
          control: FTextFieldControl.managed(
            controller: _text,
            onChange: (value) => setState(() {
              _valid = RegExp(
                r'^(transparent|#(?:[0-9a-fA-F]{3}|[0-9a-fA-F]{6}|[0-9a-fA-F]{8}))$',
              ).hasMatch(value.text);
              if (_valid) {
                _color = HSVColor.fromColor(parseAccentColor(value.text));
                widget.onPreview?.call(_hex(_color.toColor()));
              }
            }),
          ),
          maxLength: 11,
          onSubmit: (_) {
            if (_valid) Navigator.pop(context, _hex(_color.toColor()));
          },
        ),
      ],
    ),
  );
}
