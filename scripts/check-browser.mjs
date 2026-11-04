// Exercise the actual browser handlers, including asynchronous account changes.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
const source = readFileSync(new URL('../public/review.js', import.meta.url), 'utf8');
assert(!source.includes('finalizeEvent'), 'the application must never sign with a private key');
const headers = readFileSync(new URL('../src/components/header.rs', import.meta.url), 'utf8');
const compose = readFileSync(new URL('../src/components/reviews.rs', import.meta.url), 'utf8');
assert(headers.includes('data-nostr-signin') && headers.includes('data-nostr-signout'));
assert(!compose.includes('review-nsec'));
const pubkey = 'a'.repeat(64);
const handlers = {}, windowHandlers = {}, storage = new Map([['imf.nsec', 'obsolete-secret']]);
const saved = [], requests = [], dispatched = [];
class Element {
  constructor(selector = '') { this.selector = selector; this.textContent = ''; this.dataset = {}; this.hidden = false; }
  closest(selector) { return selector === this.selector ? this : null; }
}
class HTMLFormElement extends Element {
  constructor() {
    super(); this.id = 'review-form'; this.isConnected = true;
    this.dataset = { mintId: 'mint', mintUrl: 'https://mint.example', mintPubkey: '', announcer: '' };
    this.elements = { rating: { value: '4' }, content: { value: 'Useful mint' } };
    this.button = { disabled: false }; this.status = new Element(); this.resetCount = 0;
  }
  querySelector(selector) { return selector === '#review-status' ? this.status : this.button; }
  reset() { this.resetCount++; }
}
const login = new Element(), inLabel = new Element(), fields = new Element();
const context = vm.createContext({
  Element, HTMLFormElement, Event: class { constructor(type) { this.type = type; } },
  AbortSignal, console,
  MutationObserver: class { observe() {} },
  localStorage: {
    getItem: key => storage.get(key) ?? null,
    removeItem: key => storage.delete(key),
    setItem: (key, value) => { assert.equal(key, 'imf.extension'); assert.match(value, /^[0-9a-f]{64}$/); saved.push([key,value]); storage.set(key,value); },
  },
  document: {
    documentElement: {},
    getElementById: id => id === 'nostr-login-status' ? login : null,
    querySelectorAll: selector => ({ '[data-nostr-pubkey]': [inLabel], '[data-review-fields]': [fields] }[selector] || []),
    addEventListener: (name, handler) => { handlers[name] = handler; },
  },
  window: {
    addEventListener: (name, handler) => { windowHandlers[name] = handler; },
    dispatchEvent: event => dispatched.push(event.type),
    nostr: { getPublicKey: async () => pubkey, signEvent: async event => ({ ...event, id: 'e'.repeat(64), pubkey, sig: 'ok' }) },
  },
  NostrTools: { nip19: { npubEncode: key => `npub1${key}` }, verifyEvent: event => event.sig === 'ok' },
  fetch: async (url, options) => { requests.push([url,JSON.parse(options.body)]); return { ok: true, json: async () => ({ saved: true, published: false }) }; },
});
vm.runInContext(source, context);
assert(!storage.has('imf.nsec'));
assert(fields.disabled);
await vm.runInContext('signInWithExtension()', context);
assert.equal(storage.get('imf.extension'),pubkey);
assert(!fields.disabled);
assert(inLabel.textContent.startsWith('npub1'));

const submit = async form => handlers.submit({ target: form, preventDefault() {} });
const form = new HTMLFormElement();
await submit(form);
assert.equal(requests.length,1);
assert.equal(requests[0][1].kind,38000);
assert.deepEqual(requests[0][1].tags,[['k','38172'],['d','https://mint.example'],['u','https://mint.example']]);
assert.equal(form.resetCount,1);
assert.deepEqual(dispatched,['imf:review-posted']);
assert(form.status.textContent.includes('saved'));

// The signed payload cannot switch authors or change the submitted content.
for (const change of [{ pubkey: 'b'.repeat(64) }, { content: 'Changed' }, { sig: 'forged' }]) {
  await vm.runInContext('signInWithExtension()',context);
  context.window.nostr.signEvent = async event => ({ ...event, pubkey, sig: 'ok', ...change });
  const draft = new HTMLFormElement();
  await submit(draft);
  assert.equal(draft.resetCount,0);
  assert.equal(draft.status.dataset.ok,'0');
  assert(!draft.button.disabled);
}
assert.equal(requests.length,1);

// An extension must not be able to mutate the expected payload in place.
await vm.runInContext('signInWithExtension()',context);
context.window.nostr.signEvent = async event => { event.content = 'Changed in place'; return { ...event,pubkey,sig:'ok' }; };
const mutated = new HTMLFormElement(); await submit(mutated);
assert.equal(mutated.resetCount,0); assert.equal(requests.length,1);

// Signing out while the extension dialog is open must cancel submission.
await vm.runInContext('signInWithExtension()',context);
let resolveSignature;
context.window.nostr.signEvent = event => new Promise(resolve => { resolveSignature = () => resolve({ ...event,pubkey,sig:'ok' }); });
const pendingForm = new HTMLFormElement();
const pending = submit(pendingForm);
handlers.click({ target: new Element('[data-nostr-signout]') });
resolveSignature(); await pending;
assert.equal(requests.length,1);
assert(fields.disabled);
assert(!storage.has('imf.extension'));

// Cross-tab changes update all forms, and missing/denied extensions give a useful error.
windowHandlers.storage({ key:'imf.extension',newValue:pubkey });
assert(!fields.disabled);
windowHandlers.storage({ key:null,newValue:null });
assert(fields.disabled);
context.window.nostr = null;
await vm.runInContext('signInWithExtension()',context);
assert(login.textContent.includes('Install'));
context.window.nostr = { signEvent() {}, getPublicKey: async () => { throw new Error('Declined'); } };
await vm.runInContext('signInWithExtension()',context);
assert(login.textContent.includes('declined'));
assert(!storage.has('imf.extension'));

// A failed API request must keep the draft and re-enable submission.
context.window.nostr = { getPublicKey: async () => pubkey, signEvent: async event => ({ ...event,pubkey,sig:'ok' }) };
await vm.runInContext('signInWithExtension()',context);
context.fetch = async () => ({ ok:false,json:async () => ({ error:'Try again later' }) });
const retry = new HTMLFormElement(); await submit(retry);
assert.equal(retry.resetCount,0); assert.equal(retry.elements.content.value,'Useful mint');
assert.equal(retry.status.textContent,'Try again later'); assert(!retry.button.disabled);
console.log('Browser handler checks passed: extension-only login, shared identity, signed payload, cancellation and retry.');
