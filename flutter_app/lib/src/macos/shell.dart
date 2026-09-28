import 'package:flutter/widgets.dart';
import 'package:macos_ui/macos_ui.dart';

import '../app/controller.dart';
import '../app/options.dart';
import 'app.dart';
import 'pages.dart';

class MacosShell extends StatefulWidget {
  const MacosShell({super.key, required this.controller});
  final AppController controller;

  @override
  State<MacosShell> createState() => _MacosShellState();
}

class _MacosShellState extends State<MacosShell> {
  AppPage _page = AppPage.input;
  final _scrollControllers = {
    for (final page in AppPage.values) page: ScrollController(),
  };

  @override
  void dispose() {
    for (final controller in _scrollControllers.values) {
      controller.dispose();
    }
    super.dispose();
  }

  void _select(AppPage page) {
    setState(() => _page = page);
    page.onSelected(widget.controller);
  }

  @override
  Widget build(BuildContext context) => MacosWindowTitle(
    title: _page.title,
    child: MacosWindow(
      child: MacosScaffold(
        toolBar: ToolBar(
          height: 72,
          title: Text(_page.title),
          titleWidth: 90,
          automaticallyImplyLeading: false,
          actions: [
            for (final page in AppPage.values)
              CustomToolbarItem(
                inToolbarBuilder: (context) => Semantics(
                  selected: page == _page,
                  child: MacosIconButton(
                    semanticLabel: page.title,
                    boxConstraints: const BoxConstraints.tightFor(
                      width: 72,
                      height: 54,
                    ),
                    padding: const EdgeInsets.symmetric(vertical: 6),
                    backgroundColor: page == _page
                        ? MacosTheme.of(context).primaryColor
                              .withValues(alpha: 0.14)
                        : MacosColors.transparent,
                    onPressed: () => _select(page),
                    icon: Column(
                      children: [
                        MacosIcon(
                          page.cupertinoIcon,
                          size: 22,
                          color: page == _page
                              ? MacosTheme.of(context).primaryColor
                              : null,
                        ),
                        const SizedBox(height: 3),
                        Text(
                          page.title,
                          style: MacosTheme.of(context).typography.body
                              .copyWith(fontSize: 11),
                        ),
                      ],
                    ),
                  ),
                ),
                inOverflowedBuilder: (_) => ToolbarOverflowMenuItem(
                  label: page.title,
                  onPressed: () => _select(page),
                ),
              ),
            ToolBarSpacer(
              spacerUnits: (MediaQuery.sizeOf(context).width - 232) / 64,
            ),
          ],
        ),
        children: [
          ContentArea(
            builder: (context, _) => IndexedStack(
              index: _page.index,
              children: [
                MacosInputPage(
                  controller: widget.controller,
                  scrollController: _scrollControllers[AppPage.input],
                ),
                // Platform view batches will fill the new page slots.
                const SizedBox.shrink(),
                const SizedBox.shrink(),
                MacosAboutPage(
                  controller: widget.controller,
                  scrollController: _scrollControllers[AppPage.about],
                ),
                MacosDebugPage(
                  controller: widget.controller,
                  scrollController: _scrollControllers[AppPage.debug],
                ),
              ],
            ),
          ),
        ],
      ),
    ),
  );
}
