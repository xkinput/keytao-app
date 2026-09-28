import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:keytao/src/app/app.dart';
import 'package:keytao/src/app/controller.dart';
import 'package:keytao/src/app/shell.dart';
import 'package:keytao/src/macos/onboarding.dart';
import 'package:keytao/src/macos/app.dart';
import 'package:keytao/src/macos/shell.dart';
import 'package:keytao/src/onboarding/onboarding_page.dart';
import 'package:keytao/src/rust/api/types.dart';
import 'package:macos_ui/macos_ui.dart';

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
          expect(find.text('输入法'), completed ? findsOneWidget : findsNothing);
          expect(
            find.text('文件访问权限'),
            completed ? findsNothing : findsOneWidget,
          );
          expect(tester.takeException(), isNull);
        }
      }
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );

  testWidgets(
    'macOS completion selects native onboarding or shell without the bridge',
    (tester) async {
      // Flutter widget tests have no AppKit host. Mock only its channel boundary.
      final messenger =
          TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
      const windowChannel = MethodChannel(
        'macos_window_utils/window_manipulator',
      );
      const colorChannel = MethodChannel('appkit_ui_element_colors');
      const titleChannel = MethodChannel('ink.rea.keytao/window');
      var nativeTitle = '';
      var viewId = 0;
      messenger.setMockMethodCallHandler(
        windowChannel,
        (call) async => switch (call.method) {
          'isMainWindow' ||
          'addFullScreenPresentationOption' ||
          'removeFullScreenPresentationOptions' => true,
          'addVisualEffectSubview' => ++viewId,
          _ => null,
        },
      );
      messenger.setMockMethodCallHandler(
        colorChannel,
        (call) async => {'hueComponent': 0.29428158007138466},
      );
      messenger.setMockMethodCallHandler(titleChannel, (call) async {
        expect(call.method, 'setTitle');
        nativeTitle = call.arguments as String;
        return null;
      });
      addTearDown(() {
        messenger.setMockMethodCallHandler(windowChannel, null);
        messenger.setMockMethodCallHandler(colorChannel, null);
        messenger.setMockMethodCallHandler(titleChannel, null);
      });
      await const MacosWindowUtilsConfig().apply();
      final controller = AppController(
        info: const BridgeInfo(
          platform: BridgePlatform.macOs,
          appVersion: 'test',
          userRoot: '/test/keytao',
        ),
      );
      controller.uiSettings = const ImeUiSettingsDto(
        colorScheme: UiColorSchemeDto.auto,
        effectiveColorScheme: EffectiveColorSchemeDto.light,
        orientation: PanelOrientationDto.horizontal,
        embeddedComposition: true,
        accentColor: '#3B73D9',
        fontSize: 16,
        themeExists: false,
        message: '',
      );
      controller.logSettings = RuntimeLogSettingsDto(
        enabled: true,
        level: RuntimeLogLevelDto.info,
        logDir: '/test/logs',
        files: const [],
        fileCount: 0,
        totalBytes: BigInt.zero,
      );
      controller.runtimeLog = const DebugLogFileDto(
        lines: ['offline test log'],
        truncated: true,
      );
      addTearDown(controller.dispose);
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      addTearDown(tester.platformDispatcher.clearPlatformBrightnessTestValue);
      tester.view.devicePixelRatio = 1;
      for (final size in [const Size(640, 480), const Size(760, 620)]) {
        tester.view.physicalSize = size;
        for (final completed in [false, true]) {
          await tester.pumpWidget(
            KeyTaoApp(
              key: ValueKey('$size-$completed'),
              controller: controller,
              initialOnboarding: OnboardingDto(completed: completed),
            ),
          );
          await tester.pumpAndSettle();
          expect(
            find.byType(MacosOnboarding),
            completed ? findsNothing : findsOneWidget,
          );
          expect(
            find.byType(MacosShell),
            completed ? findsOneWidget : findsNothing,
          );
          expect(find.byType(OnboardingPage), findsNothing);
          expect(find.byType(AppShell), findsNothing);
          expect(find.byType(PlatformMenuBar), findsOneWidget);
          if (completed) {
            expect(find.byType(MacosTextField), findsOneWidget);
            expect(find.byType(MacosSlider), findsOneWidget);
            expect(find.byType(MacosPopupButton<String>), findsNWidgets(2));
            for (final title in ['关于', '调试', '输入法']) {
              await tester.tap(find.widgetWithText(MacosIconButton, title));
              await tester.pumpAndSettle();
              expect(
                tester
                    .widget<MacosWindowTitle>(find.byType(MacosWindowTitle))
                    .title,
                title,
              );
              expect(nativeTitle, title);
              expect(
                tester.widget<ToolBar>(find.byType(ToolBar)).title,
                isA<Text>().having((text) => text.data, 'title', title),
              );
            }
          } else {
            expect(find.text('输入法组件'), findsWidgets);
            final next = tester.widget<PushButton>(
              find.widgetWithText(PushButton, '继续'),
            );
            expect(next.onPressed, isNull);
          }
          expect(tester.takeException(), isNull);
          tester.platformDispatcher.platformBrightnessTestValue =
              Brightness.dark;
          await tester.pumpAndSettle();
          expect(
            MacosTheme.of(tester.element(find.byType(MacosWindow))).brightness,
            Brightness.dark,
          );
          expect(tester.takeException(), isNull);
          tester.platformDispatcher.platformBrightnessTestValue =
              Brightness.light;
          await tester.pumpAndSettle();
          // Drain the native visual-effect widget's zero-duration layout timer.
          await tester.pump(Duration.zero);
          await tester.pumpWidget(const SizedBox.shrink());
          await tester.pumpAndSettle();
        }
      }
    },
    variant: TargetPlatformVariant.only(TargetPlatform.macOS),
  );
}
