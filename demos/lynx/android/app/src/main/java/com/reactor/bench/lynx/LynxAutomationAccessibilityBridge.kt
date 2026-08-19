package com.reactor.bench.lynx

import android.app.Activity
import android.app.Application
import android.graphics.Rect
import android.os.Bundle
import android.os.SystemClock
import android.view.MotionEvent
import android.view.View
import android.view.ViewGroup
import android.view.accessibility.AccessibilityEvent
import android.view.accessibility.AccessibilityNodeInfo
import android.view.accessibility.AccessibilityNodeProvider
import com.lynx.tasm.LynxView
import com.lynx.tasm.behavior.ui.LynxBaseUI
import java.util.WeakHashMap

/**
 * Exposes the benchmark's real Lynx semantic nodes to Android UiAutomation.
 *
 * Sparkling 2.0.1 + Lynx 3.6 renders the page correctly but its built-in virtual
 * accessibility provider returns an empty root to UiAutomator. This bridge mirrors
 * only versioned scenario markers and controls. Every node is resolved by Lynx id,
 * uses the element's live bounds, and dispatches clicks through the LynxView. It
 * creates no visible overlay and performs no work unless accessibility is queried.
 */
class LynxAutomationAccessibilityBridge : Application.ActivityLifecycleCallbacks {
    private val attachedActivities = WeakHashMap<Activity, Boolean>()

    override fun onActivityResumed(activity: Activity) {
        if (activity.javaClass.name != "com.tiktok.sparkling.SparklingActivity") return
        attachWhenReady(activity, 0)
    }

    private fun attachWhenReady(activity: Activity, attempt: Int) {
        if (activity.isFinishing || activity.isDestroyed || attachedActivities[activity] == true) return
        val root = activity.window?.decorView ?: return
        val lynxView = findLynxView(root)
        if (lynxView == null) {
            if (attempt < MAX_ATTACH_ATTEMPTS) {
                root.postDelayed({ attachWhenReady(activity, attempt + 1) }, ATTACH_RETRY_MS)
            }
            return
        }

        val provider = ReactorNodeProvider(lynxView)
        lynxView.importantForAccessibility = View.IMPORTANT_FOR_ACCESSIBILITY_YES
        lynxView.accessibilityDelegate = object : View.AccessibilityDelegate() {
            override fun getAccessibilityNodeProvider(host: View): AccessibilityNodeProvider = provider
        }
        attachedActivities[activity] = true
        lynxView.sendAccessibilityEvent(AccessibilityEvent.TYPE_WINDOW_CONTENT_CHANGED)
    }

    private fun findLynxView(view: View): LynxView? {
        if (view is LynxView) return view
        if (view is ViewGroup) {
            for (index in 0 until view.childCount) {
                findLynxView(view.getChildAt(index))?.let { return it }
            }
        }
        return null
    }

    override fun onActivityCreated(activity: Activity, state: Bundle?) = Unit
    override fun onActivityStarted(activity: Activity) = Unit
    override fun onActivityPaused(activity: Activity) = Unit
    override fun onActivityStopped(activity: Activity) = Unit
    override fun onActivitySaveInstanceState(activity: Activity, state: Bundle) = Unit
    override fun onActivityDestroyed(activity: Activity) {
        attachedActivities.remove(activity)
    }

    private data class NodeSpec(
        val virtualId: Int,
        val selector: String,
        val label: String,
        val clickable: Boolean = false,
        val className: String = "android.widget.TextView",
    )

    private class ReactorNodeProvider(private val host: LynxView) : AccessibilityNodeProvider() {
        private val specs = listOf(
            NodeSpec(1, "reactor-ready", "Reactor ready"),
            NodeSpec(2, "list-scenario", "List scenario", true, "android.widget.Button"),
            NodeSpec(3, "update-scenario", "Update scenario", true, "android.widget.Button"),
            NodeSpec(4, "animation-scenario", "Animation scenario", true, "android.widget.Button"),
            NodeSpec(5, "list-ready", "List ready"),
            NodeSpec(6, "update-ready", "Update ready"),
            NodeSpec(7, "update-complete", "Update complete"),
            NodeSpec(8, "animation-ready", "Animation ready"),
            NodeSpec(9, "animation-complete", "Animation complete"),
        )

        override fun createAccessibilityNodeInfo(virtualViewId: Int): AccessibilityNodeInfo? {
            if (virtualViewId == View.NO_ID) return createHostNode()
            val spec = specs.firstOrNull { it.virtualId == virtualViewId } ?: return null
            val ui = findUi(spec.selector) ?: return null
            return createVirtualNode(spec, ui)
        }

        override fun findAccessibilityNodeInfosByText(
            searched: String,
            virtualViewId: Int,
        ): MutableList<AccessibilityNodeInfo> {
            return specs.asSequence()
                .filter { it.label.contains(searched, ignoreCase = true) }
                .mapNotNull { spec -> findUi(spec.selector)?.let { createVirtualNode(spec, it) } }
                .toMutableList()
        }

        override fun performAction(virtualViewId: Int, action: Int, arguments: Bundle?): Boolean {
            val spec = specs.firstOrNull { it.virtualId == virtualViewId } ?: return false
            if (action != AccessibilityNodeInfo.ACTION_CLICK || !spec.clickable) return false
            val ui = findUi(spec.selector) ?: return false
            val bounds = ui.boundingClientRect
            if (bounds.isEmpty) return false

            val eventTime = SystemClock.uptimeMillis()
            val x = bounds.exactCenterX()
            val y = bounds.exactCenterY()
            val down = MotionEvent.obtain(eventTime, eventTime, MotionEvent.ACTION_DOWN, x, y, 0)
            val up = MotionEvent.obtain(eventTime, eventTime + 16, MotionEvent.ACTION_UP, x, y, 0)
            return try {
                host.dispatchTouchEvent(down)
                host.dispatchTouchEvent(up)
            } finally {
                down.recycle()
                up.recycle()
            }
        }

        private fun createHostNode(): AccessibilityNodeInfo {
            val info = AccessibilityNodeInfo.obtain()
            info.packageName = host.context.packageName
            info.className = host.javaClass.name
            info.setSource(host)
            info.isEnabled = host.isEnabled
            info.isVisibleToUser = host.visibility == View.VISIBLE
            info.setBoundsInParent(Rect(0, 0, host.width, host.height))
            info.setBoundsInScreen(hostBoundsOnScreen())
            specs.forEach { spec ->
                if (findUi(spec.selector) != null) info.addChild(host, spec.virtualId)
            }
            return info
        }

        private fun createVirtualNode(spec: NodeSpec, ui: LynxBaseUI): AccessibilityNodeInfo {
            val localBounds = ui.boundingClientRect
            val screenBounds = Rect(localBounds)
            val hostLocation = IntArray(2)
            host.getLocationOnScreen(hostLocation)
            screenBounds.offset(hostLocation[0], hostLocation[1])

            return AccessibilityNodeInfo.obtain().apply {
                packageName = host.context.packageName
                className = spec.className
                text = spec.label
                contentDescription = spec.label
                setParent(host)
                setSource(host, spec.virtualId)
                setBoundsInParent(localBounds)
                setBoundsInScreen(screenBounds)
                isEnabled = true
                isVisibleToUser = !localBounds.isEmpty
                isFocusable = true
                isClickable = spec.clickable
                addAction(AccessibilityNodeInfo.ACTION_ACCESSIBILITY_FOCUS)
                if (spec.clickable) addAction(AccessibilityNodeInfo.ACTION_CLICK)
            }
        }

        private fun findUi(selector: String): LynxBaseUI? {
            return host.findUIByIdSelector(selector) ?: host.findUIByIdSelector("#$selector")
        }

        private fun hostBoundsOnScreen(): Rect {
            val location = IntArray(2)
            host.getLocationOnScreen(location)
            return Rect(location[0], location[1], location[0] + host.width, location[1] + host.height)
        }
    }

    private companion object {
        const val MAX_ATTACH_ATTEMPTS = 40
        const val ATTACH_RETRY_MS = 50L
    }
}
