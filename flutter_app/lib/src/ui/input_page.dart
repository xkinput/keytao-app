import 'package:flutter/widgets.dart';
import 'package:forui/forui.dart';

import '../app/controller.dart';
import '../app/options.dart';
import '../rust/api/types.dart';
import 'settings.dart';
import 'widgets.dart';

class InputPage extends StatelessWidget {
  const InputPage({super.key, required this.controller});
  final AppController controller;
  @override
  Widget build(BuildContext context) => AppPageBody(
    controller: controller,
    page: AppPage.input,
    children: [
      if (controller.isAndroid) ...[
        AppBrand(version: controller.info.appVersion),
        AndroidImeCard(controller: controller),
      ],
      if (controller.info.platform == BridgePlatform.macOs)
        MacosImeCard(controller: controller),
      SettingsCard(controller: controller),
    ],
  );
}

class MacosImeCard extends StatelessWidget {
  const MacosImeCard({super.key, required this.controller});
  final AppController controller;
  @override
  Widget build(BuildContext context) => Section(
    title: AppStrings.macosIme,
    icon: FLucideIcons.keyboard,
    trailing: FBadge(
      variant: FBadgeVariant.outline,
      child: Text(
        controller.macosImeBadgeText,
        style: TextStyle(
          color: controller.macosImeBadge == ImeBadgeState.ready
              ? successColor(context)
              : null,
        ),
      ),
    ),
    children: [
      if (controller.macosIme?.appPath case final path?)
        ValueRow(AppStrings.systemLocation, path),
      if (controller.macosImeError case final error?) ErrorMessage(error),
    ],
  );
}

class AndroidImeCard extends StatelessWidget {
  const AndroidImeCard({super.key, required this.controller});
  final AppController controller;
  @override
  Widget build(BuildContext context) {
    final c = controller;
    Widget status(String label, bool done) => Row(
      children: [
        Icon(
          done ? FLucideIcons.circleCheck : FLucideIcons.circle,
          size: 18,
          color: done ? successColor(context) : null,
        ),
        const SizedBox(width: 10),
        Expanded(child: Text(label)),
      ],
    );
    return Section(
      title: AppStrings.imeSettings,
      icon: FLucideIcons.keyboard,
      children: [
        ValueRow(AppStrings.directory, c.defaultDir),
        if (c.storage != null &&
            (c.storage!['granted'] != true ||
                c.migrationError?.isNotEmpty == true))
          ActionRow(
            children: [
              ActionButton(
                c.storageActionLabel,
                icon: FLucideIcons.folderOpen,
                onPress: c.canOpenStorageSettings
                    ? c.openStoragePermissionSettings
                    : null,
              ),
            ],
          ),
        if (c.migrationError case final error?) ErrorMessage(error),
        status(AppStrings.enableKeytao, c.ime?['enabled'] == true),
        status(AppStrings.selectKeytao, c.ime?['selected'] == true),
        if (c.imeMessage?.isNotEmpty == true) StatusMessage(c.imeMessage!),
        ActionRow(
          children: [
            ActionButton(
              AppStrings.openSystemSettings,
              icon: FLucideIcons.settings,
              onPress: c.busy ? null : c.openInputMethodSettings,
            ),
            ActionButton(
              AppStrings.chooseKeytao,
              primary: true,
              icon: FLucideIcons.keyboard,
              onPress: c.busy || c.ime?['canShowPicker'] != true
                  ? null
                  : c.showInputMethodPicker,
            ),
            ActionButton(
              AppStrings.recheck,
              icon: FLucideIcons.refreshCw,
              tooltip: AppStrings.recheck,
              onPress: c.busy ? null : c.refreshState,
            ),
          ],
        ),
      ],
    );
  }
}
