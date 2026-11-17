import 'package:flutter/widgets.dart';
import 'package:flutter/semantics.dart';
import 'package:forui/forui.dart';

import '../app/controller.dart';
import '../app/options.dart';
import '../rust/api/types.dart';
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
  String get navigationLabel => this == AppPage.extension ? '扩展' : title;

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
                    color: ChromeAccent.of(context),
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

class _AppShellState extends State<AppShell> with WidgetsBindingObserver {
  bool _cloudDialogOpen = false;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    widget.controller.setWindowTitle();
    widget.controller.addListener(_cloudSyncChanged);
    _cloudSyncChanged();
  }

  @override
  void didChangeLocales(List<Locale>? locales) {
    widget.controller.setWindowTitle();
  }

  @override
  void didUpdateWidget(covariant AppShell oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.controller != widget.controller) {
      oldWidget.controller.removeListener(_cloudSyncChanged);
      widget.controller.setWindowTitle();
      widget.controller.addListener(_cloudSyncChanged);
      _cloudSyncChanged();
    }
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
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
          final palette = AppPalette.of(context);
          final keyboardInset = MediaQuery.viewInsetsOf(context).bottom;
          final availableHeight = constraints.maxHeight - keyboardInset;
          return FScaffold(
            scaffoldStyle: FScaffoldStyleDelta.delta(
              backgroundColor: palette.ground,
              headerDecoration: DecorationDelta.value(
                BoxDecoration(
                  border: wide
                      ? Border(bottom: BorderSide(color: palette.hairline))
                      : null,
                ),
              ),
              footerDecoration: const DecorationDelta.value(BoxDecoration()),
              systemOverlayStyle: c.isAndroid
                  ? AppSystemBars.styleOf(context)
                  : null,
            ),
            childPad: false,
            header: keyboardInset > 0 && availableHeight < 160
                ? null
                : SafeArea(
                    bottom: false,
                    child: Padding(
                      padding: wide
                          ? const EdgeInsets.symmetric(
                              horizontal: 24,
                              vertical: 16,
                            )
                          : const EdgeInsets.fromLTRB(16, 20, 16, 12),
                      child: Align(
                        alignment: Alignment.topCenter,
                        child: ConstrainedBox(
                          constraints: BoxConstraints(
                            maxWidth: wide ? 1392 : 760,
                          ),
                          child: SizedBox(
                            width: double.infinity,
                            child: wide
                                ? Wrap(
                                    alignment: WrapAlignment.spaceBetween,
                                    crossAxisAlignment:
                                        WrapCrossAlignment.center,
                                    spacing: 16,
                                    runSpacing: 16,
                                    children: [
                                      _DesktopNavigation(
                                        controller: c,
                                        onSelect: select,
                                      ),
                                      Row(
                                        mainAxisSize: MainAxisSize.min,
                                        children: [
                                          _ShellStatus(controller: c),
                                          const SizedBox(width: 12),
                                          _AccountButton(
                                            controller: c,
                                            showName: true,
                                          ),
                                        ],
                                      ),
                                    ],
                                  )
                                : Row(
                                    children: [
                                      Expanded(
                                        child: Text(
                                          c.activePage.title,
                                          style: context
                                              .theme
                                              .typography
                                              .display
                                              .xl2
                                              .copyWith(
                                                fontSize: 30,
                                                fontWeight: FontWeight.w700,
                                                color: palette.ink,
                                              ),
                                        ),
                                      ),
                                      if (c.activePage == AppPage.input) ...[
                                        _ShellStatus(controller: c),
                                        const SizedBox(width: 4),
                                      ],
                                      _AccountButton(controller: c),
                                    ],
                                  ),
                          ),
                        ),
                      ),
                    ),
                  ),
            footer: wide
                ? null
                : SafeArea(
                    top: false,
                    bottom: false,
                    child: FBottomNavigationBar(
                      safeAreaBottom: true,
                      style: FBottomNavigationBarStyleDelta.delta(
                        decoration: DecorationDelta.value(
                          BoxDecoration(
                            color: palette.surface,
                            border: Border(
                              top: BorderSide(color: palette.hairline),
                            ),
                          ),
                        ),
                        padding: const EdgeInsetsGeometryDelta.value(
                          EdgeInsets.fromLTRB(4, 6, 4, 18),
                        ),
                        itemStyle: FBottomNavigationBarItemStyleDelta.delta(
                          padding: const EdgeInsetsGeometryDelta.value(
                            EdgeInsets.symmetric(vertical: 4),
                          ),
                          iconStyle: FVariants(
                            IconThemeData(color: palette.muted, size: 22),
                            variants: {
                              [FTappableVariant.selected]: IconThemeData(
                                color: accent,
                                size: 22,
                              ),
                            },
                          ),
                          textStyle: FVariants(
                            context.theme.typography.body.sm.copyWith(
                              fontSize: 12,
                              height: 1.5,
                              color: palette.muted,
                            ),
                            variants: {
                              [FTappableVariant.selected]: context
                                  .theme
                                  .typography
                                  .body
                                  .sm
                                  .copyWith(
                                    fontSize: 12,
                                    height: 1.5,
                                    color: accent,
                                    fontWeight: FontWeight.w600,
                                  ),
                            },
                          ),
                        ),
                      ),
                      index: c.activePage.index,
                      onChange: select,
                      children: [
                        for (final page in AppPage.values)
                          FBottomNavigationBarItem(
                            key: ValueKey('nav-${page.id}'),
                            icon: _navigationIcon(page, c, 22),
                            label: Text(
                              page.navigationLabel,
                              semanticsLabel: page.title,
                            ),
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

class _DesktopNavigation extends StatelessWidget {
  const _DesktopNavigation({required this.controller, required this.onSelect});
  final AppController controller;
  final ValueChanged<int> onSelect;

  @override
  Widget build(BuildContext context) {
    final palette = AppPalette.of(context);
    final accent = ChromeAccent.of(context);
    return Semantics(
      key: const ValueKey('desktop-navigation'),
      container: true,
      role: SemanticsRole.tabBar,
      label: '主导航',
      child: DecoratedBox(
        decoration: BoxDecoration(
          color: palette.controlSurface,
          borderRadius: BorderRadius.circular(12),
        ),
        child: Padding(
          padding: const EdgeInsets.all(4),
          child: Wrap(
            spacing: 4,
            runSpacing: 4,
            children: [
              for (final page in AppPage.values)
                FTappable(
                  key: ValueKey('nav-${page.id}'),
                  semanticsButton: false,
                  semanticsRole: SemanticsRole.tab,
                  selected: controller.activePage == page,
                  onPress: () => onSelect(page.index),
                  builder: (context, variants, _) => Container(
                    constraints: const BoxConstraints(minHeight: 44),
                    padding: const EdgeInsets.symmetric(
                      horizontal: 16,
                      vertical: 8,
                    ),
                    decoration: BoxDecoration(
                      color: controller.activePage == page
                          ? accent
                          : variants.contains(FTappableVariant.hovered) ||
                                variants.contains(FTappableVariant.pressed)
                          ? palette.hairline
                          : null,
                      borderRadius: BorderRadius.circular(9),
                    ),
                    child: Row(
                      mainAxisSize: MainAxisSize.min,
                      children: [
                        Text(
                          page.navigationLabel,
                          semanticsLabel: page.title,
                          style: context.theme.typography.body.sm.copyWith(
                            fontSize: 15,
                            height: 1.5,
                            fontWeight: controller.activePage == page
                                ? FontWeight.w600
                                : FontWeight.w500,
                            color: controller.activePage == page
                                ? const Color(0xFFFFFFFF)
                                : palette.ink,
                          ),
                        ),
                        if (page == AppPage.scheme &&
                            controller.hasSchemeUpdates) ...[
                          const SizedBox(width: 8),
                          Semantics(
                            label: AppStrings.checkUpdate,
                            child: Container(
                              key: const ValueKey('scheme-update-indicator'),
                              width: 7,
                              height: 7,
                              decoration: BoxDecoration(
                                shape: BoxShape.circle,
                                color: controller.activePage == page
                                    ? const Color(0xFFFFFFFF)
                                    : accent,
                              ),
                            ),
                          ),
                        ],
                      ],
                    ),
                  ),
                ),
            ],
          ),
        ),
      ),
    );
  }
}

class _AccountButton extends StatelessWidget {
  const _AccountButton({required this.controller, this.showName = false});
  final AppController controller;
  final bool showName;

  @override
  Widget build(BuildContext context) => FTooltip(
    tipBuilder: (_, _) => const Text('账号'),
    child: ConstrainedBox(
      constraints: const BoxConstraints(minHeight: 44, maxWidth: 160),
      child: FButton(
        variant: FButtonVariant.ghost,
        semanticsLabel: '账号',
        mainAxisSize: MainAxisSize.min,
        style: FButtonStyleDelta.delta(
          contentStyle: FButtonContentStyleDelta.delta(
            constraints: const BoxConstraints(minHeight: 44),
            padding: const EdgeInsetsGeometryDelta.value(
              EdgeInsets.symmetric(horizontal: 14, vertical: 8),
            ),
            textStyle: FVariants.all(
              context.theme.typography.body.sm.copyWith(
                fontSize: 15,
                fontWeight: FontWeight.w500,
                color: AppPalette.of(context).ink,
              ),
            ),
          ),
        ),
        onPress: () => showAccountDialog(context, controller),
        prefix: const Icon(FLucideIcons.user, size: 18),
        child: showName
            ? Flexible(
                child: Text(
                  controller.accountName ?? '账号',
                  overflow: TextOverflow.ellipsis,
                ),
              )
            : const SizedBox.shrink(),
      ),
    ),
  );
}

class _ShellStatus extends StatelessWidget {
  const _ShellStatus({required this.controller});
  final AppController controller;

  @override
  Widget build(BuildContext context) {
    final macos = controller.info.platform == BridgePlatform.macOs;
    final mobile = controller.isMobile;
    final ready = switch (controller.info.platform) {
      BridgePlatform.macOs => controller.macosImeBadge == ImeBadgeState.ready,
      BridgePlatform.android =>
        controller.ime?['enabled'] == true &&
            controller.ime?['selected'] == true,
      BridgePlatform.ios => controller.iosKeyboardEnabled,
      _ => controller.local?.deployed == true,
    };
    return StatusPill(
      macos
          ? controller.macosImeBadgeText
          : mobile
          ? ready
                ? '已启用'
                : '未启用'
          : controller.local == null
          ? AppStrings.checking
          : ready
          ? AppStrings.deployed
          : AppStrings.pendingDeploy,
      color: ready
          ? AppPalette.of(context).success
          : AppPalette.of(context).muted,
    );
  }
}
