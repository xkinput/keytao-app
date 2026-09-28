import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:keytao/src/app/app.dart';
import 'package:keytao/src/app/controller.dart';
import 'package:keytao/src/app/options.dart';
import 'package:keytao/src/app/shell.dart';
import 'package:keytao/src/macos/onboarding.dart';
import 'package:keytao/src/macos/shell.dart';
import 'package:keytao/src/onboarding/onboarding_page.dart';
import 'package:keytao/src/rust/api/types.dart';
import 'package:macos_ui/macos_ui.dart';

class _MacosController extends AppController {
  _MacosController()
    : super(
        info: const BridgeInfo(
          platform: BridgePlatform.macOs,
          appVersion: 'test',
          userRoot: '/test/keytao',
        ),
      );

  int logRefreshes = 0;

  @override
  Future<bool> refreshLogs() async {
    logRefreshes++;
    return true;
  }
}

void main() {
  var windowConfigured = false;
  Future<void> configureWindow() async {
    if (windowConfigured) return;
    await const MacosWindowUtilsConfig().apply();
    windowConfigured = true;
  }

  testWidgets(
    'macOS onboarding install step renders its inline install error',
    (tester) async {
      final messenger =
          TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
      const windowChannel = MethodChannel(
        'macos_window_utils/window_manipulator',
      );
      const colorChannel = MethodChannel('appkit_ui_element_colors');
      const titleChannel = MethodChannel('ink.rea.keytao/window');
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
      messenger.setMockMethodCallHandler(titleChannel, (call) async => null);
      addTearDown(() {
        messenger.setMockMethodCallHandler(windowChannel, null);
        messenger.setMockMethodCallHandler(colorChannel, null);
        messenger.setMockMethodCallHandler(titleChannel, null);
      });
      await configureWindow();
      final controller = _MacosController()
        ..macosIme = const MacosImeStatusDto(
          installed: true,
          sharedDataSource: '',
          message: '',
        )
        ..installError = AppStrings.installFailed;
      addTearDown(controller.dispose);
      await tester.pumpWidget(
        KeyTaoApp(
          controller: controller,
          initialOnboarding: const OnboardingDto(completed: false),
        ),
      );
      await tester.pumpAndSettle();
      await tester.tap(find.widgetWithText(PushButton, '继续'));
      await tester.pumpAndSettle();

      expect(find.text(SetupStep.install.title), findsWidgets);
      expect(find.text(controller.installError!), findsOneWidget);
      expect(tester.takeException(), isNull);
      await tester.pump(Duration.zero);
      await tester.pumpWidget(const SizedBox.shrink());
      await tester.pumpAndSettle();
    },
    variant: TargetPlatformVariant.only(TargetPlatform.macOS),
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
      await configureWindow();
      final controller = _MacosController();
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
      controller.appUpdate = const AppUpdateDto(
        hasUpdate: true,
        latestVersion: '1.2.1-alpha.42',
        currentVersion: 'test',
        releaseUrl: 'https://example.invalid/release',
      );
      const release = PlatformReleaseDto(
        version: '1.2.1-alpha.42',
        downloadUrls: DownloadUrlsDto(macos: 'offline'),
      );
      controller.latestRelease = const ReleaseInfoDto(
        version: '1.2.1-alpha.42',
        name: 'KeyTao',
        publishedAt: '',
        body: 'Offline changelog',
        github: release,
        gitee: release,
      );
      controller.selectedDirectory = '/test/custom/keytao';
      controller.customSchemas = const LocalSchemasDto(
        hasDefaultCustom: false,
        schemas: ['keytao', 'english'],
      );
      controller.customFiles = const [
        FileItemDto(name: 'keytao.schema.yaml', isDir: false),
      ];
      controller.customResult = const InstallResultDto(
        mergedSchemas: ['keytao'],
        logs: ['[MERGED] keytao'],
        verify: [],
      );
      controller.addOperationLogs(['[DEPLOY] Offline deployment']);
      controller.deploySteps.add(
        const DeployStep('Offline deployment', DeployStepState.ok),
      );
      controller.systemLogs = const DebugLogsDto(
        ime: DebugLogFileDto(lines: ['Offline IME log'], truncated: true),
        app: DebugLogFileDto(lines: ['Offline app log'], truncated: true),
      );
      addTearDown(controller.dispose);
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      addTearDown(tester.platformDispatcher.clearPlatformBrightnessTestValue);
      tester.view.devicePixelRatio = 1;
      for (final size in [const Size(720, 520), const Size(900, 680)]) {
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
            expect(find.byType(SidebarItems), findsOneWidget);
            expect(
              tester.widget<MacosWindow>(find.byType(MacosWindow)).sidebar,
              isNotNull,
            );
            expect(find.byType(MacosSlider), findsOneWidget);
            final sidebar = tester.widget<SidebarItems>(
              find.byType(SidebarItems),
            );
            expect(sidebar.items, hasLength(AppPage.values.length));
            final refreshes = controller.logRefreshes;
            for (final page in [...AppPage.values.skip(1), AppPage.input]) {
              await tester.tap(
                find.descendant(
                  of: find.byType(SidebarItems),
                  matching: find.text(page.title),
                ),
              );
              await tester.pumpAndSettle();
              expect(controller.activePage, page);
              expect(
                tester
                    .widget<SidebarItems>(find.byType(SidebarItems))
                    .currentIndex,
                page.index,
              );
              expect(nativeTitle, page.title);
              expect(
                tester.widget<ToolBar>(find.byType(ToolBar)).title,
                isA<Text>().having((text) => text.data, 'title', page.title),
              );
              expect(tester.takeException(), isNull);
            }
            expect(controller.logRefreshes, refreshes + 1);
            final viewMenu = tester
                .widget<PlatformMenuBar>(find.byType(PlatformMenuBar))
                .menus
                .whereType<PlatformMenu>()
                .singleWhere((menu) => menu.label == '显示');
            expect(viewMenu.menus, hasLength(AppPage.values.length));
            for (final page in AppPage.values) {
              final item = viewMenu.menus[page.index];
              expect(item.label, page.title);
              expect(
                item.shortcut,
                isA<CharacterActivator>()
                    .having(
                      (value) => value.character,
                      'character',
                      '${page.shortcutDigit}',
                    )
                    .having((value) => value.meta, 'meta', true),
              );
              item.onSelected!();
              await tester.pumpAndSettle();
              expect(controller.activePage, page);
              expect(nativeTitle, page.title);
              expect(
                tester
                    .widget<SidebarItems>(find.byType(SidebarItems))
                    .currentIndex,
                page.index,
              );
            }
            expect(controller.logRefreshes, refreshes + 2);
            controller.selectPage(AppPage.input);
            await tester.pumpAndSettle();
          } else {
            expect(find.byType(SidebarItems), findsNothing);
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
