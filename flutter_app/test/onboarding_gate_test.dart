import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:keytao/src/app/app.dart';
import 'package:keytao/src/app/controller.dart';
import 'package:keytao/src/app/options.dart';
import 'package:keytao/src/app/shell.dart';
import 'package:keytao/src/onboarding/onboarding_page.dart';
import 'package:keytao/src/rust/api/types.dart';

void main() {
  testWidgets(
    'injected completion selects onboarding or shell without native initialization',
    (tester) async {
      final controller = AppController(
        info: const BridgeInfo(
          platform: BridgePlatform.android,
          appVersion: 'test',
          userRoot: '/test/keytao',
        ),
      );
      addTearDown(controller.dispose);
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      tester.view.devicePixelRatio = 1;
      for (final width in [360.0, 1000.0]) {
        tester.view.physicalSize = Size(width, 800);
        for (final completed in [false, true]) {
          await tester.pumpWidget(
            KeyTaoApp(
              key: ValueKey('$width-$completed'),
              controller: controller,
              initialOnboarding: OnboardingDto(completed: completed),
            ),
          );
          await tester.pumpAndSettle();
          expect(
            find.byType(OnboardingPage),
            completed ? findsNothing : findsOneWidget,
          );
          expect(
            find.byType(AppShell),
            completed ? findsOneWidget : findsNothing,
          );
          expect(
            find.byType(NavigationBar),
            completed ? findsOneWidget : findsNothing,
          );
          if (completed) {
            final destinations = tester
                .widgetList<NavigationDestination>(
                  find.byType(NavigationDestination),
                )
                .toList();
            expect(
              destinations.map((destination) => destination.label),
              AppPage.values.map((page) => page.title),
            );
            expect(
              destinations.map(
                (destination) => (destination.icon as Icon).icon,
              ),
              AppPage.values.map((page) => page.materialIcon),
            );
            final inputPage = find.byKey(const PageStorageKey('ime'));
            await tester.drag(inputPage, const Offset(0, -320));
            await tester.pumpAndSettle();
            final inputScroll = tester.state<ScrollableState>(
              find
                  .descendant(of: inputPage, matching: find.byType(Scrollable))
                  .first,
            );
            final inputOffset = inputScroll.position.pixels;
            expect(inputOffset, greaterThan(0));
            await tester.tap(find.text(AppStrings.scheme).last);
            await tester.pumpAndSettle();
            expect(controller.activePage, AppPage.scheme);
            expect(
              tester
                  .widget<NavigationBar>(find.byType(NavigationBar))
                  .selectedIndex,
              AppPage.scheme.index,
            );
            await tester.tap(find.text(AppStrings.ime).last);
            await tester.pumpAndSettle();
            expect(inputScroll.position.pixels, inputOffset);
          }
          expect(
            find.text(AppStrings.androidOnboarding),
            completed ? findsNothing : findsOneWidget,
          );
          expect(
            find.text(AppStrings.grantStorage),
            completed ? findsNothing : findsOneWidget,
          );
          expect(tester.takeException(), isNull);
        }
      }
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );
}
