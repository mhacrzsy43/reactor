const { getDefaultConfig, mergeConfig } = require('@react-native/metro-config');
const path = require('path');

/**
 * Metro configuration
 * https://reactnative.dev/docs/metro
 *
 * @type {import('@react-native/metro-config').MetroConfig}
 */
const config = {
  resolver: {
    resolveRequest(context, moduleName, platform) {
      if (moduleName === './ReactorBenchmarkMode') {
        const fault = process.env.REACTOR_DEMO_FAULT;
        const variant = fault === 'memory' || fault === 'render' ? fault : 'normal';
        return {
          type: 'sourceFile',
          filePath: path.resolve(__dirname, `ReactorBenchmarkMode.${variant}.js`),
        };
      }
      if (
        moduleName === './ReactorDevToolsBootstrap' &&
        context.originModulePath === path.resolve(__dirname, 'index.js')
      ) {
        return {
          type: 'sourceFile',
          filePath: path.resolve(
            __dirname,
            process.env.REACTOR_RN_PROFILE === '1'
              ? 'ReactorDevToolsBootstrap.js'
              : 'ReactorDevToolsBootstrap.empty.js',
          ),
        };
      }
      if (
        process.env.REACTOR_RN_PROFILE === '1' &&
        context.originModulePath.endsWith(path.join('Libraries', 'Renderer', 'shims', 'ReactFabric.js')) &&
        moduleName === '../implementations/ReactFabric-prod'
      ) {
        return {
          type: 'sourceFile',
          filePath: path.resolve(__dirname, 'node_modules/react-native/Libraries/Renderer/implementations/ReactFabric-profiling.js'),
        };
      }
      return context.resolveRequest(context, moduleName, platform);
    },
  },
};

module.exports = mergeConfig(getDefaultConfig(__dirname), config);
