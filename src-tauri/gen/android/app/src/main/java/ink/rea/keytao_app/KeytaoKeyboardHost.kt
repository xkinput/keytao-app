package ink.rea.keytao_app

import android.content.Context
import android.graphics.Color
import android.view.ViewGroup
import android.widget.FrameLayout
import kotlin.math.roundToInt

class KeytaoKeyboardHost(context: Context) : FrameLayout(context) {
    private var floating = false
    private var scale = 1f
    private var marginPx = 0

    init {
        setBackgroundColor(Color.TRANSPARENT)
        clipChildren = false
        clipToPadding = false
    }

    fun updatePresentation(enabled: Boolean, nextScale: Float, marginDp: Float) {
        floating = enabled
        scale = nextScale.coerceIn(0.70f, 1f)
        marginPx = (marginDp.coerceIn(0f, 24f) * resources.displayMetrics.density).roundToInt()
        requestLayout()
    }

    override fun onMeasure(widthMeasureSpec: Int, heightMeasureSpec: Int) {
        getChildAt(0)?.let { child ->
            val width = MeasureSpec.getSize(widthMeasureSpec)
            val horizontalMargin = if (floating) marginPx else 0
            val availableWidth = (width - paddingLeft - paddingRight - horizontalMargin * 2)
                .coerceAtLeast(1)
            val targetWidth = if (floating) {
                (width * scale).roundToInt().coerceAtMost(availableWidth)
            } else {
                ViewGroup.LayoutParams.MATCH_PARENT
            }
            val layoutParams = child.layoutParams as LayoutParams
            layoutParams.width = targetWidth
            layoutParams.height = ViewGroup.LayoutParams.WRAP_CONTENT
            layoutParams.gravity = android.view.Gravity.CENTER_HORIZONTAL or android.view.Gravity.BOTTOM
            layoutParams.setMargins(horizontalMargin, if (floating) marginPx else 0, horizontalMargin, if (floating) marginPx else 0)
        }
        super.onMeasure(widthMeasureSpec, heightMeasureSpec)
    }
}
