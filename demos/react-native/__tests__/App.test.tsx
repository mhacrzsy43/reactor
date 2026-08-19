/**
 * @format
 */

import React from 'react';
import {Text} from 'react-native';
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

test('exposes the shared ready markers and list workload', async () => {
  let renderer!: ReactTestRenderer.ReactTestRenderer;
  await ReactTestRenderer.act(() => {
    renderer = ReactTestRenderer.create(<App />);
  });

  const home = JSON.stringify(renderer.toJSON());
  expect(home).toContain('Reactor ready');
  expect(home).toContain('List scenario');
  expect(home).toContain('Update scenario');
  expect(home).toContain('Animation scenario');

  const firstButton = renderer.root.findAll(
    node => node.props.accessibilityRole === 'button',
  )[0];
  await ReactTestRenderer.act(() => firstButton.props.onPress());

  const labels = renderer.root
    .findAllByType(Text)
    .map(node => textContent(node.props.children));
  expect(labels).toContain('List ready');
  expect(labels).toContain('Item 0');
  expect(labels).toContain('Deterministic value 818');
});
