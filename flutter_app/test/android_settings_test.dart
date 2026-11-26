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
      mnemonicHintsEnabled: true,
      flickKeysEnabled: false,
      numberRowEnabled: true,
      candidateFontScale: 1.17,
      doubleSpacePeriodEnabled: false,
      symbolAutoReturn: true,
      phrases: ['Custom phrase'],
      clipboardHistoryEnabled: false,
      clipboardHistoryLimit: 87,
      clipboardSuggestionEnabled: false,
      clipboardSuggestionTimeoutSec: 45,
      clipboardReturnAfterPaste: false,
      clipboardHideSensitive: false,
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
        mnemonicHintsEnabled: original.mnemonicHintsEnabled,
        flickKeysEnabled: original.flickKeysEnabled,
        numberRowEnabled: original.numberRowEnabled,
        candidateFontScale: original.candidateFontScale,
        doubleSpacePeriodEnabled: original.doubleSpacePeriodEnabled,
        symbolAutoReturn: original.symbolAutoReturn,
        phrases: original.phrases,
        clipboardHistoryEnabled: original.clipboardHistoryEnabled,
        clipboardHistoryLimit: original.clipboardHistoryLimit,
        clipboardSuggestionEnabled: original.clipboardSuggestionEnabled,
        clipboardSuggestionTimeoutSec: original.clipboardSuggestionTimeoutSec,
        clipboardReturnAfterPaste: original.clipboardReturnAfterPaste,
        clipboardHideSensitive: original.clipboardHideSensitive,
        floatingPortraitEnabled: original.floatingPortraitEnabled,
        floatingPortraitScale: original.floatingPortraitScale,
        floatingLandscapeEnabled: original.floatingLandscapeEnabled,
        floatingLandscapeScale: original.floatingLandscapeScale,
        configPath: original.configPath,
        reloadStampPath: original.reloadStampPath,
        message: original.message,
      ),
    );
    final cleared = patchAndroidSettings(original, {
      'symbolAutoReturn': false,
      'phrases': <String>[],
    });
    expect(cleared.symbolAutoReturn, isFalse);
    expect(cleared.phrases, isEmpty);
    final reordered = patchAndroidSettings(original, {
      'phrases': ['Second', 'First'],
    });
    expect(reordered.phrases, ['Second', 'First']);
    expect(reordered.symbolAutoReturn, original.symbolAutoReturn);
    expect(androidDefaults['symbolAutoReturn'], false);
    expect(androidDefaults['phrases'], ['谢谢', '好的', '收到', '稍等', '我在路上']);
    final minimum = patchAndroidSettings(original, {
      'clipboardHistoryLimit': -1,
      'clipboardSuggestionTimeoutSec': 0,
    });
    expect(minimum.clipboardHistoryLimit, 10);
    expect(minimum.clipboardSuggestionTimeoutSec, 5);
    final maximum = patchAndroidSettings(original, {
      'clipboardHistoryLimit': 999,
      'clipboardSuggestionTimeoutSec': 999,
    });
    expect(maximum.clipboardHistoryLimit, 200);
    expect(maximum.clipboardSuggestionTimeoutSec, 120);
    final integral = patchAndroidSettings(original, {
      'clipboardHistoryLimit': 75.0,
      'clipboardSuggestionTimeoutSec': 60.0,
    });
    expect(integral.clipboardHistoryLimit, 75);
    expect(integral.clipboardSuggestionTimeoutSec, 60);
    final rounded = patchAndroidSettings(original, {
      'clipboardHistoryLimit': 10 + 49 / 190 * 190,
      'clipboardSuggestionTimeoutSec': 30.5,
    });
    expect(rounded.clipboardHistoryLimit, 59);
    expect(rounded.clipboardSuggestionTimeoutSec, 31);
    for (final invalid in [
      'invalid',
      true,
      double.nan,
      double.infinity,
    ]) {
      final fallback = patchAndroidSettings(original, {
        'clipboardHistoryLimit': invalid,
        'clipboardSuggestionTimeoutSec': invalid,
      });
      expect(fallback.clipboardHistoryLimit, 50);
      expect(fallback.clipboardSuggestionTimeoutSec, 30);
    }
    final defaults = patchAndroidSettings(original, {
      'clipboardHistoryEnabled': 'false',
      'clipboardSuggestionEnabled': 0,
      'clipboardReturnAfterPaste': 'unknown',
      'clipboardHideSensitive': 0,
    });
    expect(defaults.clipboardHistoryEnabled, isTrue);
    expect(defaults.clipboardSuggestionEnabled, isTrue);
    expect(defaults.clipboardReturnAfterPaste, isTrue);
    expect(defaults.clipboardHideSensitive, isTrue);
    final reset = patchAndroidSettings(original, androidDefaults);
    expect(reset.clipboardHistoryEnabled, isTrue);
    expect(reset.clipboardHistoryLimit, 50);
    expect(reset.clipboardSuggestionEnabled, isTrue);
    expect(reset.clipboardSuggestionTimeoutSec, 30);
    expect(reset.clipboardReturnAfterPaste, isTrue);
    expect(reset.clipboardHideSensitive, isTrue);
  });
}
