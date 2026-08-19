/**
 * @format
 */

// Profiling builds resolve this module to the DevTools hook implementation before React Native
// loads its renderer. Normal benchmark builds resolve a zero-cost stub.
require('./ReactorDevToolsBootstrap');

const {AppRegistry} = require('react-native');
const App = require('./App').default;
const appName = require('./app.json').name;

AppRegistry.registerComponent(appName, () => App);
