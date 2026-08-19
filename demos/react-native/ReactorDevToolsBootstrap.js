'use strict';

const {initialize} = require('react-devtools-core');

initialize(null, true, {
  recordChangeDescriptions: true,
  recordTimeline: true,
});

const hook = global.__REACT_DEVTOOLS_GLOBAL_HOOK__;
const originalCommit = hook && hook.onCommitFiberRoot;
let commitNumber = 0;
let profileTimer;
const profileNamesById = new Map();
const profilePathsById = new Map();

function componentName(fiber) {
  const type = fiber && (fiber.elementType || fiber.type);
  if (typeof type === 'function') return type.displayName || type.name || 'Anonymous';
  if (type && typeof type === 'object') {
    const nested = type.type || type.render;
    return type.displayName || (nested && (nested.displayName || nested.name)) || undefined;
  }
  return undefined;
}

function captureTree(root) {
  try {
    const nodes = [];
    const seen = new Set();
    const stack = [{fiber: root && root.current, parentId: null, depth: 0}];
    while (stack.length && nodes.length < 300) {
      const current = stack.pop();
      const fiber = current && current.fiber;
      if (!fiber || seen.has(fiber)) continue;
      seen.add(fiber);
      const name = componentName(fiber);
      const id = name ? `${commitNumber}:${nodes.length + 1}` : current.parentId;
      if (name) {
        nodes.push({id, name, parentId: current.parentId, depth: current.depth});
      }
      if (fiber.sibling) stack.push({fiber: fiber.sibling, parentId: current.parentId, depth: current.depth});
      if (fiber.child) stack.push({fiber: fiber.child, parentId: id, depth: name ? current.depth + 1 : current.depth});
    }
    const {NativeModules} = require('react-native');
    NativeModules.ReactorDiagnostics?.appendEvent?.('component_tree', JSON.stringify({
      commit: commitNumber,
      nodeCount: nodes.length,
      truncated: stack.length > 0,
      nodes,
    }));
  } catch {
    // Diagnostics are observational and must not affect application behavior.
  }
}

function exportProfile() {
  try {
    const dataForRoots = [];
    for (const renderer of hook.rendererInterfaces.values()) {
      const data = renderer.getProfilingData();
      for (const root of data.dataForRoots || []) {
        const ids = new Set((root.initialTreeBaseDurations || []).map(entry => entry[0]));
        for (const commit of root.commitData || []) {
          for (const entry of commit.fiberActualDurations || []) ids.add(entry[0]);
        }
        const descriptors = Array.from(ids).map(id => {
          const currentName = renderer.getDisplayNameForElementID(id);
          const currentPath = renderer.getPathForElement(id);
          if (currentName) profileNamesById.set(id, currentName);
          if (currentPath) profilePathsById.set(id, currentPath);
          return {
            id,
            displayName: profileNamesById.get(id) || `Component #${id}`,
            path: profilePathsById.get(id) || [],
          };
        });
        const idsByPath = new Map(descriptors.map(item => [JSON.stringify(item.path), item.id]));
        const children = new Map(descriptors.map(item => [item.id, []]));
        for (const descriptor of descriptors) {
          const parentId = idsByPath.get(JSON.stringify(descriptor.path.slice(0, -1)));
          if (parentId != null) children.get(parentId)?.push(descriptor.id);
        }
        dataForRoots.push({
          ...root,
          snapshots: descriptors.map(item => ({
            id: item.id,
            displayName: item.displayName,
            children: children.get(item.id) || [],
          })),
        });
      }
    }
    if (!dataForRoots.length) return;
    const {NativeModules} = require('react-native');
    NativeModules.ReactorDiagnostics?.writeProfile?.(JSON.stringify({
      version: 5,
      source: 'react-devtools-core-6.1.5',
      dataForRoots,
    }));
  } catch {
    // A partial renderer must not affect the benchmark.
  }
}

function scheduleProfileExport() {
  clearTimeout(profileTimer);
  profileTimer = setTimeout(exportProfile, 120);
}

if (hook && typeof originalCommit === 'function') {
  hook.onCommitFiberRoot = function reactorCommit(rendererId, root, priorityLevel) {
    const result = originalCommit.call(hook, rendererId, root, priorityLevel);
    commitNumber += 1;
    queueMicrotask(() => captureTree(root));
    scheduleProfileExport();
    return result;
  };
}
