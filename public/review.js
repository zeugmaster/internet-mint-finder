// Signs a NIP-87 mint review (kind 38000) in the browser, with the nsec the
// user signed in with or their NIP-07 extension, and sends it straight to the
// relays this instance reads. The server never sees the review on the write
// path; it ingests it from the relays like any client.
// nostr-tools.js is lib/nostr.bundle.js from nostr-tools@2.25.2 on npm,
// unmodified (sha256 a7c4846fde84a0d6ce7596f3fcfc88435d29fdcd616887205ab97f9a16bc6407).
// shortcut: the nsec sits in localStorage, readable by any script on this
// origin; move to NIP-46 if the site ever loads third-party scripts.
const NSEC_KEY = 'imf.nsec';
// Signing in with an extension keeps only its public key, to show who is
// signed in; the extension signs each review itself.
const EXTENSION_KEY = 'imf.extension';
let extensionPubkey = null;
try { extensionPubkey = localStorage.getItem(EXTENSION_KEY); } catch (_) { /* not kept */ }

// The line beside Post review starts as a hint; a message replaces it and
// quiet() puts the hint back.
function say(msg, ok) {
  const status = document.getElementById('review-status');
  if (!status) return;
  if (status.dataset.hint === undefined) status.dataset.hint = status.textContent.trim();
  status.textContent = msg;
  status.dataset.ok = ok ? '1' : '0';
}

function quiet() {
  const status = document.getElementById('review-status');
  if (!status || status.dataset.hint === undefined) return;
  status.textContent = status.dataset.hint;
  delete status.dataset.ok;
}

function storedKey() {
  try {
    const nsec = localStorage.getItem(NSEC_KEY);
    return nsec ? NostrTools.nip19.decode(nsec).data : null;
  } catch (_) { return null; }
}

function setHidden(el, hidden) {
  if (el.hidden !== hidden) el.hidden = hidden;
}

function setText(el, text) {
  if (el && el.textContent !== text) el.textContent = text;
}

// Shows either the sign-in box or the signed-in line, and the extension
// button once an extension is found. Re-runs on DOM changes because pages are
// swapped client-side after the first load.
function render() {
  const sk = storedKey();
  const pubkey = sk ? NostrTools.getPublicKey(sk) : extensionPubkey;
  for (const el of document.querySelectorAll('[data-signer]')) {
    setHidden(el, (el.dataset.signer === 'in') !== !!pubkey);
  }
  for (const el of document.querySelectorAll('[data-extension]')) {
    setHidden(el, (el.dataset.extension === 'on') !== !!window.nostr);
  }
  const label = document.getElementById('review-npub');
  if (label && pubkey) {
    const npub = NostrTools.nip19.npubEncode(pubkey);
    const short = `${npub.slice(0, 12)}…${npub.slice(-6)}`;
    if (label.textContent !== short) { label.textContent = short; label.title = npub; }
  }
  setText(document.getElementById('review-via'), sk || !pubkey ? '' : ' with your extension');
}

function signInWith(pubkey) {
  extensionPubkey = pubkey;
  try {
    if (pubkey) localStorage.setItem(EXTENSION_KEY, pubkey);
    else localStorage.removeItem(EXTENSION_KEY);
  } catch (_) { /* kept for this page only */ }
}

render();
new MutationObserver(render).observe(document.documentElement, { childList: true, subtree: true });
// Extensions can add window.nostr after this script runs.
window.addEventListener('load', render);
setTimeout(render, 1500);

async function signInWithExtension(button) {
  if (!window.nostr) { say('No Nostr extension found in this browser.', false); return; }
  button.disabled = true;
  try {
    const pubkey = await window.nostr.getPublicKey();
    if (!/^[0-9a-f]{64}$/.test(pubkey)) throw new Error('not a public key');
    signInWith(pubkey);
    render();
    quiet();
  } catch (_) {
    say('Your extension didn’t sign you in. Try again, or paste your nsec.', false);
  } finally {
    button.disabled = false;
  }
}

document.addEventListener('click', (ev) => {
  const id = ev.target instanceof Element ? ev.target.id : '';
  if (id === 'review-signin') {
    const input = document.getElementById('review-nsec');
    const nsec = input.value.trim();
    let decoded = null;
    try { decoded = NostrTools.nip19.decode(nsec); } catch (_) { /* not bech32 */ }
    if (!decoded || decoded.type !== 'nsec') { say('That isn’t an nsec key. It starts with nsec1.', false); return; }
    try { localStorage.setItem(NSEC_KEY, nsec); } catch (_) { say('This browser won’t keep your key. Use a Nostr extension instead.', false); return; }
    input.value = '';
    signInWith(null);
    render();
    quiet();
  } else if (id === 'review-extension') {
    signInWithExtension(ev.target);
  } else if (id === 'review-signout') {
    try { localStorage.removeItem(NSEC_KEY); } catch (_) { /* nothing stored */ }
    signInWith(null);
    render();
    quiet();
  }
});

document.addEventListener('keydown', (ev) => {
  if (ev.key === 'Enter' && ev.target instanceof Element && ev.target.id === 'review-nsec') {
    document.getElementById('review-signin').click();
  }
});

document.addEventListener('submit', async (ev) => {
  const form = ev.target;
  if (!(form instanceof HTMLFormElement) || form.id !== 'review-form') return;
  ev.preventDefault();
  const sk = storedKey();
  if (!sk && !window.nostr) {
    say('Sign in with your nsec, use a Nostr extension like Alby, or review from a wallet such as Zeus or Boardwalk.', false);
    return;
  }
  const data = form.dataset;
  const rating = form.elements.rating.value;
  const text = form.elements.content.value.trim();
  if (!rating) { say('Choose a star rating first.', false); return; }
  if (!text) { say('Write a few words first.', false); return; }
  const tags = [['k', '38172'], ['d', data.mintPubkey]];
  if (data.mintUrl) tags.push(['u', data.mintUrl]);
  if (data.announcer) tags.push(['a', `38172:${data.announcer}:${data.mintPubkey}`]);
  const unsigned = {
    kind: 38000,
    created_at: Math.floor(Date.now() / 1000),
    tags,
    content: `[${rating}/5] ${text}`,
  };
  // One review at a time: the button stays off from signing (an extension
  // may ask first) until the relays answer, however that ends.
  const button = form.querySelector('button[type="submit"]');
  if (button.disabled) return;
  button.disabled = true;
  try {
    let event;
    try { event = sk ? NostrTools.finalizeEvent(unsigned, sk) : await window.nostr.signEvent(unsigned); }
    catch (_) { say('Couldn’t sign the review. Try again.', false); return; }
    // The extension may have switched accounts since sign-in.
    if (!sk && event.pubkey !== extensionPubkey) { signInWith(event.pubkey); render(); }
    const relays = (data.relays || '').split(',').filter(Boolean);
    say('Posting…', true);
    const results = await Promise.all(relays.map((url) => publish(url, event)));
    const ok = results.filter(Boolean).length;
    if (ok) {
      say('Posted. It shows up here once this site picks it up from Nostr.', true);
      form.reset();
    } else {
      say('Couldn’t post: no Nostr relay accepted it. Try again later.', false);
    }
  } finally {
    button.disabled = false;
  }
});

function publish(url, event) {
  return new Promise((resolve) => {
    let ws;
    let done = false;
    const finish = (v) => {
      if (done) return;
      done = true;
      resolve(v);
      try { ws.close(); } catch (_) { /* already closed */ }
    };
    try { ws = new WebSocket(url); } catch (_) { return resolve(false); }
    const timer = setTimeout(() => finish(false), 8000);
    ws.onopen = () => ws.send(JSON.stringify(['EVENT', event]));
    ws.onmessage = (m) => {
      try {
        const msg = JSON.parse(m.data);
        if (msg[0] === 'OK' && msg[1] === event.id) { clearTimeout(timer); finish(!!msg[2]); }
      } catch (_) { /* ignore non-JSON */ }
    };
    ws.onerror = () => { clearTimeout(timer); finish(false); };
  });
}
