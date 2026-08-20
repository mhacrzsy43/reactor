use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClockSyncPoint {
    pub source_time: f64,
    pub target_time: f64,
    /// Measurement error bound in target clock units.
    pub uncertainty: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClockMappingMethod {
    Offset,
    LinearRegression,
    PiecewiseLinear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClockQuality {
    Poor,
    Fair,
    Good,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClockSegment {
    pub source_start: f64,
    pub source_end: f64,
    pub scale: f64,
    pub offset: f64,
    pub residual_rms: f64,
    pub uncertainty: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClockMapping {
    pub source_clock: String,
    pub target_clock: String,
    pub method: ClockMappingMethod,
    pub scale: f64,
    pub offset: f64,
    pub uncertainty: f64,
    pub residual_rms: f64,
    /// Scale drift in parts per million from an ideal 1:1 clock.
    pub drift_ppm: f64,
    pub quality: ClockQuality,
    pub segments: Vec<ClockSegment>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MappedTimestamp {
    pub target_time: f64,
    pub uncertainty: f64,
    pub extrapolated: bool,
}

#[derive(Debug, Error, Clone, PartialEq)]
pub enum ClockMappingError {
    #[error("at least one clock sync point is required")]
    Empty,
    #[error("clock sync point {0} contains a non-finite or negative uncertainty value")]
    InvalidPoint(usize),
    #[error("clock sync source times must be strictly increasing")]
    NonIncreasingSource,
    #[error("linear clock fitting requires at least two distinct source times")]
    DegenerateSource,
}

impl ClockMapping {
    /// Builds an offset, least-squares linear, or piecewise-linear mapping.
    /// Piecewise mode preserves local drift and reports each interval residual.
    ///
    /// # Errors
    ///
    /// Returns an error for empty, invalid, unordered, or degenerate points.
    pub fn fit(
        source_clock: impl Into<String>,
        target_clock: impl Into<String>,
        points: &[ClockSyncPoint],
        piecewise: bool,
    ) -> Result<Self, ClockMappingError> {
        validate_points(points)?;
        if points.len() == 1 {
            let point = points[0];
            return Ok(Self {
                source_clock: source_clock.into(),
                target_clock: target_clock.into(),
                method: ClockMappingMethod::Offset,
                scale: 1.0,
                offset: point.target_time - point.source_time,
                uncertainty: point.uncertainty,
                residual_rms: 0.0,
                drift_ppm: 0.0,
                quality: quality(point.uncertainty, 0.0, 0.0),
                segments: vec![],
            });
        }

        let (scale, offset) = linear_fit(points)?;
        let residual_rms = residual_rms(points, scale, offset);
        let max_point_uncertainty = points
            .iter()
            .map(|point| point.uncertainty)
            .fold(0.0, f64::max);
        let uncertainty = max_point_uncertainty + residual_rms;
        let drift_ppm = (scale - 1.0) * 1_000_000.0;
        let segments = if piecewise {
            points
                .windows(2)
                .map(|pair| {
                    let source_span = pair[1].source_time - pair[0].source_time;
                    let segment_scale = (pair[1].target_time - pair[0].target_time) / source_span;
                    let segment_offset = pair[0].target_time - segment_scale * pair[0].source_time;
                    ClockSegment {
                        source_start: pair[0].source_time,
                        source_end: pair[1].source_time,
                        scale: segment_scale,
                        offset: segment_offset,
                        residual_rms: 0.0,
                        uncertainty: pair[0].uncertainty.max(pair[1].uncertainty),
                    }
                })
                .collect()
        } else {
            vec![]
        };
        Ok(Self {
            source_clock: source_clock.into(),
            target_clock: target_clock.into(),
            method: if piecewise {
                ClockMappingMethod::PiecewiseLinear
            } else {
                ClockMappingMethod::LinearRegression
            },
            scale,
            offset,
            uncertainty,
            residual_rms,
            drift_ppm,
            quality: quality(uncertainty, residual_rms, drift_ppm),
            segments,
        })
    }

    #[must_use]
    pub fn map(&self, source_time: f64) -> MappedTimestamp {
        if self.method != ClockMappingMethod::PiecewiseLinear || self.segments.is_empty() {
            return MappedTimestamp {
                target_time: self.scale.mul_add(source_time, self.offset),
                uncertainty: self.uncertainty,
                extrapolated: false,
            };
        }
        let first = &self.segments[0];
        let last = &self.segments[self.segments.len() - 1];
        let (segment, extrapolated, distance) = if source_time < first.source_start {
            (first, true, first.source_start - source_time)
        } else if source_time > last.source_end {
            (last, true, source_time - last.source_end)
        } else {
            (
                self.segments
                    .iter()
                    .find(|segment| source_time <= segment.source_end)
                    .unwrap_or(last),
                false,
                0.0,
            )
        };
        // Extrapolation uncertainty grows by the difference between local and
        // global scale. This prevents distant timestamps from retaining sync-point precision.
        let extrapolation_uncertainty = distance * (segment.scale - self.scale).abs();
        MappedTimestamp {
            target_time: segment.scale.mul_add(source_time, segment.offset),
            uncertainty: segment.uncertainty + segment.residual_rms + extrapolation_uncertainty,
            extrapolated,
        }
    }
}

fn validate_points(points: &[ClockSyncPoint]) -> Result<(), ClockMappingError> {
    if points.is_empty() {
        return Err(ClockMappingError::Empty);
    }
    for (index, point) in points.iter().enumerate() {
        if !point.source_time.is_finite()
            || !point.target_time.is_finite()
            || !point.uncertainty.is_finite()
            || point.uncertainty < 0.0
        {
            return Err(ClockMappingError::InvalidPoint(index));
        }
    }
    if points
        .windows(2)
        .any(|pair| pair[0].source_time >= pair[1].source_time)
    {
        return Err(ClockMappingError::NonIncreasingSource);
    }
    Ok(())
}

fn linear_fit(points: &[ClockSyncPoint]) -> Result<(f64, f64), ClockMappingError> {
    let count = usize_as_f64(points.len());
    let source_mean = points.iter().map(|point| point.source_time).sum::<f64>() / count;
    let target_mean = points.iter().map(|point| point.target_time).sum::<f64>() / count;
    let variance = points
        .iter()
        .map(|point| (point.source_time - source_mean).powi(2))
        .sum::<f64>();
    if variance <= f64::EPSILON {
        return Err(ClockMappingError::DegenerateSource);
    }
    let covariance = points
        .iter()
        .map(|point| (point.source_time - source_mean) * (point.target_time - target_mean))
        .sum::<f64>();
    let scale = covariance / variance;
    let offset = target_mean - scale * source_mean;
    Ok((scale, offset))
}

fn residual_rms(points: &[ClockSyncPoint], scale: f64, offset: f64) -> f64 {
    let sum = points
        .iter()
        .map(|point| (scale.mul_add(point.source_time, offset) - point.target_time).powi(2))
        .sum::<f64>();
    (sum / usize_as_f64(points.len())).sqrt()
}

fn quality(uncertainty: f64, residual_rms: f64, drift_ppm: f64) -> ClockQuality {
    if uncertainty <= 1.0 && residual_rms <= 0.5 && drift_ppm.abs() <= 1_000.0 {
        ClockQuality::Good
    } else if uncertainty <= 5.0 && residual_rms <= 2.0 && drift_ppm.abs() <= 5_000.0 {
        ClockQuality::Fair
    } else {
        ClockQuality::Poor
    }
}

#[allow(clippy::cast_precision_loss)]
fn usize_as_f64(value: usize) -> f64 {
    value as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_mapping_recovers_offset_and_drift() {
        let points = [
            ClockSyncPoint {
                source_time: 0.0,
                target_time: 100.0,
                uncertainty: 0.1,
            },
            ClockSyncPoint {
                source_time: 1_000.0,
                target_time: 1_101.0,
                uncertainty: 0.1,
            },
            ClockSyncPoint {
                source_time: 2_000.0,
                target_time: 2_102.0,
                uncertainty: 0.1,
            },
        ];
        let mapping = ClockMapping::fit("js", "boottime", &points, false).unwrap();
        assert_eq!(mapping.method, ClockMappingMethod::LinearRegression);
        assert!((mapping.scale - 1.001).abs() < 1.0e-12);
        assert!((mapping.offset - 100.0).abs() < 1.0e-9);
        assert!((mapping.map(1_500.0).target_time - 1_601.5).abs() < 1.0e-9);
        assert_eq!(mapping.quality, ClockQuality::Good);
    }

    #[test]
    fn piecewise_mapping_tracks_clock_change_and_extrapolation_uncertainty() {
        let points = [
            ClockSyncPoint {
                source_time: 0.0,
                target_time: 10.0,
                uncertainty: 0.2,
            },
            ClockSyncPoint {
                source_time: 100.0,
                target_time: 110.0,
                uncertainty: 0.2,
            },
            ClockSyncPoint {
                source_time: 200.0,
                target_time: 212.0,
                uncertainty: 0.4,
            },
        ];
        let mapping = ClockMapping::fit("wall", "boottime", &points, true).unwrap();
        assert_eq!(mapping.method, ClockMappingMethod::PiecewiseLinear);
        assert_eq!(mapping.segments.len(), 2);
        assert!((mapping.map(150.0).target_time - 161.0).abs() < 1.0e-9);
        let extrapolated = mapping.map(300.0);
        assert!(extrapolated.extrapolated);
        assert!(extrapolated.uncertainty > 0.4);
    }

    #[test]
    fn invalid_sync_sequences_are_rejected() {
        let points = [
            ClockSyncPoint {
                source_time: 1.0,
                target_time: 1.0,
                uncertainty: 0.0,
            },
            ClockSyncPoint {
                source_time: 1.0,
                target_time: 2.0,
                uncertainty: 0.0,
            },
        ];
        assert_eq!(
            ClockMapping::fit("a", "b", &points, false),
            Err(ClockMappingError::NonIncreasingSource)
        );
    }
}
