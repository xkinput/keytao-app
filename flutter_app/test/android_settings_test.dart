import 'package:flutter_test/flutter_test.dart';
import 'package:keytao/src/rust/api/types.dart';
import 'package:keytao/src/settings/android_settings.dart';

void main() {
  test('editing one keyboard setting preserves the entire original DTO', () {
    const original = AndroidImeInputSettingsDto(
      hapticsEnabled: false,
      hapticIntensity: 73,
      enterKeyBehavior: 'newline',
      keyPreviewEnabled: false,
      longPressDelayMs: 420,
      keyboardHeightScale: 115,
      deleteSpeed: 'fast',
      englishMode: 'schema',
      backspaceGestureMode: 'selectThenDelete',
      keyboardHeightDp: 317,
      candidateBarHeightDp: 63,
      swipeThresholdDp: 41.5,
      keySoundEnabled: false,
      keySoundVolume: 37,
      keyHintVisible: false,
      flickKeysEnabled: false,
      numberRowEnabled: true,
      candidateFontScale: 1.17,
      doubleSpacePeriodEnabled: false,
      floatingPortraitEnabled: true,
      floatingPortraitScale: 91,
      floatingLandscapeEnabled: false,
      floatingLandscapeScale: 63,
      configPath: '/test/keytao/keyboard.yaml',
      reloadStampPath: '/test/keytao/reload',
      message: 'original',
    );
    final submitted = patchAndroidSettings(original, {
      'longPressDelayMs': 812.7,
    });
    expect(
      submitted,
      AndroidImeInputSettingsDto(
        hapticsEnabled: original.hapticsEnabled,
        hapticIntensity: original.hapticIntensity,
        enterKeyBehavior: original.enterKeyBehavior,
        keyPreviewEnabled: original.keyPreviewEnabled,
        longPressDelayMs: 700,
        keyboardHeightScale: original.keyboardHeightScale,
        deleteSpeed: original.deleteSpeed,
        englishMode: original.englishMode,
        backspaceGestureMode: original.backspaceGestureMode,
        keyboardHeightDp: original.keyboardHeightDp,
        candidateBarHeightDp: original.candidateBarHeightDp,
        swipeThresholdDp: original.swipeThresholdDp,
        keySoundEnabled: original.keySoundEnabled,
        keySoundVolume: original.keySoundVolume,
        keyHintVisible: original.keyHintVisible,
        flickKeysEnabled: original.flickKeysEnabled,
        numberRowEnabled: original.numberRowEnabled,
        candidateFontScale: original.candidateFontScale,
        doubleSpacePeriodEnabled: original.doubleSpacePeriodEnabled,
        floatingPortraitEnabled: original.floatingPortraitEnabled,
        floatingPortraitScale: original.floatingPortraitScale,
        floatingLandscapeEnabled: original.floatingLandscapeEnabled,
        floatingLandscapeScale: original.floatingLandscapeScale,
        configPath: original.configPath,
        reloadStampPath: original.reloadStampPath,
        message: original.message,
      ),
    );
  });
}
