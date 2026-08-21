// React Native's Gradle bundle task passes its CLI entry point as argv[2].
// A child process is intentional: Metro initializes resolver workers before
// processing the CLI command, so setting an environment variable in-process
// is too late for their module resolution.
const {spawnSync} = require('child_process');

const [entryPoint, ...args] = process.argv.slice(2);
if (!entryPoint) throw new Error('Reactor diagnostic bundle requires a React Native CLI entry point');

const result = spawnSync(process.execPath, [entryPoint, ...args], {
  env: {...process.env, REACTOR_RN_PROFILE: '1'},
  stdio: 'inherit',
});

if (result.error) throw result.error;
process.exitCode = result.status == null ? 1 : result.status;
