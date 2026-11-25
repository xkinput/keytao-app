import 'package:flutter/widgets.dart';

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
    children: [SettingsCard.skin(controller: controller)],
  );
}
