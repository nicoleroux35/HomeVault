'use strict';

// Local hardened fork of deepmerge 4.3.1 (MIT).
//
// GHSA-gcqx-w48m-w2h4 / CVE-2026-93753: merging an object whose own keys
// include `__proto__`, `constructor` or `prototype` let a source object mutate
// the returned object's prototype. No upstream release fixes it yet, so this
// fork adds the key filter and reports 4.3.2 so dependency tooling sees a
// version outside the vulnerable range.
//
// The public API is unchanged: deepmerge(target, source, options), with
// `arrayMerge`, `customMerge`, `clone` and `isMergeableObject`.

var UNSAFE_KEYS = new Set(['__proto__', 'constructor', 'prototype']);

function defaultIsMergeableObject(value) {
  if (value === null || typeof value !== 'object') return false;
  if (Array.isArray(value)) return true;
  var proto = Object.getPrototypeOf(value);
  return proto === Object.prototype || proto === null;
}

function emptyTarget(value) {
  return Array.isArray(value) ? [] : {};
}

function cloneUnlessOtherwiseSpecified(value, options) {
  return options.clone !== false && options.isMergeableObject(value)
    ? deepmerge(emptyTarget(value), value, options)
    : value;
}

function defaultArrayMerge(target, source, options) {
  return target.concat(source).map(function (element) {
    return cloneUnlessOtherwiseSpecified(element, options);
  });
}

function getEnumerableOwnPropertySymbols(target) {
  return Object.getOwnPropertySymbols
    ? Object.getOwnPropertySymbols(target).filter(function (symbol) {
        return Object.propertyIsEnumerable.call(target, symbol);
      })
    : [];
}

function getKeys(target) {
  return Object.keys(target).concat(getEnumerableOwnPropertySymbols(target));
}

function propertyIsOnObject(object, property) {
  try {
    return property in object;
  } catch (_) {
    return false;
  }
}

function propertyIsUnsafe(target, key) {
  return (
    propertyIsOnObject(target, key) &&
    !(
      Object.prototype.hasOwnProperty.call(target, key) &&
      Object.prototype.propertyIsEnumerable.call(target, key)
    )
  );
}

function getMergeFunction(key, options) {
  if (!options.customMerge) return deepmerge;
  var customMerge = options.customMerge(key);
  return typeof customMerge === 'function' ? customMerge : deepmerge;
}

function mergeObject(target, source, options) {
  var destination = {};
  if (options.isMergeableObject(target)) {
    getKeys(target).forEach(function (key) {
      if (typeof key === 'string' && UNSAFE_KEYS.has(key)) return;
      destination[key] = cloneUnlessOtherwiseSpecified(target[key], options);
    });
  }
  getKeys(source).forEach(function (key) {
    if (typeof key === 'string' && UNSAFE_KEYS.has(key)) return;
    if (propertyIsUnsafe(target, key)) return;

    if (propertyIsOnObject(target, key) && options.isMergeableObject(source[key])) {
      destination[key] = getMergeFunction(key, options)(target[key], source[key], options);
    } else {
      destination[key] = cloneUnlessOtherwiseSpecified(source[key], options);
    }
  });
  return destination;
}

function deepmerge(target, source, options) {
  options = options || {};
  options.arrayMerge = options.arrayMerge || defaultArrayMerge;
  options.isMergeableObject = options.isMergeableObject || defaultIsMergeableObject;

  var sourceIsArray = Array.isArray(source);
  var targetIsArray = Array.isArray(target);
  if (sourceIsArray !== targetIsArray) {
    return cloneUnlessOtherwiseSpecified(source, options);
  }
  if (sourceIsArray) {
    return options.arrayMerge(target, source, options);
  }
  return mergeObject(target, source, options);
}

module.exports = deepmerge;
module.exports.default = deepmerge;
module.exports.deepmerge = deepmerge;
