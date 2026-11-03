// Signs a NIP-87 mint review (kind 38000) in the browser, with the nsec the
// user signed in with or a NIP-07 extension, and sends it straight to the
// relays this instance reads. The server never sees the review on the write
// path; it ingests it from the relays like any client.
// nostr-tools.js is lib/nostr.bundle.js from nostr-tools@2.25.2 on npm,
// unmodified (sha256 a7c4846fde84a0d6ce7596f3fcfc88435d29fdcd616887205ab97f9a16bc6407).
// Never retain a private key across page reloads. Remove keys saved by older builds.
try { localStorage.removeItem('imf.nsec'); } catch (_) { /* storage disabled */ }
let sessionKey = null;

function say(msg, ok) {
  const status = document.getElementById('review-status');
  if (status) { status.textContent = msg; status.dataset.ok = ok ? '1' : '0'; }
}

function storedKey() { return sessionKey; }

// Shows either the sign-in box or the signed-in line. Re-runs on DOM changes
// because pages are swapped client-side after the first load.
function render() {
  const sk = storedKey();
  for (const el of document.querySelectorAll('[data-signer]')) {
    const show = (el.dataset.signer === 'in') === !!sk;
    if (el.hidden === show) el.hidden = !show;
  }
  const label = document.getElementById('review-npub');
  if (label && sk) {
    const npub = NostrTools.nip19.npubEncode(NostrTools.getPublicKey(sk));
    const short = `${npub.slice(0, 12)}…${npub.slice(-6)}`;
    if (label.textContent !== short) { label.textContent = short; label.title = npub; }
  }
}
render();
new MutationObserver(render).observe(document.documentElement, { childList: true, subtree: true });

document.addEventListener('click', (ev) => {
  const id = ev.target instanceof Element ? ev.target.id : '';
  if (id === 'review-signin') {
    const input = document.getElementById('review-nsec');
    const nsec = input.value.trim();
    let decoded = null;
    try { decoded = NostrTools.nip19.decode(nsec); } catch (_) { /* not bech32 */ }
    if (!decoded || decoded.type !== 'nsec') { say('That isn’t an nsec key. It starts with nsec1.', false); return; }
    sessionKey = decoded.data;
    input.value = '';
    render();
    say('Signed in.', true);
  } else if (id === 'review-signout') {
    if (sessionKey) sessionKey.fill(0);
    sessionKey = null;
    render();
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
  let event;
  try { event = sk ? NostrTools.finalizeEvent(unsigned, sk) : await window.nostr.signEvent(unsigned); }
  catch (_) { say('Couldn’t sign the review. Try again.', false); return; }
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
