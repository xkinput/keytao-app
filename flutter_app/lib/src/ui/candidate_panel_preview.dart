import 'dart:math' as math;

import 'package:flutter/material.dart';

import '../rust/api/types.dart';
import 'mobile_keyboard_preview.dart';
import 'system_font_picker.dart';

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
    this.editorPreview = false,
    this.fontCatalog,
  }) : assert(candidateCount >= 1 && candidateCount <= 255);

  final Map<String, dynamic> theme;
  final PanelOrientationDto? orientation;
  final int candidateCount;
  final List<({String text, String comment})> candidates;
  final bool keyboard;
  final String keyboardSchemeName;
  final Widget? keyboardLogo;
  final bool editorPreview;
  final SystemFontCatalog? fontCatalog;
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
    return FutureBuilder<String>(
      key: ValueKey(family),
      future: catalog.load(family),
      builder: (context, snapshot) => _build(context, snapshot.data ?? family),
    );
  }

  Widget _build(BuildContext context, String? family) {
    if (keyboard) {
      return MobileKeyboardPreview(
        theme: theme,
        schemeName: keyboardSchemeName,
        logo: keyboardLogo,
        candidates: this.candidates,
        editorPreview: editorPreview,
        fontFamily: family,
      );
    }
    final font = theme['font'] as Map<String, dynamic>;
    final panel = theme['panel'] as Map<String, dynamic>;
    final option = theme['candidate'] as Map<String, dynamic>;
    final navigation = theme['navigation'] as Map<String, dynamic>;
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

    final widths = <double>[];
    final candidates = <Widget>[];
    for (var index = 0; index < candidateCount; index++) {
      final selected = index == 0;
      final label = '${index + 1}${option['labelSuffix']}';
      final candidate = this.candidates.isEmpty && index < _candidates.length
          ? (text: _candidates[index], comment: _comments[index])
          : index < this.candidates.length
          ? this.candidates[index]
          : (text: '', comment: '');
      final comment =
          editorPreview &&
              (candidate.comment.isEmpty || candidate.comment == '🚫')
          ? '注释'
          : candidate.comment;
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
      final width =
          measure(label, labelStyle).width +
          measure(candidate.text, textStyle).width +
          inlineGap +
          (comment.isEmpty
              ? 0
              : measure(comment, commentStyle).width + inlineGap) +
          _metric(option, 'paddingX') * 2;
      widths.add(width);
      candidates.add(
        Semantics(
          label:
              '候选 ${index + 1}，${candidate.text}${comment.isEmpty ? '' : '，注释 $comment'}',
          selected: selected,
          child: ExcludeSemantics(
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
                border: !editorPreview || _metric(option, 'borderWidth') > 0
                    ? Border.all(
                        color: _color(
                          option,
                          selected ? 'selectedBorderColor' : 'borderColor',
                        ),
                        width: !editorPreview && selected
                            ? math
                                  .max(1, _metric(option, 'borderWidth'))
                                  .toDouble()
                            : _metric(option, 'borderWidth'),
                      )
                    : null,
              ),
              padding: EdgeInsets.symmetric(
                horizontal: math.min(_metric(option, 'paddingX'), width / 2),
                vertical: _metric(option, 'paddingY'),
              ),
              child: FittedBox(
                fit: BoxFit.scaleDown,
                alignment: Alignment.centerLeft,
                child: Row(
                  mainAxisSize: MainAxisSize.min,
                  crossAxisAlignment: CrossAxisAlignment.baseline,
                  textBaseline: TextBaseline.alphabetic,
                  children: [
                    Text(
                      label,
                      style: labelStyle,
                      textScaler: TextScaler.noScaling,
                    ),
                    SizedBox(width: inlineGap),
                    Text(
                      candidate.text,
                      key: ValueKey('candidate-preview-text-$index'),
                      style: textStyle,
                      textScaler: TextScaler.noScaling,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                    ),
                    if (comment.isNotEmpty) ...[
                      SizedBox(width: inlineGap),
                      Text(
                        comment,
                        style: commentStyle,
                        textScaler: TextScaler.noScaling,
                      ),
                    ],
                  ],
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
      SizedBox(width: gap, height: gap),
      if (vertical)
        Row(mainAxisSize: MainAxisSize.min, children: [arrow('›', '下一页')])
      else ...[
        arrow('‹', '上一页'),
        SizedBox(width: gap),
        arrow('›', '下一页'),
      ],
    ];
    final naturalWidth = vertical
        ? widths.reduce(math.max)
        : widths.reduce((a, b) => a + b) +
              gap * (candidateCount + 1) +
              navSize * 2;
    final candidateList = Flex(
      key: const ValueKey('candidate-preview-candidates'),
      direction: vertical ? Axis.vertical : Axis.horizontal,
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: vertical
          ? CrossAxisAlignment.start
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
                child: Container(
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
                    border: !editorPreview || _metric(panel, 'borderWidth') > 0
                        ? Border.all(
                            color: _color(panel, 'borderColor'),
                            width: _metric(panel, 'borderWidth'),
                          )
                        : null,
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
                          Text(
                            'f',
                            semanticsLabel: '预编辑 f',
                            style: preeditStyle,
                            textScaler: TextScaler.noScaling,
                          ),
                          SizedBox(height: _metric(panel, 'preeditGap')),
                        ],
                        candidateList,
                      ],
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
