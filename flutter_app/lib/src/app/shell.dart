import 'package:flutter/material.dart';

import '../pages/about_page.dart';
import '../pages/debug_page.dart';
import '../pages/extension_page.dart';
import '../pages/input_page.dart';
import '../scheme/scheme_card.dart';
import 'controller.dart';
import 'options.dart';
import 'widgets.dart';

class AppShell extends StatelessWidget {
  const AppShell({super.key, required this.controller});
  final AppController controller;

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: controller,
    builder: (context, _) => AndroidScaffold(
      body: SafeArea(
        bottom: false,
        child: IndexedStack(
          index: controller.activePage.index,
          children: [
            InputPage(controller: controller),
            AppPageBody(
              controller: controller,
              page: AppPage.scheme,
              children: [
                SchemeCard(controller: controller),
                AddonCard(controller: controller),
              ],
            ),
            ExtensionPage(controller: controller),
            AboutPage(controller: controller),
            DebugPage(controller: controller),
          ],
        ),
      ),
      bottomNavigationBar: NavigationBar(
        selectedIndex: controller.activePage.index,
        onDestinationSelected: (index) {
          if (index == controller.activePage.index) return;
          FocusScope.of(context).unfocus();
          controller.selectPage(AppPage.values[index]);
        },
        labelBehavior: NavigationDestinationLabelBehavior.alwaysShow,
        destinations: [
          for (final page in AppPage.values)
            NavigationDestination(
              icon: Icon(page.materialIcon),
              label: page.title,
              tooltip: page.title,
            ),
        ],
      ),
    ),
  );
}
