import 'package:flutter/widgets.dart';
import 'package:macos_ui/macos_ui.dart';

import '../app/controller.dart';
import '../app/options.dart';
import 'pages.dart';

class MacosShell extends StatefulWidget {
  const MacosShell({super.key, required this.controller});
  final AppController controller;

  @override
  State<MacosShell> createState() => _MacosShellState();
}

class _MacosShellState extends State<MacosShell> {
  final _scrollControllers = {
    for (final page in AppPage.values) page: ScrollController(),
  };

  @override
  void initState() {
    super.initState();
    widget.controller.setWindowTitle();
  }

  @override
  void dispose() {
    for (final controller in _scrollControllers.values) {
      controller.dispose();
    }
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: widget.controller,
    builder: (context, _) {
      final c = widget.controller;
      final page = c.activePage;
      return MacosWindow(
        sidebar: Sidebar(
          minWidth: 190,
          startWidth: 200,
          maxWidth: 240,
          isResizable: false,
          dragClosed: false,
          windowBreakpoint: 0,
          top: Padding(
            padding: const EdgeInsets.fromLTRB(16, 12, 12, 16),
            child: Row(
              children: [
                Image.asset(logoAsset, width: 28, height: 28),
                const SizedBox(width: 10),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      const Text(AppStrings.title),
                      Text(
                        'v${c.info.appVersion}',
                        style: MacosTheme.of(context).typography.body.copyWith(
                          color: MacosDynamicColor.resolve(
                            MacosColors.secondaryLabelColor,
                            context,
                          ),
                        ),
                      ),
                    ],
                  ),
                ),
              ],
            ),
          ),
          builder: (context, scrollController) => SidebarItems(
            scrollController: scrollController,
            currentIndex: page.index,
            onChanged: (index) => c.selectPage(AppPage.values[index]),
            items: [
              for (final entry in AppPage.values)
                SidebarItem(
                  leading: MacosIcon(entry.cupertinoIcon),
                  label: Text(entry.title),
                ),
            ],
          ),
        ),
        child: MacosScaffold(
          toolBar: ToolBar(
            title: Text(page.title),
            titleWidth: 240,
            automaticallyImplyLeading: false,
          ),
          children: [
            ContentArea(
              builder: (context, _) => IndexedStack(
                index: page.index,
                children: [
                  MacosInputPage(
                    controller: c,
                    scrollController: _scrollControllers[AppPage.input],
                  ),
                  MacosSchemePage(
                    controller: c,
                    scrollController: _scrollControllers[AppPage.scheme],
                  ),
                  MacosExtensionPage(
                    controller: c,
                    scrollController: _scrollControllers[AppPage.extension],
                  ),
                  MacosAboutPage(
                    controller: c,
                    scrollController: _scrollControllers[AppPage.about],
                  ),
                  MacosDebugPage(
                    controller: c,
                    scrollController: _scrollControllers[AppPage.debug],
                  ),
                ],
              ),
            ),
          ],
        ),
      );
    },
  );
}
