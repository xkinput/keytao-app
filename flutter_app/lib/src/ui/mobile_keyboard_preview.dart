import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:forui/forui.dart';

class MobileKeyboardPreview extends StatelessWidget {
  const MobileKeyboardPreview({
    super.key,
    required this.theme,
    required this.schemeName,
    this.logo,
  });

  final Map<String, dynamic> theme;
  final String schemeName;
  final Widget? logo;

  // Layout mirrors crates/keytao-theme/default-keyboard.yaml; surfaces mirror
  // Android KeytaoKeyboardView/KeytaoTheme.kt and iOS KeyTaoIOSKeyboardView/KeyTaoIOSTheme.swift.
  // Both platforms use the resolved core theme supplied by SettingsCard.
  static const _letterRows = [
    ('qwertyuiop', '1234567890'),
    ('asdfghjkl', r'@#$%&*()_'),
    ('zxcvbnm', '-/:;"\'='),
  ];
  static const _width = 400.0;
  static const _toolbarHeight = 52.0;
  static const _keyboardHeight = 266.0;
  // The preview shows only the top two letter rows; the full keyboard is in the IME.
  static const _previewRows = 2;
  static const _previewHeight =
      _verticalGap + _previewRows * (_keyHeight + _verticalGap) + 2; // + border
  static const _keyHeight = 54.0;
  static const _horizontalGap = 4.0;
  static const _verticalGap = (_keyboardHeight - _keyHeight * 4) / 5;

  Color _color(Map<String, dynamic> section, String key) {
    final value = section[key] as Map<String, dynamic>;
    return Color.fromARGB(
      value['alpha'] as int,
      value['red'] as int,
      value['green'] as int,
      value['blue'] as int,
    );
  }

  @override
  Widget build(BuildContext context) {
    final ui = theme['ui'] as Map<String, dynamic>;
    final panel = theme['panel'] as Map<String, dynamic>;
    final candidate = theme['candidate'] as Map<String, dynamic>;
    final font = theme['font'] as Map<String, dynamic>;
    final panelColor = _color(panel, 'background');
    final dark =
        (panelColor.r * 299 + panelColor.g * 587 + panelColor.b * 114) <
        128 / 255 * 1000;
    final accent = _color(ui, 'accentColor');
    final background = Color.lerp(panelColor, accent, .07)!;
    final foreground = _color(candidate, 'foreground');
    final hintColor = _color(candidate, 'commentColor');
    final radius = (candidate['cornerRadius'] as num).toDouble() * 10 / 11;
    final configuredKey = _color(candidate, 'background');
    final keySurface = configuredKey.a > 0
        ? configuredKey
        : dark
        ? const Color(0xAA2A303A)
        : const Color(0xD2FFFFFF);
    final accentSurface = Color.lerp(background, accent, .22)!;
    final accentBorder = accent.withValues(alpha: dark ? .42 : .60);
    final keyBorder = dark ? const Color(0x16FFFFFF) : const Color(0x1C1A222C);
    final fontSize = (font['size'] as num).toDouble();
    final labelSize = math
        .min((font['labelSize'] as num).toDouble(), math.min(fontSize - 4, 16))
        .clamp(12.0, 16.0)
        .toDouble();
    final hintSize = math
        .min(
          (font['commentSize'] as num).toDouble() - 1,
          math.min(labelSize - 2, 13),
        )
        .clamp(10.0, 14.0)
        .toDouble();
    TextStyle textStyle(double size, Color color) => TextStyle(
      fontFamily: font['family'] as String?,
      fontSize: size,
      fontWeight: FontWeight.w500,
      height: 1,
      color: color,
    );

    Widget key(
      String label, {
      String? hint,
      int weight = 100,
      bool accented = false,
      IconData? icon,
    }) => Expanded(
      flex: weight,
      child: Container(
        key: ValueKey('keyboard-preview-key-$label'),
        height: _keyHeight,
        decoration: BoxDecoration(
          color: accented ? accentSurface : keySurface,
          borderRadius: BorderRadius.circular(radius),
          border: Border.all(color: accented ? accentBorder : keyBorder),
          boxShadow: const [
            BoxShadow(color: Color(0x1C1A222C), offset: Offset(0, 1.6)),
          ],
        ),
        child: Stack(
          children: [
            Center(
              child: icon == null
                  ? Padding(
                      padding: const EdgeInsets.symmetric(horizontal: 4),
                      child: FittedBox(
                        fit: BoxFit.scaleDown,
                        child: Text(
                          label,
                          textScaler: TextScaler.noScaling,
                          style: textStyle(
                            label.length > 2 ? labelSize : fontSize,
                            foreground,
                          ),
                        ),
                      ),
                    )
                  : Icon(icon, color: foreground, size: 24),
            ),
            if (hint != null)
              Positioned(
                right: 7,
                top: 4,
                child: Text(
                  hint,
                  textScaler: TextScaler.noScaling,
                  style: textStyle(hintSize, hintColor),
                ),
              ),
          ],
        ),
      ),
    );

    Widget toolbarItem(Widget child, {bool accented = true, int flex = 10}) =>
        Expanded(
          flex: flex,
          child: Container(
            height: 36,
            alignment: Alignment.center,
            decoration: BoxDecoration(
              color: accented
                  ? Color.lerp(background, accent, .13)
                  : keySurface,
              border: accented ? Border.all(color: accentBorder) : null,
              borderRadius: BorderRadius.circular(radius),
            ),
            child: child,
          ),
        );
    Widget toolbarIcon(IconData icon, {bool accented = true}) => toolbarItem(
      Icon(icon, size: 22, color: foreground),
      accented: accented,
    );

    return Semantics(
      container: true,
      label: 'KeyTao 键盘预览，$schemeName，静态展示',
      // Fade the cut-off rows out so the preview reads as a glimpse of the keyboard.
      child: ShaderMask(
        blendMode: BlendMode.dstIn,
        shaderCallback: (bounds) => const LinearGradient(
          begin: Alignment.topCenter,
          end: Alignment.bottomCenter,
          colors: [Color(0xFFFFFFFF), Color(0xFFFFFFFF), Color(0x00FFFFFF)],
          stops: [0, 0.55, 1],
        ).createShader(bounds),
        child: ExcludeSemantics(
          child: IgnorePointer(
            child: AspectRatio(
              aspectRatio: _width / (_toolbarHeight + _previewHeight),
              child: FittedBox(
                fit: BoxFit.contain,
                child: Container(
                  key: const ValueKey('mobile-keyboard-preview'),
                  width: _width,
                  height: _toolbarHeight + _previewHeight,
                  clipBehavior: Clip.antiAlias,
                  decoration: BoxDecoration(
                    color: background,
                    border: Border.all(color: _color(panel, 'borderColor')),
                    borderRadius: BorderRadius.circular(
                      (panel['cornerRadius'] as num).toDouble(),
                    ),
                  ),
                  child: Column(
                    children: [
                      SizedBox(
                        height: _toolbarHeight,
                        child: Padding(
                          padding: const EdgeInsets.symmetric(horizontal: 8),
                          child: Row(
                            spacing: 6,
                            children: [
                              toolbarIcon(FLucideIcons.grid2x2),
                              toolbarItem(
                                Text(
                                  '中 En',
                                  textScaler: TextScaler.noScaling,
                                  style: textStyle(14, foreground),
                                ),
                                flex: 14,
                              ),
                              toolbarIcon(FLucideIcons.mousePointer2),
                              toolbarIcon(FLucideIcons.clipboard),
                              toolbarIcon(FLucideIcons.smile),
                              toolbarIcon(
                                FLucideIcons.panelLeft,
                                accented: false,
                              ),
                              toolbarIcon(
                                FLucideIcons.ellipsis,
                                accented: false,
                              ),
                              if (logo != null)
                                SizedBox.square(dimension: 28, child: logo),
                            ],
                          ),
                        ),
                      ),
                      const SizedBox(height: _verticalGap),
                      for (var row = 0; row < _previewRows; row++) ...[
                        Padding(
                          padding: EdgeInsets.symmetric(
                            horizontal: row == 1 ? 24 : 5,
                          ),
                          child: Row(
                            spacing: _horizontalGap,
                            children: [
                              if (row == 2)
                                key(
                                  '⇧',
                                  weight: 130,
                                  icon: FLucideIcons.arrowBigUp,
                                ),
                              for (
                                var i = 0;
                                i < _letterRows[row].$1.length;
                                i++
                              )
                                key(
                                  _letterRows[row].$1[i],
                                  hint: _letterRows[row].$2[i],
                                ),
                              if (row == 2)
                                key(
                                  '⌫',
                                  weight: 130,
                                  accented: true,
                                  icon: FLucideIcons.delete,
                                ),
                            ],
                          ),
                        ),
                        const SizedBox(height: _verticalGap),
                      ],
                    ],
                  ),
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}
