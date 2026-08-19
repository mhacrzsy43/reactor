#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]

use reactor_protocol::{IterationMetrics, MetricSummary};

#[must_use]
pub fn mean(values: &[f64]) -> Option<f64> {
    (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
}

#[must_use]
pub fn percentile(values: &[f64], quantile: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let position = (sorted.len() - 1) as f64 * quantile.clamp(0.0, 1.0);
    let lower = position.floor() as usize;
    let upper = position.ceil() as usize;
    if lower == upper {
        Some(sorted[lower])
    } else {
        Some(sorted[lower] + (sorted[upper] - sorted[lower]) * (position - lower as f64))
    }
}

#[must_use]
pub fn aggregate_iterations(iterations: &[IterationMetrics]) -> MetricSummary {
    let successful: Vec<_> = iterations
        .iter()
        .filter(|item| matches!(item.status.as_str(), "SUCCESS" | "UNKNOWN"))
        .collect();
    let metric = |get: fn(&IterationMetrics) -> Option<f64>| {
        successful
            .iter()
            .filter_map(|item| get(item))
            .collect::<Vec<_>>()
    };
    MetricSummary {
        iteration_count: iterations.len() as u64,
        successful_iteration_count: successful.len() as u64,
        fps_mean: mean(&metric(|item| item.fps_mean)),
        fps_p10: mean(&metric(|item| item.fps_p10)),
        low_fps_sample_pct: mean(&metric(|item| item.low_fps_sample_pct)),
        ram_mean_mb: mean(&metric(|item| item.ram_mean_mb)),
        ram_peak_mb: percentile(&metric(|item| item.ram_peak_mb), 0.95),
        cpu_mean_pct: mean(&metric(|item| item.cpu_mean_pct)),
        ui_cpu_mean_pct: mean(&metric(|item| item.ui_cpu_mean_pct)),
        js_cpu_mean_pct: mean(&metric(|item| item.js_cpu_mean_pct)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolates_percentile() {
        assert_eq!(percentile(&[10.0, 20.0, 30.0], 0.25), Some(15.0));
    }
}
