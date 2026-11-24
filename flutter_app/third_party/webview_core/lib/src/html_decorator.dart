import 'dart:convert';

/// Injects an optional `<base href="...">` element and JavaScript channel
/// shims into [html] without placing markup before `<!DOCTYPE ...>`
/// declarations (preserving Standards Mode in browser engines).
String decorateHtmlDocument({
  required int controllerId,
  required String html,
  required Iterable<String> channels,
  String? baseUrl,
}) {
  final injections = StringBuffer();
  if (baseUrl != null && baseUrl.isNotEmpty) {
    final escapedBase = const HtmlEscape(
      HtmlEscapeMode.attribute,
    ).convert(baseUrl);
    injections.write('<base href="$escapedBase">');
  }
  if (channels.isNotEmpty) {
    injections.write('<script>');
    for (final name in channels) {
      final safeName = jsonEncode(name);
      injections.write(
        'window[$safeName]={postMessage:function(m){'
        'window.parent.postMessage({'
        '__webview_core_id:$controllerId,'
        '__webview_core_channel:$safeName,'
        'message:String(m)'
        '},"*");}};',
      );
    }
    injections.write('</script>');
  }
  if (injections.isEmpty) {
    return html;
  }
  final snippet = injections.toString();

  final headMatch = RegExp(
    r'<head(?:\s[^>]*)?>',
    caseSensitive: false,
  ).firstMatch(html);
  if (headMatch != null) {
    return '${html.substring(0, headMatch.end)}'
        '$snippet'
        '${html.substring(headMatch.end)}';
  }

  final htmlMatch = RegExp(
    r'<html(?:\s[^>]*)?>',
    caseSensitive: false,
  ).firstMatch(html);
  if (htmlMatch != null) {
    return '${html.substring(0, htmlMatch.end)}'
        '<head>$snippet</head>'
        '${html.substring(htmlMatch.end)}';
  }

  final doctypeMatch = RegExp(
    r'^\uFEFF?\s*<!DOCTYPE[^>]*>',
    caseSensitive: false,
  ).firstMatch(html);
  if (doctypeMatch != null) {
    return '${html.substring(0, doctypeMatch.end)}'
        '<head>$snippet</head>'
        '${html.substring(doctypeMatch.end)}';
  }

  return '$snippet$html';
}
