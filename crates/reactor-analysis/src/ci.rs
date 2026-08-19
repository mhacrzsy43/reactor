use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use crate::{AnalysisReport, AnalysisVerdict, MetricVerdict, ProfileDiffReport};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CiStatus {
    Passed,
    Regressed,
    Incompatible,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CiReport {
    pub schema_version: u32,
    pub status: CiStatus,
    pub exit_code: u8,
    pub analyses: Vec<AnalysisReport>,
    pub profile_diff: Option<ProfileDiffReport>,
}

impl CiReport {
    #[must_use]
    pub fn new(analyses: Vec<AnalysisReport>, profile_diff: Option<ProfileDiffReport>) -> Self {
        let incompatible = analyses
            .iter()
            .any(|analysis| analysis.verdict == AnalysisVerdict::Incompatible)
            || profile_diff
                .as_ref()
                .is_some_and(|profile| !profile.compatible);
        let regressed = analyses
            .iter()
            .any(|analysis| analysis.verdict == AnalysisVerdict::Regressed)
            || profile_diff
                .as_ref()
                .is_some_and(|profile| profile.regression_count > 0);
        let (status, exit_code) = if incompatible {
            (CiStatus::Incompatible, 3)
        } else if regressed {
            (CiStatus::Regressed, 2)
        } else {
            (CiStatus::Passed, 0)
        };
        Self {
            schema_version: 1,
            status,
            exit_code,
            analyses,
            profile_diff,
        }
    }

    #[must_use]
    pub fn test_count(&self) -> usize {
        let metric_count = self
            .analyses
            .iter()
            .map(|analysis| analysis.metrics.len().max(1))
            .sum::<usize>();
        metric_count
            + self
                .profile_diff
                .as_ref()
                .map_or(0, |profile| profile.components.len().max(1))
    }
}

#[must_use]
pub fn render_ci_junit(report: &CiReport) -> String {
    let failures = report
        .analyses
        .iter()
        .flat_map(|analysis| &analysis.metrics)
        .filter(|metric| metric.verdict == MetricVerdict::Regressed)
        .count()
        + report.profile_diff.as_ref().map_or(0, |profile| {
            usize::try_from(profile.regression_count).unwrap_or(usize::MAX)
        });
    let errors = report
        .analyses
        .iter()
        .map(|analysis| {
            if analysis.verdict == AnalysisVerdict::Incompatible {
                analysis.metrics.len().max(1)
            } else {
                0
            }
        })
        .sum::<usize>()
        + report.profile_diff.as_ref().map_or(0, |profile| {
            if profile.compatible {
                0
            } else {
                profile.components.len().max(1)
            }
        });
    let mut cases = String::new();
    for analysis in &report.analyses {
        if analysis.metrics.is_empty() {
            push_incompatible_case(&mut cases, analysis);
            continue;
        }
        for metric in &analysis.metrics {
            let name = format!("{}::{}", analysis.evidence.framework, metric.id);
            let _ = write!(
                cases,
                "  <testcase classname=\"reactor.performance\" name=\"{}\">",
                escape_xml(&name)
            );
            match metric.verdict {
                MetricVerdict::Regressed => {
                    let _ = write!(
                        cases,
                        "<failure message=\"{} regressed\">{}</failure>",
                        escape_xml(&metric.label),
                        escape_xml(&metric_summary(metric))
                    );
                }
                MetricVerdict::Unavailable if !analysis.compatibility.compatible => {
                    let _ = write!(
                        cases,
                        "<error message=\"incompatible baseline\">{}</error>",
                        escape_xml(&analysis.compatibility.reasons.join("; "))
                    );
                }
                _ => {}
            }
            cases.push_str("</testcase>\n");
        }
    }
    if let Some(profile) = &report.profile_diff {
        if profile.components.is_empty() {
            let _ = writeln!(
                cases,
                "  <testcase classname=\"reactor.profile\" name=\"compatibility\"><error message=\"incompatible profile\">{}</error></testcase>\n",
                escape_xml(&profile.reasons.join("; "))
            );
        } else {
            for component in &profile.components {
                let _ = write!(
                    cases,
                    "  <testcase classname=\"reactor.profile\" name=\"{}\">",
                    escape_xml(&component.name)
                );
                if component.regressed {
                    let summary = format!(
                        "render {} -> {}; total {:.2}ms -> {:.2}ms",
                        component.baseline_render_count,
                        component.current_render_count,
                        component.baseline_total_time_ms,
                        component.current_total_time_ms
                    );
                    let _ = write!(
                        cases,
                        "<failure message=\"component profile regressed\">{}</failure>",
                        escape_xml(&summary)
                    );
                }
                cases.push_str("</testcase>\n");
            }
        }
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuite name=\"Reactor Performance\" tests=\"{}\" failures=\"{failures}\" errors=\"{errors}\">\n{cases}</testsuite>\n",
        report.test_count()
    )
}

#[must_use]
pub fn render_ci_html(report: &CiReport) -> String {
    let mut sections = String::new();
    for analysis in &report.analyses {
        let mut metric_rows = String::new();
        for metric in &analysis.metrics {
            let _ = write!(
                metric_rows,
                "<tr class=\"{}\"><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{:?}</td></tr>",
                verdict_class(metric.verdict),
                escape_html(&metric.label),
                option_number(metric.baseline),
                option_number(metric.current),
                metric
                    .percent_delta
                    .map_or_else(|| "—".to_owned(), |value| format!("{value:+.1}%")),
                metric.verdict
            );
        }
        let _ = write!(
            sections,
            "<section><h2>{}</h2><p>{} → {}</p><table><thead><tr><th>Metric</th><th>Baseline</th><th>Current</th><th>Delta</th><th>Verdict</th></tr></thead><tbody>{metric_rows}</tbody></table></section>",
            escape_html(&analysis.evidence.framework),
            escape_html(&analysis.evidence.baseline_run_id),
            escape_html(&analysis.evidence.current_run_id)
        );
    }
    if let Some(profile) = &report.profile_diff {
        let mut rows = String::new();
        for component in &profile.components {
            let _ = write!(
                rows,
                "<tr class=\"{}\"><td>{}</td><td>{} → {}</td><td>{:.1} → {:.1} ms</td><td>{}</td></tr>",
                if component.regressed {
                    "regressed"
                } else {
                    "stable"
                },
                escape_html(&component.name),
                component.baseline_render_count,
                component.current_render_count,
                component.baseline_total_time_ms,
                component.current_total_time_ms,
                if component.regressed {
                    "REGRESSED"
                } else {
                    "STABLE"
                }
            );
        }
        let _ = write!(
            sections,
            "<section><h2>Component Profile Diff</h2><p>{} regressions</p><table><thead><tr><th>Component</th><th>Render</th><th>Total Time</th><th>Verdict</th></tr></thead><tbody>{rows}</tbody></table></section>",
            profile.regression_count
        );
    }
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Reactor CI Report</title><style>{}</style></head><body><main><header><div><small>REACTOR CI · ANALYSIS v1</small><h1>Performance Regression Report</h1></div><strong class=\"status {}\">{:?} · EXIT {}</strong></header>{sections}<footer>Deterministic facts generated locally. AI did not alter measurements or verdicts.</footer></main></body></html>",
        CI_CSS,
        status_class(report.status),
        report.status,
        report.exit_code
    )
}

fn push_incompatible_case(cases: &mut String, analysis: &AnalysisReport) {
    let _ = writeln!(
        cases,
        "  <testcase classname=\"reactor.performance\" name=\"{}::compatibility\"><error message=\"incompatible baseline\">{}</error></testcase>\n",
        escape_xml(&analysis.evidence.framework),
        escape_xml(&analysis.compatibility.reasons.join("; "))
    );
}

fn metric_summary(metric: &crate::MetricComparison) -> String {
    format!(
        "baseline={}, current={}, delta={}, threshold={:.1}%",
        option_number(metric.baseline),
        option_number(metric.current),
        metric
            .percent_delta
            .map_or_else(|| "n/a".to_owned(), |value| format!("{value:+.1}%")),
        metric.threshold_pct
    )
}

fn option_number(value: Option<f64>) -> String {
    value.map_or_else(|| "—".to_owned(), |number| format!("{number:.2}"))
}

fn verdict_class(verdict: MetricVerdict) -> &'static str {
    match verdict {
        MetricVerdict::Regressed | MetricVerdict::Unavailable => "regressed",
        MetricVerdict::Improved => "improved",
        MetricVerdict::Stable => "stable",
    }
}

fn status_class(status: CiStatus) -> &'static str {
    match status {
        CiStatus::Passed => "passed",
        CiStatus::Regressed | CiStatus::Incompatible => "failed",
    }
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn escape_html(value: &str) -> String {
    escape_xml(value)
}

const CI_CSS: &str = r"
:root{color-scheme:dark;font-family:Inter,-apple-system,sans-serif;background:#090b10;color:#f3f4f8}*{box-sizing:border-box}body{margin:0;background:radial-gradient(circle at 50% -20%,#4d347055,transparent 40%),#090b10}main{max-width:1100px;margin:auto;padding:48px 24px}header{display:flex;align-items:flex-start;justify-content:space-between;gap:24px}small{color:#9e8cff;letter-spacing:.12em}h1{margin:8px 0 28px;font-size:34px}.status{padding:9px 12px;border-radius:999px;font-size:12px}.status.passed{color:#75e6ad;background:#75e6ad18;border:1px solid #75e6ad55}.status.failed{color:#ff8b9a;background:#ff6b7c18;border:1px solid #ff6b7c55}section{margin:16px 0;padding:20px;border:1px solid #282d39;border-radius:14px;background:#11151d}h2{margin:0;font-size:17px}section p{color:#8e96a8;font-size:12px}table{width:100%;border-collapse:collapse;font-size:12px}th,td{padding:11px 9px;text-align:left;border-top:1px solid #282d39}th{color:#8e96a8}.regressed td:last-child{color:#ff7c8d}.improved td:last-child{color:#72dda7}.stable td:last-child{color:#a8afbd}footer{margin-top:24px;color:#737b8b;font-size:11px;text-align:center}@media(max-width:700px){header{display:block}.status{display:inline-block;margin-bottom:18px}section{overflow:auto}h1{font-size:27px}}
";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AnalysisVerdict, CompatibilityReport, EvidenceBundle};
    use serde_json::json;

    fn analysis(verdict: AnalysisVerdict) -> AnalysisReport {
        AnalysisReport {
            schema_version: 1,
            verdict,
            compatibility: CompatibilityReport {
                compatible: verdict != AnalysisVerdict::Incompatible,
                reasons: if verdict == AnalysisVerdict::Incompatible {
                    vec!["Flow mismatch".to_owned()]
                } else {
                    vec![]
                },
                warnings: vec![],
            },
            metrics: vec![],
            findings: vec![],
            evidence: EvidenceBundle {
                schema_version: 1,
                baseline_run_id: "baseline".to_owned(),
                current_run_id: "current".to_owned(),
                flow_hash: "hash".to_owned(),
                framework: "react-native".to_owned(),
                platform: "android".to_owned(),
                scenario: "list".to_owned(),
                device_class: "emulator".to_owned(),
                metric_definitions: vec![],
                raw_evidence: vec![],
                normalized_facts: json!({}),
            },
        }
    }

    #[test]
    fn ci_exit_codes_are_stable() {
        assert_eq!(
            CiReport::new(vec![analysis(AnalysisVerdict::Stable)], None).exit_code,
            0
        );
        assert_eq!(
            CiReport::new(vec![analysis(AnalysisVerdict::Regressed)], None).exit_code,
            2
        );
        assert_eq!(
            CiReport::new(vec![analysis(AnalysisVerdict::Incompatible)], None).exit_code,
            3
        );
    }

    #[test]
    fn junit_and_html_are_static_and_machine_readable() {
        let report = CiReport::new(vec![analysis(AnalysisVerdict::Incompatible)], None);
        let junit = render_ci_junit(&report);
        let html = render_ci_html(&report);
        assert!(junit.contains("errors=\"1\""));
        assert!(junit.contains("Flow mismatch"));
        assert!(html.contains("EXIT 3"));
        assert!(html.contains("Reactor CI Report"));
    }
}
