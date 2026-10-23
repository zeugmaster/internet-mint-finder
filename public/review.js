// Signs a NIP-87 mint review (kind 38000) with a NIP-07 browser extension and
// sends it straight to the relays this instance reads. The server never sees
// the review on the write path; it ingests it from the relays like any client.
document.addEventListener('submit', async (ev) => {
  const form = ev.target;
  if (!(form instanceof HTMLFormElement) || form.id !== 'review-form') return;
  ev.preventDefault();
  const status = form.querySelector('#review-status');
  const say = (msg, ok) => {
    if (status) { status.textContent = msg; status.dataset.ok = ok ? '1' : '0'; }
  };
  if (!window.nostr) {
    say('No Nostr signer found. Install a NIP-07 extension (Alby, nos2x), or review this mint from eNuts, Zeus or Boardwalk.', false);
    return;
  }
  const data = form.dataset;
  const rating = form.elements.rating.value;
  const text = form.elements.content.value.trim();
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
  try { event = await window.nostr.signEvent(unsigned); }
  catch (_) { say('Signing was cancelled.', false); return; }
  const relays = (data.relays || '').split(',').filter(Boolean);
  say('Publishing…', true);
  const results = await Promise.all(relays.map((url) => publish(url, event)));
  const ok = results.filter(Boolean).length;
  if (ok) {
    say(`Published to ${ok} of ${relays.length} relays. It shows up here once ingested.`, true);
    form.reset();
  } else {
    say('No relay accepted the review.', false);
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
