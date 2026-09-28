import 'package:flutter/material.dart';
import 'package:forui/forui.dart';

import '../app/controller.dart';
import '../app/options.dart';
import 'widgets.dart';

Color _parse(String hex) =>
    Color(0xFF000000 | int.parse(hex.substring(1), radix: 16));
String _hex(Color color) =>
    '#${(color.toARGB32() & 0xFFFFFF).toRadixString(16).padLeft(6, '0').toUpperCase()}';

class AccentPicker extends StatelessWidget {
  const AccentPicker({super.key, required this.controller});
  final AppController controller;
  Future<void> _pick(BuildContext context) async {
    final value = await showFDialog<String>(
      context: context,
      builder: (_, _, animation) => _ColorDialog(
        initial: controller.uiSettings!.accentColor,
        animation: animation,
      ),
    );
    if (value != null) await controller.saveUi(accentColor: value);
  }

  @override
  Widget build(BuildContext context) {
    final selected = controller.uiSettings!.accentColor.toUpperCase();
    Widget swatch(String hex, {bool custom = false}) => FTooltip(
      tipBuilder: (_, _) => Text(
        custom ? AppStrings.chooseAccentColor : AppStrings.useColor(hex),
      ),
      child: Semantics(
        selected: !custom && selected == hex,
        label: custom ? AppStrings.chooseAccentColor : AppStrings.useColor(hex),
        child: FTappable(
          onPress: controller.busy
              ? null
              : () => custom
                    ? _pick(context)
                    : controller.saveUi(accentColor: hex),
          child: Container(
            width: custom ? 40 : 28,
            height: 28,
            decoration: BoxDecoration(
              color: _parse(hex),
              borderRadius: BorderRadius.circular(custom ? 6 : 14),
            ),
            child: custom || selected == hex
                ? Icon(
                    custom ? FLucideIcons.pipette : FLucideIcons.check,
                    size: 16,
                    color: custom && _parse(hex).computeLuminance() > .5
                        ? Colors.black
                        : Colors.white,
                  )
                : null,
          ),
        ),
      ),
    );
    return SettingRow(
      AppStrings.accentColor,
      icon: FLucideIcons.paintbrush,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Wrap(
            spacing: 8,
            runSpacing: 8,
            children: [
              for (final hex in themeAccents.keys) swatch(hex),
              swatch(selected, custom: true),
            ],
          ),
          const SizedBox(height: 8),
          FBadge(child: Text(controller.themeBadge)),
        ],
      ),
    );
  }
}

class _ColorDialog extends StatefulWidget {
  const _ColorDialog({required this.initial, required this.animation});
  final String initial;
  final Animation<double> animation;
  @override
  State<_ColorDialog> createState() => _ColorDialogState();
}

class _ColorDialogState extends State<_ColorDialog> {
  late HSVColor _color = HSVColor.fromColor(_parse(widget.initial));
  late final _text = TextEditingController(text: _hex(_color.toColor()));
  bool _valid = true;
  void _update(HSVColor color) => setState(() {
    _color = color;
    _text.text = _hex(color.toColor());
    _valid = true;
  });
  @override
  void dispose() {
    _text.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => AppDialog(
    title: AppStrings.chooseAccentColor,
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
              label: AppStrings.chooseAccentColor,
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
        FTextField(
          label: const Text('HEX'),
          control: FTextFieldControl.managed(
            controller: _text,
            onChange: (value) => setState(() {
              _valid = RegExp(r'^#[0-9a-fA-F]{6}$').hasMatch(value.text);
              if (_valid) _color = HSVColor.fromColor(_parse(value.text));
            }),
          ),
          maxLength: 7,
          onSubmit: (_) {
            if (_valid) Navigator.pop(context, _hex(_color.toColor()));
          },
        ),
      ],
    ),
  );
}
