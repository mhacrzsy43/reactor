package com.reactor.bench.reactnative

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.os.SystemClock
import com.facebook.hermes.instrumentation.HermesSamplingProfiler
import java.io.File
import java.util.concurrent.Executors
import java.util.concurrent.ScheduledFuture
import java.util.concurrent.TimeUnit

class ReactorDiagnosticsController : BroadcastReceiver() {
  override fun onReceive(context: Context, intent: Intent) {
    val pending = goAsync()
    executor.execute {
      try {
        handle(context, intent)
      } finally {
        pending.finish()
      }
    }
  }

  private fun handle(context: Context, intent: Intent) {
    val token = intent.getStringExtra(EXTRA_TOKEN).orEmpty()
    val command = intent.getStringExtra(EXTRA_COMMAND).orEmpty()
    val leaseMs = intent.getLongExtra(EXTRA_LEASE_MS, 0L)
    if (!TOKEN_PATTERN.matches(token)) {
      writeAck(context, token, command, "failed", "invalid token")
      return
    }

    synchronized(lock) {
      try {
        when (command) {
          "status" -> writeAck(context, token, command, "collected")
          "start" -> start(context, token, command, leaseMs)
          "stopAndDump" -> stopAndDump(context, token, command)
          "abort" -> abort(context, token, command)
          else -> writeAck(context, token, command, "failed", "unsupported command")
        }
      } catch (error: Throwable) {
        if (command == "start" || command == "stopAndDump" || command == "abort") {
          runCatching { HermesSamplingProfiler.disable() }
          cancelWatchdog()
          activeToken = null
        }
        writeAck(context, token, command, "failed", error.message ?: error.javaClass.simpleName)
      }
    }
  }

  private fun start(context: Context, token: String, command: String, leaseMs: Long) {
    if (leaseMs !in MIN_LEASE_MS..MAX_LEASE_MS) {
      writeAck(context, token, command, "failed", "leaseMs outside supported range")
      return
    }
    if (activeToken != null) {
      writeAck(context, token, command, "failed", "profiler already active")
      return
    }
    cpuProfileFile(context).delete()
    HermesSamplingProfiler.enable()
    activeToken = token
    cancelWatchdog()
    watchdog = watchdogExecutor.schedule({
      synchronized(lock) {
        if (activeToken == token) {
          runCatching { HermesSamplingProfiler.disable() }
          activeToken = null
          cpuProfileFile(context).delete()
          writeAck(context, token, "leaseExpired", "failed", "sampling lease expired")
        }
      }
    }, leaseMs, TimeUnit.MILLISECONDS)
    writeAck(context, token, command, "collected")
  }

  private fun cancelWatchdog() {
    watchdog?.cancel(false)
    watchdog = null
  }

  private fun stopAndDump(context: Context, token: String, command: String) {
    if (activeToken != token) {
      writeAck(context, token, command, "failed", "token does not own active profiler")
      return
    }
    var dumpError: Throwable? = null
    try {
      HermesSamplingProfiler.dumpSampledTraceToFile(cpuProfileFile(context).absolutePath)
    } catch (error: Throwable) {
      dumpError = error
    } finally {
      HermesSamplingProfiler.disable()
      cancelWatchdog()
      activeToken = null
    }
    if (dumpError != null) throw dumpError
    val profile = cpuProfileFile(context)
    if (!profile.isFile || profile.length() == 0L) {
      writeAck(context, token, command, "failed", "profiler produced no artifact")
      return
    }
    writeAck(context, token, command, "collected", artifactBytes = profile.length())
  }

  private fun abort(context: Context, token: String, command: String) {
    if (activeToken != null && activeToken != token) {
      writeAck(context, token, command, "failed", "token does not own active profiler")
      return
    }
    if (activeToken == token) HermesSamplingProfiler.disable()
    cancelWatchdog()
    activeToken = null
    cpuProfileFile(context).delete()
    writeAck(context, token, command, "collected")
  }

  private fun root(context: Context): File {
    val directory = context.getExternalFilesDir("reactor") ?: File(context.filesDir, "reactor")
    directory.mkdirs()
    return directory
  }

  private fun cpuProfileFile(context: Context) = File(root(context), CPU_PROFILE_FILE)

  private fun writeAck(
    context: Context,
    token: String,
    command: String,
    status: String,
    error: String? = null,
    artifactBytes: Long? = null,
  ) {
    val payload = buildString {
      append("{\"schemaVersion\":1")
      append(",\"sdkVersion\":\"").append(SDK_VERSION).append('"')
      append(",\"token\":\"").append(escapeJson(token)).append('"')
      append(",\"command\":\"").append(escapeJson(command)).append('"')
      append(",\"status\":\"").append(status).append('"')
      append(",\"wallTimeMs\":").append(System.currentTimeMillis())
      append(",\"elapsedRealtimeNanos\":").append(SystemClock.elapsedRealtimeNanos())
      append(",\"diagnosticBuild\":true")
      append(",\"active\":").append(activeToken != null)
      append(",\"capabilities\":[\"react-profiler\",\"runtime-events\",\"hermes-cpu-sampling\",\"hermes-heap-jsi\"]")
      artifactBytes?.let {
        append(",\"artifact\":{\"path\":\"").append(CPU_PROFILE_FILE)
          .append("\",\"format\":\"hermes-sampling-chrome-trace-json\",\"bytes\":").append(it).append('}')
      }
      error?.let { append(",\"error\":\"").append(escapeJson(it)).append('"') }
      append('}')
    }
    val target = File(root(context), ACK_FILE)
    val temporary = File(root(context), "$ACK_FILE.tmp")
    temporary.writeText(payload)
    if (!temporary.renameTo(target)) {
      target.writeText(payload)
      temporary.delete()
    }
  }

  private fun escapeJson(value: String): String =
    value.replace("\\", "\\\\").replace("\"", "\\\"").replace("\n", "\\n").replace("\r", "\\r")

  companion object {
    const val ACTION = "com.reactor.bench.reactnative.DIAGNOSTICS"
    const val EXTRA_COMMAND = "command"
    const val EXTRA_TOKEN = "token"
    const val EXTRA_LEASE_MS = "leaseMs"
    const val ACK_FILE = "rn-controller-ack.json"
    const val CPU_PROFILE_FILE = "rn-hermes-cpu.trace.json"
    const val SDK_VERSION = "1.0.0"
    const val MIN_LEASE_MS = 1_000L
    const val MAX_LEASE_MS = 31 * 60 * 1_000L

    private val TOKEN_PATTERN = Regex("[A-Za-z0-9._-]{8,96}")
    private val executor = Executors.newSingleThreadExecutor { runnable ->
      Thread(runnable, "reactor-diagnostics-controller")
    }
    private val watchdogExecutor = Executors.newSingleThreadScheduledExecutor { runnable ->
      Thread(runnable, "reactor-diagnostics-watchdog")
    }
    private val lock = Any()
    private var activeToken: String? = null
    private var watchdog: ScheduledFuture<*>? = null
  }
}
