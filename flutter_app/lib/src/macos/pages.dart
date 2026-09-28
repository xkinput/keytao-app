import 'package:flutter/cupertino.dart';
import 'package:macos_ui/macos_ui.dart';

import '../app/controller.dart';
import '../app/options.dart';
import '../rust/api/types.dart';
import 'widgets.dart';

class MacosSchemeSelection extends StatelessWidget {
  const MacosSchemeSelection({super.key, required this.controller});
  final AppController controller;

  @override
  Widget build(BuildContext context) {
    final c = controller;
    return MacosRows(
      children: [
        MacosChoice(
          label: '方案',
          value: c.scheme,
          options: schemes,
          onChanged: c.busy ? null : c.selectScheme,
        ),
        if (c.scheme == 'keytao')
          MacosChoice(
            label: '下载来源',
            value: c.source,
            options: downloadSources,
            onChanged: c.busy ? null : c.selectSource,
          ),
        MacosSettingRow(
          '最新版本',
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Text(c.releaseVersion ?? '—'),
              const SizedBox(width: 12),
              PushButton(
                controlSize: ControlSize.regular,
                secondary: true,
                onPressed: c.busy ? null : c.refreshRelease,
                child: const Text('刷新版本'),
              ),
            ],
          ),
        ),
      ],
    );
  }
}

class MacosInputPage extends StatelessWidget {
  const MacosInputPage({
    super.key,
    required this.controller,
    this.scrollController,
  });
  final AppController controller;
  final ScrollController? scrollController;

  @override
  Widget build(BuildContext context) {
    final c = controller;
    return MacosForm(
      scrollController: scrollController,
      children: [
        MacosGroup(
          title: '输入方案',
          children: [
            MacosSchemeSelection(controller: c),
            MacosValueRow('KeyTao 目录', c.info.userRoot),
            MacosValueRow('本地状态', c.localStatus),
            MacosValueRow('本地版本', c.local?.version ?? '—'),
            if (c.local?.schemas.isNotEmpty == true)
              MacosValueRow('本地方案', c.local!.schemas.join('、')),
            MacosActionRow(
              children: [
                PushButton(
                  controlSize: ControlSize.regular,
                  secondary: true,
                  onPressed: c.busy ? null : () => c.open(c.info.userRoot),
                  child: const Text('打开目录'),
                ),
                PushButton(
                  controlSize: ControlSize.regular,
                  secondary: true,
                  onPressed: c.busy ? null : c.refreshState,
                  child: const Text('检查本地'),
                ),
                PushButton(
                  controlSize: ControlSize.regular,
                  secondary: true,
                  onPressed: c.canDeploy ? c.deploy : null,
                  child: const Text('部署'),
                ),
                PushButton(
                  controlSize: ControlSize.regular,
                  onPressed: c.busy ? null : c.install,
                  child: const Text('安装方案'),
                ),
              ],
            ),
            if (c.busy || c.progress.isNotEmpty || c.hasOperationLog)
              MacosOperationProgress(controller: c),
          ],
        ),
        MacosGroup(
          title: '输入测试',
          children: const [
            Padding(
              padding: EdgeInsets.all(12),
              child: MacosTextField(
                placeholder: '输入测试',
                minLines: 2,
                maxLines: 5,
              ),
            ),
          ],
        ),
        MacosSettings(controller: c),
      ],
    );
  }
}

class MacosSettings extends StatelessWidget {
  const MacosSettings({super.key, required this.controller});
  final AppController controller;

  @override
  Widget build(BuildContext context) {
    final c = controller;
    final s = c.uiSettings;
    return MacosGroup(
      title: '候选窗设置',
      children: [
        if (s == null)
          MacosSettingRow(
            '设置未读取',
            child: PushButton(
              controlSize: ControlSize.regular,
              secondary: true,
              onPressed: c.busy ? null : c.refreshState,
              child: const Text('重新读取'),
            ),
          )
        else ...[
          MacosChoice<UiColorSchemeDto>(
            label: '配色方案',
            value: s.colorScheme,
            options: colorSchemes,
            onChanged: c.busy ? null : (value) => c.saveUi(colorScheme: value),
          ),
          MacosChoice<PanelOrientationDto>(
            label: '候选排列',
            value: s.orientation,
            options: orientations,
            onChanged: c.busy ? null : (value) => c.saveUi(orientation: value),
          ),
          MacosChoice(
            label: '英文模式',
            value: c.desktopEnglishMode,
            options: c.englishModes,
            disabled: c.disabledEnglishModes,
            onChanged: c.busy ? null : c.saveDesktopEnglish,
          ),
          MacosSettingRow(
            '嵌入模式',
            child: MacosSwitch(
              value: s.embeddedComposition,
              size: ControlSize.regular,
              onChanged: c.busy ? null : c.saveEmbedded,
            ),
          ),
          MacosFontSlider(
            value: s.fontSize,
            onChanged: c.busy ? null : (value) => c.saveUi(fontSize: value),
          ),
          MacosChoice<String>(
            label: '主题色',
            value: s.accentColor.toUpperCase(),
            options: themeAccents,
            onChanged: c.busy ? null : (value) => c.saveUi(accentColor: value),
          ),
          MacosActionRow(
            children: [
              PushButton(
                controlSize: ControlSize.regular,
                secondary: true,
                onPressed: c.canOpenTheme ? () => c.open(s.themePath!) : null,
                child: const Text('打开主题文件'),
              ),
            ],
          ),
        ],
      ],
    );
  }
}

class MacosAboutPage extends StatelessWidget {
  const MacosAboutPage({
    super.key,
    required this.controller,
    this.scrollController,
  });
  final AppController controller;
  final ScrollController? scrollController;

  @override
  Widget build(BuildContext context) => MacosForm(
    scrollController: scrollController,
    children: [
      MacosGroup(
        title: '关于 KeyTao',
        children: [
          for (final entry in controller.aboutValues.entries)
            MacosValueRow(entry.key, entry.value),
          MacosActionRow(
            children: [
              PushButton(
                controlSize: ControlSize.regular,
                secondary: true,
                onPressed: controller.busy
                    ? null
                    : () => controller.open(repositoryUrl),
                child: const Text('GitHub'),
              ),
            ],
          ),
        ],
      ),
    ],
  );
}

class MacosDebugPage extends StatelessWidget {
  const MacosDebugPage({
    super.key,
    required this.controller,
    this.scrollController,
  });
  final AppController controller;
  final ScrollController? scrollController;

  @override
  Widget build(BuildContext context) {
    final c = controller;
    final settings = c.logSettings;
    return MacosForm(
      scrollController: scrollController,
      children: [
        MacosGroup(
          title: '运行日志',
          children: [
            MacosSettingRow(
              '记录运行日志',
              child: MacosSwitch(
                size: ControlSize.regular,
                value: settings?.enabled ?? false,
                onChanged: c.canEditLogs
                    ? (value) => c.saveLogSettings(value, settings!.level)
                    : null,
              ),
            ),
            MacosChoice<RuntimeLogLevelDto>(
              label: '日志级别',
              value: settings?.level ?? RuntimeLogLevelDto.info,
              options: logLevels,
              onChanged: c.canEditLogs
                  ? (value) => c.saveLogSettings(settings!.enabled, value)
                  : null,
            ),
            MacosActionRow(
              children: [
                if (c.busy) const ProgressCircle(radius: 8),
                PushButton(
                  controlSize: ControlSize.regular,
                  secondary: true,
                  onPressed: c.busy ? null : c.clearLogs,
                  child: const Text('清空'),
                ),
                PushButton(
                  controlSize: ControlSize.regular,
                  onPressed: c.busy ? null : c.refreshLogs,
                  child: const Text('刷新'),
                ),
              ],
            ),
            if (c.logLineCount case final count?) MacosValueRow('日志', count),
            SizedBox(
              height: 300,
              child: MacosLogView(
                key: const PageStorageKey('runtimeLogs'),
                lines: c.logLines,
              ),
            ),
          ],
        ),
      ],
    );
  }
}
