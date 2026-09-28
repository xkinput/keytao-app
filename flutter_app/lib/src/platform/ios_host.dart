import 'package:flutter/services.dart';

class IosHost {
  const IosHost();
  static const channel = MethodChannel('keytao/ios');

  Future<Map<String, dynamic>> paths() async =>
      (await channel.invokeMapMethod<String, dynamic>('getPaths'))!;

  Future<void> openSettings() => channel.invokeMethod<void>('openSettings');

  Future<void> openUrl(String url) =>
      channel.invokeMethod<void>('openUrl', {'url': url});
}
