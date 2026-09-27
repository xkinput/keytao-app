import '../rust/api/types.dart';

const androidDefaults = <String, Object>{
  'hapticsEnabled': true,
  'hapticIntensity': 42,
  'enterKeyBehavior': 'system',
  'keyPreviewEnabled': true,
  'longPressDelayMs': 300,
  'keyboardHeightScale': 100,
  'deleteSpeed': 'standard',
  'englishMode': 'ascii',
  'backspaceGestureMode': 'immediate',
  'keyboardHeightDp': 266,
  'candidateBarHeightDp': 52,
  'swipeThresholdDp': 34.0,
  'keySoundEnabled': true,
  'keySoundVolume': 100,
  'keyHintVisible': true,
  'flickKeysEnabled': true,
  'numberRowEnabled': false,
  'candidateFontScale': 1.0,
  'doubleSpacePeriodEnabled': true,
  'floatingPortraitEnabled': false,
  'floatingPortraitScale': 88,
  'floatingLandscapeEnabled': true,
  'floatingLandscapeScale': 72,
};

// Normalize edited fields like App.tsx, preserving every untouched DTO field.
AndroidImeInputSettingsDto patchAndroidSettings(
  AndroidImeInputSettingsDto original,
  Map<String, Object> patch,
) {
  bool flag(String key, bool current) => patch[key] as bool? ?? current;
  int integer(String key, int current, int min, int max) =>
      patch.containsKey(key)
      ? (patch[key] as num).round().clamp(min, max)
      : current;
  String choice(
    String key,
    String current,
    List<String> allowed,
    String fallback,
  ) => patch.containsKey(key)
      ? (allowed.contains(patch[key]) ? patch[key] as String : fallback)
      : current;
  final height = patch.containsKey('keyboardHeightScale')
      ? (patch['keyboardHeightScale'] as num).round()
      : original.keyboardHeightScale;
  return AndroidImeInputSettingsDto(
    hapticsEnabled: flag('hapticsEnabled', original.hapticsEnabled),
    hapticIntensity: integer(
      'hapticIntensity',
      original.hapticIntensity,
      1,
      100,
    ),
    enterKeyBehavior: choice('enterKeyBehavior', original.enterKeyBehavior, [
      'newline',
      'system',
    ], 'system'),
    keyPreviewEnabled: flag('keyPreviewEnabled', original.keyPreviewEnabled),
    longPressDelayMs: integer(
      'longPressDelayMs',
      original.longPressDelayMs,
      100,
      700,
    ),
    keyboardHeightScale:
        !patch.containsKey('keyboardHeightScale') ||
            (height >= 85 && height <= 130 && height % 5 == 0)
        ? height
        : 100,
    deleteSpeed: choice('deleteSpeed', original.deleteSpeed, [
      'slow',
      'standard',
      'fast',
    ], 'standard'),
    englishMode: choice('englishMode', original.englishMode, [
      'schema',
      'ascii',
    ], 'ascii'),
    backspaceGestureMode: choice(
      'backspaceGestureMode',
      original.backspaceGestureMode,
      ['selectThenDelete', 'immediate'],
      'immediate',
    ),
    keyboardHeightDp: integer(
      'keyboardHeightDp',
      original.keyboardHeightDp,
      160,
      420,
    ),
    candidateBarHeightDp: integer(
      'candidateBarHeightDp',
      original.candidateBarHeightDp,
      36,
      96,
    ),
    swipeThresholdDp: patch.containsKey('swipeThresholdDp')
        ? (patch['swipeThresholdDp'] as num).round().clamp(12, 96).toDouble()
        : original.swipeThresholdDp,
    keySoundEnabled: flag('keySoundEnabled', original.keySoundEnabled),
    keySoundVolume: integer('keySoundVolume', original.keySoundVolume, 0, 100),
    keyHintVisible: flag('keyHintVisible', original.keyHintVisible),
    flickKeysEnabled: flag('flickKeysEnabled', original.flickKeysEnabled),
    numberRowEnabled: flag('numberRowEnabled', original.numberRowEnabled),
    candidateFontScale: patch.containsKey('candidateFontScale')
        ? double.parse((patch['candidateFontScale'] as num).toStringAsFixed(2))
              .clamp(0.8, 1.4)
        : original.candidateFontScale,
    doubleSpacePeriodEnabled: flag(
      'doubleSpacePeriodEnabled',
      original.doubleSpacePeriodEnabled,
    ),
    floatingPortraitEnabled: flag(
      'floatingPortraitEnabled',
      original.floatingPortraitEnabled,
    ),
    floatingPortraitScale: integer(
      'floatingPortraitScale',
      original.floatingPortraitScale,
      70,
      100,
    ),
    floatingLandscapeEnabled: flag(
      'floatingLandscapeEnabled',
      original.floatingLandscapeEnabled,
    ),
    floatingLandscapeScale: integer(
      'floatingLandscapeScale',
      original.floatingLandscapeScale,
      45,
      100,
    ),
    configPath: original.configPath,
    reloadStampPath: original.reloadStampPath,
    message: original.message,
  );
}
