import assert from 'node:assert/strict';
import deepmerge from './index.mjs';

// The ordinary contract the build relies on.
assert.deepEqual(deepmerge({ a: { b: 1 } }, { a: { c: 2 } }), { a: { b: 1, c: 2 } });
assert.deepEqual(
  deepmerge({ a: [1] }, { a: [2] }, { arrayMerge: (_target, source) => source }),
  { a: [2] }
);

// The vulnerability: JSON.parse creates own `__proto__`/`constructor` keys.
const hostile = JSON.parse(
  '{"__proto__":{"polluted":true},"constructor":{"prototype":{"polluted2":true}},"safe":1}'
);
const merged = deepmerge({}, hostile);
assert.equal({}.polluted, undefined, 'global Object.prototype is untouched');
assert.equal(merged.polluted, undefined);
assert.equal(merged.polluted2, undefined, 'constructor.prototype is not merged');
assert.equal(Object.getPrototypeOf(merged), Object.prototype);
assert.equal(merged.safe, 1);

// Inherited properties are not copied as own keys.
const target = Object.create({ inherited: 1 });
target.own = 1;
const inherited = deepmerge(target, { inherited: 2, other: 3 });
assert.equal(Object.prototype.hasOwnProperty.call(inherited, 'inherited'), false);
assert.equal(inherited.other, 3);

console.log('deepmerge local fork: ok');
