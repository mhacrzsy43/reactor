package com.reactor.bench.reactnative

import com.facebook.react.bridge.RuntimeExecutor

object ReactorHermesDiagnostics {
  init {
    System.loadLibrary("appmodules")
  }

  @JvmStatic external fun capture(
    runtimeExecutor: RuntimeExecutor,
    statsPath: String,
    snapshotPath: String,
    label: String,
  )
}
