import 'dart:math' as math;

import 'package:flutter/material.dart';

import '../app/controller.dart';
import '../rust/api/types.dart';
import 'mobile_keyboard_preview.dart';
import 'system_font_picker.dart';

final _candidatePanelPreviews = Expando<_CandidatePanelPreviewCache>();

class _CandidatePanelPreviewCache {
  _CandidatePanelPreviewCache(AppController controller) {
    var savingTheme = controller.savingTheme;
    var settings = controller.uiSettings;
    controller.addListener(() {
      if ((savingTheme && !controller.savingTheme) ||
          !identical(settings, controller.uiSettings)) {
        themes.clear();
      }
      savingTheme = controller.savingTheme;
      settings = controller.uiSettings;
    });
  }

  final themes =
      <
        (UiColorSchemeDto, String, double?, String, double, bool),
        Map<String, dynamic>
      >{};
}

extension CandidatePanelPreviewCache on AppController {
  Map<String, dynamic> cachedCandidatePanelPreview({
    required UiColorSchemeDto colorScheme,
    required String accentColor,
    required double? cornerRadius,
    required String backgroundColor,
    required double fontSize,
    required bool systemDark,
  }) {
    final themes =
        (_candidatePanelPreviews[this] ??= _CandidatePanelPreviewCache(
          this,
        )).themes;
    final key = (
      colorScheme,
      accentColor,
      cornerRadius,
      backgroundColor,
      fontSize,
      systemDark,
    );
    final theme =
        themes.remove(key) ??
        resolveCandidatePanelPreview(
          colorScheme: colorScheme,
          accentColor: accentColor,
          cornerRadius: cornerRadius,
          backgroundColor: backgroundColor,
          fontSize: fontSize,
          systemDark: systemDark,
        );
    if (themes.length == 2) themes.remove(themes.keys.first);
    return themes[key] = theme;
  }
}

Widget themePreviewEditTarget(
  Widget child,
  String path,
  ValueChanged<String>? onEditField,
) => onEditField == null
    ? child
    : MouseRegion(
        cursor: SystemMouseCursors.click,
        child: GestureDetector(
          key: ValueKey(('theme-preview-edit', path)),
          behavior: HitTestBehavior.opaque,
          onTap: () => onEditField(path),
          child: child,
        ),
      );

class CandidatePanelPreview extends StatelessWidget {
  const CandidatePanelPreview({
    super.key,
    required this.theme,
    this.orientation,
    required this.showPreedit,
    this.candidateCount = 6,
    this.candidates = const [],
    this.keyboard = false,
    this.keyboardSchemeName = 'KeyTao',
    this.keyboardLogo,
    this.keyboardSettings,
    this.editorPreview = false,
    this.fontCatalog,
    this.highlightPath,
    this.onEditField,
  }) : assert(candidateCount >= 1 && candidateCount <= 255);

  final Map<String, dynamic> theme;
  final PanelOrientationDto? orientation;
  final int candidateCount;
  final List<({String text, String comment})> candidates;
  final bool keyboard;
  final String keyboardSchemeName;
  final Widget? keyboardLogo;
  final AndroidImeInputSettingsDto? keyboardSettings;
  final bool editorPreview;
  final SystemFontCatalog? fontCatalog;
  final String? highlightPath;
  final ValueChanged<String>? onEditField;
  // The IME shows the preedit in the panel only when it is not embedded.
  final bool showPreedit;

  static const _candidates = [
    '这',
    '之',
    '支',
    '只能',
    '服从',
    '中国',
    '支持',
    '之间',
    '之前',
    '之后',
  ];
  static const _comments = [
    '🚫',
    '',
    '',
    '~a',
    '~c',
    '~g',
    '~i',
    '~j',
    '~q',
    '~h',
  ];

  double _metric(Map<String, dynamic> section, String name) =>
      (section[name] as num).toDouble();

  Color _color(Map<String, dynamic> section, String name) {
    final value = section[name] as Map<String, dynamic>;
    return Color.fromARGB(
      value['alpha'] as int,
      value['red'] as int,
      value['green'] as int,
      value['blue'] as int,
    );
  }

  @override
  Widget build(BuildContext context) {
    final family = theme['font']['effectiveFamily'] as String?;
    final catalog = fontCatalog;
    if (family == null || family.isEmpty || catalog == null) {
      return _build(context, family);
    }
    final load = catalog.load(family);
    if (load == null) return _build(context, family);
    return FutureBuilder<String?>(
      key: ValueKey(family),
      future: load,
      builder: (context, snapshot) => _build(
        context,
        snapshot.data ?? (catalog.usesFontLoader ? family : null),
      ),
    );
  }

  Widget _build(BuildContext context, String? family) {
    if (keyboard) {
      return MobileKeyboardPreview(
        theme: theme,
        schemeName: keyboardSchemeName,
        logo: keyboardLogo,
        keyboardSettings: keyboardSettings,
        candidates: this.candidates,
        editorPreview: editorPreview,
        fontFamily: family,
        highlightPath: highlightPath,
        onEditField: onEditField,
      );
    }
    final font = theme['font'] as Map<String, dynamic>;
    final panel = theme['panel'] as Map<String, dynamic>;
    final option = theme['candidate'] as Map<String, dynamic>;
    final navigation = theme['navigation'] as Map<String, dynamic>;
    Widget edit(String path, Widget child) =>
        themePreviewEditTarget(child, path, onEditField);
    final vertical = orientation == null
        ? panel['orientation'] == 'vertical'
        : orientation == PanelOrientationDto.vertical;
    final fontSize = _metric(font, 'size');
    final weight = switch (font['weight']) {
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
    TextStyle style(double size, Color color, FontWeight weight) => TextStyle(
      inherit: false,
      fontFamily: family,
      fontSize: size,
      fontWeight: weight,
      color: color,
    );
    Size measure(String text, TextStyle style) {
      final painter = TextPainter(
        text: TextSpan(text: text, style: style),
        textDirection: TextDirection.ltr,
        maxLines: 1,
      )..layout();
      final size = painter.size;
      painter.dispose();
      return size;
    }

    final padding = EdgeInsets.fromLTRB(
      _metric(panel, 'paddingLeft'),
      _metric(panel, 'paddingTop'),
      _metric(panel, 'paddingRight'),
      _metric(panel, 'paddingBottom'),
    );
    final gap = _metric(panel, 'gap');
    final inlineGap = _metric(option, 'inlineGap');
    final foreground = _color(option, 'foreground');
    const preeditText = 'f';
    final preeditStyle = style(
      _metric(font, 'preeditSize'),
      foreground,
      weight,
    );
    // CandidatePanel.swift's CandidateNavigationButton uses a 1.3x system font.
    final arrowStyle = style(
      fontSize * 1.3,
      _color(navigation, 'foreground'),
      FontWeight.w400,
    );
    final navSize = math.max(
      _metric(navigation, 'buttonSize'),
      measure('›', arrowStyle).height,
    );
    Widget arrow(String symbol, String label) => Semantics(
      label: label,
      child: ExcludeSemantics(
        child: SizedBox.square(
          dimension: navSize,
          child: Center(
            child: Text(
              symbol,
              style: arrowStyle,
              textScaler: TextScaler.noScaling,
            ),
          ),
        ),
      ),
    );

    final usesSample = this.candidates.isEmpty;
    final sourceCount = usesSample
        ? _candidates.length
        : this.candidates.length;
    final visibleCount = math.min(candidateCount, sourceCount);
    final showPrevious = editorPreview && !vertical;
    final showNext =
        editorPreview ||
        (usesSample
            ? sourceCount > candidateCount
            : sourceCount >= candidateCount);
    final previewCandidates = List.generate(visibleCount, (index) {
      final candidate = usesSample
          ? (text: _candidates[index], comment: _comments[index])
          : this.candidates[index];
      final comment =
          editorPreview &&
              (candidate.comment.isEmpty || candidate.comment == '🚫')
          ? '注释'
          : candidate.comment;
      return (text: candidate.text, comment: comment);
    });
    final commentsBelow = option['commentPosition'] == 'below';
    final alignStackedRow =
        !vertical &&
        commentsBelow &&
        previewCandidates.any((candidate) => candidate.comment.isNotEmpty);
    final widths = <double>[];
    final candidates = <Widget>[];
    for (var index = 0; index < previewCandidates.length; index++) {
      final selected = index == 0;
      final label = '${index + 1}${option['labelSuffix']}';
      final candidate = previewCandidates[index];
      final comment = candidate.comment;
      final labelStyle = style(
        _metric(font, 'labelSize'),
        _color(option, selected ? 'selectedLabelColor' : 'labelColor'),
        FontWeight.w600,
      );
      final textStyle = style(
        fontSize,
        _color(option, selected ? 'selectedForeground' : 'foreground'),
        weight,
      );
      final commentStyle = style(
        _metric(font, 'commentSize'),
        _color(option, selected ? 'selectedCommentColor' : 'commentColor'),
        FontWeight.w400,
      );
      final stacked = commentsBelow && (alignStackedRow || comment.isNotEmpty);
      final labelSize = measure(label, labelStyle);
      final wordSize = measure(candidate.text, textStyle);
      final commentWidth = comment.isEmpty
          ? 0.0
          : measure(comment, commentStyle).width;
      final width =
          labelSize.width +
          inlineGap +
          (stacked
              ? math.max(wordSize.width, commentWidth)
              : wordSize.width +
                    (comment.isEmpty ? 0 : commentWidth + inlineGap)) +
          _metric(option, 'paddingX') * 2;
      final word = themePreviewOutline(
        edit(
          'pc.font.size',
          Text(
            candidate.text,
            key: ValueKey('candidate-preview-text-$index'),
            style: textStyle,
            textScaler: TextScaler.noScaling,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
          ),
        ),
        switch (highlightPath) {
          'pc.candidate.foreground' => index == 1 || visibleCount == 1,
          'pc.candidate.selectedForeground' ||
          'pc.font.size' ||
          'common.font.family' ||
          'common.font.weight' => selected,
          _ => false,
        },
      );
      final annotation = themePreviewOutline(
        edit(
          'pc.font.commentSize',
          Text(comment, style: commentStyle, textScaler: TextScaler.noScaling),
        ),
        switch (highlightPath) {
          'pc.candidate.commentColor' => index == 1 || visibleCount == 1,
          'pc.candidate.selectedCommentColor' ||
          'pc.font.commentSize' => selected,
          _ => false,
        },
      );
      final number = themePreviewOutline(
        edit(
          'pc.font.labelSize',
          Text(label, style: labelStyle, textScaler: TextScaler.noScaling),
        ),
        switch (highlightPath) {
          'pc.candidate.labelColor' => index == 1 || visibleCount == 1,
          'pc.candidate.selectedLabelColor' ||
          'pc.candidate.labelSuffix' ||
          'pc.font.labelSize' => selected,
          _ => false,
        },
      );
      final highlightItem = switch (highlightPath) {
        'pc.candidate.background' => index == 2 || visibleCount == 1,
        'pc.candidate.borderColor' ||
        'pc.candidate.hoverBackground' => index == 1 || visibleCount == 1,
        'common.accentColor' ||
        'pc.candidate.selectedBackground' ||
        'pc.candidate.selectedBorderColor' ||
        'pc.candidate.borderWidth' ||
        'pc.candidate.cornerRadius' ||
        'pc.candidate.commentPosition' => selected,
        _ => false,
      };
      widths.add(width);
      candidates.add(
        Semantics(
          label:
              '候选 ${index + 1}，${candidate.text}${comment.isEmpty ? '' : '，注释 $comment'}',
          selected: selected,
          child: ExcludeSemantics(
            excluding: onEditField == null,
            child: edit(
              selected
                  ? 'pc.candidate.selectedBackground'
                  : 'pc.candidate.background',
              CustomPaint(
                foregroundPainter: selected
                    ? themePreviewCandidateIndicator(
                        option['selectedIndicatorColor'],
                        _metric(option, 'paddingX'),
                      )
                    : null,
                child: Container(
                  width: width,
                  constraints: BoxConstraints(
                    minHeight: _metric(option, 'minHeight'),
                  ),
                  decoration: BoxDecoration(
                    color: _color(
                      option,
                      selected
                          ? 'selectedBackground'
                          : editorPreview && index == 1
                          ? 'hoverBackground'
                          : 'background',
                    ),
                    borderRadius: BorderRadius.circular(
                      _metric(option, 'cornerRadius'),
                    ),
                  ),
                  foregroundDecoration: BoxDecoration(
                    borderRadius: BorderRadius.circular(
                      _metric(option, 'cornerRadius'),
                    ),
                    border:
                        themePreviewBorder(highlightItem) ??
                        (!editorPreview || _metric(option, 'borderWidth') > 0
                            ? Border.all(
                                color: _color(
                                  option,
                                  selected
                                      ? 'selectedBorderColor'
                                      : 'borderColor',
                                ),
                                width: !editorPreview && selected
                                    ? math
                                          .max(
                                            1,
                                            _metric(option, 'borderWidth'),
                                          )
                                          .toDouble()
                                    : _metric(option, 'borderWidth'),
                              )
                            : null),
                  ),
                  padding: EdgeInsets.symmetric(
                    horizontal: math.min(
                      _metric(option, 'paddingX'),
                      width / 2,
                    ),
                    vertical: _metric(option, 'paddingY'),
                  ),
                  child: FittedBox(
                    fit: BoxFit.scaleDown,
                    alignment: alignStackedRow
                        ? Alignment.topLeft
                        : Alignment.centerLeft,
                    child: Row(
                      mainAxisSize: MainAxisSize.min,
                      crossAxisAlignment: stacked
                          ? CrossAxisAlignment.start
                          : CrossAxisAlignment.baseline,
                      textBaseline: TextBaseline.alphabetic,
                      children: [
                        if (stacked)
                          Padding(
                            padding: EdgeInsets.only(
                              top: math.max(
                                0,
                                (wordSize.height - labelSize.height) / 2,
                              ),
                            ),
                            child: number,
                          )
                        else
                          number,
                        SizedBox(width: inlineGap),
                        if (stacked)
                          Column(
                            mainAxisSize: MainAxisSize.min,
                            crossAxisAlignment: CrossAxisAlignment.start,
                            spacing: inlineGap / 2,
                            children: [
                              word,
                              if (comment.isNotEmpty) annotation,
                            ],
                          )
                        else ...[
                          word,
                          if (comment.isNotEmpty) ...[
                            SizedBox(width: inlineGap),
                            annotation,
                          ],
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
    final items = <Widget>[
      for (var index = 0; index < candidates.length; index++) ...[
        if (index > 0) SizedBox(width: gap, height: gap),
        candidates[index],
      ],
      if (showPrevious) ...[SizedBox(width: gap), arrow('‹', '上一页')],
      if (showNext) ...[SizedBox(width: gap, height: gap), arrow('›', '下一页')],
    ];
    final navCount = (showPrevious ? 1 : 0) + (showNext ? 1 : 0);
    final candidateWidth = vertical
        ? math.max(widths.reduce(math.max), showNext ? navSize : 0.0)
        : widths.reduce((a, b) => a + b) +
              gap * (visibleCount + navCount - 1) +
              navSize * navCount;
    final naturalWidth = showPreedit
        ? math.max(candidateWidth, measure(preeditText, preeditStyle).width)
        : candidateWidth;
    final candidateList = Flex(
      key: const ValueKey('candidate-preview-candidates'),
      direction: vertical ? Axis.vertical : Axis.horizontal,
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: vertical
          ? CrossAxisAlignment.start
          : alignStackedRow
          ? CrossAxisAlignment.stretch
          : CrossAxisAlignment.center,
      children: items,
    );
    final panelPreview = Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        RepaintBoundary(
          key: const ValueKey('candidate-preview-image'),
          child: LayoutBuilder(
            builder: (context, constraints) {
              final width = math.max(
                naturalWidth + padding.horizontal,
                _metric(panel, 'minWidth'),
              );
              return Semantics(
                container: true,
                label: '候选窗预览',
                child: edit(
                  'pc.panel.background',
                  Container(
                    key: const ValueKey('candidate-panel-preview'),
                    width: math.min(width, constraints.maxWidth),
                    constraints: BoxConstraints(
                      maxHeight: math.min(360, _metric(panel, 'maxHeight')),
                    ),
                    clipBehavior: Clip.antiAlias,
                    decoration: BoxDecoration(
                      color: _color(panel, 'background'),
                      boxShadow: editorPreview && panel['shadowStyle'] != null
                          ? [
                              BoxShadow(
                                color: _color(panel['shadowStyle'], 'color'),
                                blurRadius: _metric(
                                  panel['shadowStyle'],
                                  'blurRadius',
                                ),
                                offset: Offset(
                                  0,
                                  _metric(panel['shadowStyle'], 'offsetY'),
                                ),
                              ),
                            ]
                          : null,
                      borderRadius: BorderRadius.circular(
                        _metric(panel, 'cornerRadius'),
                      ),
                    ),
                    foregroundDecoration: BoxDecoration(
                      borderRadius: BorderRadius.circular(
                        _metric(panel, 'cornerRadius'),
                      ),
                      border:
                          themePreviewBorder(
                            (highlightPath?.startsWith('pc.panel.') ?? false) ||
                                highlightPath == 'common.backgroundColor' ||
                                highlightPath == 'common.cornerRadius',
                          ) ??
                          (!editorPreview || _metric(panel, 'borderWidth') > 0
                              ? Border.all(
                                  color: _color(panel, 'borderColor'),
                                  width: _metric(panel, 'borderWidth'),
                                )
                              : null),
                    ),
                    padding: padding,
                    child: FittedBox(
                      fit: BoxFit.scaleDown,
                      alignment: Alignment.topLeft,
                      child: Column(
                        mainAxisSize: MainAxisSize.min,
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          if (showPreedit) ...[
                            themePreviewOutline(
                              edit(
                                'pc.font.preeditSize',
                                Text(
                                  preeditText,
                                  semanticsLabel: '预编辑 $preeditText',
                                  style: preeditStyle,
                                  textScaler: TextScaler.noScaling,
                                ),
                              ),
                              highlightPath == 'pc.font.preeditSize',
                            ),
                            SizedBox(height: _metric(panel, 'preeditGap')),
                          ],
                          if (alignStackedRow)
                            IntrinsicHeight(child: candidateList)
                          else
                            candidateList,
                        ],
                      ),
                    ),
                  ),
                ),
              );
            },
          ),
        ),
      ],
    );
    return panelPreview;
  }
}
