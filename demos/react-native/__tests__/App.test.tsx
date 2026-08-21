/**
 * @format
 */

import React from 'react';
import {Text, TextInput} from 'react-native';
import ReactTestRenderer from 'react-test-renderer';

jest.mock('react-native-safe-area-context', () => {
  const ReactModule = require('react');
  return {
    SafeAreaProvider: ({children}: {children: React.ReactNode}) =>
      ReactModule.createElement(ReactModule.Fragment, null, children),
    useSafeAreaInsets: () => ({top: 0, right: 0, bottom: 0, left: 0}),
  };
});

import App from '../App';

function textContent(value: unknown): string {
  if (Array.isArray(value)) {
    return value.map(textContent).join('');
  }
  return value === null || value === undefined ? '' : String(value);
}

function buttonByLabel(renderer: ReactTestRenderer.ReactTestRenderer, label: string) {
  return renderer.root.findAll(
    node => node.props.accessibilityRole === 'button' && typeof node.props.onPress === 'function' &&
      node.findAllByType(Text).some(text => textContent(text.props.children) === label),
  ).at(-1)!;
}

test('authenticates before exposing shared workloads and memory verification', async () => {
  let renderer!: ReactTestRenderer.ReactTestRenderer;
  await ReactTestRenderer.act(() => {
    renderer = ReactTestRenderer.create(<App />);
  });

  const inputs = renderer.root.findAllByType(TextInput);
  await ReactTestRenderer.act(() => {
    inputs[0].props.onChangeText('test');
    inputs[1].props.onChangeText('test');
  });
  const signInSubmit = renderer.root.findAll(
    node => node.props.testID === 'auth-submit-signin' && typeof node.props.onPress === 'function',
  ).at(-1)!;
  expect(signInSubmit.props.accessibilityRole).toBe('button');
  await ReactTestRenderer.act(() => signInSubmit.props.onPress());

  const home = JSON.stringify(renderer.toJSON());
  expect(home).toContain('Reactor ready');
  expect(home).toContain('List scenario');
  expect(home).toContain('Update scenario');
  expect(home).toContain('Animation scenario');
  expect(home).toContain('Memory scenario');

  await ReactTestRenderer.act(() => buttonByLabel(renderer, 'Memory scenario').props.onPress());

  const labels = renderer.root
    .findAllByType(Text)
    .map(node => textContent(node.props.children));
  expect(labels).toContain('Memory ready');
  expect(labels).toContain('Memory cycle 0 complete');
  await ReactTestRenderer.act(() => buttonByLabel(renderer, 'Run memory cycle').props.onPress());
  expect(renderer.root.findAllByType(Text).map(node => textContent(node.props.children))).toContain('Memory cycle 1 complete');
});
