package com.reactor.bench.reactnative

import com.facebook.react.bridge.ReactApplicationContext
import com.facebook.react.bridge.ReactContextBaseJavaModule
import com.facebook.react.bridge.ReactMethod
import android.os.Debug
import android.os.SystemClock
import java.io.File

class ReactorDiagnosticsModule(private val context: ReactApplicationContext) :
  ReactContextBaseJavaModule(context) {

  private val lock = Any()

  override fun getName() = "ReactorDiagnostics"

  override fun getConstants(): MutableMap<String, Any> =
    mutableMapOf(
      "diagnosticBuild" to BuildConfig.REACTOR_DIAGNOSTIC,
      "sdkVersion" to "1.0.0",
      "protocolVersion" to 1,
      "capabilities" to
        listOf(
          "react-profiler",
          "runtime-events",
          "hermes-heap-jsi",
          if (BuildConfig.REACTOR_DIAGNOSTIC) "hermes-cpu-sampling" else "hermes-cpu-unavailable",
        ),
    )

  private fun evidenceFile(): File {
    val root = context.getExternalFilesDir("reactor") ?: File(context.filesDir, "reactor")
    root.mkdirs()
    return File(root, "rn-diagnostics.ndjson")
  }

  private fun profileFile(): File = File(evidenceFile().parentFile, "rn-react-devtools-profile.json")

  private fun hermesStatsFile(): File = File(evidenceFile().parentFile, "rn-hermes-heap-stats.ndjson")

  private fun hermesSnapshotFile(): File = File(evidenceFile().parentFile, "rn-hermes.heapsnapshot")

  private fun javaHeapFile(): File = File(evidenceFile().parentFile, "rn-java.hprof")

  @ReactMethod
  fun reset() {
    synchronized(lock) {
      evidenceFile().writeText("")
      profileFile().delete()
      hermesStatsFile().delete()
      hermesSnapshotFile().delete()
      javaHeapFile().delete()
    }
  }

  @ReactMethod
  fun captureHermesHeap(label: String, snapshot: Boolean) {
    if (!BuildConfig.REACTOR_DIAGNOSTIC || label.length > 96) return
    val executor = context.runtimeExecutor ?: return
    val snapshotPath = if (snapshot) hermesSnapshotFile().absolutePath else ""
    ReactorHermesDiagnostics.capture(executor, hermesStatsFile().absolutePath, snapshotPath, label)
    if (snapshot) {
      Thread({
        try {
          Debug.dumpHprofData(javaHeapFile().absolutePath)
        } catch (_: Throwable) {
          // A failed diagnostic dump must not alter the Flow result.
        }
      }, "reactor-java-hprof").start()
    }
  }

  @ReactMethod
  fun appendEvent(kind: String, payloadJson: String) {
    if (!kind.matches(Regex("[a-z_]{1,48}")) || payloadJson.length > 64 * 1024) return
    val line =
      "{\"schemaVersion\":1,\"kind\":\"$kind\"," +
        "\"timestampMs\":${System.currentTimeMillis()}," +
        "\"elapsedRealtimeNanos\":${SystemClock.elapsedRealtimeNanos()}," +
        "\"payload\":$payloadJson}\n"
    synchronized(lock) {
      evidenceFile().appendText(line)
    }
  }

  @ReactMethod
  fun writeProfile(profileJson: String) {
    if (profileJson.length > 16 * 1024 * 1024) return
    synchronized(lock) {
      val target = profileFile()
      val temporary = File(target.parentFile, "${target.name}.tmp")
      temporary.writeText(profileJson)
      if (!temporary.renameTo(target)) {
        target.writeText(profileJson)
        temporary.delete()
      }
    }
  }
}
