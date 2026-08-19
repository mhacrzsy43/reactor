//! Versioned, transport-neutral contracts shared by every Reactor frontend and runner.

mod flow;
mod result;

pub use flow::{
    Coordinate, Flow, FlowLock, FlowTrialEvidence, FlowValidationError, FlowValidationReport,
    FlowWarning, GenerationProvenance, Platform, Selector, Step, SwipeDirection, TrialMode,
    canonical_flow_hash, navigation_destination_marker, requires_navigation_intent, validate_flow,
};
pub use result::{
    AndroidNativeMetrics, DeviceMetadata, IosMetricAvailability, IosNativeMetrics,
    IterationMetrics, MetricSummary, NormalizedResult, ResultSource,
};
