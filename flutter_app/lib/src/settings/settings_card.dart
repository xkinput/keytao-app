import 'package:flutter/material.dart';

import '../app/controller.dart';
import '../app/options.dart';
import '../app/widgets.dart';
import '../rust/api/types.dart';

class SettingsCard extends StatelessWidget {
  const SettingsCard({super.key, required this.controller});
  final AppController controller;

  @override
  Widget build(BuildContext context) =>
      controller.isAndroid ? _android(context) : _desktop(context);

  Widget _unavailable(String title) => SectionCard(
    title: title,
    children: [
      const Text('设置未读取'),
      const SizedBox(height: 16),
      OutlinedButton(
        onPressed: controller.busy ? null : controller.refreshState,
        child: const Text('重新读取'),
      ),
    ],
  );

  Widget _android(BuildContext context) {
    final c = controller;
    final s = c.androidSettings;
    final enabled = !c.busy && s != null;
    void save(String key, Object value) => c.saveAndroid({key: value});

    Widget slider(
      String label,
      String field,
      num value,
      double min,
      double max,
      int divisions,
      String unit, {
      bool available = true,
      String? semanticLabel,
    }) => SliderSetting(
      label: label,
      value: value.toDouble(),
      min: min,
      max: max,
      divisions: divisions,
      unit: unit,
      semanticLabel: semanticLabel,
      onChanged: enabled && available ? (value) => save(field, value) : null,
    );
    Widget toggle(String label, String field, bool value, {String? hint}) =>
        SwitchListTile(
          contentPadding: EdgeInsets.zero,
          title: Text(label),
          subtitle: hint == null ? null : Text(hint),
          value: value,
          onChanged: enabled ? (value) => save(field, value) : null,
        );
    Widget choice(
      String label,
      String field,
      String value,
      Map<String, String> options, {
      IconData? icon,
    }) => ChoiceSetting<String>(
      label: label,
      value: value,
      options: options,
      currentLabel: options[value],
      icon: icon,
      onChanged: enabled ? (value) => save(field, value) : null,
    );
    return SectionCard(
      title: AppStrings.mobileKeyboard,
      icon: Icons.keyboard_outlined,
      children: [
        Text(
          AppStrings.gestures,
          style: Theme.of(context).textTheme.titleSmall,
        ),
        const SizedBox(height: 16),
        slider(
          AppStrings.longPressDelay,
          'longPressDelayMs',
          s?.longPressDelayMs ?? 300,
          100,
          700,
          60,
          'ms',
        ),
        ChoiceSetting<String>(
          label: AppStrings.englishMode,
          value: s?.englishMode ?? 'ascii',
          currentLabel: c.englishModeLabel,
          hint: c.englishModeHint,
          options: c.androidEnglishModes,
          disabled: c.disabledAndroidEnglishModes,
          onChanged: enabled ? (value) => save('englishMode', value) : null,
        ),
        slider(
          AppStrings.keyboardHeight,
          'keyboardHeightScale',
          s?.keyboardHeightScale ?? 100,
          85,
          130,
          9,
          '%',
        ),
        choice(
          AppStrings.deleteSpeed,
          'deleteSpeed',
          s?.deleteSpeed ?? 'standard',
          const {
            'slow': AppStrings.slow,
            'standard': AppStrings.standard,
            'fast': AppStrings.fast,
          },
          icon: Icons.tune,
        ),
        choice(
          AppStrings.backspaceGesture,
          'backspaceGestureMode',
          s?.backspaceGestureMode ?? 'immediate',
          const {
            'immediate': AppStrings.immediateDelete,
            'selectThenDelete': AppStrings.selectThenDelete,
          },
        ),
        toggle(
          AppStrings.flickKeys,
          'flickKeysEnabled',
          s?.flickKeysEnabled ?? true,
          hint: AppStrings.flickKeysHint,
        ),
        toggle(
          AppStrings.doubleSpacePeriod,
          'doubleSpacePeriodEnabled',
          s?.doubleSpacePeriodEnabled ?? true,
          hint: AppStrings.doubleSpaceHint,
        ),
        const SizedBox(height: 16),
        slider(
          AppStrings.swipeThreshold,
          'swipeThresholdDp',
          s?.swipeThresholdDp ?? 34,
          12,
          96,
          84,
          'dp',
        ),
        const Divider(),
        Text(AppStrings.layout, style: Theme.of(context).textTheme.titleSmall),
        const SizedBox(height: 8),
        toggle(
          AppStrings.floatingPortrait,
          'floatingPortraitEnabled',
          s?.floatingPortraitEnabled ?? false,
        ),
        slider(
          AppStrings.scale,
          'floatingPortraitScale',
          s?.floatingPortraitScale ?? 88,
          70,
          100,
          30,
          '%',
          available: s?.floatingPortraitEnabled ?? false,
          semanticLabel: AppStrings.floatingScale(AppStrings.floatingPortrait),
        ),
        toggle(
          AppStrings.floatingLandscape,
          'floatingLandscapeEnabled',
          s?.floatingLandscapeEnabled ?? true,
        ),
        slider(
          AppStrings.scale,
          'floatingLandscapeScale',
          s?.floatingLandscapeScale ?? 72,
          45,
          100,
          55,
          '%',
          available: s?.floatingLandscapeEnabled ?? true,
          semanticLabel: AppStrings.floatingScale(AppStrings.floatingLandscape),
        ),
        choice(
          AppStrings.enterKey,
          'enterKeyBehavior',
          s?.enterKeyBehavior ?? 'system',
          const {
            'system': AppStrings.smartEnter,
            'newline': AppStrings.newline,
          },
          icon: Icons.keyboard_return,
        ),
        if (s?.configPath?.isNotEmpty == true)
          ValueRow(AppStrings.config, s!.configPath!),
        const SizedBox(height: 8),
        OutlinedButton(
          onPressed: enabled ? c.resetAndroidSettings : null,
          child: const Text(AppStrings.resetMobileKeyboard),
        ),
      ],
    );
  }

  Widget _desktop(BuildContext context) {
    final c = controller;
    final s = c.uiSettings;
    if (s == null) return _unavailable('候选窗设置');
    return SectionCard(
      title: '候选窗设置',
      children: [
        ChoiceSetting<UiColorSchemeDto>(
          label: '配色方案',
          value: s.colorScheme,
          options: colorSchemes,
          onChanged: c.busy ? null : (value) => c.saveUi(colorScheme: value),
        ),
        ChoiceSetting<PanelOrientationDto>(
          label: '候选排列',
          value: s.orientation,
          options: orientations,
          onChanged: c.busy ? null : (value) => c.saveUi(orientation: value),
        ),
        ChoiceSetting<EnglishModeDto>(
          label: '英文模式',
          value: c.desktopEnglishMode,
          options: c.englishModes,
          disabled: c.disabledEnglishModes,
          onChanged: c.busy ? null : c.saveDesktopEnglish,
        ),
        SwitchListTile(
          contentPadding: EdgeInsets.zero,
          title: const Text('嵌入模式'),
          value: s.embeddedComposition,
          onChanged: c.busy ? null : c.saveEmbedded,
        ),
        const SizedBox(height: 16),
        SliderSetting(
          label: '候选字号',
          value: s.fontSize,
          min: candidateFontMin,
          max: candidateFontMax,
          divisions: candidateFontDivisions,
          onChanged: c.busy ? null : (value) => c.saveUi(fontSize: value),
        ),
        Text('主题色', style: Theme.of(context).textTheme.titleSmall),
        const SizedBox(height: 8),
        Wrap(
          spacing: 8,
          runSpacing: 8,
          children: [
            for (final entry in themeAccents.entries)
              ChoiceChip(
                label: Text(entry.value),
                avatar: CircleAvatar(
                  backgroundColor: Color(
                    int.parse('FF${entry.key.substring(1)}', radix: 16),
                  ),
                ),
                selected: s.accentColor.toUpperCase() == entry.key,
                onSelected: c.busy
                    ? null
                    : (_) => c.saveUi(accentColor: entry.key),
              ),
          ],
        ),
        const SizedBox(height: 24),
        OutlinedButton.icon(
          onPressed: c.canOpenTheme ? () => c.open(s.themePath!) : null,
          icon: const Icon(Icons.description_outlined),
          label: const Text('打开主题文件'),
        ),
      ],
    );
  }
}
