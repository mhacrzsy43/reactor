# React Native iOS compatibility

The bridge contract uses protocol version 1 for all supported React Native versions. Capability names and `availability` are authoritative: callers must not infer that Hermes data exists merely because Hermes is enabled.

| React Native | Architecture baseline | DevTools profile classification | iOS runtime events and sandbox paths | Public JSI heap access from bridge | Public Hermes CPU sampling from bridge |
| --- | --- | --- | --- | --- | --- |
| 0.83 | New Architecture | DevTools 6 | Contract-compatible; integration compile not verified here | Unavailable unless the host supplies a supported runtime executor | Unavailable |
| 0.84 | New Architecture | DevTools 6 | Contract-compatible; integration compile not verified here | Unavailable unless the host supplies a supported runtime executor | Unavailable |
| 0.85 | New Architecture | DevTools 6 | Contract-compatible; integration compile not verified here | Unavailable unless the host supplies a supported runtime executor | Unavailable |
| 0.86 | New Architecture | DevTools 6 | Contract-compatible; integration compile not verified here | Unavailable unless the host supplies a supported runtime executor | Unavailable |
| 0.87 | New Architecture | DevTools 6 | Compiled by the Reactor RN 0.87 demo target | Unavailable in the demo legacy native module | Unavailable in the demo native module |

## Contract rules

- `protocolVersion`, `sdkVersion`, `diagnosticBuild`, `capabilities`, `sandboxPaths`, and `availability` form the native capability handshake.
- Sandbox paths are absolute app-container paths. They are evidence locations, not promises that every optional artifact exists.
- `react-profiler`, `runtime-events`, and `react-devtools-profile` describe supported capture paths.
- `hermes-heap-unavailable` and `hermes-cpu-unavailable`, plus structured `availability` reasons, explicitly classify unsupported runtime access.
- The iOS demo does not call private React Native or Hermes APIs. It does not synthesize profiler or heap data when runtime support is unavailable.
- DevTools 6/7 classification describes the profile producer generation. RN 0.83-0.87 currently declare `react-devtools-core` 6.x; classify a future or host-supplied 7.x producer from its actual package/profile metadata rather than RN version. Consumers should continue to inspect profile schema/version fields rather than relying only on this label.
