package com.infradise.webview_core

import android.annotation.SuppressLint
import android.content.Context
import android.graphics.Bitmap
import android.os.Handler
import android.os.Looper
import android.util.Base64
import android.view.View
import android.view.ViewGroup
import android.webkit.CookieManager
import android.webkit.JavascriptInterface
import android.webkit.WebChromeClient
import android.webkit.WebResourceError
import android.webkit.WebResourceRequest
import android.webkit.WebStorage
import android.webkit.WebView
import android.webkit.WebViewClient
import io.flutter.embedding.engine.plugins.FlutterPlugin
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel
import io.flutter.plugin.common.MethodChannel.MethodCallHandler
import io.flutter.plugin.common.MethodChannel.Result
import io.flutter.plugin.common.StandardMessageCodec
import io.flutter.plugin.platform.PlatformView
import io.flutter.plugin.platform.PlatformViewFactory
import kotlin.math.roundToInt
import org.json.JSONArray
import org.json.JSONObject
import org.json.JSONTokener

class WebviewCorePlugin : FlutterPlugin, MethodCallHandler {
    private var channel: MethodChannel? = null
    private var flutterAssets: FlutterPlugin.FlutterAssets? = null
    private val instances = HashMap<Int, AndroidWebViewInstance>()

    override fun onAttachedToEngine(binding: FlutterPlugin.FlutterPluginBinding) {
        flutterAssets = binding.flutterAssets
        val methodChannel = MethodChannel(
            binding.binaryMessenger,
            "com.infradise.webview_core"
        )
        methodChannel.setMethodCallHandler(this)
        channel = methodChannel

        binding.platformViewRegistry.registerViewFactory(
            "com.infradise.webview_core/view",
            WebViewFactory(this)
        )
    }

    override fun onDetachedFromEngine(binding: FlutterPlugin.FlutterPluginBinding) {
        val copy = ArrayList(instances.values)
        instances.clear()
        for (item in copy) {
            item.destroy()
        }
        channel?.setMethodCallHandler(null)
        channel = null
        flutterAssets = null
    }

    internal fun createPlatformView(context: Context, rawArgs: Any?): PlatformView {
        val map = rawArgs as? Map<*, *> ?: emptyMap<Any?, Any?>()
        val controllerId = (map["controllerId"] as? Number)?.toInt() ?: 0
        instances.remove(controllerId)?.destroy()
        val instance = AndroidWebViewInstance(
            context = context,
            controllerId = controllerId,
            creationArgs = map,
            plugin = this
        )
        instances[controllerId] = instance
        return instance
    }

    internal fun removeInstance(controllerId: Int, target: AndroidWebViewInstance) {
        if (instances[controllerId] === target) {
            instances.remove(controllerId)
        }
    }

    internal fun sendEvent(method: String, arguments: Map<String, Any?>) {
        val ch = channel ?: return
        if (Looper.myLooper() == Looper.getMainLooper()) {
            ch.invokeMethod(method, arguments)
        } else {
            Handler(Looper.getMainLooper()).post {
                channel?.invokeMethod(method, arguments)
            }
        }
    }

    internal fun requestNavigationDecision(
        controllerId: Int,
        url: String,
        isMainFrame: Boolean,
        callback: (Boolean) -> Unit
    ) {
        val payload = mapOf(
            "controllerId" to controllerId,
            "url" to url,
            "isMainFrame" to isMainFrame
        )
        val invokeBlock = Runnable {
            val ch = channel
            if (ch == null) {
                callback(true)
                return@Runnable
            }
            ch.invokeMethod(
                "onNavigationRequest",
                payload,
                object : Result {
                    override fun success(result: Any?) {
                        callback((result as? Boolean) ?: true)
                    }

                    override fun error(
                        errorCode: String,
                        errorMessage: String?,
                        errorDetails: Any?
                    ) {
                        callback(true)
                    }

                    override fun notImplemented() {
                        callback(true)
                    }
                }
            )
        }
        if (Looper.myLooper() == Looper.getMainLooper()) {
            invokeBlock.run()
        } else {
            Handler(Looper.getMainLooper()).post(invokeBlock)
        }
    }

    override fun onMethodCall(call: MethodCall, result: Result) {
        val args = call.arguments as? Map<*, *>
        if (call.method == "clearCookies") {
            val cookieManager = CookieManager.getInstance()
            cookieManager.removeAllCookies { removed ->
                cookieManager.flush()
                result.success(removed ?: false)
            }
            return
        }
        if (call.method == "setCookie") {
            val name = args?.get("name") as? String ?: ""
            val value = args?.get("value") as? String ?: ""
            val domain = args?.get("domain") as? String ?: ""
            val path = args?.get("path") as? String ?: "/"
            if (name.isNotEmpty() && domain.isNotEmpty()) {
                val cleanHost = domain.trimStart('.')
                val url = "https://$cleanHost$path"
                val header = "$name=$value; Domain=$domain; Path=$path"
                val cookieManager = CookieManager.getInstance()
                cookieManager.setCookie(url, header) {
                    cookieManager.flush()
                    result.success(null)
                }
            } else {
                result.success(null)
            }
            return
        }

        val controllerId = (args?.get("controllerId") as? Number)?.toInt()
        if (controllerId == null) {
            result.error("INVALID_ARGS", "Missing controllerId", null)
            return
        }
        if (call.method == "dispose") {
            instances.remove(controllerId)?.destroy()
            result.success(null)
            return
        }

        val instance = instances[controllerId]
        if (instance == null) {
            result.success(null)
            return
        }
        instance.handleCall(call.method, args, flutterAssets, result)
    }
}

private class WebViewFactory(
    private val plugin: WebviewCorePlugin
) : PlatformViewFactory(StandardMessageCodec.INSTANCE) {
    override fun create(context: Context, viewId: Int, args: Any?): PlatformView {
        return plugin.createPlatformView(context, args)
    }
}

@SuppressLint("SetJavaScriptEnabled")
internal class AndroidWebViewInstance(
    context: Context,
    private val controllerId: Int,
    creationArgs: Map<*, *>,
    private val plugin: WebviewCorePlugin
) : PlatformView {
    private val mainHandler = Handler(Looper.getMainLooper())
    private val webView: WebView = WebView(context)
    private val defaultUserAgent: String
    private var hasNavigationDecision: Boolean = false
    private var approvedNavigationUrl: String? = null
    private val registeredChannels = HashSet<String>()
    private var destroyed: Boolean = false

    init {
        val settings = webView.settings
        defaultUserAgent = settings.userAgentString ?: ""
        settings.domStorageEnabled = true
        settings.allowFileAccess = true
        settings.displayZoomControls = false

        val jsEnabled = creationArgs["javaScriptEnabled"] as? Boolean ?: true
        settings.javaScriptEnabled = jsEnabled

        val zoomEnabled = creationArgs["zoomEnabled"] as? Boolean ?: true
        settings.setSupportZoom(zoomEnabled)
        settings.builtInZoomControls = zoomEnabled

        val customUa = creationArgs["userAgent"] as? String
        if (!customUa.isNullOrEmpty()) {
            settings.userAgentString = customUa
        }

        hasNavigationDecision = creationArgs["hasNavigationDecision"] as? Boolean ?: false

        val bgColor = (creationArgs["backgroundColor"] as? Number)?.toInt() ?: 0
        webView.setBackgroundColor(bgColor)

        val initialChannels = creationArgs["channelNames"] as? List<*>
        if (initialChannels != null) {
            for (rawName in initialChannels) {
                val name = rawName as? String
                if (!name.isNullOrEmpty()) {
                    addJsChannelInternal(name)
                }
            }
        }

        webView.webChromeClient = object : WebChromeClient() {
            override fun onProgressChanged(view: WebView?, newProgress: Int) {
                if (destroyed) return
                plugin.sendEvent(
                    "onProgress",
                    mapOf(
                        "controllerId" to controllerId,
                        "progress" to newProgress
                    )
                )
            }
        }

        webView.webViewClient = object : WebViewClient() {
            override fun onPageStarted(view: WebView?, url: String?, favicon: Bitmap?) {
                if (destroyed) return
                val safeUrl = url ?: ""
                plugin.sendEvent(
                    "onPageStarted",
                    mapOf("controllerId" to controllerId, "url" to safeUrl)
                )
                plugin.sendEvent(
                    "onUrlChange",
                    mapOf("controllerId" to controllerId, "url" to safeUrl)
                )
            }

            override fun onPageFinished(view: WebView?, url: String?) {
                if (destroyed) return
                val safeUrl = url ?: ""
                plugin.sendEvent(
                    "onPageFinished",
                    mapOf("controllerId" to controllerId, "url" to safeUrl)
                )
                plugin.sendEvent(
                    "onUrlChange",
                    mapOf("controllerId" to controllerId, "url" to safeUrl)
                )
            }

            override fun shouldOverrideUrlLoading(
                view: WebView?,
                request: WebResourceRequest?
            ): Boolean {
                if (destroyed || request == null || !hasNavigationDecision) {
                    return false
                }
                val targetUrl = request.url?.toString() ?: return false
                if (targetUrl == "about:blank") {
                    return false
                }
                val isMainFrame = request.isForMainFrame
                if (isMainFrame) {
                    if (approvedNavigationUrl == targetUrl) {
                        approvedNavigationUrl = null
                        return false
                    }
                    val reqHeaders = request.requestHeaders ?: emptyMap()
                    val reqMethod = request.method?.uppercase() ?: "GET"
                    plugin.requestNavigationDecision(
                        controllerId = controllerId,
                        url = targetUrl,
                        isMainFrame = true
                    ) { allow ->
                        if (!destroyed && allow) {
                            approvedNavigationUrl = targetUrl
                            loadRequestInternal(targetUrl, reqMethod, reqHeaders, null)
                        }
                    }
                    return true
                }

                val isHttp = targetUrl.startsWith("http://") || targetUrl.startsWith("https://")
                plugin.requestNavigationDecision(
                    controllerId = controllerId,
                    url = targetUrl,
                    isMainFrame = false
                ) { allow ->
                    if (!destroyed && !allow && isHttp) {
                        val quoted = JSONObject.quote(targetUrl)
                        webView.evaluateJavascript(
                            "Array.from(document.querySelectorAll('iframe')).forEach(function(f){" +
                                "if(f.src===$quoted){f.src='about:blank';}});",
                            null
                        )
                    }
                }
                return !isHttp
            }

            override fun onReceivedError(
                view: WebView?,
                request: WebResourceRequest?,
                error: WebResourceError?
            ) {
                if (destroyed || error == null) return
                plugin.sendEvent(
                    "onWebResourceError",
                    mapOf(
                        "controllerId" to controllerId,
                        "errorCode" to error.errorCode,
                        "description" to (error.description?.toString() ?: "Unknown"),
                        "errorType" to mapErrorCode(error.errorCode),
                        "isForMainFrame" to (request?.isForMainFrame ?: false),
                        "url" to request?.url?.toString()
                    )
                )
            }
        }

        val initialHtml = creationArgs["initialHtml"] as? String
        val initialUrl = creationArgs["initialUrl"] as? String
        if (initialHtml != null) {
            val baseUrl = creationArgs["initialBaseUrl"] as? String ?: "about:blank"
            webView.loadDataWithBaseURL(baseUrl, initialHtml, "text/html", "UTF-8", null)
        } else if (initialUrl != null) {
            val method = (creationArgs["initialMethod"] as? String ?: "GET").uppercase()
            val headers = extractHeaders(creationArgs["initialHeaders"])
            val body = creationArgs["initialBody"] as? ByteArray
            loadRequestInternal(initialUrl, method, headers, body)
        }

        mainHandler.post {
            if (!destroyed) {
                plugin.sendEvent("onViewCreated", mapOf("controllerId" to controllerId))
            }
        }
    }

    override fun getView(): View = webView

    override fun dispose() {
        plugin.removeInstance(controllerId, this)
        destroy()
    }

    fun destroy() {
        if (destroyed) return
        destroyed = true
        for (name in registeredChannels) {
            webView.removeJavascriptInterface(name)
        }
        registeredChannels.clear()
        (webView.parent as? ViewGroup)?.removeView(webView)
        webView.stopLoading()
        webView.webChromeClient = null
        webView.destroy()
    }

    private fun addJsChannelInternal(name: String) {
        if (registeredChannels.add(name)) {
            webView.addJavascriptInterface(JsBridge(name), name)
        }
    }

    private inner class JsBridge(private val channelName: String) {
        @JavascriptInterface
        fun postMessage(message: String?) {
            if (destroyed) return
            plugin.sendEvent(
                "onJavaScriptMessage",
                mapOf(
                    "controllerId" to controllerId,
                    "channel" to channelName,
                    "message" to (message ?: "")
                )
            )
        }
    }

    private fun extractHeaders(raw: Any?): Map<String, String> {
        val source = raw as? Map<*, *> ?: return emptyMap()
        val out = HashMap<String, String>(source.size)
        for ((k, v) in source) {
            if (k is String && v is String) {
                out[k] = v
            }
        }
        return out
    }

    private fun loadRequestInternal(
        url: String,
        method: String,
        headers: Map<String, String>,
        body: ByteArray?
    ) {
        val upper = method.ifEmpty { "GET" }.uppercase()
        if (upper == "GET") {
            if (headers.isNotEmpty()) {
                webView.loadUrl(url, headers)
            } else {
                webView.loadUrl(url)
            }
            return
        }
        if (upper == "POST" && headers.isEmpty()) {
            webView.postUrl(url, body ?: ByteArray(0))
            return
        }

        val headersJson = JSONObject(headers).toString()
        val bodyExpr = if (body != null && body.isNotEmpty() && upper != "HEAD") {
            val b64 = Base64.encodeToString(body, Base64.NO_WRAP)
            "Uint8Array.from(atob(${JSONObject.quote(b64)}),function(c){return c.charCodeAt(0);})"
        } else {
            "undefined"
        }
        val bootstrapHtml =
            "<!DOCTYPE html><html><head><meta charset=\"utf-8\"></head><body><script>" +
                "fetch(${JSONObject.quote(url)},{method:${JSONObject.quote(upper)}," +
                "headers:$headersJson,body:$bodyExpr})" +
                ".then(function(r){return r.text();})" +
                ".then(function(t){document.open();document.write(t);document.close();});" +
                "</script></body></html>"
        webView.loadDataWithBaseURL(url, bootstrapHtml, "text/html", "UTF-8", url)
    }

    private fun displayDensity(): Float {
        return webView.resources.displayMetrics.density.coerceAtLeast(0.01f)
    }

    fun handleCall(
        method: String,
        args: Map<*, *>,
        flutterAssets: FlutterPlugin.FlutterAssets?,
        result: Result
    ) {
        if (destroyed) {
            result.success(null)
            return
        }
        when (method) {
            "loadHtmlString" -> {
                val html = args["html"] as? String ?: ""
                val baseUrl = args["baseUrl"] as? String ?: "about:blank"
                webView.loadDataWithBaseURL(baseUrl, html, "text/html", "UTF-8", null)
                result.success(null)
            }
            "loadRequest" -> {
                val url = args["url"] as? String ?: "about:blank"
                val reqMethod = (args["method"] as? String ?: "GET").uppercase()
                val headers = extractHeaders(args["headers"])
                val body = args["body"] as? ByteArray
                approvedNavigationUrl = url
                loadRequestInternal(url, reqMethod, headers, body)
                result.success(null)
            }
            "loadFile" -> {
                val path = args["path"] as? String ?: ""
                val fileUrl = if (path.startsWith("file://")) path else "file://$path"
                approvedNavigationUrl = fileUrl
                webView.loadUrl(fileUrl)
                result.success(null)
            }
            "loadFlutterAsset" -> {
                val key = args["assetKey"] as? String
                if (key.isNullOrEmpty() || flutterAssets == null) {
                    result.success(false)
                    return
                }
                val assetPath = flutterAssets.getAssetFilePathByName(key)
                val assetUrl = "file:///android_asset/$assetPath"
                approvedNavigationUrl = assetUrl
                webView.loadUrl(assetUrl)
                result.success(true)
            }
            "setJavaScriptEnabled" -> {
                val enabled = args["enabled"] as? Boolean ?: true
                webView.settings.javaScriptEnabled = enabled
                result.success(null)
            }
            "setBackgroundColor" -> {
                val color = (args["color"] as? Number)?.toInt() ?: 0
                webView.setBackgroundColor(color)
                result.success(null)
            }
            "setUserAgent" -> {
                val ua = args["userAgent"] as? String
                webView.settings.userAgentString = if (ua.isNullOrEmpty()) defaultUserAgent else ua
                result.success(null)
            }
            "enableZoom" -> {
                val enabled = args["enabled"] as? Boolean ?: true
                webView.settings.setSupportZoom(enabled)
                webView.settings.builtInZoomControls = enabled
                result.success(null)
            }
            "setHasNavigationDecision" -> {
                hasNavigationDecision = args["enabled"] as? Boolean ?: false
                result.success(null)
            }
            "runJavaScript" -> {
                val script = args["script"] as? String ?: ""
                webView.evaluateJavascript(script, null)
                result.success(null)
            }
            "runJavaScriptReturningResult" -> {
                val script = args["script"] as? String ?: ""
                webView.evaluateJavascript(script) { rawJson ->
                    result.success(decodeJsResult(rawJson))
                }
            }
            "addJavaScriptChannel" -> {
                val name = args["name"] as? String
                if (!name.isNullOrEmpty()) {
                    addJsChannelInternal(name)
                }
                result.success(null)
            }
            "removeJavaScriptChannel" -> {
                val name = args["name"] as? String
                if (!name.isNullOrEmpty() && registeredChannels.remove(name)) {
                    webView.removeJavascriptInterface(name)
                }
                result.success(null)
            }
            "reload" -> {
                webView.reload()
                result.success(null)
            }
            "stopLoading" -> {
                webView.stopLoading()
                result.success(null)
            }
            "canGoBack" -> result.success(webView.canGoBack())
            "canGoForward" -> result.success(webView.canGoForward())
            "goBack" -> {
                if (webView.canGoBack()) webView.goBack()
                result.success(null)
            }
            "goForward" -> {
                if (webView.canGoForward()) webView.goForward()
                result.success(null)
            }
            "currentUrl" -> result.success(webView.url)
            "getTitle" -> result.success(webView.title)
            "scrollTo" -> {
                val density = displayDensity()
                val x = (((args["x"] as? Number)?.toDouble() ?: 0.0) * density).roundToInt()
                val y = (((args["y"] as? Number)?.toDouble() ?: 0.0) * density).roundToInt()
                webView.scrollTo(x, y)
                result.success(null)
            }
            "scrollBy" -> {
                val density = displayDensity()
                val x = (((args["x"] as? Number)?.toDouble() ?: 0.0) * density).roundToInt()
                val y = (((args["y"] as? Number)?.toDouble() ?: 0.0) * density).roundToInt()
                webView.scrollBy(x, y)
                result.success(null)
            }
            "getScrollPosition" -> {
                val density = displayDensity().toDouble()
                result.success(
                    mapOf(
                        "x" to (webView.scrollX.toDouble() / density),
                        "y" to (webView.scrollY.toDouble() / density)
                    )
                )
            }
            "clearCache" -> {
                webView.clearCache(true)
                result.success(null)
            }
            "clearLocalStorage" -> {
                WebStorage.getInstance().deleteAllData()
                result.success(null)
            }
            else -> result.notImplemented()
        }
    }

    private fun decodeJsResult(rawJson: String?): Any? {
        if (rawJson == null || rawJson == "null" || rawJson == "undefined") {
            return null
        }
        return try {
            when (val parsed = JSONTokener(rawJson).nextValue()) {
                JSONObject.NULL -> null
                is String, is Number, is Boolean -> parsed
                is JSONObject, is JSONArray -> parsed.toString()
                else -> rawJson
            }
        } catch (_: Exception) {
            rawJson
        }
    }

    private fun mapErrorCode(code: Int): String {
        return when (code) {
            WebViewClient.ERROR_AUTHENTICATION -> "authentication"
            WebViewClient.ERROR_BAD_URL -> "badUrl"
            WebViewClient.ERROR_CONNECT -> "connect"
            WebViewClient.ERROR_FAILED_SSL_HANDSHAKE -> "failedSslHandshake"
            WebViewClient.ERROR_FILE -> "file"
            WebViewClient.ERROR_FILE_NOT_FOUND -> "fileNotFound"
            WebViewClient.ERROR_HOST_LOOKUP -> "hostLookup"
            WebViewClient.ERROR_IO -> "io"
            WebViewClient.ERROR_PROXY_AUTHENTICATION -> "proxyAuthentication"
            WebViewClient.ERROR_REDIRECT_LOOP -> "redirectLoop"
            WebViewClient.ERROR_TIMEOUT -> "timeout"
            WebViewClient.ERROR_TOO_MANY_REQUESTS -> "tooManyRequests"
            WebViewClient.ERROR_UNSAFE_RESOURCE -> "unsafeResource"
            WebViewClient.ERROR_UNSUPPORTED_AUTH_SCHEME -> "unsupportedAuthScheme"
            WebViewClient.ERROR_UNSUPPORTED_SCHEME -> "unsupportedScheme"
            else -> "unknown"
        }
    }
}
