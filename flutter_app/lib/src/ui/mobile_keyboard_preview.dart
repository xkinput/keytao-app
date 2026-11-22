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

class MobileKeyboardPreview extends StatelessWidget {
  const MobileKeyboardPreview({
    super.key,
    required this.theme,
    required this.schemeName,
    this.logo,
    this.candidates = const [],
    this.editorPreview = false,
    this.fontFamily,
    this.highlightPath,
    this.keyboardSettings,
    this.onEditField,
  });

  final Map<String, dynamic> theme;
  final String schemeName;
  final Widget? logo;
  final List<({String text, String comment})> candidates;
  final bool editorPreview;
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
    final settings = keyboardSettings;
    final json = settings?.keyboardLayoutJson;
    if (json == null || json.isEmpty) return null;
    return _layoutCache[settings!] ??= jsonDecode(json) as Map<String, dynamic>;
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
    final rows = [
      if (keyboardSettings?.numberRowEnabled == true)
        [
          for (final (index, key) in letterRows.first.indexed)
            index < 10
                ? _PreviewKey(
                    '1234567890'[index],
                    hint: r'!@#$%^&*()'[index],
                    weight: key.weight,
                  )
                : key,
        ],
      ...letterRows,
    ];
    final barHeight = keyboardSettings?.candidateBarHeightDp.toDouble() ?? 52.0;
    final heightScale = (keyboardSettings?.keyboardHeightScale ?? 100) / 100;
    final configuredHeight =
        (keyboardSettings?.keyboardHeightDp ?? 266) *
        heightScale *
        rows.length /
        letterRows.length;
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
    final visibleHeight = editorPreview
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

    Widget key(
      _PreviewKey spec, {
      required double width,
      bool accentSample = false,
    }) {
      final action = spec.action;
      final label = switch (action) {
        'space' when schemeName.isNotEmpty => schemeName,
        'mode' => theme['modeHint']['chineseText'] as String? ?? spec.label,
        _ => spec.label,
      };
      final hint = keyboardSettings?.keyHintVisible == false ? null : spec.hint;
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
      final foreground = _color(style, 'foreground');
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
      return edit(
        Container(
          key: ValueKey('keyboard-preview-key-${spec.label}'),
          width: width,
          height: keyHeight,
          decoration: BoxDecoration(
            color: _color(style, 'background'),
            borderRadius: BorderRadius.circular(_metric(style, 'cornerRadius')),
            border: Border.all(
              color: _color(style, 'borderColor'),
              width: _metric(style, 'borderWidth'),
              style: _metric(style, 'borderWidth') > 0
                  ? BorderStyle.solid
                  : BorderStyle.none,
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
          child: Stack(
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
                          _color(style, 'hintForeground'),
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
          ),
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
            if (candidateField != null)
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
                                  Container(
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
                                                    previewCandidates.length ==
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
            else
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
            SizedBox(height: verticalGap),
            for (final (index, row) in rows.indexed) ...[
              keyRow(row, index),
              SizedBox(height: verticalGap),
            ],
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
        child: editorPreview
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
    if (!editorPreview) {
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

class _PreviewKey {
  const _PreviewKey(
    this.label, {
    this.hint,
    this.action,
    this.weight = 1,
    this.accent = false,
  });

  factory _PreviewKey.fromJson(Map<String, dynamic> key) => _PreviewKey(
    key['label'] as String,
    hint: key['hint'] as String?,
    action: (key['action'] as Map<String, dynamic>?)?['type'] as String?,
    weight: math.max(0.25, (key['weight'] as num?)?.toDouble() ?? 1),
    accent: key['style'] == 'accent',
  );

  final String label;
  final String? hint;
  final String? action;
  final double weight;
  final bool accent;
}
