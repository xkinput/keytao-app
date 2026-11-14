import 'dart:math' as math;

import 'package:flutter/material.dart';

import '../rust/api/types.dart';

class CandidatePanelPreview extends StatelessWidget {
  const CandidatePanelPreview({
    super.key,
    required this.theme,
    required this.orientation,
  });

  final Map<String, dynamic> theme;
  final PanelOrientationDto orientation;

  static const _candidates = ['这', '之', '支', '只能', '服从', '中国'];
  static const _comments = ['🚫', '', '', '~a', '~c', '~g'];

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
    final font = theme['font'] as Map<String, dynamic>;
    final panel = theme['panel'] as Map<String, dynamic>;
    final option = theme['candidate'] as Map<String, dynamic>;
    final navigation = theme['navigation'] as Map<String, dynamic>;
    final vertical = orientation == PanelOrientationDto.vertical;
    final fontSize = _metric(font, 'size');
    final family = font['family'] as String?;
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
      fontFamily: family == null || family.isEmpty ? null : family,
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

    final paddingX = _metric(panel, 'paddingX');
    final paddingY = _metric(panel, 'paddingY');
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
    for (var index = 0; index < _candidates.length; index++) {
      final selected = index == 0;
      final label = '${index + 1}${option['labelSuffix']}';
      final comment = _comments[index];
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
      final naturalWidth =
          measure(label, labelStyle).width +
          measure(_candidates[index], textStyle).width +
          inlineGap +
          (comment.isEmpty
              ? 0
              : measure(comment, commentStyle).width + inlineGap) +
          _metric(option, 'paddingX') * 2;
      final width = math.min(naturalWidth, _metric(option, 'maxWidth'));
      widths.add(width);
      candidates.add(
        Semantics(
          label:
              '候选 ${index + 1}，${_candidates[index]}${comment.isEmpty ? '' : '，注释 $comment'}',
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
                  selected ? 'selectedBackground' : 'background',
                ),
                borderRadius: BorderRadius.circular(
                  _metric(option, 'cornerRadius'),
                ),
              ),
              foregroundDecoration: BoxDecoration(
                borderRadius: BorderRadius.circular(
                  _metric(option, 'cornerRadius'),
                ),
                border: Border.all(
                  color: _color(
                    option,
                    selected ? 'selectedBorderColor' : 'borderColor',
                  ),
                  width: selected
                      ? math.max(1, _metric(option, 'borderWidth')).toDouble()
                      : _metric(option, 'borderWidth'),
                ),
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
                      _candidates[index],
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
        : widths.reduce((a, b) => a + b) + gap * 6 + navSize * 2;
    final candidateList = Flex(
      key: const ValueKey('candidate-preview-candidates'),
      direction: vertical ? Axis.vertical : Axis.horizontal,
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: vertical
          ? CrossAxisAlignment.start
          : CrossAxisAlignment.center,
      children: items,
    );
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        const Text('候选窗预览'),
        const SizedBox(height: 10),
        RepaintBoundary(
          key: const ValueKey('candidate-preview-image'),
          child: LayoutBuilder(
            builder: (context, constraints) {
              final width = math.min(
                math.max(
                  naturalWidth + paddingX * 2,
                  _metric(panel, 'minWidth'),
                ),
                _metric(panel, 'maxWidth'),
              );
              return Semantics(
                container: true,
                label: '候选窗预览',
                child: Container(
                  key: const ValueKey('candidate-panel-preview'),
                  width: math.min(width, constraints.maxWidth),
                  constraints: BoxConstraints(
                    maxHeight: _metric(panel, 'maxHeight'),
                  ),
                  clipBehavior: Clip.antiAlias,
                  decoration: BoxDecoration(
                    color: _color(panel, 'background'),
                    borderRadius: BorderRadius.circular(
                      _metric(panel, 'cornerRadius'),
                    ),
                  ),
                  foregroundDecoration: BoxDecoration(
                    borderRadius: BorderRadius.circular(
                      _metric(panel, 'cornerRadius'),
                    ),
                    border: Border.all(
                      color: _color(panel, 'borderColor'),
                      width: _metric(panel, 'borderWidth'),
                    ),
                  ),
                  padding: EdgeInsets.symmetric(
                    horizontal: paddingX,
                    vertical: paddingY,
                  ),
                  child: SingleChildScrollView(
                    child: Column(
                      mainAxisSize: MainAxisSize.min,
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                          'f',
                          semanticsLabel: '预编辑 zu',
                          style: preeditStyle,
                          textScaler: TextScaler.noScaling,
                        ),
                        SizedBox(height: _metric(panel, 'preeditGap')),
                        SingleChildScrollView(
                          scrollDirection: Axis.horizontal,
                          child: candidateList,
                        ),
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
  }
}
