import 'package:flutter/cupertino.dart';
import 'package:macos_ui/macos_ui.dart';

import '../app/controller.dart';
import '../app/options.dart';
import '../rust/api/types.dart';
import 'color_picker.dart';
import 'widgets.dart';

export 'debug.dart';
export 'extension.dart';
export 'scheme.dart';

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
        MacosUpdateBanner(controller: c),
        MacosGroup(
          title: AppStrings.macosIme,
          icon: CupertinoIcons.keyboard,
          trailing: MacosBadge(
            c.macosImeBadgeText,
            color: c.macosImeBadge == ImeBadgeState.ready
                ? MacosColors.systemGreenColor
                : null,
          ),
          children: [
            if (c.macosIme?.installed == true)
              const MacosMessage(
                AppStrings.macosLoginWarning,
                color: MacosColors.systemOrangeColor,
                icon: CupertinoIcons.exclamationmark_triangle,
              ),
            if (c.macosIme?.appPath case final path?)
              MacosValueRow(AppStrings.systemLocation, path),
            if (c.macosImeError case final error?)
              MacosMessage(
                error,
                color: MacosColors.systemRedColor,
                icon: CupertinoIcons.exclamationmark_triangle,
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
      title: AppStrings.imeSettings,
      icon: CupertinoIcons.paintbrush,
      children: [
        if (s == null)
          MacosSettingRow(
            AppStrings.checking,
            child: PushButton(
              controlSize: ControlSize.regular,
              secondary: true,
              onPressed: c.busy ? null : c.refreshState,
              child: const Text(AppStrings.recheck),
            ),
          )
        else ...[
          MacosChoice<UiColorSchemeDto>(
            value: s.colorScheme,
            options: colorSchemes,
            icons: const {
              UiColorSchemeDto.auto: CupertinoIcons.desktopcomputer,
              UiColorSchemeDto.light: CupertinoIcons.sun_max,
              UiColorSchemeDto.dark: CupertinoIcons.moon,
            },
            onChanged: c.busy ? null : (value) => c.saveUi(colorScheme: value),
          ),
          MacosChoice<PanelOrientationDto>(
            value: s.orientation,
            options: orientations,
            icons: const {
              PanelOrientationDto.horizontal:
                  CupertinoIcons.rectangle_split_3x1,
              PanelOrientationDto.vertical: CupertinoIcons.line_horizontal_3,
            },
            onChanged: c.busy ? null : (value) => c.saveUi(orientation: value),
          ),
          MacosChoice(
            label: AppStrings.englishMode,
            value: c.desktopEnglishMode,
            options: c.englishModes,
            disabled: c.disabledEnglishModes,
            onChanged: c.busy ? null : c.saveDesktopEnglish,
          ),
          if (c.englishModeHint case final hint?) MacosMessage(hint),
          MacosSettingRow(
            AppStrings.embedded,
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                Text(s.embeddedComposition ? AppStrings.on : AppStrings.off),
                const SizedBox(width: 8),
                MacosSwitch(
                  value: s.embeddedComposition,
                  size: ControlSize.regular,
                  onChanged: c.busy ? null : c.saveEmbedded,
                ),
              ],
            ),
          ),
          const MacosMessage(AppStrings.embeddedDescription),
          MacosFontSlider(
            value: s.fontSize,
            onChanged: c.busy ? null : (value) => c.saveUi(fontSize: value),
          ),
          MacosAccentPicker(controller: c),
          if (s.themePath case final path?)
            Padding(
              padding: const EdgeInsets.all(12),
              child: Row(
                children: [
                  Expanded(child: Text('${AppStrings.theme}$path')),
                  MacosTooltip(
                    message: AppStrings.openTheme,
                    child: MacosIconButton(
                      semanticLabel: AppStrings.openTheme,
                      icon: const MacosIcon(CupertinoIcons.folder_open),
                      onPressed: c.canOpenTheme ? c.openTheme : null,
                    ),
                  ),
                ],
              ),
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
      MacosUpdateBanner(controller: controller),
      Padding(
        padding: const EdgeInsets.symmetric(vertical: 24),
        child: Column(
          children: [
            Image.asset(logoAsset, width: 80, height: 80),
            const SizedBox(height: 12),
            Text(
              AppStrings.title,
              style: MacosTheme.of(context).typography.title1,
            ),
            const SizedBox(height: 6),
            Text('v${controller.info.appVersion}'),
            const SizedBox(height: 12),
            const Text(AppStrings.tagline, textAlign: TextAlign.center),
          ],
        ),
      ),
      MacosGroup(
        title: AppStrings.about,
        icon: CupertinoIcons.info_circle,
        children: [
          for (final entry in controller.aboutValues.entries)
            MacosValueRow(switch (entry.key) {
              'KeyTao' => AppStrings.keytaoVersion,
              'librime' => AppStrings.librimeVersion,
              'OpenCC' => AppStrings.openccVersion,
              'KeyTao 目录' => AppStrings.keytaoDirectory,
              _ => entry.key,
            }, entry.value),
          MacosSettingRow(
            'GitHub',
            child: PushButton(
              controlSize: ControlSize.regular,
              secondary: true,
              onPressed: controller.busy
                  ? null
                  : () => controller.open(repositoryUrl),
              child: Text(Uri.parse(repositoryUrl).path.substring(1)),
            ),
          ),
        ],
      ),
    ],
  );
}
