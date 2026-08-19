# Reactor architecture

Reactor is the product-facing tool. Maestro and native profilers are replaceable implementation adapters, not user-facing commands.

## Layers

1. **Scenario protocol** — framework-neutral JSON describing workload, duration, semantic entry points, and completion markers.
2. **Automation engine** — compiles the protocol to an executable flow. The first engine targets Maestro.
3. **Platform collector** — Android uses Flashlight/Perfetto-backed data; iOS uses xctrace-derived data.
4. **Normalizer** — converts collector-specific records into the versioned Reactor result schema.
5. **Reporter** — compares only compatible platform, device, scenario, and metric definitions.

## Maestro ownership boundary

Reactor owns:

- the scenario DSL and semantic IDs;
- flow generation and measured-window boundaries;
- framework/app selection, iteration order, retries, and artifact layout;
- metric normalization, validation, and reports.

Maestro owns:

- Android/iOS UI discovery and device interaction;
- its platform drivers and low-level command execution.

The default engine is a checksum-verified, project-local Maestro release. `maestroOverridePath` can point to the executable produced by a local fork. This makes a driver fork immediately usable without changing scenario, collector, or report code.

Source vendoring is intentionally deferred until a concrete upstream driver limitation is confirmed. If that occurs, create a fork pinned to `maestroSourceRef`, keep the change minimal, and set `maestroOverridePath` to the fork's built executable.
