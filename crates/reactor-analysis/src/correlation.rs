use serde::{Deserialize, Serialize};

use crate::ClockQuality;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimeRange {
    pub start: f64,
    pub end: f64,
}

impl TimeRange {
    #[must_use]
    pub fn normalized(self) -> Self {
        if self.start <= self.end {
            self
        } else {
            Self {
                start: self.end,
                end: self.start,
            }
        }
    }

    #[must_use]
    pub fn duration(self) -> f64 {
        let range = self.normalized();
        range.end - range.start
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CorrelationRelation {
    Overlaps,
    AdjacentBefore,
    AdjacentAfter,
    Contains,
    ContainedBy,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CorrelationConfidence {
    Unavailable,
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticCorrelation {
    pub left_id: String,
    pub right_id: String,
    pub relation: CorrelationRelation,
    pub overlap: f64,
    pub overlap_ratio: f64,
    pub gap: f64,
    pub clock_uncertainty: f64,
    pub confidence: CorrelationConfidence,
    pub reasons: Vec<String>,
}

/// Correlates two intervals after explicit clock mapping. The result is a
/// temporal candidate only; it never asserts causality.
#[must_use]
pub fn correlate_intervals(
    left_id: impl Into<String>,
    left: TimeRange,
    right_id: impl Into<String>,
    right: TimeRange,
    clock_uncertainty: f64,
    clock_quality: Option<ClockQuality>,
    adjacency_limit: f64,
) -> DiagnosticCorrelation {
    let left_id = left_id.into();
    let right_id = right_id.into();
    let left = left.normalized();
    let right = right.normalized();
    if !valid_range(left)
        || !valid_range(right)
        || !clock_uncertainty.is_finite()
        || clock_uncertainty < 0.0
        || clock_quality.is_none()
    {
        return DiagnosticCorrelation {
            left_id,
            right_id,
            relation: CorrelationRelation::Unavailable,
            overlap: 0.0,
            overlap_ratio: 0.0,
            gap: 0.0,
            clock_uncertainty: clock_uncertainty.max(0.0),
            confidence: CorrelationConfidence::Unavailable,
            reasons: vec!["缺少有效时间范围或显式时钟映射".to_owned()],
        };
    }

    let overlap = (left.end.min(right.end) - left.start.max(right.start)).max(0.0);
    let shorter_duration = left.duration().min(right.duration());
    let overlap_ratio = if shorter_duration > 0.0 {
        (overlap / shorter_duration).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (relation, gap) = if overlap > 0.0 {
        let relation = if left.start <= right.start && left.end >= right.end {
            CorrelationRelation::Contains
        } else if right.start <= left.start && right.end >= left.end {
            CorrelationRelation::ContainedBy
        } else {
            CorrelationRelation::Overlaps
        };
        (relation, 0.0)
    } else if left.end <= right.start {
        (CorrelationRelation::AdjacentBefore, right.start - left.end)
    } else {
        (CorrelationRelation::AdjacentAfter, left.start - right.end)
    };

    let mut reasons = vec![if overlap > 0.0 {
        format!("时间窗口重叠 {:.1}%", overlap_ratio * 100.0)
    } else {
        format!("时间窗口相距 {gap:.3}")
    }];
    let quality = clock_quality.unwrap_or(ClockQuality::Poor);
    reasons.push(format!(
        "时钟质量为 {quality:?}，不确定度为 {clock_uncertainty:.3}"
    ));
    let confidence = confidence(
        overlap_ratio,
        gap,
        clock_uncertainty,
        quality,
        adjacency_limit.max(0.0),
    );
    if confidence == CorrelationConfidence::Low && overlap == 0.0 && gap > adjacency_limit {
        reasons.push("间隔超过相邻候选窗口".to_owned());
    }
    reasons.push("仅表示时间相关候选，不构成因果证明".to_owned());

    DiagnosticCorrelation {
        left_id,
        right_id,
        relation,
        overlap,
        overlap_ratio,
        gap,
        clock_uncertainty,
        confidence,
        reasons,
    }
}

fn confidence(
    overlap_ratio: f64,
    gap: f64,
    uncertainty: f64,
    quality: ClockQuality,
    adjacency_limit: f64,
) -> CorrelationConfidence {
    if quality == ClockQuality::Good && overlap_ratio >= 0.5 && uncertainty <= 1.0 {
        CorrelationConfidence::High
    } else if quality >= ClockQuality::Fair
        && (overlap_ratio > 0.0 || gap <= adjacency_limit)
        && uncertainty <= 5.0
    {
        CorrelationConfidence::Medium
    } else {
        CorrelationConfidence::Low
    }
}

fn valid_range(range: TimeRange) -> bool {
    range.start.is_finite() && range.end.is_finite() && range.start <= range.end
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strong_overlap_with_good_clock_is_high_confidence() {
        let correlation = correlate_intervals(
            "frame-1",
            TimeRange {
                start: 100.0,
                end: 120.0,
            },
            "commit-1",
            TimeRange {
                start: 105.0,
                end: 115.0,
            },
            0.2,
            Some(ClockQuality::Good),
            5.0,
        );
        assert_eq!(correlation.relation, CorrelationRelation::Contains);
        assert!((correlation.overlap_ratio - 1.0).abs() < f64::EPSILON);
        assert_eq!(correlation.confidence, CorrelationConfidence::High);
        assert!(
            correlation
                .reasons
                .iter()
                .any(|reason| reason.contains("不构成因果"))
        );
    }

    #[test]
    fn poor_clock_caps_confidence_and_missing_mapping_is_unavailable() {
        let low = correlate_intervals(
            "frame",
            TimeRange {
                start: 0.0,
                end: 10.0,
            },
            "sample",
            TimeRange {
                start: 0.0,
                end: 10.0,
            },
            10.0,
            Some(ClockQuality::Poor),
            2.0,
        );
        assert_eq!(low.confidence, CorrelationConfidence::Low);

        let unavailable = correlate_intervals(
            "frame",
            TimeRange {
                start: 0.0,
                end: 10.0,
            },
            "commit",
            TimeRange {
                start: 20.0,
                end: 30.0,
            },
            0.0,
            None,
            2.0,
        );
        assert_eq!(unavailable.confidence, CorrelationConfidence::Unavailable);
        assert_eq!(unavailable.relation, CorrelationRelation::Unavailable);
    }
}
