// Non-Metro consumers such as Jest and TypeScript use the remediated mode by default.
// Metro replaces this module with the selected deterministic fault variant at bundle time.
export const BENCHMARK_MODE = 'normal';
export const RETAIN_MEMORY_CYCLES = false;
export const DUPLICATE_RENDER_CYCLES = false;
