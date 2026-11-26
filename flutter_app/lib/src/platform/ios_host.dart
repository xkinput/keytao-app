import 'package:flutter/services.dart';

class IosHost {
  const IosHost();
  static const channel = MethodChannel('keytao/ios');

  Future<Map<String, dynamic>> paths() async =>
      (await channel.invokeMapMethod<String, dynamic>('getPaths'))!;

  Future<Map<String, dynamic>> keyboardStatus() async =>
      (await channel.invokeMapMethod<String, dynamic>('keyboardStatus'))!;

  Future<Map<String, dynamic>?> importLocalSchemeDirectory() =>
      channel.invokeMapMethod<String, dynamic>('importLocalSchemeDirectory');

  Future<Map<String, dynamic>?> importLocalSchemeArchive() =>
      channel.invokeMapMethod<String, dynamic>('importLocalSchemeArchive');

  Future<void> openSettings() => channel.invokeMethod<void>('openSettings');

  Future<void> openUrl(String url) =>
      channel.invokeMethod<void>('openUrl', {'url': url});

  Future<void> shareThemeFile(String fileName, String json) =>
      channel.invokeMethod<void>('shareThemeFile', {
        'fileName': fileName,
        'json': json,
      });
}
