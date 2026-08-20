import type {
  DiagnosticManifest,
  TimelineItem,
  TimelineRange,
  TimelineTrackAvailability,
  TimelineTrackKind,
} from "./types.ts";

export const TIMELINE_TRACKS: readonly TimelineTrackKind[] = [
  "iterations",
  "frames",
  "react_commits",
  "js_samples",
  "runtime_events",
];

export const TIMELINE_TRACK_LABELS: Record<TimelineTrackKind, string> = {
  iterations: "迭代",
  frames: "帧",
  react_commits: "React Commit",
  js_samples: "JS 采样",
  runtime_events: "运行时事件",
};

export const MIN_VIEWPORT_MS = 1;

export function normalizeTimelineRange(range: TimelineRange, bounds?: TimelineRange, minimumDuration = MIN_VIEWPORT_MS): TimelineRange {
  let startMs = Number.isFinite(range.startMs) ? range.startMs : bounds?.startMs ?? 0;
  let endMs = Number.isFinite(range.endMs) ? range.endMs : bounds?.endMs ?? startMs + minimumDuration;
  if (startMs > endMs) [startMs, endMs] = [endMs, startMs];
  const minDuration = Math.max(0.001, minimumDuration);
  if (endMs - startMs < minDuration) {
    const center = (startMs + endMs) / 2;
    startMs = center - minDuration / 2;
    endMs = center + minDuration / 2;
  }
  if (!bounds) return { startMs, endMs };
  const lower = Math.min(bounds.startMs, bounds.endMs);
  const upper = Math.max(bounds.startMs, bounds.endMs);
  const available = Math.max(0, upper - lower);
  if (available <= minDuration) return { startMs: lower, endMs: upper };
  const duration = Math.min(endMs - startMs, available);
  startMs = Math.max(lower, Math.min(startMs, upper - duration));
  return { startMs, endMs: startMs + duration };
}

export function zoomTimelineRange(viewport: TimelineRange, factor: number, anchorMs: number, bounds: TimelineRange): TimelineRange {
  const duration = Math.max(MIN_VIEWPORT_MS, viewport.endMs - viewport.startMs);
  const nextDuration = Math.max(MIN_VIEWPORT_MS, duration * factor);
  const ratio = Math.max(0, Math.min(1, (anchorMs - viewport.startMs) / duration));
  return normalizeTimelineRange({
    startMs: anchorMs - nextDuration * ratio,
    endMs: anchorMs + nextDuration * (1 - ratio),
  }, bounds);
}

export function panTimelineRange(viewport: TimelineRange, deltaMs: number, bounds: TimelineRange): TimelineRange {
  return normalizeTimelineRange({ startMs: viewport.startMs + deltaMs, endMs: viewport.endMs + deltaMs }, bounds);
}

export function timeAtCanvasX(x: number, width: number, viewport: TimelineRange): number {
  if (width <= 0) return viewport.startMs;
  const ratio = Math.max(0, Math.min(1, x / width));
  return viewport.startMs + ratio * (viewport.endMs - viewport.startMs);
}

export function canvasXAtTime(timeMs: number, width: number, viewport: TimelineRange): number {
  const duration = viewport.endMs - viewport.startMs;
  if (duration <= 0 || width <= 0) return 0;
  return (timeMs - viewport.startMs) / duration * width;
}

export function itemIntersectsRange(item: Pick<TimelineItem, "startMs" | "endMs">, range: TimelineRange): boolean {
  return item.endMs >= range.startMs && item.startMs <= range.endMs;
}

export function trackAvailability(manifest: DiagnosticManifest, kind: TimelineTrackKind): TimelineTrackAvailability {
  return manifest.tracks.find((track) => track.kind === kind) ?? {
    kind,
    state: "not_collected",
    reason: "清单未声明此轨道的采集结果",
  };
}

export function availabilityLabel(track: TimelineTrackAvailability): string {
  if (track.state === "available") return track.count === undefined ? "可用" : `${track.count} 项`;
  if (track.state === "unsupported") return `不支持${track.reason ? `：${track.reason}` : ""}`;
  if (track.state === "failed") return `采集失败${track.reason ? `：${track.reason}` : ""}`;
  if (track.state === "unavailable") return `不可用${track.reason ? `：${track.reason}` : ""}`;
  return `未采集${track.reason ? `：${track.reason}` : ""}`;
}

export function formatTimelineTime(valueMs: number): string {
  if (Math.abs(valueMs) >= 60_000) {
    const minutes = Math.floor(valueMs / 60_000);
    return `${minutes}:${((valueMs % 60_000) / 1000).toFixed(1).padStart(4, "0")}`;
  }
  if (Math.abs(valueMs) >= 1000) return `${(valueMs / 1000).toFixed(2)} s`;
  return `${valueMs.toFixed(valueMs >= 100 ? 0 : 1)} ms`;
}

export function timelineItemLabel(item: TimelineItem): string {
  const duration = item.durationMs ?? Math.max(0, item.endMs - item.startMs);
  return `${TIMELINE_TRACK_LABELS[item.track]} · ${item.label} · ${formatTimelineTime(item.startMs)} · ${formatTimelineTime(duration)}`;
}
