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
    this.fontFamily,
  });

  final Map<String, dynamic> theme;
  final String schemeName;
  final Widget? logo;
  final List<({String text, String comment})> candidates;
  final bool editorPreview;
  final String? fontFamily;

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
  static const _candidateBarHeight = 52.0;
  static const _toolbarHeight = 52.0;
  static const _keyHeight = 54.0;
  static const _horizontalGap = 4.0;
  static const _verticalGap = (266.0 - _keyHeight * 4) / 5;
  static const _height =
      _candidateBarHeight + 3 * _keyHeight + 4 * _verticalGap;
  static const _homeHeight =
      _toolbarHeight + _verticalGap + 2 * (_keyHeight + _verticalGap) + 2;

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
    final surfaceShadow = [
      BoxShadow(
        color: _color(baseKey, 'shadowColor'),
        offset: Offset(0, _metric(baseKey, 'shadowOffsetY')),
      ),
    ];
    final character = {
      ...baseKey,
      ...types['character'] as Map<String, dynamic>,
    };
    final function = {...baseKey, ...types['function'] as Map<String, dynamic>};
    final space = {...function, ...types['space'] as Map<String, dynamic>};
    final enter = {...function, ...types['enter'] as Map<String, dynamic>};
    final accent = {...character, ...classes['accent'] as Map<String, dynamic>};
    TextStyle textStyle(double size, Color color, FontWeight weight) =>
        TextStyle(
          fontFamily: fontFamily ?? theme['font']['effectiveFamily'] as String?,
          fontSize: size,
          fontWeight: weight,
          height: 1,
          color: color,
        );

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

    Widget tile(Widget child, {bool active = false}) => Container(
      height: 34,
      alignment: Alignment.center,
      decoration: BoxDecoration(
        color: _color(
          toolbar,
          active ? 'activeBackground' : 'buttonBackground',
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
      child: child,
    );
    Widget toolbarIcon(IconData icon, {bool active = false}) => SizedBox(
      width: 44,
      child: tile(
        Icon(icon, size: 22, color: _color(toolbar, 'foreground')),
        active: active,
      ),
    );

    final previewCandidates = candidates.isEmpty ? _candidates : candidates;
    final candidateScale = _metric(candidateBar, 'fontScale');
    final candidateWeight = _weight(theme['font']['weight']);
    final showComment = candidateBar['showComment'] as bool;
    final commentsBelow = candidateBar['commentPosition'] == 'below';
    final candidateHeight = commentsBelow &&
            showComment &&
            previewCandidates.any((candidate) => candidate.comment.trim().isNotEmpty)
        ? 44.8
        : 38.0;
    Widget candidateContents(int index) {
      final candidate = previewCandidates[index];
      final hasComment = showComment && candidate.comment.trim().isNotEmpty;
      final stacked = hasComment && commentsBelow;
      Text styled(
        String value,
        double size,
        String field,
        String selectedField,
      ) => Text(
        value,
        textScaler: TextScaler.noScaling,
        style: textStyle(
          size * candidateScale,
          _color(candidateBar, index == 0 ? selectedField : field),
          candidateWeight,
        ).copyWith(height: stacked ? 1 : null),
      );
      final label = styled(
        '${index + 1}.',
        14,
        'labelColor',
        'selectedLabelColor',
      );
      final word = styled(
        candidate.text,
        20,
        'foreground',
        'selectedForeground',
      );
      final annotation = styled(
        candidate.comment,
        12,
        'commentColor',
        'selectedCommentColor',
      );
      final row = Row(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: stacked
            ? CrossAxisAlignment.start
            : CrossAxisAlignment.center,
        spacing: _metric(candidateBar, 'inlineGap'),
        children: [
          if (stacked)
            Padding(
              padding: EdgeInsets.only(top: 3 * candidateScale),
              child: label,
            )
          else
            label,
          if (stacked)
            Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.start,
              spacing: 1,
              children: [word, annotation],
            )
          else ...[
            word,
            if (hasComment) annotation,
          ],
        ],
      );
      return stacked
          ? FittedBox(
              fit: BoxFit.scaleDown,
              alignment: Alignment.centerLeft,
              child: row,
            )
          : row;
    }

    final height = editorPreview ? _height : _homeHeight;
    Widget content = AspectRatio(
      aspectRatio: _width / height,
      child: FittedBox(
        fit: BoxFit.contain,
        child: Container(
          key: const ValueKey('mobile-keyboard-preview'),
          width: _width,
          height: height,
          clipBehavior: Clip.antiAlias,
          decoration: BoxDecoration(color: _color(keyboard, 'background')),
          child: Column(
            children: [
              if (editorPreview)
                Container(
                  key: const ValueKey('mobile-candidate-bar-preview'),
                  height: _candidateBarHeight,
                  color: _color(candidateBar, 'background'),
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
                                Container(
                                  height: candidateHeight,
                                  padding: EdgeInsets.symmetric(
                                    horizontal: _metric(
                                      candidateBar,
                                      'paddingX',
                                    ),
                                  ),
                                  decoration: BoxDecoration(
                                    color: _color(
                                      candidateBar,
                                      index == 0
                                          ? 'selectedBackground'
                                          : 'itemBackground',
                                    ),
                                    border: Border.all(
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
                                    boxShadow: index == 0
                                        ? surfaceShadow
                                        : null,
                                  ),
                                  child: candidateContents(index),
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
                )
              else
                Container(
                  height: _toolbarHeight,
                  color: _color(toolbar, 'background'),
                  padding: const EdgeInsets.symmetric(horizontal: 8),
                  child: SingleChildScrollView(
                    scrollDirection: Axis.horizontal,
                    child: Row(
                      spacing: 6,
                      children: [
                        toolbarIcon(FLucideIcons.grid2x2, active: true),
                        SizedBox(
                          width: 58,
                          child: tile(
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
                            active: true,
                          ),
                        ),
                        toolbarIcon(FLucideIcons.mousePointer2, active: true),
                        toolbarIcon(FLucideIcons.clipboard, active: true),
                        toolbarIcon(FLucideIcons.smile, active: true),
                        toolbarIcon(FLucideIcons.panelLeft),
                        toolbarIcon(FLucideIcons.keyboard),
                        if (logo != null)
                          SizedBox.square(dimension: 28, child: logo),
                      ],
                    ),
                  ),
                ),
              const SizedBox(height: _verticalGap),
              for (var row = 0; row < _letterRows.length; row++) ...[
                Padding(
                  padding: EdgeInsets.symmetric(horizontal: row == 1 ? 24 : 5),
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
              if (editorPreview) ...[
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
                const SizedBox(height: _verticalGap),
              ],
            ],
          ),
        ),
      ),
    );
    if (!editorPreview) {
      content = ShaderMask(
        blendMode: BlendMode.dstIn,
        shaderCallback: (bounds) => const LinearGradient(
          begin: Alignment.topCenter,
          end: Alignment.bottomCenter,
          colors: [Color(0xFFFFFFFF), Color(0xFFFFFFFF), Color(0x00FFFFFF)],
          stops: [0, 0.55, 1],
        ).createShader(bounds),
        child: content,
      );
    }
    return Semantics(
      container: true,
      label: 'KeyTao 键盘预览，$schemeName，静态展示',
      child: ExcludeSemantics(child: IgnorePointer(child: content)),
    );
  }
}
