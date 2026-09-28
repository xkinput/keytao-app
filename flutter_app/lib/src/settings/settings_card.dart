import 'package:flutter/material.dart';

import '../app/controller.dart';
import '../app/options.dart';
import '../app/widgets.dart';
import '../rust/api/types.dart';
import 'android_settings.dart';

class SettingsCard extends StatelessWidget {
  const SettingsCard({super.key, required this.controller});
  final AppController controller;

  @override
  Widget build(BuildContext context) =>
      controller.isAndroid ? _android() : _desktop(context);

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

  Widget _android() {
    final c = controller;
    final s = c.androidSettings;
    if (s == null) return _unavailable('键盘设置');
    void save(String key, Object value) {
      c.saveAndroid({key: value});
    }

    Widget slider(
      String label,
      String field,
      num value,
      double min,
      double max,
      int divisions,
      String unit, {
      bool enabled = true,
    }) => SliderSetting(
      label: label,
      value: value.toDouble(),
      min: min,
      max: max,
      divisions: divisions,
      unit: unit,
      onChanged: c.busy || !enabled ? null : (value) => save(field, value),
    );
    Widget toggle(String label, String field, bool value) => SwitchListTile(
      contentPadding: EdgeInsets.zero,
      title: Text(label),
      value: value,
      onChanged: c.busy ? null : (value) => save(field, value),
    );
    return SectionCard(
      title: '键盘设置',
      children: [
        slider(
          '长按延迟',
          'longPressDelayMs',
          s.longPressDelayMs,
          100,
          700,
          60,
          ' ms',
        ),
        ChoiceSetting<String>(
          label: '英文模式',
          value: s.englishMode,
          options: c.androidEnglishModes,
          disabled: c.disabledAndroidEnglishModes,
          onChanged: c.busy ? null : (value) => save('englishMode', value),
        ),
        slider(
          '键盘高度',
          'keyboardHeightScale',
          s.keyboardHeightScale,
          85,
          130,
          9,
          '%',
        ),
        ChoiceSetting<String>(
          label: '删除速度',
          value: s.deleteSpeed,
          options: const {'slow': '慢', 'standard': '标准', 'fast': '快'},
          onChanged: c.busy ? null : (value) => save('deleteSpeed', value),
        ),
        ChoiceSetting<String>(
          label: '退格滑动模式',
          value: s.backspaceGestureMode,
          options: const {'immediate': '即时删除', 'selectThenDelete': '选中后删除'},
          onChanged: c.busy
              ? null
              : (value) => save('backspaceGestureMode', value),
        ),
        toggle('下拉输入角标符号', 'flickKeysEnabled', s.flickKeysEnabled),
        toggle(
          '双击空格输入句号',
          'doubleSpacePeriodEnabled',
          s.doubleSpacePeriodEnabled,
        ),
        const SizedBox(height: 16),
        slider(
          '滑动判定阈值',
          'swipeThresholdDp',
          s.swipeThresholdDp,
          12,
          96,
          84,
          ' dp',
        ),
        toggle('竖屏悬浮键盘', 'floatingPortraitEnabled', s.floatingPortraitEnabled),
        slider(
          '竖屏缩放',
          'floatingPortraitScale',
          s.floatingPortraitScale,
          70,
          100,
          30,
          '%',
          enabled: s.floatingPortraitEnabled,
        ),
        toggle(
          '横屏悬浮键盘',
          'floatingLandscapeEnabled',
          s.floatingLandscapeEnabled,
        ),
        slider(
          '横屏缩放',
          'floatingLandscapeScale',
          s.floatingLandscapeScale,
          45,
          100,
          55,
          '%',
          enabled: s.floatingLandscapeEnabled,
        ),
        ChoiceSetting<String>(
          label: '回车键',
          value: s.enterKeyBehavior,
          options: const {'system': '智能判断', 'newline': '始终换行'},
          onChanged: c.busy ? null : (value) => save('enterKeyBehavior', value),
        ),
        OutlinedButton(
          onPressed: c.busy ? null : () => c.saveAndroid(androidDefaults),
          child: const Text('恢复默认设置'),
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
