import 'package:flutter/widgets.dart';
import 'package:forui/forui.dart';

import '../app/controller.dart';
import '../app/options.dart';
import 'about_page.dart';
import 'debug_page.dart';
import 'extension_page.dart';
import 'input_page.dart';
import 'scheme_page.dart';
import 'widgets.dart';

extension PageIcon on AppPage {
  IconData get icon => switch (this) {
    AppPage.input => FLucideIcons.keyboard,
    AppPage.scheme => FLucideIcons.download,
    AppPage.extension => FLucideIcons.settings,
    AppPage.about => FLucideIcons.info,
    AppPage.debug => FLucideIcons.scrollText,
  };
}

class AppShell extends StatefulWidget {
  const AppShell({super.key, required this.controller});
  final AppController controller;
  @override
  State<AppShell> createState() => _AppShellState();
}

class _AppShellState extends State<AppShell> {
  @override
  void initState() {
    super.initState();
    widget.controller.setWindowTitle();
  }

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: widget.controller,
    builder: (context, _) {
      final c = widget.controller;
      void select(int index) {
        if (index == c.activePage.index) return;
        FocusScope.of(context).unfocus();
        c.selectPage(AppPage.values[index]);
      }

      final shell = LayoutBuilder(
        builder: (context, constraints) {
          final wide = constraints.maxWidth >= 720;
          return FScaffold(
            scaffoldStyle: FScaffoldStyleDelta.delta(
              systemOverlayStyle: c.isAndroid
                  ? AppSystemBars.styleOf(context)
                  : null,
            ),
            childPad: false,
            sidebar: wide
                ? FSidebar(
                    style: const FSidebarStyleDelta.delta(
                      constraints: BoxConstraints.tightFor(width: 210),
                      headerPadding: EdgeInsetsGeometryDelta.value(
                        EdgeInsets.zero,
                      ),
                      contentPadding: EdgeInsetsGeometryDelta.value(
                        EdgeInsets.symmetric(horizontal: 10),
                      ),
                    ),
                    header: Padding(
                      padding: const EdgeInsets.fromLTRB(18, 24, 12, 28),
                      child: AppBrand(
                        version: c.info.appVersion,
                        compact: true,
                      ),
                    ),
                    children: [
                      for (final page in AppPage.values)
                        FSidebarItem(
                          key: ValueKey('nav-${page.id}'),
                          icon: Icon(page.icon),
                          label: Text(page.title),
                          selected: c.activePage == page,
                          onPress: () => select(page.index),
                        ),
                    ],
                  )
                : null,
            header: SafeArea(
              bottom: false,
              child: Padding(
                padding: const EdgeInsets.fromLTRB(20, 20, 20, 12),
                child: Align(
                  alignment: Alignment.centerLeft,
                  child: Text(
                    c.activePage.title,
                    style: context.theme.typography.display.xl2.copyWith(
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                ),
              ),
            ),
            footer: wide
                ? null
                : SafeArea(
                    top: false,
                    child: FBottomNavigationBar(
                      index: c.activePage.index,
                      onChange: select,
                      children: [
                        for (final page in AppPage.values)
                          FBottomNavigationBarItem(
                            key: ValueKey('nav-${page.id}'),
                            icon: Icon(page.icon),
                            label: Text(page.title),
                          ),
                      ],
                    ),
                  ),
            child: IndexedStack(
              index: c.activePage.index,
              children: [
                InputPage(controller: c),
                SchemePage(controller: c),
                ExtensionPage(controller: c),
                AboutPage(controller: c),
                DebugPage(controller: c),
              ],
            ),
          );
        },
      );
      return c.isAndroid ? AppSystemBars(child: shell) : shell;
    },
  );
}
