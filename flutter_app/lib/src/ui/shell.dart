import 'package:flutter/gestures.dart'
    show
        DragStartBehavior,
        HorizontalDragGestureRecognizer,
        PointerDeviceKind,
        computePanSlop,
        kTouchSlop;
import 'package:flutter/widgets.dart';
import 'package:flutter/semantics.dart';
import 'package:forui/forui.dart';

import '../app/controller.dart';
import '../app/options.dart';
import '../rust/api/types.dart';
import 'about_page.dart';
import 'input_page.dart';
import 'keyboard_library.dart';
import 'library_page.dart';
import 'login.dart';
import 'scheme_page.dart';
import 'skin_page.dart';
import 'theme.dart';
import 'website_page.dart';
import 'widgets.dart';

const _mobileChromeTopExtent = 8.0;
const _mobileChromeCollapseExtent = 96.0;

extension PageIcon on AppPage {
  IconData get icon => switch (this) {
    AppPage.input => FLucideIcons.keyboard,
    AppPage.scheme => FLucideIcons.download,
    AppPage.skin => FLucideIcons.palette,
    AppPage.library => FLucideIcons.library,
    AppPage.about => FLucideIcons.info,
    AppPage.website => FLucideIcons.globe,
  };
}

bool _showSchemeUpdateIndicator(AppPage page, AppController controller) =>
    page ==
        (controller.info.platform == BridgePlatform.macOs
            ? AppPage.input
            : AppPage.scheme) &&
    controller.hasSchemeUpdates;

Widget _navigationIcon(AppPage page, AppController controller, double size) =>
    _showSchemeUpdateIndicator(page, controller)
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
  bool _recommendedKeyboardDialogOpen = false;
  late AppPage _chromePage = widget.controller.activePage;
  final _mobileChrome = ValueNotifier(1.0);
  double? _scrollAnchor;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    widget.controller.setWindowTitle();
    widget.controller.addListener(_controllerChanged);
    _controllerChanged();
  }

  @override
  void didChangeLocales(List<Locale>? locales) {
    widget.controller.setWindowTitle();
  }

  @override
  void didUpdateWidget(covariant AppShell oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.controller != widget.controller) {
      oldWidget.controller.removeListener(_controllerChanged);
      widget.controller.setWindowTitle();
      widget.controller.addListener(_controllerChanged);
      _controllerChanged();
    }
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    widget.controller.removeListener(_controllerChanged);
    _mobileChrome.dispose();
    super.dispose();
  }

  void _controllerChanged() {
    final c = widget.controller;
    if (_chromePage != c.activePage) {
      _chromePage = c.activePage;
      _mobileChrome.value = 1;
      _scrollAnchor = null;
    }
    if (c.isMobile &&
        !_recommendedKeyboardDialogOpen &&
        !_cloudDialogOpen &&
        !c.busy &&
        !c.loggingIn &&
        c.pendingRecommendedKeyboard != null) {
      WidgetsBinding.instance.addPostFrameCallback((_) async {
        if (!mounted ||
            _recommendedKeyboardDialogOpen ||
            _cloudDialogOpen ||
            c.busy ||
            c.loggingIn) {
          return;
        }
        final recommended = c.pendingRecommendedKeyboard;
        if (recommended == null) return;
        _recommendedKeyboardDialogOpen = true;
        final confirmed = await showRecommendedKeyboardDialog(
          context,
          c,
          recommended,
        );
        _recommendedKeyboardDialogOpen = false;
        if (confirmed == true) {
          await c.applyKeyboardProfile(recommended.id);
        } else {
          c.dismissRecommendedKeyboard();
        }
      });
      return;
    }
    if (_cloudDialogOpen ||
        c.busy ||
        (c.savingTheme && c.pendingCloudProfile != null) ||
        c.loggingIn ||
        (c.pendingCloudProfile == null && c.pendingCloudActiveScheme == null)) {
      return;
    }
    WidgetsBinding.instance.addPostFrameCallback((_) async {
      if (!mounted ||
          _cloudDialogOpen ||
          c.busy ||
          (c.savingTheme && c.pendingCloudProfile != null) ||
          c.loggingIn) {
        return;
      }
      final profile = c.pendingCloudProfile;
      final active = c.pendingCloudActiveScheme;
      if (profile == null && active == null) return;
      _cloudDialogOpen = true;
      final confirmed = await showFDialog<bool>(
        context: context,
        builder: (context, _, animation) {
          final cloudScheme = profile?['active_scheme'] as String?;
          return AppDialog(
            title: profile != null
                ? '同步云端账号配置'
                : '启用${c.packageDisplayName(active!)}？',
            animation: animation,
            actions: [
              ActionButton(
                profile != null ? '暂不应用' : AppStrings.cancel,
                onPress: () => Navigator.pop(context, false),
              ),
              ActionButton(
                profile != null ? '下载并应用' : '启用',
                primary: true,
                onPress: () => Navigator.pop(context, true),
              ),
            ],
            child: profile != null
                ? Column(
                    mainAxisSize: MainAxisSize.min,
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    spacing: 6,
                    children: [
                      const Text('检测到账号云端保存的输入方案、外观与输入设置与当前设备不同。'),
                      if (cloudScheme != null && cloudScheme.isNotEmpty)
                        Text('云端当前方案：${c.packageDisplayName(cloudScheme)}'),
                      const Text('点击「下载并应用」将用云端配置更新本机设置；点击「暂不应用」将保留当前设备设置。'),
                    ],
                  )
                : const SizedBox.shrink(),
          );
        },
      );
      if (profile != null) {
        if (confirmed == true) await c.applyCloudProfile();
        c.dismissCloudProfileOffer();
      } else {
        c.dismissCloudActiveScheme();
        if (confirmed == true) await c.activateSchemePackage(active!);
      }
      _cloudDialogOpen = false;
      if (mounted) _controllerChanged();
    });
  }

  void _mobileScrolled(AppPage page, double offset) {
    if (!widget.controller.isMobile ||
        page != widget.controller.activePage ||
        !offset.isFinite) {
      return;
    }
    final previous = _scrollAnchor ?? offset;
    _scrollAnchor = offset;
    if (offset <= _mobileChromeTopExtent) {
      _mobileChrome.value = 1;
    } else {
      _mobileChrome.value =
          (_mobileChrome.value -
                  (offset - previous) / _mobileChromeCollapseExtent)
              .clamp(0.0, 1.0);
    }
  }

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: widget.controller,
    builder: (context, _) {
      final c = widget.controller;
      void select(AppPage page) {
        if (page == c.activePage) return;
        FocusScope.of(context).unfocus();
        c.selectPage(page);
      }

      final shell = LayoutBuilder(
        builder: (context, constraints) {
          final wide = !c.isMobile && constraints.maxWidth >= 720;
          final palette = AppPalette.of(context);
          final accent = ChromeAccent.of(context);
          final keyboardInset = MediaQuery.viewInsetsOf(context).bottom;
          final availableHeight = constraints.maxHeight - keyboardInset;
          final hideHeaderForKeyboard =
              keyboardInset > 0 && availableHeight < 160;
          final mobileWebsite = c.isMobile && c.activePage == AppPage.website;
          final reduceMotion = MediaQuery.disableAnimationsOf(context);
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
            header: hideHeaderForKeyboard || mobileWebsite
                ? null
                : SafeArea(
                    bottom: false,
                    child: ScrollChrome(
                      fraction: _mobileChrome,
                      alignment: Alignment.bottomCenter,
                      child: Padding(
                        padding: wide
                            ? const EdgeInsets.symmetric(
                                horizontal: 18,
                                vertical: 12,
                              )
                            : const EdgeInsets.symmetric(horizontal: 12),
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
                                                  fontSize: 20,
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
                  ),
            footer: wide
                ? null
                : SafeArea(
                    top: false,
                    child: ScrollChrome(
                      fraction: _mobileChrome,
                      alignment: Alignment.topCenter,
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
                            EdgeInsets.fromLTRB(4, 1, 4, 2),
                          ),
                          itemStyle: FBottomNavigationBarItemStyleDelta.delta(
                            padding: const EdgeInsetsGeometryDelta.value(
                              EdgeInsets.symmetric(vertical: 4),
                            ),
                            iconStyle: FVariants(
                              IconThemeData(color: palette.muted, size: 20),
                              variants: {
                                [FTappableVariant.selected]: IconThemeData(
                                  color: accent,
                                  size: 20,
                                ),
                              },
                            ),
                            textStyle: FVariants(
                              context.theme.typography.body.sm.copyWith(
                                fontSize: 12,
                                height: 1.2,
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
                                      height: 1.2,
                                      color: palette.ink,
                                      fontWeight: FontWeight.w600,
                                    ),
                              },
                            ),
                          ),
                        ),
                        index: c.visiblePages.indexOf(c.activePage),
                        onChange: (index) => select(c.visiblePages[index]),
                        children: [
                          for (final page in c.visiblePages)
                            FBottomNavigationBarItem(
                              key: ValueKey('nav-${page.id}'),
                              icon: _navigationIcon(page, c, 20),
                              label: Text(
                                page.title,
                                textAlign: TextAlign.center,
                              ),
                            ),
                        ],
                      ),
                    ),
                  ),
            child: _PageDeck(
              activePage: c.activePage,
              animate: !reduceMotion,
              touchOnly: c.isMobile,
              nativeWebsiteOverlay: c.isLinux,
              onSelect: select,
              children: {
                AppPage.input: InputPage(controller: c),
                if (c.visiblePages.contains(AppPage.scheme))
                  AppPage.scheme: SchemePage(controller: c),
                AppPage.skin: SkinPage(controller: c),
                AppPage.library: LibraryPage(controller: c),
                AppPage.about: AboutPage(controller: c),
                if (!c.websiteHidden)
                  AppPage.website: WebsitePage(
                    controller: c,
                    active: c.activePage == AppPage.website,
                    mobileAccount: c.isMobile
                        ? _AccountButton(controller: c)
                        : null,
                    mobileToolbarVisible: !hideHeaderForKeyboard,
                    mobileToolbarFraction: _mobileChrome,
                    onScrollOffset: c.isMobile
                        ? (offset) => _mobileScrolled(AppPage.website, offset)
                        : null,
                  ),
              },
              onScrollOffset: c.isMobile ? _mobileScrolled : null,
            ),
          );
        },
      );
      return c.isAndroid ? AppSystemBars(child: shell) : shell;
    },
  );
}

class _PageDeck extends StatefulWidget {
  const _PageDeck({
    required this.activePage,
    required this.animate,
    required this.touchOnly,
    required this.nativeWebsiteOverlay,
    required this.onSelect,
    required this.children,
    required this.onScrollOffset,
  });

  final AppPage activePage;
  final bool animate;
  final bool touchOnly;
  final bool nativeWebsiteOverlay;
  final ValueChanged<AppPage> onSelect;
  final Map<AppPage, Widget> children;
  final void Function(AppPage, double)? onScrollOffset;

  @override
  State<_PageDeck> createState() => _PageDeckState();
}

enum _PageDragOwner { touch, precise }

class _PageDeckState extends State<_PageDeck>
    with SingleTickerProviderStateMixin {
  late final _slide =
      AnimationController(
        vsync: this,
        duration: const Duration(milliseconds: 190),
        value: 1,
      )..addStatusListener((status) {
        if (status == AnimationStatus.completed && _outgoing != null) {
          setState(() => _outgoing = null);
        }
      });
  AppPage? _outgoing;
  AppPage? _incoming;
  final _retainedConstraints = <AppPage, BoxConstraints>{};
  double _direction = 1;
  double _dragDistance = 0;
  _PageDragOwner? _dragOwner;

  @override
  void didUpdateWidget(covariant _PageDeck oldWidget) {
    super.didUpdateWidget(oldWidget);
    final switching = widget.activePage != oldWidget.activePage;
    // The GTK WebView is an independent overlay. Keep it in its final bounds.
    final switchingNativeWebsite =
        switching &&
        widget.nativeWebsiteOverlay &&
        (oldWidget.activePage == AppPage.website ||
            widget.activePage == AppPage.website);
    final targetRemoved =
        _incoming != null && !widget.children.containsKey(_incoming);
    if (targetRemoved ||
        switchingNativeWebsite ||
        (!widget.animate && (switching || _outgoing != null))) {
      _dragOwner = null;
      _outgoing = null;
      _incoming = null;
      _slide.value = 1;
    } else if (switching) {
      final progress = _incoming == widget.activePage ? _slide.value : 0.0;
      _dragOwner = null;
      _incoming = null;
      _outgoing = oldWidget.activePage;
      _direction = widget.activePage.index > oldWidget.activePage.index
          ? 1
          : -1;
      _slide.value = progress;
      if (progress == 1) {
        _outgoing = null;
      } else {
        _slide.animateTo(1, curve: Curves.easeOutCubic);
      }
    }
  }

  @override
  void dispose() {
    _slide.dispose();
    super.dispose();
  }

  void _startDrag(_PageDragOwner owner) {
    if (_dragOwner != null && _dragOwner != owner) return;
    _dragOwner = owner;
    _dragDistance = 0;
    if (_outgoing != null || _incoming != null) {
      setState(() {
        _outgoing = null;
        _incoming = null;
      });
      _slide.value = 0;
    }
  }

  void _updateDrag(_PageDragOwner owner, DragUpdateDetails details) {
    if (_dragOwner != owner) return;
    final width = context.size?.width ?? 0;
    if (width <= 0) return;
    _dragDistance += details.primaryDelta ?? 0;
    final direction = _dragDistance < 0 ? 1 : -1;
    final pages = widget.children.keys.toList(growable: false);
    final adjacent = pages.indexOf(widget.activePage) + direction;
    final target =
        _dragDistance != 0 && adjacent >= 0 && adjacent < pages.length
        ? pages[adjacent]
        : null;
    if (_incoming != target) {
      setState(() {
        _incoming = target;
        _direction = direction.toDouble();
      });
    }
    _slide.value = target == null
        ? 0
        : (_dragDistance.abs() / width).clamp(0.0, 1.0);
  }

  void _endDrag(_PageDragOwner owner, DragEndDetails details) {
    if (_dragOwner != owner) return;
    final target = _incoming;
    final velocity = -(details.primaryVelocity ?? 0) * _direction;
    if (target != null &&
        (_slide.value >= .35 || (velocity > 500 && _slide.value > .06))) {
      _dragOwner = null;
      widget.onSelect(target);
    } else {
      _cancelDrag(owner);
    }
  }

  void _cancelDrag(_PageDragOwner owner) {
    if (_dragOwner != owner) return;
    _dragOwner = null;
    if (_incoming == null) return;
    if (!widget.animate || _slide.value == 0) {
      setState(() => _incoming = null);
      return;
    }
    _slide.animateBack(0, curve: Curves.easeOutCubic).then((_) {
      if (mounted) setState(() => _incoming = null);
    });
  }

  Widget _page(AppPage page, Widget child) {
    final active = page == widget.activePage;
    final entering =
        _incoming ?? (_outgoing == null ? null : widget.activePage);
    final leaving = _incoming == null ? _outgoing : widget.activePage;
    final moving = page == entering || page == leaving;
    final visible = active || moving;
    final offset = Offset(_direction, 0);
    return Positioned.fill(
      key: ValueKey(page),
      child: Visibility(
        visible: visible,
        maintainState: true,
        child: ConstraintsTransformBox(
          constraintsTransform: (constraints) {
            // Keep hidden pages from relaying out with every change in bar height.
            final previous = _retainedConstraints[page];
            if (visible || previous == null) {
              return _retainedConstraints[page] = constraints;
            }
            if (previous.minWidth == constraints.minWidth &&
                previous.maxWidth == constraints.maxWidth) {
              return previous;
            }
            return _retainedConstraints[page] = previous.copyWith(
              minWidth: constraints.minWidth,
              maxWidth: constraints.maxWidth,
            );
          },
          child: ExcludeSemantics(
            excluding: !active,
            child: ExcludeFocus(
              excluding: !active,
              child: IgnorePointer(
                ignoring: !active,
                child: TickerMode(
                  enabled: visible,
                  child: SlideTransition(
                    key: ValueKey('page-slide-${page.id}'),
                    position: moving
                        ? Tween(
                            begin: page == entering ? offset : Offset.zero,
                            end: page == leaving ? -offset : Offset.zero,
                          ).animate(_slide)
                        : const AlwaysStoppedAnimation(Offset.zero),
                    child: NotificationListener<Notification>(
                      onNotification: (notification) {
                        if (!active || widget.onScrollOffset == null) {
                          return false;
                        }
                        final metrics = switch (notification) {
                          ScrollStartNotification(depth: 0, :final metrics) ||
                          ScrollUpdateNotification(
                            depth: 0,
                            :final metrics,
                          ) => metrics,
                          // Resizing can return a short page to the top without
                          // firing any scroll updates.
                          ScrollMetricsNotification(depth: 0, :final metrics)
                              when metrics.extentBefore <=
                                  _mobileChromeTopExtent =>
                            metrics,
                          _ => null,
                        };
                        if (metrics != null && metrics.axis == Axis.vertical) {
                          widget.onScrollOffset?.call(
                            page,
                            metrics.extentBefore,
                          );
                        }
                        return false;
                      },
                      child: RepaintBoundary(child: child),
                    ),
                  ),
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final deck = GestureDetector(
      behavior: HitTestBehavior.opaque,
      supportedDevices: const {PointerDeviceKind.touch},
      dragStartBehavior: DragStartBehavior.down,
      onHorizontalDragStart: (_) => _startDrag(_PageDragOwner.touch),
      onHorizontalDragUpdate: (details) =>
          _updateDrag(_PageDragOwner.touch, details),
      onHorizontalDragEnd: (details) => _endDrag(_PageDragOwner.touch, details),
      onHorizontalDragCancel: () => _cancelDrag(_PageDragOwner.touch),
      child: ClipRect(
        child: Stack(
          children: [
            for (final entry in widget.children.entries)
              _page(entry.key, entry.value),
          ],
        ),
      ),
    );
    if (widget.touchOnly) return deck;
    return RawGestureDetector(
      excludeFromSemantics: true,
      gestures: {
        _PageDeckPreciseDragRecognizer:
            GestureRecognizerFactoryWithHandlers<
              _PageDeckPreciseDragRecognizer
            >(() => _PageDeckPreciseDragRecognizer(debugOwner: this), (
              recognizer,
            ) {
              recognizer
                ..onlyAcceptDragOnThreshold = true
                ..dragStartBehavior = DragStartBehavior.down
                ..gestureSettings = MediaQuery.maybeGestureSettingsOf(context)
                ..multitouchDragStrategy = ScrollConfiguration.of(context)
                    .getMultitouchDragStrategy(context);
              recognizer.onStart = (_) => _startDrag(_PageDragOwner.precise);
              recognizer.onUpdate = (details) =>
                  _updateDrag(_PageDragOwner.precise, details);
              recognizer.onEnd = (details) =>
                  _endDrag(_PageDragOwner.precise, details);
              recognizer.onCancel = () => _cancelDrag(_PageDragOwner.precise);
            }),
      },
      child: deck,
    );
  }
}

class _PageDeckPreciseDragRecognizer extends HorizontalDragGestureRecognizer {
  _PageDeckPreciseDragRecognizer({super.debugOwner})
    : super(
        supportedDevices: const {
          PointerDeviceKind.mouse,
          PointerDeviceKind.stylus,
          PointerDeviceKind.invertedStylus,
        },
      );

  // Give text selection, clicks, and scrollbar thumbs time to claim the drag.
  @override
  bool hasSufficientGlobalDistanceToAccept(
    PointerDeviceKind pointerDeviceKind,
    double? deviceTouchSlop,
  ) {
    // A stylus needs kPanSlop before text selection starts.
    final panSlop = computePanSlop(pointerDeviceKind, gestureSettings);
    return globalDistanceMoved.abs() >
        (panSlop > kTouchSlop ? panSlop : kTouchSlop);
  }
}

class _DesktopNavigation extends StatelessWidget {
  const _DesktopNavigation({required this.controller, required this.onSelect});
  final AppController controller;
  final ValueChanged<AppPage> onSelect;

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
              for (final page in controller.visiblePages)
                FTappable(
                  key: ValueKey('nav-${page.id}'),
                  semanticsButton: false,
                  semanticsRole: SemanticsRole.tab,
                  selected: controller.activePage == page,
                  onPress: () => onSelect(page),
                  builder: (context, variants, _) => Container(
                    constraints: const BoxConstraints(minHeight: 44),
                    padding: const EdgeInsets.symmetric(
                      horizontal: 12,
                      vertical: 6,
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
                          page.title,
                          semanticsLabel: page.title,
                          style: context.theme.typography.body.sm.copyWith(
                            fontSize: 13,
                            height: 1.5,
                            fontWeight: controller.activePage == page
                                ? FontWeight.w600
                                : FontWeight.w500,
                            color: controller.activePage == page
                                ? ChromeAccent.foreground(accent)
                                : palette.ink,
                          ),
                        ),
                        if (_showSchemeUpdateIndicator(page, controller)) ...[
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
                                    ? ChromeAccent.foreground(accent)
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
  Widget build(BuildContext context) {
    final label = controller.accountUser == null ? '账号' : '我的';
    return FTooltip(
      tipBuilder: (_, _) => Text(label),
      child: ConstrainedBox(
        constraints: const BoxConstraints(
          minWidth: 44,
          minHeight: 44,
          maxWidth: 160,
        ),
        child: FButton(
          variant: FButtonVariant.ghost,
          semanticsLabel: label,
          mainAxisSize: MainAxisSize.min,
          style: FButtonStyleDelta.delta(
            contentStyle: FButtonContentStyleDelta.delta(
              constraints: const BoxConstraints(minWidth: 44, minHeight: 44),
              padding: const EdgeInsetsGeometryDelta.value(
                EdgeInsets.symmetric(horizontal: 10, vertical: 6),
              ),
              textStyle: FVariants.all(
                context.theme.typography.body.sm.copyWith(
                  fontSize: 13,
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
                    controller.accountName ?? label,
                    overflow: TextOverflow.ellipsis,
                  ),
                )
              : const SizedBox.shrink(),
        ),
      ),
    );
  }
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
      _ => controller.hasUsableBuild,
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
