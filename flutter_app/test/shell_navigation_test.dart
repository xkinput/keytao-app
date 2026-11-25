import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:keytao/src/app/app.dart';
import 'package:keytao/src/app/options.dart' show AppPage;
import 'package:keytao/src/rust/api/types.dart';

import 'view_fixture.dart';

Future<ViewController> mountShell(
  WidgetTester tester,
  BridgePlatform platform,
) async {
  tester.view.devicePixelRatio = 1;
  tester.view.physicalSize = platform == BridgePlatform.android
      ? const Size(390, 800)
      : const Size(1000, 800);
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  final controller = ViewController(platform);
  addTearDown(controller.dispose);
  await tester.pumpWidget(
    KeyTaoApp(
      controller: controller,
      initialOnboarding: const OnboardingDto(completed: true),
    ),
  );
  await tester.pumpAndSettle();
  return controller;
}

double offsetOf(WidgetTester tester, String page) => tester
    .widget<SlideTransition>(find.byKey(ValueKey('page-slide-$page')))
    .position
    .value
    .dx;

void main() {
  test('page IDs stay stable and legacy debug IDs open about', () {
    expect(AppPage.fromId('debug'), AppPage.about);
    for (final page in AppPage.values) {
      expect(AppPage.fromId(page.id), page);
    }
    expect(AppPage.fromId('unknown'), isNull);
  });

  for (final platform in [BridgePlatform.macOs, BridgePlatform.android]) {
    testWidgets('tabs are visible in order on ${platform.name}', (
      tester,
    ) async {
      await mountShell(tester, platform);

      var previousCenter = 0.0;
      for (final (id, label) in const [
        ('ime', '输入'),
        ('scheme', '方案'),
        ('skin', '皮肤'),
        ('library', '词库与设置'),
        ('about', '关于'),
        ('website', '网站'),
      ]) {
        final tab = find.byKey(ValueKey('nav-$id'));
        expect(tab.hitTestable(), findsOneWidget);
        expect(
          find.descendant(of: tab, matching: find.text(label)),
          findsOneWidget,
        );
        final center = tester.getCenter(tab).dx;
        expect(center, greaterThan(previousCenter));
        previousCenter = center;
      }
      expect(find.byKey(const ValueKey('nav-debug')), findsNothing);
      expect(tester.takeException(), isNull);
    });

    testWidgets('tabs slide in navigation order on ${platform.name}', (
      tester,
    ) async {
      await mountShell(tester, platform);

      await tester.tap(find.byKey(const ValueKey('nav-scheme')));
      await tester.pump();
      expect(offsetOf(tester, 'scheme'), greaterThan(0));
      await tester.pumpAndSettle();
      expect(offsetOf(tester, 'scheme'), 0);

      await tester.tap(find.byKey(const ValueKey('nav-ime')));
      await tester.pump();
      expect(offsetOf(tester, 'ime'), lessThan(0));
      await tester.pumpAndSettle();
      expect(offsetOf(tester, 'ime'), 0);
    });

    testWidgets('adjacent pages follow a horizontal drag on ${platform.name}', (
      tester,
    ) async {
      final controller = await mountShell(tester, platform);
      final width = tester.view.physicalSize.width;
      final start = Offset(width * .75, 170);
      final swipe = await tester.startGesture(start);
      await swipe.moveBy(Offset(-width * .16, 0));
      await tester.pump();
      final firstOffset = offsetOf(tester, 'ime');
      expect(firstOffset, inExclusiveRange(-.35, 0));
      expect(offsetOf(tester, 'scheme'), closeTo(1 + firstOffset, .001));
      await swipe.moveBy(Offset(-width * .23, 0));
      await tester.pump();
      final lastOffset = offsetOf(tester, 'ime');
      expect(lastOffset, lessThan(firstOffset));
      expect(controller.activePage, AppPage.input);
      await swipe.up();
      await tester.pump();
      expect(offsetOf(tester, 'scheme'), closeTo(1 + lastOffset, .001));
      await tester.pumpAndSettle();
      expect(controller.activePage, AppPage.scheme);
      expect(offsetOf(tester, 'scheme'), 0);

      final shortDrag = await tester.startGesture(Offset(width * .25, 170));
      await shortDrag.moveBy(Offset(width * .16, 0));
      await tester.pump();
      expect(offsetOf(tester, 'scheme'), greaterThan(0));
      await shortDrag.up();
      await tester.pumpAndSettle();
      expect(controller.activePage, AppPage.scheme);
      expect(offsetOf(tester, 'scheme'), 0);

      final swipeBack = await tester.startGesture(Offset(width * .25, 170));
      await swipeBack.moveBy(Offset(width * .42, 0));
      await tester.pump();
      expect(offsetOf(tester, 'ime'), inExclusiveRange(-1, 0));
      await swipeBack.up();
      await tester.pumpAndSettle();
      expect(controller.activePage, AppPage.input);
    });
  }

  testWidgets('mobile chrome uses less space while scrolling', (tester) async {
    await mountShell(tester, BridgePlatform.android);
    final nav = find.byKey(const ValueKey('nav-ime'));
    final page = find.byKey(const PageStorageKey('ime'));
    final inactivePage = find.byKey(
      const PageStorageKey('scheme'),
      skipOffstage: false,
    );
    final inactiveHeight = tester.getSize(inactivePage).height;
    final expandedHeight = tester.getSize(page).height;
    expect(nav.hitTestable(), findsOneWidget);

    final swipe = await tester.startGesture(const Offset(12, 310));
    await swipe.moveBy(const Offset(0, -24));
    await swipe.moveBy(const Offset(0, -28));
    await tester.pump();
    final partialHeight = tester.getSize(page).height;
    expect(partialHeight, greaterThan(expandedHeight));
    expect(tester.getSize(inactivePage).height, inactiveHeight);
    await tester.pump(const Duration(milliseconds: 150));
    expect(tester.getSize(page).height, partialHeight);
    await swipe.moveBy(const Offset(0, -24));
    await tester.pump();
    expect(tester.getSize(page).height, greaterThan(partialHeight));
    await swipe.moveBy(const Offset(0, -100));
    await tester.pump();
    expect(nav.hitTestable(), findsNothing);
    final collapsedHeight = tester.getSize(page).height;
    expect(tester.getSize(inactivePage).height, inactiveHeight);
    await swipe.up();
    await tester.pumpAndSettle();

    final reverse = await tester.startGesture(const Offset(12, 310));
    await reverse.moveBy(const Offset(0, 24));
    await reverse.moveBy(const Offset(0, 40));
    await tester.pump();
    expect(
      tester.getSize(page).height,
      inExclusiveRange(expandedHeight, collapsedHeight),
    );
    await reverse.moveBy(const Offset(0, 140));
    await reverse.up();
    await tester.pumpAndSettle();
    expect(nav.hitTestable(), findsOneWidget);
    expect(tester.getSize(page).height, expandedHeight);

    tester.view.physicalSize = const Size(390, 860);
    await tester.pumpAndSettle();
    final resizedHeight = tester.getSize(page).height;
    expect(tester.getSize(inactivePage).height, inactiveHeight);
    await tester.tap(find.byKey(const ValueKey('nav-scheme')));
    await tester.pumpAndSettle();
    expect(tester.getSize(inactivePage).height, resizedHeight);
  });

  testWidgets('navigation returns when expanded content fits the viewport', (
    tester,
  ) async {
    await mountShell(tester, BridgePlatform.android);
    final page = find.byKey(const PageStorageKey('ime'));
    final scroll = tester
        .state<ScrollableState>(
          find.descendant(of: page, matching: find.byType(Scrollable)).first,
        )
        .position;
    final size = tester.view.physicalSize;
    tester.view.physicalSize = Size(
      size.width,
      size.height + scroll.maxScrollExtent - 35,
    );
    await tester.pumpAndSettle();
    expect(scroll.maxScrollExtent, closeTo(35, 1));

    await tester.dragFrom(const Offset(12, 310), const Offset(0, -80));
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('nav-ime')).hitTestable(), findsOneWidget);
  });

  testWidgets('system reduced motion makes tab changes immediate', (
    tester,
  ) async {
    tester.binding.platformDispatcher.accessibilityFeaturesTestValue =
        const FakeAccessibilityFeatures(disableAnimations: true);
    addTearDown(
      tester.binding.platformDispatcher.clearAccessibilityFeaturesTestValue,
    );
    await mountShell(tester, BridgePlatform.android);
    await tester.tap(find.byKey(const ValueKey('nav-scheme')));
    await tester.pump();
    final slide = tester.widget<SlideTransition>(
      find.byKey(const ValueKey('page-slide-scheme')),
    );
    expect(slide.position.value, Offset.zero);
    expect(find.byKey(const PageStorageKey('ime')), findsNothing);
    await tester.pump(const Duration(milliseconds: 150));
  });
}
