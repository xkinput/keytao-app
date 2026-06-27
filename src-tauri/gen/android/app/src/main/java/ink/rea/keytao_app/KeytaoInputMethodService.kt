package ink.rea.keytao_app

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Intent
import android.content.res.Configuration
import android.inputmethodservice.InputMethodService
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.view.KeyEvent
import android.view.View
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputMethodManager
import java.util.concurrent.Executors

class KeytaoInputMethodService : InputMethodService(), KeytaoKeyboardView.Listener {
    private lateinit var engine: KeytaoImeEngine
    private val mainHandler = Handler(Looper.getMainLooper())
    private val candidateExecutor = Executors.newSingleThreadExecutor()
    private val clipboardHistory = mutableListOf<String>()
    private val clipboardListener = ClipboardManager.OnPrimaryClipChangedListener {
        rememberCurrentClipboard(suggest = true)
    }
    private var clipboardManager: ClipboardManager? = null
    private var keyboardView: KeytaoKeyboardView? = null
    private var currentState = KeytaoImeState.empty()
    private var composing = false
    private var selectionModeActive = false
    private var shiftPressedWithoutKey = false
    private var pendingShiftKeyCode = 0
    private var inputAvailable = false
    private var unavailableMessage = "请先在 KeyTao App 安装键道方案"
    private val backspaceRestoreStack = mutableListOf<String>()

    override fun onCreate() {
        super.onCreate()
        engine = KeytaoImeEngine(applicationContext)
        clipboardManager = getSystemService(ClipboardManager::class.java)
        clipboardManager?.addPrimaryClipChangedListener(clipboardListener)
        rememberCurrentClipboard(suggest = false)
    }

    override fun onDestroy() {
        clipboardManager?.removePrimaryClipChangedListener(clipboardListener)
        if (::engine.isInitialized) {
            engine.close()
        }
        candidateExecutor.shutdownNow()
        super.onDestroy()
    }

    override fun onCreateInputView(): View {
        val view = KeytaoKeyboardView(this)
        view.listener = this
        view.updateConfig(KeytaoAndroidImeConfig.load(this))
        view.updateTheme(KeytaoThemeResolver.resolve(this))
        view.updateState(currentState)
        keyboardView = view
        refreshInputAvailability()
        return view
    }

    override fun onStartInput(attribute: EditorInfo?, restarting: Boolean) {
        super.onStartInput(attribute, restarting)
        val ready = refreshInputAvailability()
        currentState = if (ready) engine.reset().withoutTransientCommit() else KeytaoImeState.empty()
        composing = false
        keyboardView?.updateState(currentState)
    }

    override fun onStartInputView(info: EditorInfo?, restarting: Boolean) {
        super.onStartInputView(info, restarting)
        if (engine.reloadIfNeeded()) {
            currentState = engine.state().withoutTransientCommit()
        }
        keyboardView?.updateTheme(KeytaoThemeResolver.resolve(this))
        keyboardView?.updateConfig(KeytaoAndroidImeConfig.load(this))
        refreshInputAvailability()
        keyboardView?.updateState(currentState)
    }

    override fun onConfigurationChanged(newConfig: Configuration) {
        super.onConfigurationChanged(newConfig)
        keyboardView?.updateTheme(KeytaoThemeResolver.resolve(this))
    }

    override fun onFinishInput() {
        currentInputConnection?.finishComposingText()
        composing = false
        currentState = if (inputAvailable) engine.reset().withoutTransientCommit() else KeytaoImeState.empty()
        keyboardView?.updateState(currentState)
        super.onFinishInput()
    }

    override fun onEvaluateFullscreenMode(): Boolean = false

    override fun onKeyDown(keyCode: Int, event: KeyEvent): Boolean {
        if (isShiftKey(keyCode)) {
            shiftPressedWithoutKey = true
            pendingShiftKeyCode = keyCode
            return super.onKeyDown(keyCode, event)
        }
        if (shiftPressedWithoutKey) {
            shiftPressedWithoutKey = false
            pendingShiftKeyCode = 0
        }
        if (!inputAvailable) {
            return super.onKeyDown(keyCode, event)
        }
        val key = AndroidKeyMapper.fromAndroidKeyEvent(event) ?: return super.onKeyDown(keyCode, event)
        if (currentState.asciiMode && !currentState.hasComposition && key.keyCode != AndroidKeyMapper.XK_F4) {
            return super.onKeyDown(keyCode, event)
        }
        if (shouldBypassHardwareKey(key, event)) {
            return super.onKeyDown(keyCode, event)
        }
        val result = engine.processKey(key.keyCode, key.modifiers)
        if (!result.accepted && !result.hasComposition) return super.onKeyDown(keyCode, event)
        applyState(result)
        return true
    }

    override fun onKeyUp(keyCode: Int, event: KeyEvent): Boolean {
        if (!isShiftKey(keyCode)) return super.onKeyUp(keyCode, event)
        val shouldToggle = shiftPressedWithoutKey && pendingShiftKeyCode == keyCode
        shiftPressedWithoutKey = false
        pendingShiftKeyCode = 0
        if (!shouldToggle) return super.onKeyUp(keyCode, event)
        if (!inputAvailable) return super.onKeyUp(keyCode, event)

        val shiftKeysym = if (keyCode == KeyEvent.KEYCODE_SHIFT_RIGHT) {
            AndroidKeyMapper.XK_SHIFT_R
        } else {
            AndroidKeyMapper.XK_SHIFT_L
        }
        val result = engine.processKey(shiftKeysym, AndroidKeyMapper.RIME_RELEASE_MASK)
        if (result.accepted || result.hasComposition) {
            applyState(result)
        } else {
            applyState(engine.setAsciiMode(!currentState.asciiMode))
        }
        return true
    }

    override fun onKeyCommand(command: KeyCommand) {
        if (!inputAvailable && command.requiresInstalledSchema()) {
            showUnavailableMessage()
            return
        }
        when (command.type) {
            KeyCommandTypes.INPUT -> handleTextInput(command.value.orEmpty(), command.fallbackValue)
            KeyCommandTypes.DIRECT_INPUT -> commitDirect(command.value.orEmpty())
            KeyCommandTypes.RIME_INPUT -> handleRimeInput(command.value.orEmpty(), command.fallbackValue)
            KeyCommandTypes.BACKSPACE -> handleBackspace()
            KeyCommandTypes.BACKSPACE_GESTURE -> handleBackspaceGesture(command.value.orEmpty())
            KeyCommandTypes.ENTER -> handleEnter()
            KeyCommandTypes.SPACE -> handleSpace()
            KeyCommandTypes.SHIFT -> keyboardView?.toggleShift()
            KeyCommandTypes.MODE -> handleMode(command.value)
            KeyCommandTypes.OPEN_PAGE -> openAppPage(command.value)
            KeyCommandTypes.KEYBOARD_PICKER -> showKeyboardPicker()
            KeyCommandTypes.KEYBOARD_MODE -> keyboardView?.setKeyboardLayer(command.value)
            KeyCommandTypes.NEXT_PAGE -> applyState(engine.changePage(backward = false))
            KeyCommandTypes.PREVIOUS_PAGE -> applyState(engine.changePage(backward = true))
            KeyCommandTypes.RESET -> applyState(engine.reset())
            KeyCommandTypes.RIME_MENU -> openRimeMenu()
            KeyCommandTypes.EDIT -> handleEditAction(command.value.orEmpty(), command.fallbackValue)
            KeyCommandTypes.PANEL -> Unit
        }
    }

    override fun onCandidate(index: Int, global: Boolean) {
        if (!inputAvailable) {
            showUnavailableMessage()
            return
        }
        applyState(
            if (global) {
                engine.selectCandidateGlobal(index)
            } else {
                engine.selectCandidate(index)
            }
        )
    }

    override fun onRequestExpandCandidates(callback: (List<KeytaoCandidate>) -> Unit) {
        if (!inputAvailable) {
            showUnavailableMessage()
            callback(emptyList())
            return
        }
        candidateExecutor.execute {
            val candidates = engine.allCandidates(expandedCandidateLimit)
            mainHandler.post {
                callback(candidates)
            }
        }
    }

    override fun onRequestClipboardHistory(callback: (List<String>) -> Unit) {
        rememberCurrentClipboard(suggest = false)
        callback(clipboardHistory.toList())
    }

    private fun handleTextInput(text: String, fallbackValue: String? = null) {
        if (text.isEmpty()) return
        val fallbackText = fallbackValue ?: text
        if (currentState.asciiMode) {
            commitDirect(fallbackText)
            return
        }
        if (text.codePointCount(0, text.length) != 1) {
            commitDirect(fallbackText)
            return
        }

        val key = AndroidKeyMapper.fromText(text)
        if (key == null) {
            commitDirect(fallbackText)
            return
        }

        val result = engine.processKey(key.keyCode, key.modifiers)
        if (!result.accepted && !result.hasComposition) {
            commitDirect(fallbackText)
        } else {
            applyState(result)
        }
    }

    private fun handleRimeInput(sequence: String, fallbackValue: String?) {
        if (sequence.isEmpty()) return
        val fallbackText = fallbackValue ?: sequence
        if (currentState.asciiMode) {
            commitDirect(fallbackText)
            return
        }

        var latest: KeytaoImeState? = null
        val codePoints = sequence.codePoints().toArray()
        for (codePoint in codePoints) {
            val text = String(Character.toChars(codePoint))
            val key = AndroidKeyMapper.fromText(text)
            if (key == null) {
                engine.reset()
                commitDirect(fallbackText)
                return
            }
            val result = engine.processKey(key.keyCode, key.modifiers)
            if (!result.accepted && !result.hasComposition) {
                engine.reset()
                commitDirect(fallbackText)
                return
            }
            latest = result
        }

        latest?.let { applyState(it) }
    }

    private fun handleBackspace() {
        if (!currentState.hasComposition && !composing) {
            deleteOneBeforeCursorForRestore()
            selectionModeActive = false
            return
        }
        val result = engine.processKey(AndroidKeyMapper.XK_BACK_SPACE, 0)
        if (result.accepted || result.hasComposition) {
            applyState(result)
        } else {
            deleteOneBeforeCursorForRestore(resetComposition = false)
            composing = false
            currentState = engine.reset().withoutTransientCommit()
            keyboardView?.updateState(currentState)
        }
    }

    private fun handleBackspaceGesture(action: String) {
        when (action) {
            "delete" -> deleteOneBeforeCursorForRestore()
            "restore" -> restoreOneBackspaceText()
            "deleteAll" -> deleteAllBeforeCursorForRestore()
            "restoreAll" -> restoreAllBackspaceText()
        }
    }

    private fun deleteOneBeforeCursorForRestore(resetComposition: Boolean = true): Boolean {
        if (resetComposition) clearCompositionBeforeEdit()
        val connection = currentInputConnection ?: return false
        val before = connection.getTextBeforeCursor(backspaceUnitContextLimit, 0)?.toString().orEmpty()
        val deleted = lastTextUnit(before)
        if (deleted == null) {
            connection.deleteSurroundingText(1, 0)
            selectionModeActive = false
            return false
        }
        connection.deleteSurroundingText(deleted.length, 0)
        backspaceRestoreStack.add(deleted)
        selectionModeActive = false
        return true
    }

    private fun deleteAllBeforeCursorForRestore() {
        clearCompositionBeforeEdit()
        val connection = currentInputConnection ?: return
        val before = connection.getTextBeforeCursor(backspaceContextLimit, 0)?.toString().orEmpty()
        if (before.isEmpty()) return
        connection.deleteSurroundingText(before.length, 0)
        backspaceRestoreStack.addAll(textUnits(before).asReversed())
        selectionModeActive = false
    }

    private fun restoreOneBackspaceText(): Boolean {
        val connection = currentInputConnection ?: return false
        if (backspaceRestoreStack.isEmpty()) return false
        val text = backspaceRestoreStack.removeAt(backspaceRestoreStack.lastIndex)
        connection.commitText(text, 1)
        selectionModeActive = false
        return true
    }

    private fun restoreAllBackspaceText() {
        val connection = currentInputConnection ?: return
        if (backspaceRestoreStack.isEmpty()) return
        val restored = buildString {
            while (backspaceRestoreStack.isNotEmpty()) {
                append(backspaceRestoreStack.removeAt(backspaceRestoreStack.lastIndex))
            }
        }
        connection.commitText(restored, 1)
        selectionModeActive = false
    }

    private fun lastTextUnit(text: String): String? {
        if (text.isEmpty()) return null
        val start = text.offsetByCodePoints(text.length, -1)
        return text.substring(start)
    }

    private fun textUnits(text: String): List<String> {
        if (text.isEmpty()) return emptyList()
        val units = mutableListOf<String>()
        var index = 0
        while (index < text.length) {
            val next = text.offsetByCodePoints(index, 1)
            units.add(text.substring(index, next))
            index = next
        }
        return units
    }

    private fun handleSpace() {
        if (currentState.hasComposition) {
            applyState(engine.processKey(AndroidKeyMapper.XK_SPACE, 0))
        } else {
            commitDirect(" ")
        }
    }

    private fun handleEnter() {
        if (currentState.hasComposition) {
            applyState(engine.processKey(AndroidKeyMapper.XK_RETURN, 0))
            return
        }

        val action = currentInputEditorInfo?.imeOptions?.and(EditorInfo.IME_MASK_ACTION)
            ?: EditorInfo.IME_ACTION_NONE
        if (action != EditorInfo.IME_ACTION_NONE) {
            currentInputConnection?.performEditorAction(action)
        } else {
            currentInputConnection?.sendKeyEvent(KeyEvent(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_ENTER))
            currentInputConnection?.sendKeyEvent(KeyEvent(KeyEvent.ACTION_UP, KeyEvent.KEYCODE_ENTER))
        }
    }

    private fun handleMode(value: String?) {
        val target = when (value) {
            "ascii", "english", "en" -> true
            "chinese", "zh", "cn" -> false
            else -> !currentState.asciiMode
        }
        applyState(engine.setAsciiMode(target))
    }

    private fun openRimeMenu() {
        applyState(engine.processKey(AndroidKeyMapper.XK_F4, 0))
    }

    private fun handleEditAction(action: String, value: String?) {
        when (action) {
            "copy" -> copySelection(cut = false)
            "cut" -> copySelection(cut = true)
            "paste" -> pasteClipboard()
            "tab" -> commitDirect("\t")
            "lineStart" -> moveToLineBoundary(start = true)
            "lineEnd" -> moveToLineBoundary(start = false)
            "selectAll" -> selectAllText()
            "toggleSelection" -> toggleSelectionMode()
            "selectLeft" -> extendSelection(left = true)
            "selectRight" -> extendSelection(left = false)
            "pasteText" -> value?.takeIf { it.isNotEmpty() }?.let {
                keyboardView?.clearRecentClipboardSuggestion()
                commitDirect(it)
            }
        }
    }

    private fun copySelection(cut: Boolean) {
        clearCompositionBeforeEdit()
        val action = if (cut) android.R.id.cut else android.R.id.copy
        performContextAction(action) {
            sendKeyStroke(
                if (cut) KeyEvent.KEYCODE_X else KeyEvent.KEYCODE_C,
                KeyEvent.META_CTRL_ON or KeyEvent.META_CTRL_LEFT_ON,
            )
        }
        if (cut) selectionModeActive = false
    }

    private fun pasteClipboard() {
        clearCompositionBeforeEdit()
        keyboardView?.clearRecentClipboardSuggestion()
        performContextAction(android.R.id.paste) {
            currentClipboardText()?.let { commitDirect(it) }
        }
    }

    private fun selectAllText() {
        clearCompositionBeforeEdit()
        performContextAction(android.R.id.selectAll) {
            sendKeyStroke(KeyEvent.KEYCODE_A, KeyEvent.META_CTRL_ON or KeyEvent.META_CTRL_LEFT_ON)
        }
        selectionModeActive = true
    }

    private fun toggleSelectionMode() {
        clearCompositionBeforeEdit()
        selectionModeActive = !selectionModeActive
    }

    private fun extendSelection(left: Boolean) {
        clearCompositionBeforeEdit()
        sendKeyStroke(
            if (left) KeyEvent.KEYCODE_DPAD_LEFT else KeyEvent.KEYCODE_DPAD_RIGHT,
            KeyEvent.META_SHIFT_ON or KeyEvent.META_SHIFT_LEFT_ON,
        )
        selectionModeActive = true
    }

    private fun moveToLineBoundary(start: Boolean) {
        clearCompositionBeforeEdit()
        sendKeyStroke(if (start) KeyEvent.KEYCODE_MOVE_HOME else KeyEvent.KEYCODE_MOVE_END)
        selectionModeActive = false
    }

    private fun sendKeyStroke(keyCode: Int, metaState: Int = 0) {
        val connection = currentInputConnection ?: return
        val now = SystemClock.uptimeMillis()
        connection.sendKeyEvent(KeyEvent(now, now, KeyEvent.ACTION_DOWN, keyCode, 0, metaState))
        connection.sendKeyEvent(KeyEvent(now, now, KeyEvent.ACTION_UP, keyCode, 0, metaState))
    }

    private fun performContextAction(action: Int, fallback: () -> Unit = {}) {
        val performed = currentInputConnection?.performContextMenuAction(action) == true
        if (!performed) fallback()
    }

    private fun clearCompositionBeforeEdit() {
        if (!currentState.hasComposition && !composing) return
        currentInputConnection?.finishComposingText()
        composing = false
        currentState = engine.reset().withoutTransientCommit()
        keyboardView?.updateState(currentState)
    }

    private fun currentClipboardText(): String? {
        val clip = clipboardManager?.primaryClip ?: return null
        if (clip.itemCount <= 0) return null
        return clip.getItemAt(0)
            ?.coerceToText(this)
            ?.toString()
            ?.takeIf { it.isNotEmpty() }
    }

    private fun setClipboardText(text: String) {
        clipboardManager?.setPrimaryClip(ClipData.newPlainText("KeyTao", text))
        rememberClipboardText(text, suggest = false)
    }

    private fun rememberCurrentClipboard(suggest: Boolean) {
        currentClipboardText()?.let { rememberClipboardText(it, suggest) }
    }

    private fun rememberClipboardText(text: String, suggest: Boolean) {
        if (text.isBlank()) return
        val wasFirst = clipboardHistory.firstOrNull() == text
        clipboardHistory.remove(text)
        clipboardHistory.add(0, text)
        while (clipboardHistory.size > clipboardHistoryLimit) {
            clipboardHistory.removeAt(clipboardHistory.lastIndex)
        }
        if (suggest && !wasFirst) {
            keyboardView?.showRecentClipboardSuggestion(text)
        }
    }

    private fun refreshInputAvailability(): Boolean {
        val writable = engine.isUserDataWritable()
        val installed = engine.hasInstalledSchema()
        if (writable && installed && !engine.nativeReady) {
            engine.ensureReady()
        }
        val message = when {
            !writable -> "请授予 KeyTao 文件访问权限后安装键道方案"
            !installed -> "请先在 KeyTao App 安装键道方案"
            !engine.nativeReady -> "RIME 运行库未就绪，请重新安装 KeyTao"
            else -> ""
        }
        inputAvailable = message.isEmpty()
        unavailableMessage = message.ifEmpty { "请先在 KeyTao App 安装键道方案" }
        keyboardView?.updateAvailability(inputAvailable, unavailableMessage)
        return inputAvailable
    }

    private fun showUnavailableMessage() {
        refreshInputAvailability()
        keyboardView?.showMessage(unavailableMessage)
    }

    private fun commitDirect(text: String) {
        val connection = currentInputConnection ?: return
        val hadComposition = composing || currentState.hasComposition
        backspaceRestoreStack.clear()
        connection.beginBatchEdit()
        connection.commitText(text, 1)
        composing = false
        selectionModeActive = false
        connection.endBatchEdit()
        currentState = if (!engine.nativeReady) {
            currentState.withoutTransientCommit()
        } else if (hadComposition) {
            engine.reset().withoutTransientCommit()
        } else {
            engine.state().withoutTransientCommit()
        }
        keyboardView?.updateState(currentState)
    }

    private fun applyState(state: KeytaoImeState) {
        val connection = currentInputConnection
        if (connection != null) {
            connection.beginBatchEdit()
            if (state.committed.isNotEmpty()) {
                backspaceRestoreStack.clear()
                connection.commitText(state.committed, 1)
                composing = false
                selectionModeActive = false
            }

            if (state.preedit.isNotEmpty()) {
                connection.setComposingText(state.preedit, 1)
                composing = true
            } else if (composing) {
                connection.commitText("", 1)
                composing = false
            }
            connection.endBatchEdit()
        }

        currentState = state.withoutTransientCommit()
        keyboardView?.updateState(currentState)
    }

    private fun openAppPage(page: String?) {
        currentInputConnection?.finishComposingText()
        composing = false
        requestHideSelf(0)
        val intent = Intent(this, MainActivity::class.java).apply {
            addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP)
            putExtra("keytao_page", page ?: "home")
        }
        startActivity(intent)
    }

    private fun showKeyboardPicker() {
        val manager = getSystemService(InputMethodManager::class.java)
        manager?.showInputMethodPicker()
    }

    private fun isShiftKey(keyCode: Int): Boolean {
        return keyCode == KeyEvent.KEYCODE_SHIFT_LEFT || keyCode == KeyEvent.KEYCODE_SHIFT_RIGHT
    }

    private fun shouldBypassHardwareKey(key: RimeKey, event: KeyEvent): Boolean {
        if (currentState.hasComposition) return false
        if (event.isCtrlPressed || event.isAltPressed) return true
        return when (key.keyCode) {
            AndroidKeyMapper.XK_SPACE,
            AndroidKeyMapper.XK_RETURN,
            AndroidKeyMapper.XK_BACK_SPACE,
            AndroidKeyMapper.XK_DELETE,
            AndroidKeyMapper.XK_TAB,
            AndroidKeyMapper.XK_ESCAPE,
            AndroidKeyMapper.XK_HOME,
            AndroidKeyMapper.XK_END,
            AndroidKeyMapper.XK_PAGE_UP,
            AndroidKeyMapper.XK_PAGE_DOWN,
            AndroidKeyMapper.XK_LEFT,
            AndroidKeyMapper.XK_UP,
            AndroidKeyMapper.XK_RIGHT,
            AndroidKeyMapper.XK_DOWN -> true
            else -> false
        }
    }

    companion object {
        private const val clipboardHistoryLimit = 24
        private const val expandedCandidateLimit = 96
        private const val backspaceUnitContextLimit = 64
        private const val backspaceContextLimit = 8192
    }

    private fun KeyCommand.requiresInstalledSchema(): Boolean {
        return when (type) {
            KeyCommandTypes.OPEN_PAGE,
            KeyCommandTypes.BACKSPACE,
            KeyCommandTypes.BACKSPACE_GESTURE,
            KeyCommandTypes.KEYBOARD_PICKER,
            KeyCommandTypes.KEYBOARD_MODE,
            KeyCommandTypes.SHIFT,
            KeyCommandTypes.DIRECT_INPUT,
            KeyCommandTypes.EDIT,
            KeyCommandTypes.PANEL -> false
            else -> true
        }
    }
}
