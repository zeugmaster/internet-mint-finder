// NIP-07 identity is shared by the whole site. Only a public key is remembered;
// each review's signature is the server's proof of authorship, not localStorage.
// nostr-tools.js is the existing, locally served nostr-tools 2.25.2 bundle.
const EXTENSION_KEY = 'imf.extension';
const validPubkey = key => typeof key === 'string' && /^[0-9a-f]{64}$/.test(key);
let extensionPubkey = null;
let accountVersion = 0;
let signingIn = false;
try {
  localStorage.removeItem('imf.nsec'); // Remove private keys saved by older builds.
  const remembered = localStorage.getItem(EXTENSION_KEY);
  if (validPubkey(remembered)) extensionPubkey = remembered;
  else localStorage.removeItem(EXTENSION_KEY);
} catch (_) { /* Storage is optional. */ }

function setText(el, text) {
  if (el && el.textContent !== text) el.textContent = text;
}
function setHidden(el, hidden) {
  if (el.hidden !== hidden) el.hidden = hidden;
}
function render() {
  for (const el of document.querySelectorAll('[data-signer]')) {
    setHidden(el, (el.dataset.signer === 'in') !== !!extensionPubkey);
  }
  for (const el of document.querySelectorAll('[data-nostr-pubkey]')) {
    const npub = extensionPubkey ? NostrTools.nip19.npubEncode(extensionPubkey) : '';
    setText(el, npub ? `${npub.slice(0, 12)}…${npub.slice(-6)}` : '');
    el.title = npub;
  }
  for (const el of document.querySelectorAll('[data-review-fields]')) el.disabled = !extensionPubkey;
  for (const el of document.querySelectorAll('[data-nostr-signin]')) el.disabled = signingIn;
}
function signInWith(pubkey) {
  extensionPubkey = validPubkey(pubkey) ? pubkey : null;
  accountVersion++;
  try {
    if (extensionPubkey) localStorage.setItem(EXTENSION_KEY, extensionPubkey);
    else localStorage.removeItem(EXTENSION_KEY);
  } catch (_) { /* Kept only for this page when storage is unavailable. */ }
  render();
}
function loginMessage(message) {
  const status = document.getElementById('nostr-login-status');
  if (status) { setText(status, message); setHidden(status, !message); }
}
function say(form, message, ok) {
  if (!form.isConnected) return;
  const status = form.querySelector('#review-status');
  if (status) { setText(status, message); status.dataset.ok = ok ? '1' : '0'; }
}
async function signInWithExtension() {
  if (signingIn) return;
  if (!window.nostr?.getPublicKey || !window.nostr?.signEvent) {
    loginMessage('Install a Nostr signing extension such as Alby or nos2x, then try again.');
    return;
  }
  signingIn = true;
  const version = accountVersion;
  render();
  try {
    const pubkey = await window.nostr.getPublicKey();
    if (version !== accountVersion) return;
    if (!validPubkey(pubkey)) throw new Error('Invalid public key');
    signInWith(pubkey);
    loginMessage('');
  } catch (_) {
    loginMessage('Sign-in was declined or unavailable. Try again when your extension is ready.');
  } finally { signingIn = false; render(); }
}

render();
new MutationObserver(render).observe(document.documentElement, { childList: true, subtree: true });
window.addEventListener('load', render);
window.addEventListener('storage', ev => {
  if (ev.key === EXTENSION_KEY || ev.key === null) {
    extensionPubkey = validPubkey(ev.newValue) ? ev.newValue : null;
    accountVersion++;
    render();
  }
});
document.addEventListener('click', ev => {
  if (!(ev.target instanceof Element)) return;
  if (ev.target.closest('[data-nostr-signin]')) signInWithExtension();
  if (ev.target.closest('[data-nostr-signout]')) { signInWith(null); loginMessage(''); }
});

document.addEventListener('submit', async ev => {
  const form = ev.target;
  if (!(form instanceof HTMLFormElement) || form.id !== 'review-form') return;
  ev.preventDefault();
  if (!extensionPubkey || !window.nostr?.signEvent) {
    say(form, 'Sign in with a Nostr signing extension first.', false);
    return;
  }
  const rating = form.elements.rating.value;
  const text = form.elements.content.value.trim();
  if (!/^[1-5]$/.test(rating)) { say(form, 'Choose a star rating first.', false); return; }
  if (!text || [...text].length > 1000) { say(form, 'Write between 1 and 1,000 characters.', false); return; }
  const button = form.querySelector('button[type="submit"]');
  if (button.disabled) return;
  const data = { ...form.dataset };
  const version = accountVersion;
  const author = extensionPubkey;
  const tags = [['k', '38172'], ['d', data.mintPubkey || data.mintUrl], ['u', data.mintUrl]];
  if (data.announcer && data.mintPubkey) tags.push(['a', `38172:${data.announcer}:${data.mintPubkey}`]);
  const unsigned = { kind: 38000, created_at: Math.floor(Date.now() / 1000), tags, content: `[${rating}/5] ${text}` };
  button.disabled = true;
  say(form, 'Approve the review in your extension…', true);
  try {
    const event = await window.nostr.signEvent(JSON.parse(JSON.stringify(unsigned)));
    if (version !== accountVersion || !form.isConnected) return;
    if (event.pubkey !== author) {
      signInWith(null);
      throw new Error('Your extension changed accounts. Sign in again before posting.');
    }
    // Also reject extensions that return a different payload than the one approved here.
    if (event.kind !== unsigned.kind || event.content !== unsigned.content || event.created_at !== unsigned.created_at
      || JSON.stringify(event.tags) !== JSON.stringify(unsigned.tags) || !NostrTools.verifyEvent(event)) {
      throw new Error('The extension returned an invalid or changed review. Please try again.');
    }
    say(form, 'Saving your review…', true);
    const response = await fetch(`/api/mints/${encodeURIComponent(data.mintId)}/reviews`, {
      method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(event),
      signal: AbortSignal.timeout(15000),
    });
    const result = await response.json();
    if (!response.ok || !result.saved) throw new Error(result.error || 'Could not save your review. Please try again.');
    say(form, result.published ? 'Review posted.' : 'Review saved. It will be sent to Nostr when a relay accepts it.', true);
    form.reset();
    if (form.isConnected) window.dispatchEvent(new Event('imf:review-posted'));
  } catch (error) {
    say(form, error?.name === 'TimeoutError'
      ? 'The request timed out. Your review may have been saved; check the page before retrying.'
      : (error?.message || 'Could not sign or save your review. Your draft is still here.'), false);
  } finally { button.disabled = false; }
});
