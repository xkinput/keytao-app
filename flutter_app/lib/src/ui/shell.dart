import 'package:flutter/widgets.dart';
import 'package:forui/forui.dart';

import '../app/controller.dart';
import '../app/options.dart';
import 'about_page.dart';
import 'color_picker.dart';
import 'debug_page.dart';
import 'extension_page.dart';
import 'input_page.dart';
import 'login.dart';
import 'scheme_page.dart';
import 'theme.dart';
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

Widget _navigationIcon(AppPage page, AppController controller, double size) =>
    page == AppPage.scheme && controller.hasSchemeUpdates
    ? Semantics(
        label: AppStrings.checkUpdate,
        child: Stack(
          clipBehavior: Clip.none,
          children: [
            Icon(page.icon, size: size),
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
    : Icon(page.icon, size: size);

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
      final accent = parseAccentColor(c.uiSettings?.accentColor ?? '#3B73D9');
      void select(int index) {
        if (index == c.activePage.index) return;
        FocusScope.of(context).unfocus();
        c.selectPage(AppPage.values[index]);
      }

      final shell = LayoutBuilder(
        builder: (context, constraints) {
          final wide = constraints.maxWidth >= 720;
          final touch = context.platformVariant.touch;
          final iconSize = touch ? 18.0 : 16.0;
          final display = context.theme.typography.display;
          final titleStyle = (touch ? display.xl2 : display.xl).copyWith(
            fontWeight: FontWeight.w500,
          );
          final keyboardInset = MediaQuery.viewInsetsOf(context).bottom;
          final availableHeight = constraints.maxHeight - keyboardInset;
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
                      constraints: BoxConstraints.tightFor(width: 216),
                      headerPadding: EdgeInsetsGeometryDelta.value(
                        EdgeInsets.zero,
                      ),
                      contentPadding: EdgeInsetsGeometryDelta.value(
                        EdgeInsets.symmetric(horizontal: 8),
                      ),
                    ),
                    header: Padding(
                      padding: const EdgeInsets.fromLTRB(16, 24, 12, 24),
                      child: AppBrand(
                        version: c.info.appVersion,
                        compact: true,
                      ),
                    ),
                    children: [
                      for (final page in AppPage.values)
                        AnimatedContainer(
                          duration: MediaQuery.disableAnimationsOf(context)
                              ? Duration.zero
                              : const Duration(milliseconds: 150),
                          curve: Curves.easeOutCubic,
                          decoration: BoxDecoration(
                            color: c.activePage == page
                                ? accent.withValues(alpha: .12)
                                : const Color(0x00000000),
                            borderRadius: BorderRadius.circular(8),
                          ),
                          foregroundDecoration: BoxDecoration(
                            border: Border.all(
                              color: c.activePage == page
                                  ? accent
                                  : const Color(0x00000000),
                            ),
                            borderRadius: BorderRadius.circular(8),
                          ),
                          child: FSidebarItem(
                            key: ValueKey('nav-${page.id}'),
                            style: FSidebarItemStyleDelta.delta(
                              backgroundColor: c.activePage == page
                                  ? const FVariants.all(Color(0x00000000))
                                  : null,
                              borderRadius: BorderRadius.circular(8),
                            ),
                            icon: _navigationIcon(page, c, iconSize),
                            label: Text(page.title),
                            selected: c.activePage == page,
                            onPress: () => select(page.index),
                          ),
                        ),
                    ],
                  )
                : null,
            header: keyboardInset > 0 && availableHeight < 160
                ? null
                : SafeArea(
                    bottom: false,
                    child: Padding(
                      padding: EdgeInsets.fromLTRB(
                        touch ? 16 : 32,
                        20,
                        touch ? 16 : 32,
                        12,
                      ),
                      child: Align(
                        alignment: Alignment.topCenter,
                        child: ConstrainedBox(
                          constraints: const BoxConstraints(maxWidth: 760),
                          child: Row(
                            children: [
                              Expanded(
                                child: Text(
                                  c.activePage.title,
                                  style: titleStyle,
                                ),
                              ),
                              FTooltip(
                                tipBuilder: (_, _) => const Text('账号'),
                                child: FButton.icon(
                                  variant: FButtonVariant.ghost,
                                  onPress: () => showAccountDialog(context, c),
                                  child: Icon(
                                    FLucideIcons.user,
                                    size: iconSize,
                                    semanticLabel: '账号',
                                  ),
                                ),
                              ),
                            ],
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
                            icon: _navigationIcon(page, c, iconSize),
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
      return ChromeAccent(
        color: accent,
        child: c.isAndroid ? AppSystemBars(child: shell) : shell,
      );
    },
  );
}
