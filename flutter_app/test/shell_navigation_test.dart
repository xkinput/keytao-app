import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:keytao/src/app/app.dart';
import 'package:keytao/src/rust/api/types.dart';

import 'view_fixture.dart';

Future<void> mountShell(WidgetTester tester, BridgePlatform platform) async {
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
}

void main() {
  for (final platform in [BridgePlatform.macOs, BridgePlatform.android]) {
    testWidgets('tabs slide in navigation order on ${platform.name}', (
      tester,
    ) async {
      await mountShell(tester, platform);
      double offsetOf(String page) => tester
          .widget<SlideTransition>(find.byKey(ValueKey('page-slide-$page')))
          .position
          .value
          .dx;

      await tester.tap(find.byKey(const ValueKey('nav-scheme')));
      await tester.pump();
      expect(offsetOf('scheme'), greaterThan(0));
      await tester.pumpAndSettle();
      expect(offsetOf('scheme'), 0);

      await tester.tap(find.byKey(const ValueKey('nav-ime')));
      await tester.pump();
      expect(offsetOf('ime'), lessThan(0));
      await tester.pumpAndSettle();
      expect(offsetOf('ime'), 0);
    });
  }

  testWidgets('mobile chrome uses less space while scrolling', (tester) async {
    await mountShell(tester, BridgePlatform.android);
    final nav = find.byKey(const ValueKey('nav-ime'));
    final page = find.byKey(const PageStorageKey('ime'));
    final expandedHeight = tester.getSize(page).height;
    expect(nav.hitTestable(), findsOneWidget);

    await tester.dragFrom(const Offset(12, 310), const Offset(0, -150));
    await tester.pumpAndSettle();
    expect(nav.hitTestable(), findsNothing);
    expect(tester.getSize(page).height, greaterThan(expandedHeight));

    await tester.dragFrom(const Offset(12, 310), const Offset(0, 150));
    await tester.pumpAndSettle();
    expect(nav.hitTestable(), findsOneWidget);
    expect(tester.getSize(page).height, expandedHeight);
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
