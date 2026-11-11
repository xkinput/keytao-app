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

Widget _navigationIcon(AppPage page, AppController controller) =>
    page == AppPage.scheme && controller.hasSchemeUpdates
    ? Semantics(
        label: AppStrings.checkUpdate,
        child: Stack(
          clipBehavior: Clip.none,
          children: [
            Icon(page.icon),
            Positioned(
              top: -2,
              right: -3,
              child: Builder(
                builder: (context) => Container(
                  key: const ValueKey('scheme-update-indicator'),
                  width: 7,
                  height: 7,
                  decoration: BoxDecoration(
                    color: context.theme.colors.primary,
                    shape: BoxShape.circle,
                  ),
                ),
              ),
            ),
          ],
        ),
      )
    : Icon(page.icon);

class AppShell extends StatefulWidget {
  const AppShell({super.key, required this.controller});
  final AppController controller;
  @override
  State<AppShell> createState() => _AppShellState();
}

class _AppShellState extends State<AppShell> {
  bool _cloudDialogOpen = false;

  @override
  void initState() {
    super.initState();
    widget.controller.setWindowTitle();
    widget.controller.addListener(_cloudSyncChanged);
    _cloudSyncChanged();
  }

  @override
  void didUpdateWidget(covariant AppShell oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.controller != widget.controller) {
      oldWidget.controller.removeListener(_cloudSyncChanged);
      widget.controller.addListener(_cloudSyncChanged);
      _cloudSyncChanged();
    }
  }

  @override
  void dispose() {
    widget.controller.removeListener(_cloudSyncChanged);
    super.dispose();
  }

  void _cloudSyncChanged() {
    final c = widget.controller;
    if (_cloudDialogOpen ||
        c.busy ||
        c.loggingIn ||
        (c.pendingCloudProfile == null && c.pendingCloudActiveScheme == null)) {
      return;
    }
    WidgetsBinding.instance.addPostFrameCallback((_) async {
      if (!mounted || _cloudDialogOpen || c.busy || c.loggingIn) return;
      final profile = c.pendingCloudProfile;
      final active = c.pendingCloudActiveScheme;
      if (profile == null && active == null) return;
      _cloudDialogOpen = true;
      final confirmed = await showFDialog<bool>(
        context: context,
        builder: (context, _, animation) => AppDialog(
          title: profile != null ? '从云端下载' : '启用${schemeDisplayName(active!)}？',
          animation: animation,
          actions: [
            ActionButton(
              AppStrings.cancel,
              onPress: () => Navigator.pop(context, false),
            ),
            ActionButton(
              profile != null ? '应用' : '启用',
              primary: true,
              onPress: () => Navigator.pop(context, true),
            ),
          ],
          child: const SizedBox.shrink(),
        ),
      );
      if (profile != null) {
        if (confirmed == true) await c.applyCloudProfile();
        c.dismissCloudProfileOffer();
      } else {
        c.dismissCloudActiveScheme();
        if (confirmed == true) await c.activateSchemePackage(active!);
      }
      _cloudDialogOpen = false;
      if (mounted) _cloudSyncChanged();
    });
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
                          icon: _navigationIcon(page, c),
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
                            icon: _navigationIcon(page, c),
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
