import 'package:flutter/material.dart';

import '../pages/about_page.dart';
import '../pages/debug_page.dart';
import '../scheme/scheme_card.dart';
import '../settings/settings_card.dart';
import 'controller.dart';
import 'options.dart';
import 'widgets.dart';

class AppShell extends StatefulWidget {
  const AppShell({super.key, required this.controller});
  final AppController controller;
  @override
  State<AppShell> createState() => _AppShellState();
}

class _AppShellState extends State<AppShell>
    with SingleTickerProviderStateMixin {
  late final TabController _tabs;
  @override
  void initState() {
    super.initState();
    _tabs = TabController(length: AppPage.values.length, vsync: this)
      ..addListener(_tabChanged);
  }

  void _tabChanged() {
    AppPage.values[_tabs.index].onSelected(widget.controller);
  }

  @override
  void dispose() {
    _tabs.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final c = widget.controller;
    return Scaffold(
      appBar: AppBar(
        title: ContentWidth(
          child: Row(
            children: [
              Text(
                'KeyTao',
                style: Theme.of(context).textTheme.headlineSmall
                    ?.copyWith(fontWeight: FontWeight.w700),
              ),
              const SizedBox(width: 16),
              Text(
                c.info.appVersion,
                style: Theme.of(context).textTheme.labelMedium,
              ),
            ],
          ),
        ),
        bottom: TabBar(
          controller: _tabs,
          tabs: [for (final page in AppPage.values) Tab(text: page.title)],
        ),
      ),
      body: SafeArea(
        child: Column(
          children: [
            if (c.error != null)
              ContentWidth(
                child: Padding(
                  padding: const EdgeInsets.all(16),
                  child: Semantics(
                    liveRegion: true,
                    child: SelectableText(
                      c.error!,
                      style: TextStyle(
                        color: Theme.of(context).colorScheme.error,
                      ),
                    ),
                  ),
                ),
              ),
            Expanded(
              child: TabBarView(
                controller: _tabs,
                children: [
                  SingleChildScrollView(
                    key: const PageStorageKey('ime'),
                    padding: const EdgeInsets.all(16),
                    child: ContentWidth(
                      child: Column(
                        children: [
                          SchemeCard(controller: c),
                          const SizedBox(height: 16),
                          SettingsCard(controller: c),
                        ],
                      ),
                    ),
                  ),
                  // Platform view batches will fill the new page slots.
                  const SizedBox.shrink(),
                  const SizedBox.shrink(),
                  AboutPage(controller: c),
                  DebugPage(controller: c),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}
