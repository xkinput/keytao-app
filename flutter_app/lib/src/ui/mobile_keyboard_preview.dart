import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:forui/forui.dart';

class MobileKeyboardPreview extends StatelessWidget {
  const MobileKeyboardPreview({
    super.key,
    required this.theme,
    required this.schemeName,
    this.logo,
    this.candidates = const [],
    this.editorPreview = false,
  });

  final Map<String, dynamic> theme;
  final String schemeName;
  final Widget? logo;
  final List<({String text, String comment})> candidates;
  final bool editorPreview;

  static const _letterRows = [
    ('qwertyuiop', '1234567890'),
    ('asdfghjkl', r'@#$%&*()_'),
  ];
  static const _candidates = [
    (text: '这', comment: 'zhè'),
    (text: '之', comment: 'zhī'),
    (text: '支', comment: 'zhī'),
    (text: '只能', comment: 'zhǐ néng'),
  ];
  static const _width = 400.0;
  static const _candidateBarHeight = 60.0;
  static const _keyHeight = 54.0;
  static const _horizontalGap = 4.0;
  static const _verticalGap = (_keyboardHeight - _keyHeight * 4) / 5;
  static const _height =
      _candidateBarHeight + 3 * _keyHeight + 4 * _verticalGap;

  double _metric(Map<String, dynamic> section, String name) =>
      (section[name] as num).toDouble();

  Color _color(Map<String, dynamic> section, String key) {
    final value = section[key] as Map<String, dynamic>;
    return Color.fromARGB(
      value['alpha'] as int,
      value['red'] as int,
      value['green'] as int,
      value['blue'] as int,
    );
  }

  FontWeight _weight(dynamic value) => switch (value) {
    'ultralight' => FontWeight.w100,
    'thin' => FontWeight.w200,
    'light' => FontWeight.w300,
    'medium' => FontWeight.w500,
    'semibold' => FontWeight.w600,
    'bold' => FontWeight.w700,
    'heavy' => FontWeight.w800,
    'black' => FontWeight.w900,
    _ => FontWeight.w400,
  };

  @override
  Widget build(BuildContext context) {
    if (!editorPreview) return _buildHomePreview(context);
    final mobile = theme['mobile'] as Map<String, dynamic>;
    final keyboard = mobile['keyboard'] as Map<String, dynamic>;
    final candidateBar = mobile['candidateBar'] as Map<String, dynamic>;
    final types = keyboard['keyTypes'] as Map<String, dynamic>;
    final classes = keyboard['keyClasses'] as Map<String, dynamic>;
    final baseKey = keyboard['key'] as Map<String, dynamic>;
    final character = {
      ...baseKey,
      ...types['character'] as Map<String, dynamic>,
    };
    final function = {...baseKey, ...types['function'] as Map<String, dynamic>};
    final space = {...function, ...types['space'] as Map<String, dynamic>};
    final enter = {...function, ...types['enter'] as Map<String, dynamic>};
    final accent = {...character, ...classes['accent'] as Map<String, dynamic>};
    TextStyle textStyle(double size, Color color, FontWeight weight) =>
        TextStyle(fontSize: size, fontWeight: weight, height: 1, color: color);

    Widget key(
      String label,
      Map<String, dynamic> style, {
      String? hint,
      int flex = 100,
      IconData? icon,
    }) {
      final fontScale = _metric(style, 'fontScale');
      final foreground = _color(style, 'foreground');
      return Expanded(
        flex: flex,
        child: Container(
          key: ValueKey('keyboard-preview-key-$label'),
          height: _keyHeight,
          decoration: BoxDecoration(
            color: _color(style, 'background'),
            borderRadius: BorderRadius.circular(_metric(style, 'cornerRadius')),
            border: _metric(style, 'borderWidth') > 0
                ? Border.all(
                    color: _color(style, 'borderColor'),
                    width: _metric(style, 'borderWidth'),
                  )
                : null,
            boxShadow: [
              BoxShadow(
                color: _color(style, 'shadowColor'),
                offset: Offset(0, _metric(style, 'shadowOffsetY')),
              ),
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
                              (label.length > 2 ? 14 : 20) * fontScale,
                              foreground,
                              _weight(style['fontWeight']),
                            ),
                          ),
                        ),
                      )
                    : Icon(icon, color: foreground, size: 24 * fontScale),
              ),
              if (hint != null)
                Positioned(
                  right: 7,
                  top: 4,
                  child: Text(
                    hint,
                    textScaler: TextScaler.noScaling,
                    style: textStyle(
                      12 * _metric(style, 'hintFontScale'),
                      _color(style, 'hintForeground'),
                      _weight(style['fontWeight']),
                    ),
                  ),
                ),
            ],
          ),
        ),
      );
    }

    final previewCandidates = (candidates.isEmpty ? _candidates : candidates)
        .take(4)
        .toList();
    final candidateScale = _metric(candidateBar, 'fontScale');
    final showComment = candidateBar['showComment'] as bool;
    final candidateWeight = _weight(theme['font']['weight']);
    return Semantics(
      container: true,
      label: 'KeyTao 键盘预览，$schemeName，静态展示',
      child: ExcludeSemantics(
        child: IgnorePointer(
          child: AspectRatio(
            aspectRatio: _width / _height,
            child: FittedBox(
              fit: BoxFit.contain,
              child: Container(
                key: const ValueKey('mobile-keyboard-preview'),
                width: _width,
                height: _height,
                clipBehavior: Clip.antiAlias,
                decoration: BoxDecoration(
                  color: _color(keyboard, 'background'),
                ),
                child: Column(
                  children: [
                    Container(
                      key: const ValueKey('mobile-candidate-bar-preview'),
                      height: _candidateBarHeight,
                      color: _color(candidateBar, 'background'),
                      child: Row(
                        children: [
                          if (logo != null)
                            Padding(
                              padding: const EdgeInsets.symmetric(
                                horizontal: 6,
                              ),
                              child: SizedBox.square(
                                dimension: 28,
                                child: logo,
                              ),
                            ),
                          for (
                            var index = 0;
                            index < previewCandidates.length;
                            index++
                          )
                            Expanded(
                              child: Container(
                                height: _candidateBarHeight,
                                color: index == 0
                                    ? _color(candidateBar, 'selectedBackground')
                                    : null,
                                padding: const EdgeInsets.symmetric(
                                  horizontal: 4,
                                ),
                                child: Column(
                                  mainAxisAlignment: MainAxisAlignment.center,
                                  children: [
                                    Text(
                                      previewCandidates[index].text,
                                      maxLines: 1,
                                      overflow: TextOverflow.ellipsis,
                                      textScaler: TextScaler.noScaling,
                                      style: textStyle(
                                        20 * candidateScale,
                                        _color(
                                          candidateBar,
                                          index == 0
                                              ? 'selectedForeground'
                                              : 'foreground',
                                        ),
                                        candidateWeight,
                                      ),
                                    ),
                                    if (showComment &&
                                        previewCandidates[index]
                                            .comment
                                            .isNotEmpty) ...[
                                      const SizedBox(height: 3),
                                      Text(
                                        previewCandidates[index].comment,
                                        maxLines: 1,
                                        overflow: TextOverflow.ellipsis,
                                        textScaler: TextScaler.noScaling,
                                        style: textStyle(
                                          12 * candidateScale,
                                          _color(candidateBar, 'commentColor'),
                                          candidateWeight,
                                        ),
                                      ),
                                    ],
                                  ],
                                ),
                              ),
                            ),
                        ],
                      ),
                    ),
                    const SizedBox(height: _verticalGap),
                    for (var row = 0; row < _letterRows.length; row++) ...[
                      Padding(
                        padding: EdgeInsets.symmetric(
                          horizontal: row == 1 ? 24 : 5,
                        ),
                        child: Row(
                          spacing: _horizontalGap,
                          children: [
                            for (var i = 0; i < _letterRows[row].$1.length; i++)
                              key(
                                _letterRows[row].$1[i],
                                character,
                                hint: _letterRows[row].$2[i],
                              ),
                          ],
                        ),
                      ),
                      const SizedBox(height: _verticalGap),
                    ],
                    Padding(
                      padding: const EdgeInsets.symmetric(horizontal: 5),
                      child: Row(
                        spacing: _horizontalGap,
                        children: [
                          key('123', function, flex: 115),
                          key('符', accent),
                          key('空格', space, flex: 360),
                          key(
                            '↵',
                            enter,
                            flex: 145,
                            icon: FLucideIcons.cornerDownLeft,
                          ),
                        ],
                      ),
                    ),
                  ],
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }

  static const _toolbarHeight = 52.0;
  static const _keyboardHeight = 266.0;
  static const _previewRows = 2;
  static const _previewHeight =
      _verticalGap + _previewRows * (_keyHeight + _verticalGap) + 2;

  Widget _buildHomePreview(BuildContext context) {
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
