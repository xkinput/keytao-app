import 'package:flutter/material.dart';
import 'package:forui/forui.dart';

class MobileKeyboardPreview extends StatelessWidget {
  const MobileKeyboardPreview({
    super.key,
    required this.theme,
    required this.schemeName,
    this.logo,
    this.candidates = const [],
  });

  final Map<String, dynamic> theme;
  final String schemeName;
  final Widget? logo;
  final List<({String text, String comment})> candidates;

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
  static const _verticalGap = 10.0;
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
}
