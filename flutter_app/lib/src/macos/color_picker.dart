import 'package:flutter/cupertino.dart';
import 'package:macos_ui/macos_ui.dart';

import '../app/controller.dart';
import '../app/options.dart';
import 'widgets.dart';

Color _parseColor(String hex) =>
    Color(0xFF000000 | int.parse(hex.substring(1), radix: 16));
String _hexColor(Color color) =>
    '#${(color.toARGB32() & 0xFFFFFF).toRadixString(16).padLeft(6, '0').toUpperCase()}';

class MacosAccentPicker extends StatelessWidget {
  const MacosAccentPicker({super.key, required this.controller});
  final AppController controller;

  Future<void> _pick(BuildContext context) async {
    final hex = await showMacosSheet<String>(
      context: context,
      builder: (context) =>
          _ColorSheet(initial: controller.uiSettings!.accentColor),
    );
    if (hex != null) await controller.saveUi(accentColor: hex);
  }

  @override
  Widget build(BuildContext context) {
    final selected = controller.uiSettings!.accentColor.toUpperCase();
    return MacosSettingRow(
      AppStrings.accentColor,
      icon: CupertinoIcons.paintbrush,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.end,
        children: [
          Wrap(
            spacing: 8,
            runSpacing: 8,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [
              for (final hex in themeAccents.keys)
                MacosTooltip(
                  message: AppStrings.useColor(hex),
                  child: Semantics(
                    selected: selected == hex,
                    child: MacosIconButton(
                      semanticLabel: AppStrings.useColor(hex),
                      boxConstraints: const BoxConstraints.tightFor(
                        width: 28,
                        height: 28,
                      ),
                      padding: EdgeInsets.zero,
                      shape: BoxShape.circle,
                      backgroundColor: _parseColor(hex),
                      onPressed: controller.busy
                          ? null
                          : () => controller.saveUi(accentColor: hex),
                      icon: selected == hex
                          ? const MacosIcon(
                              CupertinoIcons.checkmark,
                              color: MacosColors.white,
                              size: 16,
                            )
                          : const SizedBox(width: 16, height: 16),
                    ),
                  ),
                ),
              MacosTooltip(
                message: AppStrings.chooseAccentColor,
                child: MacosIconButton(
                  semanticLabel: AppStrings.chooseAccentColor,
                  boxConstraints: const BoxConstraints.tightFor(
                    width: 40,
                    height: 28,
                  ),
                  backgroundColor: _parseColor(selected),
                  icon: MacosIcon(
                    CupertinoIcons.eyedropper,
                    size: 16,
                    color: _parseColor(selected).computeLuminance() > 0.5
                        ? MacosColors.black
                        : MacosColors.white,
                  ),
                  onPressed: controller.busy ? null : () => _pick(context),
                ),
              ),
            ],
          ),
          const SizedBox(height: 8),
          MacosBadge(controller.themeBadge),
        ],
      ),
    );
  }
}

class _ColorSheet extends StatefulWidget {
  const _ColorSheet({required this.initial});
  final String initial;

  @override
  State<_ColorSheet> createState() => _ColorSheetState();
}

class _ColorSheetState extends State<_ColorSheet> {
  late Color _color = _parseColor(widget.initial);
  late final _text = TextEditingController(text: _hexColor(_color));
  bool _valid = true;

  void _changeChannel(int index, double value) {
    final channels = [
      (_color.r * 255).round(),
      (_color.g * 255).round(),
      (_color.b * 255).round(),
    ];
    channels[index] = value.round();
    setState(() {
      _color = Color.fromARGB(255, channels[0], channels[1], channels[2]);
      _text.text = _hexColor(_color);
      _valid = true;
    });
  }

  @override
  void dispose() {
    _text.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => MacosSheet(
    insetPadding: const EdgeInsets.all(32),
    child: SizedBox(
      width: 380,
      height: 340,
      child: Padding(
        padding: const EdgeInsets.all(20),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Text(
              AppStrings.chooseAccentColor,
              style: MacosTheme.of(context).typography.headline,
            ),
            const SizedBox(height: 16),
            Container(
              height: 44,
              decoration: BoxDecoration(
                color: _color,
                borderRadius: BorderRadius.circular(6),
                border: Border.all(color: MacosTheme.of(context).dividerColor),
              ),
            ),
            const SizedBox(height: 12),
            for (final entry in ['R', 'G', 'B'].asMap().entries)
              Padding(
                padding: const EdgeInsets.symmetric(vertical: 6),
                child: Row(
                  children: [
                    SizedBox(width: 24, child: Text(entry.value)),
                    Expanded(
                      child: MacosSlider(
                        value: [_color.r, _color.g, _color.b][entry.key] * 255,
                        min: 0,
                        max: 255,
                        semanticLabel: entry.value,
                        color: MacosTheme.of(context).primaryColor,
                        onChanged: (value) => _changeChannel(entry.key, value),
                      ),
                    ),
                  ],
                ),
              ),
            MacosTextField(
              controller: _text,
              prefix: const Padding(
                padding: EdgeInsets.only(left: 8),
                child: Text('HEX'),
              ),
              onChanged: (value) => setState(() {
                _valid = RegExp(r'^#[0-9a-fA-F]{6}$').hasMatch(value);
                if (_valid) _color = _parseColor(value);
              }),
              onSubmitted: (_) {
                if (_valid) Navigator.pop(context, _hexColor(_color));
              },
            ),
            const Spacer(),
            Row(
              mainAxisAlignment: MainAxisAlignment.end,
              children: [
                PushButton(
                  controlSize: ControlSize.regular,
                  secondary: true,
                  onPressed: () => Navigator.pop(context),
                  child: const Text(AppStrings.cancel),
                ),
                const SizedBox(width: 8),
                PushButton(
                  controlSize: ControlSize.regular,
                  onPressed: _valid
                      ? () => Navigator.pop(context, _hexColor(_color))
                      : null,
                  child: const Text(AppStrings.apply),
                ),
              ],
            ),
          ],
        ),
      ),
    ),
  );
}
