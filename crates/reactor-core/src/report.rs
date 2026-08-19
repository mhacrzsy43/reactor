use std::fmt::Write as _;

use reactor_protocol::NormalizedResult;

/// Renders a self-contained, offline HTML report from normalized results.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn render_html_report(title: &str, results: &[NormalizedResult]) -> String {
    let synthetic = results.iter().any(|result| result.source.synthetic);
    let emulator = !synthetic
        && results
            .iter()
            .any(|result| result.device.physical == Some(false));
    let source_label = if synthetic {
        "产品导览 · 模拟数据"
    } else if emulator {
        "模拟器测量 · 同机回归基线"
    } else {
        "物理设备测量"
    };
    let mut html = String::from(
        "<!doctype html><html lang=\"zh-CN\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">",
    );
    write!(
        html,
        "<title>{}</title><style>{}</style></head><body><main>",
        escape(title),
        CSS
    )
    .expect("writing to String cannot fail");
    write!(
        html,
        "<header><div><p class=\"eyebrow\">REACTOR PERFORMANCE REPORT</p><h1>{}</h1>\
         <p>{}</p></div><span class=\"badge\">{}</span></header>",
        escape(title),
        escape(source_label),
        if synthetic {
            "SIMULATED"
        } else if emulator {
            "EMULATOR"
        } else {
            "MEASURED"
        }
    )
    .expect("writing to String cannot fail");
    if synthetic || emulator {
        write!(
            html,
            "<aside class=\"warning\">{}</aside>",
            if synthetic {
                "模拟数据仅用于体验 Reactor 产品流程，不得用于框架性能结论。"
            } else {
                "模拟器数据只适合同一主机、同一模拟器配置下的开发回归，不得与物理真机混排。"
            }
        )
        .expect("writing to String cannot fail");
    }
    html.push_str(
        "<section class=\"comparison\"><div class=\"section-title\"><div><p class=\"eyebrow\">COMPARISON</p><h2>统一指标对比</h2></div><p>↓ 越低越好 · ↑ 越高越好</p></div>\
         <div class=\"table-scroll\"><table><thead><tr><th>场景</th><th>框架</th>\
         <th>P95 帧耗时 ↓</th><th>Jank ↓</th><th>冷启动 ↓</th><th>原生 PSS ↓</th>\
         <th>平均 FPS ↑</th><th>平均 CPU ↓</th><th>成功迭代</th></tr></thead><tbody>",
    );
    for result in results {
        let android = result.android_native.as_ref();
        write!(
            html,
            "<tr><td>{}</td><td><strong>{}</strong></td><td>{}</td><td>{}</td><td>{}</td>\
             <td>{}</td><td>{}</td><td>{}</td><td>{}/{}</td></tr>",
            escape(&result.scenario),
            escape(&result.framework),
            number(android.and_then(|metrics| metrics.frame_time_p95_ms), " ms"),
            number(android.and_then(|metrics| metrics.jank_frame_pct), "%"),
            number(android.and_then(|metrics| metrics.startup_time_ms), " ms"),
            number(android.and_then(|metrics| metrics.memory_pss_mb), " MB"),
            number(result.summary.fps_mean, ""),
            number(result.summary.cpu_mean_pct, "%"),
            result.summary.successful_iteration_count,
            result.summary.iteration_count,
        )
        .expect("writing to String cannot fail");
    }
    html.push_str("</tbody></table></div></section>");
    html.push_str("<section class=\"cards\">");
    for result in results {
        let android = result.android_native.as_ref();
        let ios = result.ios_native.as_ref();
        write!(
            html,
            "<article><div class=\"card-title\"><h2>{}</h2><span>{}</span></div>\
             <div class=\"hero\"><strong>{}</strong><small>{}</small></div>\
             <dl><div><dt>P10 FPS</dt><dd>{}</dd></div>\
             <div><dt>低帧率采样</dt><dd>{}</dd></div>\
             <div><dt>平均 CPU</dt><dd>{}</dd></div>\
             <div><dt>UI CPU</dt><dd>{}</dd></div>\
             <div><dt>JS CPU</dt><dd>{}</dd></div>\
             <div><dt>平均内存</dt><dd>{}</dd></div>\
             <div><dt>峰值内存</dt><dd>{}</dd></div>\
             <div><dt>成功迭代</dt><dd>{}/{}</dd></div></dl>",
            escape(&result.framework),
            escape(&result.scenario),
            number(
                android
                    .and_then(|metrics| metrics.frame_time_p95_ms)
                    .or_else(|| ios.and_then(|metrics| metrics.cpu_mean_pct))
                    .or(result.summary.fps_mean),
                if android.is_some() {
                    " ms"
                } else if ios.is_some() {
                    "%"
                } else {
                    ""
                },
            ),
            if android.is_some() {
                "P95 帧耗时"
            } else if ios.is_some() {
                "Time Profiler CPU"
            } else {
                "平均 FPS"
            },
            number(result.summary.fps_p10, ""),
            number(result.summary.low_fps_sample_pct, "%"),
            number(result.summary.cpu_mean_pct, "%"),
            number(result.summary.ui_cpu_mean_pct, "%"),
            number(result.summary.js_cpu_mean_pct, "%"),
            number(result.summary.ram_mean_mb, " MB"),
            number(result.summary.ram_peak_mb, " MB"),
            result.summary.successful_iteration_count,
            result.summary.iteration_count,
        )
        .expect("writing to String cannot fail");
        if let Some(native) = android {
            write!(
                html,
                "<div class=\"native\"><h3>Android 原生指标</h3><dl>\
                 <div><dt>帧样本</dt><dd>{}</dd></div>\
                 <div><dt>P50 / P95 / P99</dt><dd>{} / {} / {}</dd></div>\
                 <div><dt>Jank</dt><dd>{} / {} 帧</dd></div>\
                 <div><dt>超帧预算</dt><dd>{}</dd></div>\
                 <div><dt>冷启动</dt><dd>{}</dd></div>\
                 <div><dt>原生 PSS</dt><dd>{}</dd></div>\
                 <div><dt>热状态（前 → 后）</dt><dd>{} → {}</dd></div>\
                 </dl><p>{} · Trace Processor {}</p></div>",
                native.frame_count,
                number(native.frame_time_p50_ms, " ms"),
                number(native.frame_time_p95_ms, " ms"),
                number(native.frame_time_p99_ms, " ms"),
                number(native.jank_frame_pct, "%"),
                native.jank_frame_count,
                number(native.over_budget_frame_pct, "%"),
                number(native.startup_time_ms, " ms"),
                number(native.memory_pss_mb, " MB"),
                thermal(native.thermal_status_before),
                thermal(native.thermal_status_after),
                escape(&native.definitions_version),
                escape(&native.trace_processor_version),
            )
            .expect("writing to String cannot fail");
            if let Some(leak) = &native.memory_leak {
                let verdict = match leak.verdict.as_str() {
                    "suspected_leak" => "疑似泄漏",
                    "stable" => "趋势稳定",
                    _ => "证据不足",
                };
                write!(
                    html,
                    "<div class=\"native leak\"><h3>同进程循环内存 · {}</h3><dl>\
                     <div><dt>行为循环</dt><dd>{} 轮</dd></div>\
                     <div><dt>PSS 增长斜率</dt><dd>{}</dd></div>\
                     <div><dt>首尾差</dt><dd>{}</dd></div>\
                     <div><dt>单调增长</dt><dd>{}</dd></div>\
                     <div><dt>冷却回落</dt><dd>{}</dd></div>\
                     <div><dt>置信度</dt><dd>{}</dd></div></dl>\
                     <p>进程趋势只允许标记疑似泄漏；确认泄漏需要堆对象保留证据。 · {}</p></div>",
                    verdict,
                    leak.cycles,
                    number(leak.slope_mb_per_cycle, " MB/轮"),
                    number(leak.end_delta_mb, " MB"),
                    number(leak.monotonic_growth_pct, "%"),
                    number(leak.cooldown_recovery_mb, " MB"),
                    escape(&leak.confidence),
                    escape(&leak.definitions_version),
                )
                .expect("writing to String cannot fail");
            }
        }
        if let Some(native) = ios {
            write!(
                html,
                "<div class=\"native\"><h3>iOS xctrace 原生指标</h3><dl>\
                 <div><dt>CPU Running 样本</dt><dd>{}</dd></div>\
                 <div><dt>采样 CPU</dt><dd>{}</dd></div>\
                 <div><dt>录制时长</dt><dd>{}</dd></div>\
                 <div><dt>帧</dt><dd>{}</dd></div>\
                 <div><dt>启动</dt><dd>{}</dd></div>\
                 <div><dt>内存</dt><dd>{}</dd></div>\
                 <div><dt>能耗</dt><dd>{}</dd></div>\
                 </dl><p>{} · xctrace {}</p></div>",
                native.cpu_sample_count,
                number(native.cpu_mean_pct, "%"),
                number(Some(native.recording_duration_ms), " ms"),
                escape(&native.availability.frames),
                escape(&native.availability.startup),
                escape(&native.availability.memory),
                escape(&native.availability.energy),
                escape(&native.definitions_version),
                escape(&native.xctrace_version),
            )
            .expect("writing to String cannot fail");
        }
        html.push_str("</article>");
    }
    html.push_str("</section><section class=\"evidence\"><h2>可追溯证据</h2><table><thead><tr><th>框架</th><th>应用 / 版本</th><th>Flow SHA-256</th><th>采集器</th><th>设备 / OS</th><th>原始文件</th></tr></thead><tbody>");
    for result in results {
        write!(
            html,
            "<tr><td>{}</td><td>{}</td><td><code>{}</code></td><td>{}</td><td>{}</td><td>{}</td></tr>",
            escape(&result.framework),
            escape(&format!(
                "{} · {}",
                result.app_id.as_deref().unwrap_or("未记录"),
                result.app_version.as_deref().unwrap_or("版本未记录")
            )),
            escape(&result.flow_hash),
            escape(result.android_native.as_ref().map_or_else(
                || {
                    result
                        .ios_native
                        .as_ref()
                        .map_or(result.adapter.as_str(), |native| native.collector.as_str())
                },
                |native| native.collector.as_str(),
            )),
            escape(&format!(
                "{} · {}",
                result
                    .device
                    .name
                    .as_deref()
                    .or(result.device.id.as_deref())
                    .unwrap_or("未记录"),
                result.device.os_version.as_deref().unwrap_or("OS 未记录")
            )),
            escape(result.android_native.as_ref().map_or_else(
                || {
                    result.ios_native.as_ref().map_or_else(
                        || {
                            result
                                .source
                                .raw_file
                                .as_deref()
                                .unwrap_or("无（产品导览）")
                        },
                        |native| native.trace_archive_file.as_str(),
                    )
                },
                |native| native.perfetto_trace_file.as_str(),
            )),
        )
        .expect("writing to String cannot fail");
    }
    html.push_str("</tbody></table></section><footer>Reactor · AI 只参与测量前 Flow 准备，测量窗口执行锁定产物。</footer></main></body></html>");
    html
}

fn number(value: Option<f64>, unit: &str) -> String {
    value.map_or_else(|| "—".to_owned(), |value| format!("{value:.1}{unit}"))
}

fn thermal(value: Option<u32>) -> String {
    value.map_or_else(|| "—".to_owned(), |value| value.to_string())
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

const CSS: &str = r#"
:root{color-scheme:dark;font-family:Inter,-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif;background:#080b12;color:#f6f7fb}*{box-sizing:border-box}body{margin:0;background:radial-gradient(circle at 50% -20%,#38255e55,transparent 38%),#080b12}main{max-width:1180px;margin:auto;padding:56px 28px}header{display:flex;justify-content:space-between;gap:24px;align-items:flex-start;margin-bottom:24px}.eyebrow{font-size:12px;letter-spacing:.16em;color:#9f8cff}h1{font-size:36px;margin:8px 0}header p{color:#aab0c0}.badge{border:1px solid #6e5bea;background:#6e5bea22;color:#c9c0ff;padding:8px 12px;border-radius:999px;font-size:12px;font-weight:700}.warning{border:1px solid #f2a93b55;background:#f2a93b12;color:#ffd897;padding:14px 16px;border-radius:12px;margin:22px 0}.comparison,.cards article,.evidence{border:1px solid #252a38;background:#111620;border-radius:18px;padding:22px}.comparison{margin:18px 0}.section-title{display:flex;align-items:end;justify-content:space-between;gap:18px;margin-bottom:12px}.section-title h2{margin:4px 0 0}.section-title p{color:#8d95a8;font-size:12px}.table-scroll{overflow:auto}.cards{display:grid;grid-template-columns:repeat(auto-fit,minmax(280px,1fr));gap:16px}.card-title{display:flex;justify-content:space-between;gap:12px}.card-title h2{font-size:18px;margin:0}.card-title span{color:#8d95a8;font-size:12px}.hero{margin:24px 0}.hero strong{font-size:44px;display:block}.hero small{color:#8d95a8}dl{margin:0}dl div{display:flex;justify-content:space-between;gap:18px;padding:10px 0;border-top:1px solid #252a38}dt{color:#9ba2b2}dd{margin:0;font-weight:700;text-align:right}.native{margin-top:20px;padding-top:4px}.native h3{font-size:13px;color:#c9c0ff;margin:0 0 10px}.native p{font-size:11px;color:#737b8e;margin:12px 0 0}.evidence{margin-top:18px;overflow:auto}.evidence h2{margin-top:0}table{width:100%;border-collapse:collapse;font-size:13px}th,td{text-align:left;padding:12px;border-bottom:1px solid #252a38;white-space:nowrap}th{color:#8d95a8}code{font-size:11px;color:#c7bbff}footer{text-align:center;color:#737b8e;font-size:12px;margin-top:28px}@media(max-width:620px){header,.section-title{display:block}.badge{display:inline-block;margin-top:8px}h1{font-size:28px}main{padding:32px 18px}}
"#;

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use reactor_protocol::{
        AndroidNativeMetrics, DeviceMetadata, MetricSummary, NormalizedResult, ResultSource,
    };

    use super::*;

    #[test]
    fn marks_synthetic_reports_and_escapes_content() {
        let result = NormalizedResult {
            schema_version: 1,
            run_id: "run".to_owned(),
            created_at: Utc::now(),
            framework: "<lynx>".to_owned(),
            platform: "android".to_owned(),
            scenario: "list".to_owned(),
            adapter: "tour".to_owned(),
            build_mode: "release".to_owned(),
            flow_hash: "abc".to_owned(),
            app_id: Some("com.reactor.fixture".to_owned()),
            app_version: Some("1.0 (1)".to_owned()),
            device: DeviceMetadata {
                id: None,
                name: None,
                os_version: None,
                refresh_rate: 60.0,
                physical: None,
            },
            source: ResultSource {
                name: None,
                status: Some("SYNTHETIC".to_owned()),
                raw_file: None,
                synthetic: true,
            },
            android_native: None,
            ios_native: None,
            iterations: vec![],
            summary: MetricSummary {
                iteration_count: 0,
                successful_iteration_count: 0,
                fps_mean: None,
                fps_p10: None,
                low_fps_sample_pct: None,
                ram_mean_mb: None,
                ram_peak_mb: None,
                cpu_mean_pct: None,
                ui_cpu_mean_pct: None,
                js_cpu_mean_pct: None,
            },
            warnings: vec![],
        };
        let native_result = NormalizedResult {
            android_native: Some(AndroidNativeMetrics {
                schema_version: 1,
                definitions_version: "android-native-v1".to_owned(),
                collector: "perfetto-frametimeline-v1".to_owned(),
                trace_processor_version: "57.2".to_owned(),
                perfetto_trace_file: "perfetto.pftrace".to_owned(),
                frame_count: 120,
                frame_time_mean_ms: Some(16.1),
                frame_time_p50_ms: Some(15.8),
                frame_time_p95_ms: Some(22.0),
                frame_time_p99_ms: Some(31.5),
                jank_frame_count: 7,
                jank_frame_pct: Some(5.8),
                over_budget_frame_pct: Some(8.3),
                startup_time_ms: Some(278.0),
                memory_pss_mb: Some(51.7),
                thermal_status_before: Some(0),
                thermal_status_after: Some(1),
                memory_leak: None,
                warnings: vec![],
            }),
            ..result.clone()
        };
        let html = render_html_report("Tour", &[result]);
        assert!(html.contains("SIMULATED"));
        assert!(html.contains("&lt;lynx&gt;"));
        let native_html = render_html_report("Native", &[native_result]);
        assert!(native_html.contains("Android 原生指标"));
        assert!(native_html.contains("统一指标对比"));
        assert!(native_html.contains("P95 帧耗时"));
        assert!(native_html.contains("perfetto.pftrace"));
        assert!(native_html.contains("Trace Processor 57.2"));
    }
}
