import 'dart:ui' show PlatformDispatcher;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:forui/forui.dart';
import 'package:keytao/src/app/app.dart';
import 'package:keytao/src/app/controller.dart';
import 'package:keytao/src/app/options.dart';
import 'package:keytao/src/rust/api/types.dart';
import 'package:keytao/src/ui/onboarding.dart';
import 'package:keytao/src/ui/shell.dart';

import 'view_fixture.dart';

void unifiedGateCases(BridgePlatform platform) {
  final target = switch (platform) {
    BridgePlatform.macOs => TargetPlatform.macOS,
    BridgePlatform.android => TargetPlatform.android,
    BridgePlatform.windows => TargetPlatform.windows,
    BridgePlatform.linux => TargetPlatform.linux,
    BridgePlatform.ios => TargetPlatform.iOS,
  };
  final desktopUngated =
      platform == BridgePlatform.windows || platform == BridgePlatform.linux;
  final mobile =
      platform == BridgePlatform.android || platform == BridgePlatform.ios;
  testWidgets(
    '${platform.name} onboarding opens on the first incomplete step',
    (tester) async {
      final c = ViewController(platform)
        ..local = const LocalSchemaDto(
          installed: false,
          deployed: false,
          version: '',
          schemas: [],
        );
      if (c.isAndroid) c.storage = {'granted': false, 'writable': false};
      addTearDown(c.dispose);
      const channel = MethodChannel('ink.rea.keytao/window');
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        channel,
        (_) async => null,
      );
      addTearDown(
        () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
          channel,
          null,
        ),
      );
      await tester.pumpWidget(
        KeyTaoApp(
          controller: c,
          initialOnboarding: const OnboardingDto(completed: false),
        ),
      );
      await tester.pumpAndSettle();
      if (desktopUngated) {
        expect(find.byType(OnboardingPage), findsNothing);
        expect(find.byType(AppShell), findsOneWidget);
      } else {
        expect(
          find.byKey(
            ValueKey(switch (platform) {
              BridgePlatform.ios => 'setup-enable',
              BridgePlatform.android => 'setup-storage',
              _ => 'setup-install',
            }),
          ),
          findsOneWidget,
        );
      }
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
      await tester.pumpAndSettle();
    },
    variant: TargetPlatformVariant.only(target),
  );
  for (final width in [400.0, 1000.0]) {
    for (final completed in [false, true]) {
      testWidgets(
        '${platform.name} $width completed=$completed gates unified navigation',
        (tester) async {
          final showShell = completed || desktopUngated;
          final c = ViewController(platform);
          if (c.isAndroid && !completed) {
            c.storage = {'granted': false, 'writable': false};
          }
          addTearDown(c.dispose);
          tester.view.devicePixelRatio = 1;
          tester.view.physicalSize = Size(width, 860);
          addTearDown(tester.view.resetPhysicalSize);
          addTearDown(tester.view.resetDevicePixelRatio);
          addTearDown(
            tester.platformDispatcher.clearPlatformBrightnessTestValue,
          );
          final titleCalls = <String>[];
          const channel = MethodChannel('ink.rea.keytao/window');
          tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
            channel,
            (call) async {
              expect(call.method, 'setTitle');
              titleCalls.add(call.arguments as String);
              return null;
            },
          );
          addTearDown(
            () => tester.binding.defaultBinaryMessenger
                .setMockMethodCallHandler(channel, null),
          );
          await tester.pumpWidget(
            KeyTaoApp(
              controller: c,
              initialOnboarding: OnboardingDto(completed: completed),
            ),
          );
          await tester.pumpAndSettle();
          expect(
            find.byType(OnboardingPage),
            showShell ? findsNothing : findsOneWidget,
          );
          expect(
            find.byType(AppShell),
            showShell ? findsOneWidget : findsNothing,
          );
          final desktopNavigation = !c.isMobile && width >= 720;
          expect(
            find.byKey(const ValueKey('desktop-navigation')),
            showShell && desktopNavigation ? findsOneWidget : findsNothing,
          );
          expect(
            find.byType(FBottomNavigationBar),
            showShell && !desktopNavigation ? findsOneWidget : findsNothing,
          );
          expect(
            find.byType(PlatformMenuBar),
            platform == BridgePlatform.macOs ? findsOneWidget : findsNothing,
          );
          if (showShell) {
            expect(AppPage.values.map((page) => page.id), [
              'ime',
              'scheme',
              'skin',
              'library',
              'about',
              'website',
            ]);
            expect(AppPage.about.shortcutDigit, 5);
            expect(AppPage.library.shortcutDigit, 4);
            expect(find.byKey(const ValueKey('nav-extension')), findsNothing);
            expect(find.byKey(const ValueKey('nav-website')), findsOneWidget);
            if (platform == BridgePlatform.windows) {
              expect(find.text(AppStrings.windowsIme), findsOneWidget);
              expect(
                find.text(AppStrings.windowsPackage(true)),
                findsOneWidget,
              );
              expect(find.text(AppStrings.embedded), findsOneWidget);
            }
            if (platform == BridgePlatform.linux) {
              expect(find.text(AppStrings.linuxIme), findsOneWidget);
              expect(find.text(AppStrings.kdeProcesses(1)), findsOneWidget);
              expect(find.text(AppStrings.embedded), findsOneWidget);
            }
            if (platform == BridgePlatform.ios) {
              expect(find.text(AppStrings.iosOnboarding), findsOneWidget);
              expect(find.text('候选与键盘'), findsOneWidget);
              expect(find.text(AppStrings.candidateFontSize), findsNothing);
              expect(find.text(AppStrings.openStoragePermission), findsNothing);
              expect(find.text(AppStrings.chooseKeytao), findsNothing);
            }
            final input = find.byKey(const PageStorageKey('ime'));
            await tester.drag(input, const Offset(0, -300));
            await tester.pumpAndSettle();
            final scroll = tester.state<ScrollableState>(
              find
                  .descendant(of: input, matching: find.byType(Scrollable))
                  .first,
            );
            if (platform == BridgePlatform.android) {
              expect(scroll.position.pixels, greaterThan(0));
            }
            final schemeTab = find.byKey(const ValueKey('nav-scheme'));
            if (c.isMobile && schemeTab.hitTestable().evaluate().isEmpty) {
              await tester.drag(input, const Offset(0, 80));
              await tester.pumpAndSettle();
            }
            final offset = scroll.position.pixels;
            for (final page in [
              AppPage.scheme,
              AppPage.skin,
              AppPage.library,
              AppPage.about,
              AppPage.input,
            ]) {
              await tester.tap(find.byKey(ValueKey('nav-${page.id}')));
              await tester.pumpAndSettle();
              expect(c.activePage, page);
              if (!c.isMobile) {
                expect(
                  titleCalls.last,
                  formatWindowTitle(
                    page.title,
                    PlatformDispatcher.instance.locale,
                  ),
                );
              }
              expect(find.text(AppStrings.appUpdate), findsOneWidget);
              expect(tester.takeException(), isNull);
            }
            expect(scroll.position.pixels, offset);
            expect(c.logRefreshes, 1);
            if (platform == BridgePlatform.macOs) {
              final viewMenu = tester
                  .widget<PlatformMenuBar>(find.byType(PlatformMenuBar))
                  .menus
                  .whereType<PlatformMenu>()
                  .singleWhere((menu) => menu.label == '显示');
              expect(viewMenu.menus.length, AppPage.values.length);
              for (final page in AppPage.values) {
                final item = viewMenu.menus[page.index];
                expect(
                  (item.shortcut as CharacterActivator).character,
                  '${page.shortcutDigit}',
                );
                expect((item.shortcut as CharacterActivator).meta, isTrue);
                if (page == AppPage.website) continue;
                item.onSelected!();
                await tester.pumpAndSettle();
                expect(c.activePage, page);
                expect(
                  titleCalls.last,
                  formatWindowTitle(
                    page.title,
                    PlatformDispatcher.instance.locale,
                  ),
                );
              }
            }
            if (platform != BridgePlatform.macOs) {
              Focus.of(tester.element(find.byType(AppShell))).requestFocus();
              await tester.pump();
              final modifier = platform == BridgePlatform.ios
                  ? LogicalKeyboardKey.metaLeft
                  : LogicalKeyboardKey.controlLeft;
              await tester.sendKeyDownEvent(modifier);
              try {
                for (final (key, character, page) in const [
                  (LogicalKeyboardKey.digit3, '3', AppPage.skin),
                  (LogicalKeyboardKey.digit4, '4', AppPage.library),
                  (LogicalKeyboardKey.digit5, '5', AppPage.about),
                  (LogicalKeyboardKey.digit1, '1', AppPage.input),
                ]) {
                  await tester.sendKeyEvent(key, character: character);
                  await tester.pumpAndSettle();
                  expect(c.activePage, page);
                  expect(find.byKey(PageStorageKey(page.id)), findsOneWidget);
                  expect(tester.takeException(), isNull);
                }
              } finally {
                await tester.sendKeyUpEvent(modifier);
              }
            }
          }
          tester.platformDispatcher.platformBrightnessTestValue =
              Brightness.dark;
          await tester.pumpAndSettle();
          final colors = FTheme.of(tester.element(find.byType(FScaffold)))
              .colors;
          expect(colors.brightness, Brightness.dark);
          expect(tester.takeException(), isNull);
          await tester.pumpWidget(const SizedBox.shrink());
          await tester.pumpAndSettle();
        },
        variant: TargetPlatformVariant.only(target),
      );
    }
    if (mobile) continue;
    testWidgets(
      '${platform.name} $width onboarding install error stays inline',
      (tester) async {
        final c = ViewController(platform)
          ..installError = AppStrings.installFailed;
        addTearDown(c.dispose);
        tester.view.devicePixelRatio = 1;
        tester.view.physicalSize = Size(width, 860);
        addTearDown(tester.view.resetPhysicalSize);
        addTearDown(tester.view.resetDevicePixelRatio);
        const channel = MethodChannel('ink.rea.keytao/window');
        tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
          channel,
          (_) async => null,
        );
        addTearDown(
          () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
            channel,
            null,
          ),
        );
        await tester.pumpWidget(
          KeyTaoApp(
            controller: c,
            initialOnboarding: const OnboardingDto(completed: false),
          ),
        );
        await tester.pumpAndSettle();
        if (desktopUngated) {
          await tester.tap(find.byKey(const ValueKey('nav-scheme')));
        } else {
          await tester.tap(find.byKey(const ValueKey('setup-nav-install')));
        }
        await tester.pumpAndSettle();
        if (!desktopUngated) {
          expect(find.byKey(const ValueKey('setup-install')), findsOneWidget);
        }
        expect(find.text(AppStrings.installFailed), findsOneWidget);
        expect(find.byType(FDialog), findsNothing);
        expect(tester.takeException(), isNull);
        await tester.pumpWidget(const SizedBox.shrink());
        await tester.pumpAndSettle();
      },
      variant: TargetPlatformVariant.only(target),
    );
  }
}
