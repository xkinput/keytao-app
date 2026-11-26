import 'dart:convert';
import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:forui/forui.dart';

import '../rust/api/types.dart';

Border? themePreviewBorder(bool highlighted) =>
    highlighted ? Border.all(color: const Color(0xFFE02020), width: 0.5) : null;

Widget themePreviewOutline(Widget child, bool highlighted) => DecoratedBox(
  position: DecorationPosition.foreground,
  decoration: BoxDecoration(border: themePreviewBorder(highlighted)),
  child: child,
);

CustomPainter? themePreviewCandidateIndicator(Object? color, double paddingX) {
  if (color is! Map<String, dynamic> || color['alpha'] == 0 || paddingX <= 0) {
    return null;
  }
  return _CandidateIndicatorPainter(
    Color.fromARGB(
      color['alpha'] as int,
      color['red'] as int,
      color['green'] as int,
      color['blue'] as int,
    ),
    paddingX,
  );
}

class _CandidateIndicatorPainter extends CustomPainter {
  const _CandidateIndicatorPainter(this.color, this.paddingX);

  final Color color;
  final double paddingX;

  @override
  void paint(Canvas canvas, Size size) {
    final width = math.min(3.0, math.min(paddingX, size.width / 2) / 3);
    final height = math.min(20.0, size.height * 0.55);
    final mark = Rect.fromLTWH(
      width,
      (size.height - height) / 2,
      width,
      height,
    );
    canvas.drawRRect(
      RRect.fromRectAndRadius(mark, Radius.circular(width / 2)),
      Paint()..color = color,
    );
  }

  @override
  bool shouldRepaint(_CandidateIndicatorPainter oldDelegate) =>
      color != oldDelegate.color || paddingX != oldDelegate.paddingX;
}

class MobileKeyboardPreview extends StatelessWidget {
  const MobileKeyboardPreview({
    super.key,
    required this.theme,
    required this.schemeName,
    this.logo,
    this.candidates = const [],
    this.showCandidates = false,
    this.editorPreview = false,
    this.showToolbar = true,
    this.fullHeight = false,
    this.layoutJsonOverride,
    this.mnemonicHintsOverride,
    this.fontFamily,
    this.highlightPath,
    this.keyboardSettings,
    this.onEditField,
  });

  final Map<String, dynamic> theme;
  final String schemeName;
  final Widget? logo;
  final List<({String text, String comment})> candidates;
  final bool showCandidates;
  final bool editorPreview;
  final bool showToolbar;
  final bool fullHeight;
  final String? layoutJsonOverride;
  final bool? mnemonicHintsOverride;
  final String? fontFamily;
  final String? highlightPath;
  final AndroidImeInputSettingsDto? keyboardSettings;
  final ValueChanged<String>? onEditField;

  static const _letterRows = [
    ('qwertyuiop', '1234567890'),
    ('asdfghjkl', r'@#$%&*()_'),
    ('zxcvbnm', "-/:;\"'="),
  ];
  static const _candidates = [
    (text: '这', comment: 'zhè'),
    (text: '之', comment: 'zhī'),
    (text: '支', comment: 'zhī'),
    (text: '只能', comment: 'zhǐ néng'),
  ];
  static const _width = 400.0;
  static const _punctuation = {'，', '。', ',', '.'};
  static final _layoutCache = Expando<Map<String, dynamic>>();
  static final _overrideLayoutCache = <String, Map<String, dynamic>>{};

  static final _bundledRows = <List<_PreviewKey>>[
    for (final (index, (labels, hints)) in _letterRows.indexed)
      [
        if (index == 2) const _PreviewKey('⇧', action: 'shift', weight: 1.3),
        for (final (index, label) in labels.split('').indexed)
          _PreviewKey(label, hint: hints[index]),
        if (index == 2)
          const _PreviewKey('⌫', action: 'backspace', weight: 1.3),
      ],
    const [
      _PreviewKey('123', action: 'keyboardMode', weight: 1.15),
      _PreviewKey('，', hint: '!'),
      _PreviewKey('空格', action: 'space', weight: 4.6),
      _PreviewKey('。', hint: '?'),
      _PreviewKey('↵', action: 'enter', weight: 1.45),
    ],
  ];

  Map<String, dynamic>? _layout() {
    final override = layoutJsonOverride;
    if (override != null && override.isNotEmpty) {
      return _overrideLayoutCache[override] ??=
          jsonDecode(override) as Map<String, dynamic>;
    }
    final settings = keyboardSettings;
    final json = settings?.keyboardLayoutJson;
    if (json == null || json.isEmpty) return null;
    return _layoutCache[settings!] ??= jsonDecode(json) as Map<String, dynamic>;
  }

  Color? _parseHexColor(String? value) {
    if (value == null) return null;
    final hex = value.trim().replaceFirst('#', '');
    if (hex.length != 6 && hex.length != 8) return null;
    final parsed = int.tryParse(hex, radix: 16);
    if (parsed == null) return null;
    if (hex.length == 8) {
      final r = (parsed >> 24) & 0xFF;
      final g = (parsed >> 16) & 0xFF;
      final b = (parsed >> 8) & 0xFF;
      final a = parsed & 0xFF;
      return Color.fromARGB(a, r, g, b);
    }
    return Color(0xFF000000 | parsed);
  }

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
    final mobile = theme['mobile'] as Map<String, dynamic>;
    final keyboard = mobile['keyboard'] as Map<String, dynamic>;
    final candidateBar = mobile['candidateBar'] as Map<String, dynamic>;
    final toolbar = keyboard['toolbar'] as Map<String, dynamic>;
    final types = keyboard['keyTypes'] as Map<String, dynamic>;
    final classes = keyboard['keyClasses'] as Map<String, dynamic>;
    final baseKey = keyboard['key'] as Map<String, dynamic>;
    final layout = _layout();
    final isDarkTheme =
        (theme['ui'] as Map<String, dynamic>?)?['effectiveColorScheme'] ==
        'dark';
    final rawKeyGroups =
        (layout?['keyGroups'] as Map<String, dynamic>?) ?? const {};
    final layoutHasMnemonics = layout?['hasMnemonics'] == true;
    final showMnemonics =
        layoutHasMnemonics &&
        (mnemonicHintsOverride ??
            keyboardSettings?.mnemonicHintsEnabled ??
            false);
    final numberRowMode = (layout?['numberRow'] as String?) ?? 'auto';
    final nativeRows = layout?['rows'] as List<dynamic>?;
    final letterRows = nativeRows == null || nativeRows.isEmpty
        ? _bundledRows
        : [
            for (final row in nativeRows.cast<List<dynamic>>())
              row
                  .cast<Map<String, dynamic>>()
                  .map(_PreviewKey.fromJson)
                  .toList(),
          ];
    final includeNumberRow =
        showToolbar &&
        numberRowMode == 'auto' &&
        keyboardSettings?.numberRowEnabled == true;
    final rows = [
      if (includeNumberRow)
        [
          for (var index = 0; index < 10; index++)
            _PreviewKey('1234567890'[index], hint: r'!@#$%^&*()'[index]),
        ],
      ...letterRows,
    ];
    final barHeight = showToolbar
        ? (keyboardSettings?.candidateBarHeightDp.toDouble() ?? 52.0)
        : 0.0;
    final heightScale = showToolbar
        ? (keyboardSettings?.keyboardHeightScale ?? 100) / 100
        : 1.0;
    final baseHeight =
        (layout?['height'] as num?)?.toDouble() ??
        keyboardSettings?.keyboardHeightDp.toDouble() ??
        266.0;
    final configuredHeight =
        baseHeight * heightScale * rows.length / math.max(1, letterRows.length);
    final outerInset = (layout?['outerInset'] as num?)?.toDouble() ?? 5.0;
    final gapFloor = (layout?['verticalGap'] as num?)?.toDouble() ?? 5.0;
    final maxKeyHeight = (layout?['maxKeyHeight'] as num?)?.toDouble() ?? 54.0;
    final keyHeight = math.min(
      maxKeyHeight * heightScale,
      math.max(
        36.0,
        (configuredHeight - gapFloor * (rows.length + 1)) / rows.length,
      ),
    );
    final verticalGap = math.max(
      gapFloor,
      (configuredHeight - keyHeight * rows.length) / (rows.length + 1),
    );
    final height =
        barHeight + keyHeight * rows.length + verticalGap * (rows.length + 1);
    final fadeStart = barHeight + keyHeight + 2 * verticalGap;
    final showFull = editorPreview || fullHeight;
    final visibleHeight = showFull
        ? height
        : math.min(height, fadeStart + keyHeight);
    double rowWeight(List<_PreviewKey> row) =>
        row.fold(0.0, (weight, key) => weight + key.weight);
    final referenceRow = rows.reduce((a, b) {
      if (a.length != b.length) return a.length > b.length ? a : b;
      return rowWeight(a) >= rowWeight(b) ? a : b;
    });
    final maximumRowWidth = _width - 2 * outerInset;
    final requestedGap = (layout?['horizontalGap'] as num?)?.toDouble() ?? 4.0;
    final horizontalGap = math.min(
      requestedGap,
      maximumRowWidth / math.max(1, 2 * (referenceRow.length - 1)),
    );
    final referenceUnitWidth =
        (maximumRowWidth - horizontalGap * (referenceRow.length - 1)) /
        rowWeight(referenceRow);
    final path = highlightPath ?? '';
    const toolbarPath = 'mobile.keyboard.toolbar.';
    const candidatePath = 'mobile.candidateBar.';
    final toolbarField = path.startsWith(toolbarPath)
        ? path.substring(toolbarPath.length)
        : null;
    final candidateField = path.startsWith(candidatePath)
        ? path.substring(candidatePath.length)
        : null;
    final surfaceShadow = [
      BoxShadow(
        color: _color(baseKey, 'shadowColor'),
        offset: Offset(0, _metric(baseKey, 'shadowOffsetY')),
      ),
    ];
    Widget edit(Widget child, String field, String label) {
      final callback = onEditField;
      if (callback == null) return child;
      return Semantics(
        button: true,
        label: label,
        child: GestureDetector(
          behavior: HitTestBehavior.opaque,
          onTap: () => callback(field),
          child: child,
        ),
      );
    }

    TextStyle textStyle(double size, Color color, FontWeight weight) =>
        TextStyle(
          fontFamily: fontFamily ?? theme['font']['effectiveFamily'] as String?,
          fontSize: size,
          fontWeight: weight,
          height: 1,
          color: color,
        );

    Map<String, dynamic>? resolveGroup(String? groupName) {
      if (groupName == null || groupName.isEmpty) return null;
      final group = rawKeyGroups[groupName];
      if (group is! Map<String, dynamic>) return null;
      final preferred = group[isDarkTheme ? 'dark' : 'light'];
      if (preferred is Map<String, dynamic>) return preferred;
      final fallback = group[isDarkTheme ? 'light' : 'dark'];
      return fallback is Map<String, dynamic> ? fallback : null;
    }

    Widget key(
      _PreviewKey spec, {
      required double width,
      double? heightOverride,
      bool accentSample = false,
    }) {
      final effectiveHeight = heightOverride ?? keyHeight;
      final action = spec.action;
      final label = switch (action) {
        'space' when schemeName.isNotEmpty => schemeName,
        'mode' => theme['modeHint']['chineseText'] as String? ?? spec.label,
        _ => spec.label,
      };
      final activeMnemonic = showMnemonics ? spec.mnemonic : null;
      final groupStyle = resolveGroup(activeMnemonic?.group ?? spec.group);
      final hint = keyboardSettings?.keyHintVisible == false
          ? null
          : (spec.slots?.topRight ?? spec.hint);
      final function = const {
        'mode',
        'keyboardMode',
        'space',
        'enter',
        'backspace',
      }.contains(action);
      final punctuation = _punctuation.contains(spec.label);
      final icon = switch (action) {
        'enter' => FLucideIcons.cornerDownLeft,
        'backspace' => FLucideIcons.delete,
        'shift' => FLucideIcons.arrowBigUp,
        _ => null,
      };
      final style = {
        ...baseKey,
        ...types[function ? 'function' : 'character'] as Map<String, dynamic>,
        if (const {'space', 'enter', 'backspace', 'shift'}.contains(action))
          ...types[action] as Map<String, dynamic>,
        if (spec.accent || accentSample)
          ...classes['accent'] as Map<String, dynamic>,
        if (punctuation && !accentSample)
          ...classes['accentPunctuation'] as Map<String, dynamic>,
      };
      final fontScale = _metric(style, 'fontScale');
      final bgColor =
          _parseHexColor(groupStyle?['background'] as String?) ??
          _color(style, 'background');
      final foreground =
          _parseHexColor(groupStyle?['foreground'] as String?) ??
          _color(style, 'foreground');
      final hintColor =
          _parseHexColor(groupStyle?['hintForeground'] as String?) ??
          _color(style, 'hintForeground');
      final borderColor =
          _parseHexColor(groupStyle?['borderColor'] as String?) ??
          _color(style, 'borderColor');
      final borderWidth = groupStyle?['borderColor'] != null
          ? math.max(1.0, _metric(style, 'borderWidth'))
          : _metric(style, 'borderWidth');
      if (spec.stack.isNotEmpty) {
        return Container(
          key: ValueKey('keyboard-preview-key-${spec.label}'),
          width: width,
          height: effectiveHeight,
          decoration: BoxDecoration(
            color: bgColor,
            borderRadius: BorderRadius.circular(_metric(style, 'cornerRadius')),
            border: Border.all(
              color: borderColor,
              width: borderWidth,
              style: borderWidth > 0 ? BorderStyle.solid : BorderStyle.none,
            ),
            boxShadow: [
              BoxShadow(
                color: _color(style, 'shadowColor'),
                offset: Offset(0, _metric(style, 'shadowOffsetY')),
              ),
            ],
          ),
          child: Column(
            children: [
              for (final item in spec.stack)
                Expanded(
                  child: Center(
                    child: Text(
                      item,
                      textScaler: TextScaler.noScaling,
                      style: textStyle(
                        15 * fontScale,
                        foreground,
                        _weight(style['fontWeight']),
                      ),
                    ),
                  ),
                ),
            ],
          ),
        );
      }
      final target = switch ((
        punctuation,
        spec.accent || accentSample,
        action,
      )) {
        (true, _, _) when !accentSample =>
          'mobile.keyboard.keyClasses.accentPunctuation.',
        (_, true, _) => 'mobile.keyboard.keyClasses.accent.',
        (_, _, 'space') => 'mobile.keyboard.keyTypes.space.',
        (_, _, 'enter') => 'mobile.keyboard.keyTypes.enter.',
        _ when function => 'mobile.keyboard.keyTypes.function.',
        _ => 'mobile.keyboard.key.',
      };
      final sample =
          spec.label == 'q' ||
          spec.label == '，' ||
          action == 'space' ||
          action == 'enter' ||
          action == 'keyboardMode' ||
          spec.accent ||
          accentSample;
      final field = sample && path.startsWith(target)
          ? path.substring(target.length)
          : null;
      final legend =
          field == 'foreground' ||
          field == 'fontScale' ||
          spec.label == 'q' && path.startsWith('common.font.');
      final hintHighlighted =
          field == 'hintForeground' || field == 'hintFontScale';
      Widget body;
      if (activeMnemonic != null && activeMnemonic.hasText) {
        final topText = activeMnemonic.topText;
        body = Padding(
          padding: const EdgeInsets.symmetric(horizontal: 2, vertical: 2),
          child: Column(
            mainAxisAlignment: MainAxisAlignment.center,
            children: [
              Expanded(
                flex: 6,
                child: Row(
                  mainAxisAlignment: MainAxisAlignment.center,
                  children: [
                    Flexible(
                      child: FittedBox(
                        fit: BoxFit.scaleDown,
                        child: Text(
                          label,
                          textScaler: TextScaler.noScaling,
                          style: textStyle(
                            15 * fontScale,
                            foreground,
                            _weight(style['fontWeight']),
                          ),
                        ),
                      ),
                    ),
                    if (topText != null) ...[
                      const SizedBox(width: 2),
                      Flexible(
                        child: FittedBox(
                          fit: BoxFit.scaleDown,
                          child: Text(
                            topText,
                            textScaler: TextScaler.noScaling,
                            style: textStyle(
                              9 * _metric(style, 'hintFontScale'),
                              hintColor,
                              _weight(style['fontWeight']),
                            ),
                          ),
                        ),
                      ),
                    ],
                  ],
                ),
              ),
              for (final line in activeMnemonic.bottomLines)
                Expanded(
                  flex: 4,
                  child: FittedBox(
                    fit: BoxFit.scaleDown,
                    child: Text(
                      line,
                      textScaler: TextScaler.noScaling,
                      style: textStyle(
                        8.5 * _metric(style, 'hintFontScale'),
                        hintColor,
                        _weight(style['fontWeight']),
                      ),
                    ),
                  ),
                ),
            ],
          ),
        );
      } else if (spec.slots != null && spec.slots!.hasContent) {
        final slots = spec.slots!;
        final mainText = slots.labelSub != null
            ? '$label ${slots.labelSub}'
            : label;
        body = Padding(
          padding: const EdgeInsets.symmetric(horizontal: 3, vertical: 2),
          child: Column(
            children: [
              if (slots.hasTop)
                SizedBox(
                  height: effectiveHeight * 0.24,
                  child: Row(
                    mainAxisAlignment: MainAxisAlignment.spaceBetween,
                    children: [
                      if (slots.topLeft != null)
                        Flexible(
                          child: FittedBox(
                            fit: BoxFit.scaleDown,
                            child: Text(
                              slots.topLeft!,
                              textScaler: TextScaler.noScaling,
                              style: textStyle(8.5, hintColor, FontWeight.w500),
                            ),
                          ),
                        ),
                      if (slots.topCenter != null)
                        Expanded(
                          child: Center(
                            child: FittedBox(
                              fit: BoxFit.scaleDown,
                              child: Text(
                                slots.topCenter!,
                                textScaler: TextScaler.noScaling,
                                style: textStyle(
                                  8.5,
                                  hintColor,
                                  FontWeight.w500,
                                ),
                              ),
                            ),
                          ),
                        ),
                      if (slots.topRight != null)
                        Flexible(
                          child: FittedBox(
                            fit: BoxFit.scaleDown,
                            child: Text(
                              slots.topRight!,
                              textScaler: TextScaler.noScaling,
                              style: textStyle(8.5, hintColor, FontWeight.w500),
                            ),
                          ),
                        ),
                    ],
                  ),
                ),
              Expanded(
                child: Center(
                  child: FittedBox(
                    fit: BoxFit.scaleDown,
                    child: icon == null
                        ? Text(
                            mainText,
                            textScaler: TextScaler.noScaling,
                            style: textStyle(
                              (mainText.length > 2 ? 13 : 17) * fontScale,
                              foreground,
                              _weight(style['fontWeight']),
                            ),
                          )
                        : Icon(icon, color: foreground, size: 20 * fontScale),
                  ),
                ),
              ),
              if (slots.bottomLines.isNotEmpty)
                SizedBox(
                  height: effectiveHeight * 0.26,
                  child: Center(
                    child: FittedBox(
                      fit: BoxFit.scaleDown,
                      child: Text(
                        slots.bottomLines.first,
                        textScaler: TextScaler.noScaling,
                        style: textStyle(8.5, hintColor, FontWeight.w400),
                      ),
                    ),
                  ),
                ),
            ],
          ),
        );
      } else {
        body = Stack(
          children: [
            Center(
              child: edit(
                icon == null
                    ? Padding(
                        padding: const EdgeInsets.symmetric(horizontal: 4),
                        child: FittedBox(
                          fit: BoxFit.scaleDown,
                          child: themePreviewOutline(
                            Text(
                              label,
                              textScaler: TextScaler.noScaling,
                              style: textStyle(
                                (label.length > 2 ? 14 : 20) * fontScale,
                                foreground,
                                _weight(style['fontWeight']),
                              ),
                            ),
                            legend,
                          ),
                        ),
                      )
                    : themePreviewOutline(
                        Icon(icon, color: foreground, size: 24 * fontScale),
                        legend,
                      ),
                '${target}foreground',
                '$label 字色',
              ),
            ),
            if (hint != null)
              Positioned(
                right: 7,
                top: 4,
                child: edit(
                  themePreviewOutline(
                    Text(
                      hint,
                      textScaler: TextScaler.noScaling,
                      style: textStyle(
                        12 * _metric(style, 'hintFontScale'),
                        hintColor,
                        _weight(style['fontWeight']),
                      ),
                    ),
                    hintHighlighted,
                  ),
                  'mobile.keyboard.key.hintForeground',
                  '$label 提示字色',
                ),
              ),
          ],
        );
      }
      return edit(
        Container(
          key: ValueKey('keyboard-preview-key-${spec.label}'),
          width: width,
          height: effectiveHeight,
          decoration: BoxDecoration(
            color: bgColor,
            borderRadius: BorderRadius.circular(_metric(style, 'cornerRadius')),
            border: Border.all(
              color: borderColor,
              width: borderWidth,
              style: borderWidth > 0 ? BorderStyle.solid : BorderStyle.none,
            ),
            boxShadow: [
              BoxShadow(
                color: _color(style, 'shadowColor'),
                offset: Offset(0, _metric(style, 'shadowOffsetY')),
              ),
            ],
          ),
          foregroundDecoration: BoxDecoration(
            border: themePreviewBorder(
              field != null && !legend && !hintHighlighted ||
                  spec.label == 'q' && path == 'common.cornerRadius',
            ),
          ),
          child: body,
        ),
        '${target}background',
        '$label 底色',
      );
    }

    final accentField = path.startsWith('mobile.keyboard.keyClasses.accent.');
    final sampleAccent =
        accentField && !rows.expand((row) => row).any((key) => key.accent);
    Widget keyRow(List<_PreviewKey> row, int index) {
      if (row.isEmpty) return SizedBox(height: keyHeight);
      final gapWidth = horizontalGap * (row.length - 1);
      final weight = rowWeight(row);
      final fillsWidth =
          index == 0 ||
          index == rows.length - 1 ||
          row.length <= 5 ||
          row.any((key) => key.action == 'shift' || key.action == 'backspace');
      final rowWidth = fillsWidth
          ? maximumRowWidth
          : math.min(maximumRowWidth, referenceUnitWidth * weight + gapWidth);
      final unitWidth = (rowWidth - gapWidth) / weight;
      return Row(
        mainAxisAlignment: MainAxisAlignment.center,
        spacing: horizontalGap,
        children: [
          for (final spec in row)
            key(
              spec,
              width: unitWidth * spec.weight,
              accentSample: sampleAccent && spec.label == '，',
            ),
        ],
      );
    }

    Widget buildRowsSurface() {
      final hasSpans = rows.any((row) => row.any((k) => k.rowSpan > 1));
      if (!hasSpans) {
        return Column(
          children: [
            SizedBox(height: verticalGap),
            for (final (index, row) in rows.indexed) ...[
              keyRow(row, index),
              SizedBox(height: verticalGap),
            ],
          ],
        );
      }
      final rowsHeight =
          keyHeight * rows.length + verticalGap * (rows.length + 1);
      final positioned = <Widget>[];
      var activeSpans = <({double weight, int remaining})>[];
      for (var rowIndex = 0; rowIndex < rows.length; rowIndex++) {
        final row = rows[rowIndex];
        final spanCount = activeSpans.length;
        final spanWeight = activeSpans.fold(0.0, (sum, s) => sum + s.weight);
        final totalCount = spanCount + row.length;
        final totalWeight = spanWeight + rowWeight(row);
        final gapWidth = horizontalGap * math.max(0, totalCount - 1);
        final unitWidth = (maximumRowWidth - gapWidth) / math.max(1, totalWeight);
        var x = outerInset;
        for (final span in activeSpans) {
          x += unitWidth * span.weight + horizontalGap;
        }
        final top = verticalGap + rowIndex * (keyHeight + verticalGap);
        final nextSpans = <({double weight, int remaining})>[
          for (final s in activeSpans)
            if (s.remaining > 1) (weight: s.weight, remaining: s.remaining - 1),
        ];
        for (final spec in row) {
          final w = unitWidth * spec.weight;
          final spanRows = math.min(spec.rowSpan, rows.length - rowIndex);
          final h = keyHeight * spanRows + verticalGap * (spanRows - 1);
          positioned.add(
            Positioned(
              left: x,
              top: top,
              child: key(
                spec,
                width: w,
                heightOverride: h,
                accentSample: sampleAccent && spec.label == '，',
              ),
            ),
          );
          if (spanRows > 1) {
            nextSpans.add((weight: spec.weight, remaining: spanRows - 1));
          }
          x += w + horizontalGap;
        }
        activeSpans = nextSpans;
      }
      return SizedBox(
        width: _width,
        height: rowsHeight,
        child: Stack(children: positioned),
      );
    }

    Widget tile(
      Widget child, {
      bool active = false,
      String? background,
      bool highlighted = false,
    }) => edit(
      Container(
        height: math.min(34, barHeight - 12),
        alignment: Alignment.center,
        decoration: BoxDecoration(
          color: _color(
            toolbar,
            background ?? (active ? 'activeBackground' : 'buttonBackground'),
          ),
          border: Border.all(
            color: _color(
              toolbar,
              active ? 'activeBorderColor' : 'buttonBorderColor',
            ),
            width: _metric(toolbar, 'buttonBorderWidth'),
            style: _metric(toolbar, 'buttonBorderWidth') > 0
                ? BorderStyle.solid
                : BorderStyle.none,
          ),
          borderRadius: BorderRadius.circular(
            _metric(toolbar, 'buttonCornerRadius'),
          ),
          boxShadow: surfaceShadow,
        ),
        foregroundDecoration: BoxDecoration(
          border: themePreviewBorder(highlighted),
        ),
        child: child,
      ),
      '$toolbarPath${active ? 'activeBackground' : 'buttonBackground'}',
      '工具栏按钮底色',
    );
    Widget toolbarIcon(IconData icon, String label, {bool active = false}) {
      final activeSample = icon == FLucideIcons.grid2x2;
      final plainSample = icon == FLucideIcons.panelLeft;
      return SizedBox(
        width: 46,
        child: tile(
          edit(
            Icon(
              icon,
              size: 22,
              color: _color(
                toolbar,
                activeSample && toolbarField == 'selectedForeground'
                    ? 'selectedForeground'
                    : 'foreground',
              ),
            ),
            '${toolbarPath}foreground',
            '$label 图标字色',
          ),
          active: active,
          background:
              activeSample &&
                  (toolbarField == 'selectedBackground' ||
                      toolbarField == 'pressedBackground')
              ? toolbarField
              : null,
          highlighted:
              activeSample &&
                  switch (toolbarField) {
                    'activeBackground' ||
                    'activeBorderColor' ||
                    'selectedBackground' ||
                    'selectedForeground' ||
                    'pressedBackground' => true,
                    _ => false,
                  } ||
              plainSample &&
                  switch (toolbarField) {
                    'foreground' ||
                    'buttonBackground' ||
                    'buttonBorderColor' ||
                    'buttonBorderWidth' ||
                    'buttonCornerRadius' => true,
                    _ => false,
                  },
        ),
      );
    }

    final previewCandidates = candidates.isEmpty ? _candidates : candidates;
    final candidateScale =
        _metric(candidateBar, 'fontScale') *
        (keyboardSettings?.candidateFontScale ?? 1);
    final candidateWeight = _weight(theme['font']['weight']);
    final showComment = candidateBar['showComment'] as bool;
    final commentsBelow = candidateBar['commentPosition'] == 'below';
    final annotationIndex = switch (candidateField) {
      'commentColor' when previewCandidates.length > 1 => 1,
      'commentColor' || 'selectedCommentColor' || 'commentFontSize' => 0,
      _ => null,
    };
    String commentFor(int index) {
      final comment = previewCandidates[index].comment;
      return showComment && index == annotationIndex && comment.trim().isEmpty
          ? '注释'
          : comment;
    }

    final alignStackedRow =
        commentsBelow &&
        showComment &&
        previewCandidates.asMap().keys.any(
          (index) => commentFor(index).trim().isNotEmpty,
        );
    final candidateHeight = math.min(
      alignStackedRow ? 44.8 : 38.0,
      barHeight - 7.2,
    );
    final commentFontSize =
        (candidateBar['commentFontSize'] as num?)?.toDouble() ??
        12 * candidateScale;
    Widget candidateContents(int index) {
      final candidate = previewCandidates[index];
      final comment = commentFor(index);
      final hasComment = showComment && comment.trim().isNotEmpty;
      final normalSample = index == 1 || previewCandidates.length == 1;
      Widget styled(
        String value,
        double size,
        String field,
        String selectedField,
        bool highlighted,
        String description,
      ) => edit(
        themePreviewOutline(
          Text(
            value,
            textScaler: TextScaler.noScaling,
            style: textStyle(
              size,
              _color(candidateBar, index == 0 ? selectedField : field),
              candidateWeight,
            ),
          ),
          highlighted,
        ),
        '$candidatePath${index == 0 ? selectedField : field}',
        description,
      );
      final label = styled(
        '${index + 1}.',
        14 * candidateScale,
        'labelColor',
        'selectedLabelColor',
        candidateField == 'labelColor' && normalSample ||
            candidateField == 'selectedLabelColor' && index == 0,
        '候选序号字色',
      );
      final word = styled(
        candidate.text,
        20 * candidateScale,
        'foreground',
        'selectedForeground',
        candidateField == 'foreground' && normalSample ||
            (candidateField == 'selectedForeground' ||
                    candidateField == 'fontScale') &&
                index == 0,
        '候选字色',
      );
      final annotation = styled(
        comment,
        commentFontSize,
        'commentColor',
        'selectedCommentColor',
        index == annotationIndex,
        '候选注释字色',
      );
      final row = Row(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: alignStackedRow
            ? CrossAxisAlignment.start
            : CrossAxisAlignment.center,
        spacing: _metric(candidateBar, 'inlineGap'),
        children: [
          if (alignStackedRow)
            Padding(
              padding: EdgeInsets.only(top: 3 * candidateScale),
              child: label,
            )
          else
            label,
          if (alignStackedRow)
            Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.start,
              spacing: 1,
              children: [
                word,
                SizedBox(
                  height: commentFontSize,
                  child: hasComment ? annotation : null,
                ),
              ],
            )
          else ...[
            word,
            if (hasComment) annotation,
          ],
        ],
      );
      return alignStackedRow
          ? FittedBox(
              fit: BoxFit.scaleDown,
              alignment: Alignment.topLeft,
              child: row,
            )
          : row;
    }

    final keyboardSurface = edit(
      Container(
        key: const ValueKey('mobile-keyboard-preview'),
        width: _width,
        height: height,
        clipBehavior: Clip.antiAlias,
        decoration: BoxDecoration(color: _color(keyboard, 'background')),
        foregroundDecoration: BoxDecoration(
          border: themePreviewBorder(
            path == 'mobile.keyboard.background' ||
                path == 'common.backgroundColor',
          ),
        ),
        child: Column(
          children: [
            if (showToolbar && (showCandidates || candidateField != null))
              edit(
                Container(
                  key: const ValueKey('mobile-candidate-bar-preview'),
                  height: barHeight,
                  decoration: BoxDecoration(
                    color: _color(candidateBar, 'background'),
                  ),
                  foregroundDecoration: BoxDecoration(
                    border: themePreviewBorder(
                      candidateField == 'background' ||
                          candidateField == 'showComment' ||
                          !showComment && annotationIndex != null,
                    ),
                  ),
                  padding: const EdgeInsets.symmetric(horizontal: 8),
                  child: Row(
                    spacing: 6,
                    children: [
                      Expanded(
                        child: SingleChildScrollView(
                          scrollDirection: Axis.horizontal,
                          child: Row(
                            spacing: 6,
                            children: [
                              for (
                                var index = 0;
                                index < previewCandidates.length;
                                index++
                              )
                                edit(
                                  CustomPaint(
                                    foregroundPainter: index == 0
                                        ? themePreviewCandidateIndicator(
                                            candidateBar['selectedIndicatorColor'],
                                            _metric(candidateBar, 'paddingX'),
                                          )
                                        : null,
                                    child: Container(
                                      height: candidateHeight,
                                      padding: EdgeInsets.symmetric(
                                        horizontal: _metric(
                                          candidateBar,
                                          'paddingX',
                                        ),
                                        vertical: alignStackedRow ? 3 : 0,
                                      ),
                                      decoration: BoxDecoration(
                                        color: _color(
                                          candidateBar,
                                          index == 0
                                              ? 'selectedBackground'
                                              : 'itemBackground',
                                        ),
                                        borderRadius: BorderRadius.circular(
                                          _metric(candidateBar, 'cornerRadius'),
                                        ),
                                        boxShadow: index == 0
                                            ? surfaceShadow
                                            : null,
                                      ),
                                      foregroundDecoration: BoxDecoration(
                                        border:
                                            themePreviewBorder(
                                              switch (candidateField) {
                                                'itemBackground' ||
                                                'borderColor' ||
                                                'borderWidth' =>
                                                  index == 1 ||
                                                      previewCandidates
                                                              .length ==
                                                          1,
                                                'selectedBackground' ||
                                                'selectedBorderColor' ||
                                                'selectedBorderWidth' ||
                                                'cornerRadius' ||
                                                'paddingX' ||
                                                'inlineGap' ||
                                                'commentPosition' => index == 0,
                                                _ =>
                                                  index == 0 &&
                                                      path ==
                                                          'common.accentColor',
                                              },
                                            ) ??
                                            Border.all(
                                              color: _color(
                                                candidateBar,
                                                index == 0
                                                    ? 'selectedBorderColor'
                                                    : 'borderColor',
                                              ),
                                              width: _metric(
                                                candidateBar,
                                                index == 0
                                                    ? 'selectedBorderWidth'
                                                    : 'borderWidth',
                                              ),
                                              style:
                                                  _metric(
                                                        candidateBar,
                                                        index == 0
                                                            ? 'selectedBorderWidth'
                                                            : 'borderWidth',
                                                      ) >
                                                      0
                                                  ? BorderStyle.solid
                                                  : BorderStyle.none,
                                            ),
                                        borderRadius: BorderRadius.circular(
                                          _metric(candidateBar, 'cornerRadius'),
                                        ),
                                      ),
                                      child: candidateContents(index),
                                    ),
                                  ),
                                  index == 0
                                      ? '${candidatePath}selectedBackground'
                                      : '${candidatePath}itemBackground',
                                  '候选底色',
                                ),
                            ],
                          ),
                        ),
                      ),
                      SizedBox(
                        width: 38,
                        child: tile(
                          Icon(
                            FLucideIcons.chevronDown,
                            size: 22,
                            color: _color(toolbar, 'foreground'),
                          ),
                        ),
                      ),
                    ],
                  ),
                ),
                '${candidatePath}background',
                '候选栏背景',
              )
            else if (showToolbar)
              edit(
                Container(
                  height: barHeight,
                  decoration: BoxDecoration(
                    color: _color(toolbar, 'background'),
                  ),
                  foregroundDecoration: BoxDecoration(
                    border: themePreviewBorder(toolbarField == 'background'),
                  ),
                  padding: const EdgeInsets.symmetric(horizontal: 8),
                  child: Row(
                    spacing: 8,
                    children: [
                      Expanded(
                        child: SingleChildScrollView(
                          scrollDirection: Axis.horizontal,
                          child: Row(
                            spacing: 6,
                            children: [
                              toolbarIcon(
                                FLucideIcons.grid2x2,
                                '方案',
                                active: true,
                              ),
                              SizedBox(
                                width: 58,
                                child: tile(
                                  edit(
                                    Row(
                                      mainAxisSize: MainAxisSize.min,
                                      spacing: 5,
                                      children: [
                                        Text(
                                          '中',
                                          textScaler: TextScaler.noScaling,
                                          style: textStyle(
                                            14,
                                            _color(toolbar, 'foreground'),
                                            _weight(baseKey['fontWeight']),
                                          ),
                                        ),
                                        Text(
                                          'En',
                                          textScaler: TextScaler.noScaling,
                                          style: textStyle(
                                            12,
                                            _color(baseKey, 'hintForeground'),
                                            _weight(baseKey['fontWeight']),
                                          ),
                                        ),
                                      ],
                                    ),
                                    '${toolbarPath}foreground',
                                    '中英切换字色',
                                  ),
                                  active: true,
                                ),
                              ),
                              toolbarIcon(
                                FLucideIcons.mousePointer2,
                                '选择',
                                active: true,
                              ),
                              if (keyboardSettings?.clipboardHistoryEnabled !=
                                  false)
                                toolbarIcon(
                                  FLucideIcons.clipboard,
                                  '剪贴板',
                                  active: true,
                                ),
                              toolbarIcon(
                                FLucideIcons.smile,
                                '表情',
                                active: true,
                              ),
                              toolbarIcon(FLucideIcons.panelLeft, '布局'),
                              if (Theme.of(context).platform ==
                                  TargetPlatform.android)
                                toolbarIcon(
                                  FLucideIcons.pictureInPicture2,
                                  '悬浮',
                                ),
                            ],
                          ),
                        ),
                      ),
                      if (logo != null)
                        SizedBox.square(dimension: 30, child: logo),
                    ],
                  ),
                ),
                '${toolbarPath}background',
                '工具栏背景',
              ),
            buildRowsSurface(),
          ],
        ),
      ),
      'mobile.keyboard.background',
      '键盘背景',
    );
    Widget content = AspectRatio(
      aspectRatio: _width / visibleHeight,
      child: FittedBox(
        fit: BoxFit.contain,
        child: showFull
            ? keyboardSurface
            : ClipRect(
                key: const ValueKey('mobile-keyboard-preview-viewport'),
                child: Align(
                  alignment: Alignment.topCenter,
                  heightFactor: visibleHeight / height,
                  child: keyboardSurface,
                ),
              ),
      ),
    );
    if (!showFull) {
      content = ShaderMask(
        blendMode: BlendMode.dstIn,
        shaderCallback: (bounds) => LinearGradient(
          begin: Alignment.topCenter,
          end: Alignment.bottomCenter,
          colors: const [
            Color(0xFFFFFFFF),
            Color(0xFFFFFFFF),
            Color(0x00FFFFFF),
          ],
          stops: [0, fadeStart / visibleHeight, 1],
        ).createShader(bounds),
        child: content,
      );
    }
    return Semantics(
      container: true,
      label: 'KeyTao 键盘预览，$schemeName${onEditField == null ? '，静态展示' : ''}',
      child: onEditField == null
          ? ExcludeSemantics(child: IgnorePointer(child: content))
          : content,
    );
  }
}

class _PreviewSlots {
  const _PreviewSlots({
    this.topLeft,
    this.topCenter,
    this.topRight,
    this.labelSub,
    this.bottomLines = const [],
  });

  factory _PreviewSlots.fromJson(Map<String, dynamic> json) {
    final lines = <String>[
      if (json['bottomLines'] is List)
        for (final item in json['bottomLines'] as List)
          if (item is String && item.isNotEmpty) item,
    ];
    if (lines.isEmpty && json['bottomCenter'] is String) {
      final center = json['bottomCenter'] as String;
      if (center.isNotEmpty) lines.add(center);
    }
    return _PreviewSlots(
      topLeft: (json['topLeft'] as String?)?.isNotEmpty == true
          ? json['topLeft'] as String
          : null,
      topCenter: (json['topCenter'] as String?)?.isNotEmpty == true
          ? json['topCenter'] as String
          : null,
      topRight: (json['topRight'] as String?)?.isNotEmpty == true
          ? json['topRight'] as String
          : null,
      labelSub: (json['labelSub'] as String?)?.isNotEmpty == true
          ? json['labelSub'] as String
          : null,
      bottomLines: lines,
    );
  }

  final String? topLeft;
  final String? topCenter;
  final String? topRight;
  final String? labelSub;
  final List<String> bottomLines;

  bool get hasTop => topLeft != null || topCenter != null || topRight != null;
  bool get hasContent => hasTop || labelSub != null || bottomLines.isNotEmpty;
}

class _PreviewMnemonic {
  const _PreviewMnemonic({
    this.top,
    this.topBadge,
    this.bottomLines = const [],
    this.group,
  });

  factory _PreviewMnemonic.fromJson(Map<String, dynamic> json) =>
      _PreviewMnemonic(
        top: (json['top'] as String?)?.isNotEmpty == true
            ? json['top'] as String
            : null,
        topBadge: (json['topBadge'] as String?)?.isNotEmpty == true
            ? json['topBadge'] as String
            : null,
        bottomLines: [
          if (json['bottomLines'] is List)
            for (final item in json['bottomLines'] as List)
              if (item is String && item.isNotEmpty) item,
        ],
        group: (json['group'] as String?)?.isNotEmpty == true
            ? json['group'] as String
            : null,
      );

  final String? top;
  final String? topBadge;
  final List<String> bottomLines;
  final String? group;

  String? get topText {
    final parts = [?top, ?topBadge];
    return parts.isEmpty ? null : parts.join(' ');
  }

  bool get hasText => topText != null || bottomLines.isNotEmpty;
}

class _PreviewKey {
  const _PreviewKey(
    this.label, {
    this.hint,
    this.action,
    this.weight = 1,
    this.accent = false,
    this.group,
    this.rowSpan = 1,
    this.stack = const [],
    this.slots,
    this.mnemonic,
  });

  factory _PreviewKey.fromJson(Map<String, dynamic> key) => _PreviewKey(
    key['label'] as String,
    hint: key['hint'] as String?,
    action: (key['action'] as Map<String, dynamic>?)?['type'] as String?,
    weight: math.max(0.25, (key['weight'] as num?)?.toDouble() ?? 1),
    accent: key['style'] == 'accent',
    group: key['group'] as String?,
    rowSpan: ((key['rowSpan'] as num?)?.toInt() ?? 1).clamp(1, 8),
    stack: [
      if (key['stack'] is List)
        for (final item in key['stack'] as List)
          if (item is Map<String, dynamic> && item['label'] is String)
            item['label'] as String,
    ],
    slots: key['slots'] is Map<String, dynamic>
        ? _PreviewSlots.fromJson(key['slots'] as Map<String, dynamic>)
        : null,
    mnemonic: key['mnemonic'] is Map<String, dynamic>
        ? _PreviewMnemonic.fromJson(key['mnemonic'] as Map<String, dynamic>)
        : null,
  );

  final String label;
  final String? hint;
  final String? action;
  final double weight;
  final bool accent;
  final String? group;
  final int rowSpan;
  final List<String> stack;
  final _PreviewSlots? slots;
  final _PreviewMnemonic? mnemonic;
}
