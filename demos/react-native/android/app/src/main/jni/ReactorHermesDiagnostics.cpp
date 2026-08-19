#include <fbjni/fbjni.h>
#include <jsi/instrumentation.h>
#include <react/jni/JRuntimeExecutor.h>

#include <fstream>
#include <string>

namespace reactor::diagnostics {

namespace jni = facebook::jni;

std::string escapeJson(const std::string& input) {
  std::string output;
  output.reserve(input.size());
  for (const char value : input) {
    switch (value) {
      case '\\': output += "\\\\"; break;
      case '"': output += "\\\""; break;
      case '\n': output += "\\n"; break;
      case '\r': output += "\\r"; break;
      case '\t': output += "\\t"; break;
      default: output += value; break;
    }
  }
  return output;
}

class ReactorHermesDiagnostics final
    : public jni::JavaClass<ReactorHermesDiagnostics> {
 public:
  static constexpr auto kJavaDescriptor =
      "Lcom/reactor/bench/reactnative/ReactorHermesDiagnostics;";

  static void capture(
      jni::alias_ref<jclass>,
      jni::alias_ref<facebook::react::JRuntimeExecutor::javaobject> runtimeExecutor,
      const std::string& statsPath,
      const std::string& snapshotPath,
      const std::string& label) {
    auto executor = runtimeExecutor->cthis()->get();
    executor([statsPath, snapshotPath, label](facebook::jsi::Runtime& runtime) {
      try {
        auto& instrumentation = runtime.instrumentation();
        const auto stats = instrumentation.getHeapInfo(true);
        std::ofstream output(statsPath, std::ios::app);
        output << "{\"label\":\"" << escapeJson(label) << "\",\"stats\":{";
        bool first = true;
        for (const auto& [key, value] : stats) {
          if (!first) output << ',';
          first = false;
          output << '"' << escapeJson(key) << "\":" << value;
        }
        output << "}}\n";
        output.close();
        if (!snapshotPath.empty()) {
          instrumentation.collectGarbage("Reactor diagnostic heap snapshot");
          instrumentation.createSnapshotToFile(snapshotPath);
        }
      } catch (const std::exception& error) {
        std::ofstream output(statsPath, std::ios::app);
        output << "{\"label\":\"" << escapeJson(label)
               << "\",\"error\":\"" << escapeJson(error.what()) << "\"}\n";
      }
    });
  }

  static void registerNatives() {
    javaClassLocal()->registerNatives({makeNativeMethod("capture", capture)});
  }
};

} // namespace reactor::diagnostics

void registerReactorHermesDiagnostics() {
  reactor::diagnostics::ReactorHermesDiagnostics::registerNatives();
}
