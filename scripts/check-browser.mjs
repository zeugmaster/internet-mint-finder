// Security regressions against the actual browser handlers, without a browser dependency.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
const html = readFileSync(new URL('../public/dashboard.html', import.meta.url), 'utf8');
for (const [, source] of html.matchAll(/<script\b[^>]*>([\s\S]*?)<\/script>/g)) new vm.Script(source);
assert(!/<script[^>]+src=["']https?:/.test(html), 'scripts must be served locally');
const fn = name => {
  const source = html.match(new RegExp(`(?:async )?function ${name}\\([^]*?\\n}`));
  assert(source, name);
  return source[0];
};
const payload = 'https://mint.example/<img src=x onerror=alert(1)>" autofocus onfocus="alert(2)';
const tbody = { innerHTML: '' };
const context = vm.createContext({
  document: { querySelector: () => tbody }, state: { scatter_group: 'mint' },
  fmt: new Intl.NumberFormat('en'), compact: String, console,
  getJSON: async () => [{ from: payload, to: payload, mint: payload, swaps: 1,
    total_amount: 50, total_fee: 2, fee_pct: 0.04, attempts: 1, error_rate: 0, avg_latency_ms: 20 }],
});
vm.runInContext([fn('escapeHTML'), fn('renderFees'), fn('renderMintTable')].join('\n'), context);
for (const render of ['renderFees()', 'renderMintTable()']) {
  await vm.runInContext(render, context);
  assert(tbody.innerHTML.includes('&lt;img'));
  assert(tbody.innerHTML.includes('&quot; autofocus'));
  assert(!tbody.innerHTML.includes('<img'));
  assert(!tbody.innerHTML.includes('" autofocus'));
}
const review = readFileSync(new URL('../public/review.js', import.meta.url), 'utf8');
const handlers = {};
class Element { constructor(id) { this.id = id; } }
const input = { value: 'nsec1-test-key' };
const key = new Uint8Array(32).fill(9);
const storage = [];
const browser = vm.createContext({ Element, HTMLFormElement: class {}, Uint8Array,
  document: { documentElement: {}, querySelectorAll: () => [],
    getElementById: id => id === 'review-nsec' ? input : null,
    addEventListener: (name, handler) => { handlers[name] = handler; } },
  MutationObserver: class { observe() {} },
  localStorage: { removeItem: name => storage.push(name), setItem: () => assert.fail('persisted private key') },
  NostrTools: { nip19: { decode: () => ({ type: 'nsec', data: key }) } },
});
vm.runInContext(review, browser);
assert.deepEqual(storage, ['imf.nsec']);
assert.equal(vm.runInContext('storedKey()', browser), null);
handlers.click({ target: new Element('review-signin') });
assert.equal(input.value, '');
assert.equal(vm.runInContext('storedKey()', browser), key);
handlers.click({ target: new Element('review-signout') });
assert.equal(vm.runInContext('storedKey()', browser), null);
assert(key.every(byte => byte === 0));
console.log('Browser security checks passed');
