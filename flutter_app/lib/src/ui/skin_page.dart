import 'package:flutter/widgets.dart';
import 'package:forui/forui.dart';

import '../app/controller.dart';
import '../app/options.dart';
import 'settings.dart';
import 'widgets.dart';

class SkinPage extends StatelessWidget {
  const SkinPage({super.key, required this.controller});
  final AppController controller;

  @override
  Widget build(BuildContext context) => AppPageBody(
    controller: controller,
    page: AppPage.skin,
    fullWidth: true,
    spacing: 0,
    children: [
      SettingsCard.skin(controller: controller),
      if (!controller.isMobile && controller.uiSettings?.themePath != null)
        HairlineGridCell(
          key: const ValueKey('skin-theme-path'),
          child: ValueRow(
            AppStrings.theme,
            controller.uiSettings!.themePath!,
            trailing: FTooltip(
              tipBuilder: (_, _) => const Text(AppStrings.openTheme),
              child: FButton.icon(
                onPress: controller.canOpenTheme ? controller.openTheme : null,
                variant: FButtonVariant.ghost,
                child: const Icon(
                  FLucideIcons.folderOpen,
                  size: 16,
                  semanticLabel: AppStrings.openTheme,
                ),
              ),
            ),
          ),
        ),
    ],
  );
}
