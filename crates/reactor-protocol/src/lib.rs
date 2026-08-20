//! Versioned, transport-neutral contracts shared by every Reactor frontend and runner.

mod flow;
mod result;

pub use flow::{
    Coordinate, ExpandedFlowStep, Flow, FlowLock, FlowTrialEvidence, FlowValidationError,
    FlowValidationReport, FlowWarning, GenerationProvenance, InputValue, Platform,
    PromptInputReference, SecretInputReference, Selector, Step, SwipeDirection, TotpInputReference,
    TrialMode, VariableInputReference, canonical_flow_hash, navigation_destination_marker,
    requires_navigation_intent, validate_flow,
};
pub use result::{
    AndroidMemoryCheckpoint, AndroidMemoryLeakReport, AndroidNativeMetrics, ArtifactIntegrity,
    ArtifactRef, ArtifactTimeRangeV1, BuildIdentityV1, CollectorDiagnosticV1, CollectorStatus,
    DeviceMetadata, DiagnosticCaptureMode, DiagnosticCollectorPlanV1, DiagnosticPlanV1,
    DiagnosticResourceLimitsV1, FrameworkDiagnosticsV1, IosMetricAvailability, IosNativeMetrics,
    IterationMetrics, MetricSummary, NormalizedResult, ReactNativeDiagnosticEvent,
    ReactNativeDiagnosticsSummary, ReactNativeDiagnosticsView, ReactNativeFrameworkDiagnosticsV1,
    ResultSource, RunMode,
};
