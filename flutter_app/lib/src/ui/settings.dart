import 'package:flutter/material.dart';
import 'package:forui/forui.dart';

import '../app/controller.dart';
import '../app/options.dart';
import '../rust/api/types.dart';
import 'color_picker.dart';
import 'widgets.dart';

class SettingsCard extends StatelessWidget {
  const SettingsCard({super.key, required this.controller});
  final AppController controller;
  @override
  Widget build(BuildContext context) =>
      controller.isMobile ? _mobile(context) : _desktop(context);

  Widget _desktop(BuildContext context) {
    final c = controller;
    final s = c.uiSettings;
    return Section(
      title: AppStrings.imeSettings,
      icon: FLucideIcons.palette,
      children: [
        if (s == null)
          SettingRow(
            AppStrings.checking,
            child: ActionButton(
              AppStrings.recheck,
              onPress: c.busy ? null : c.refreshState,
            ),
          )
        else ...[
          ChoiceSetting<UiColorSchemeDto>(
            label: '配色方案',
            value: s.colorScheme,
            options: colorSchemes,
            icons: const {
              UiColorSchemeDto.auto: FLucideIcons.monitor,
              UiColorSchemeDto.light: FLucideIcons.sun,
              UiColorSchemeDto.dark: FLucideIcons.moon,
            },
            onChanged: c.busy ? null : (value) => c.saveUi(colorScheme: value),
          ),
          ChoiceSetting<PanelOrientationDto>(
            label: '候选排列',
            value: s.orientation,
            options: orientations,
            icons: const {
              PanelOrientationDto.horizontal: FLucideIcons.columns3,
              PanelOrientationDto.vertical: FLucideIcons.rows3,
            },
            onChanged: c.busy ? null : (value) => c.saveUi(orientation: value),
          ),
          ChoiceSetting<EnglishModeDto>(
            label: AppStrings.englishMode,
            value: c.desktopEnglishMode,
            options: c.englishModes,
            disabled: c.disabledEnglishModes,
            hint: c.englishModeHint,
            onChanged: c.busy ? null : c.saveDesktopEnglish,
          ),
          if (c.showsEmbeddedComposition)
            SwitchSetting(
              AppStrings.embedded,
              value: s.embeddedComposition,
              stateLabel: s.embeddedComposition
                  ? AppStrings.on
                  : AppStrings.off,
              hint: AppStrings.embeddedDescription,
              onChanged: c.busy ? null : c.saveEmbedded,
            ),
          SliderSetting(
            label: AppStrings.candidateFontSize,
            value: s.fontSize,
            min: candidateFontMin,
            max: candidateFontMax,
            divisions: candidateFontDivisions,
            icon: FLucideIcons.type,
            onChanged: c.busy ? null : (value) => c.saveUi(fontSize: value),
          ),
          AccentPicker(controller: c),
          if (s.themePath case final path?)
            Row(
              children: [
                Expanded(child: SelectableText('${AppStrings.theme}$path')),
                const SizedBox(width: 8),
                FTooltip(
                  tipBuilder: (_, _) => const Text(AppStrings.openTheme),
                  child: FButton.icon(
                    onPress: c.canOpenTheme ? c.openTheme : null,
                    variant: FButtonVariant.ghost,
                    child: const Icon(
                      FLucideIcons.folderOpen,
                      semanticLabel: AppStrings.openTheme,
                    ),
                  ),
                ),
              ],
            ),
        ],
        if (c.imeUiError case final error?) ErrorMessage(error),
      ],
    );
  }

  Widget _mobile(BuildContext context) {
    final c = controller;
    final s = c.androidSettings;
    final enabled = !c.busy && s != null;
    void save(String field, Object value) => c.saveAndroid({field: value});
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
        SwitchSetting(
          label,
          value: value,
          hint: hint,
          onChanged: enabled ? (value) => save(field, value) : null,
        );
    Widget choice(
      String label,
      String field,
      String value,
      Map<String, String> options,
    ) => ChoiceSetting<String>(
      label: label,
      value: value,
      options: options,
      currentLabel: options[value],
      onChanged: enabled ? (value) => save(field, value) : null,
    );
    return Section(
      title: AppStrings.mobileKeyboard,
      icon: FLucideIcons.keyboard,
      children: [
        const Text(AppStrings.gestures),
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
        slider(
          AppStrings.swipeThreshold,
          'swipeThresholdDp',
          s?.swipeThresholdDp ?? 34,
          12,
          96,
          84,
          'dp',
        ),
        const Text(AppStrings.layout),
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
        ),
        if (s?.configPath?.isNotEmpty == true)
          ValueRow(AppStrings.config, s!.configPath!),
        ActionRow(
          children: [
            ActionButton(
              AppStrings.resetMobileKeyboard,
              onPress: enabled ? c.resetAndroidSettings : null,
            ),
          ],
        ),
      ],
    );
  }
}
