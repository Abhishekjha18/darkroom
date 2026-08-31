// The QR pairing page. Deliberately separate from app.js — this page's job
// is "get a phone onto the timeline and let the person at the laptop pick
// which folder that timeline shows", not render one.
//
// The QR image itself never changes: it's /qr.png, computed once at
// startup from this address, and the address doesn't change when the
// folder does. Switching folders only ever changes what /api/photos
// answers — so the same code, scanned any time, shows whatever darkroom is
// currently pointed at.

'use strict';

const $ = (id) => document.getElementById(id);

const fallbackUrl = $('fallback-url');
const editToggle = $('edit-toggle');
const editBox = $('edit-box');
const rootInput = $('root-input');
const rootGo = $('root-go');
const editMsg = $('edit-msg');

fallbackUrl.textContent = location.origin;
fallbackUrl.href = location.origin;

let currentRoot = '';

function refreshCurrentRoot() {
  return fetch('/api/photos')
    .then((r) => r.json())
    .then((data) => { currentRoot = data.root; });
}

// `kind` picks the colour: 'ok' (green, switch confirmed), 'error' (the
// default red), or 'busy' (neutral — indexing is still running, not a
// problem).
function showMsg(text, kind) {
  editMsg.textContent = text;
  editMsg.classList.toggle('ok', kind === 'ok');
  editMsg.classList.toggle('busy', kind === 'busy');
  editMsg.hidden = false;
}

function openEdit() {
  rootInput.value = currentRoot;
  editMsg.hidden = true;
  editBox.hidden = false;
  rootInput.focus();
  rootInput.select();
}

function closeEdit() {
  editBox.hidden = true;
}

editToggle.addEventListener('click', () => (editBox.hidden ? openEdit() : closeEdit()));

// Watches one indexing pass to completion, then reports the new folder and
// how many photos it found — the confirmation that answers "did switching
// actually work", without a second QR code that would just encode the same
// address as the first.
function watchSwitch() {
  const es = new EventSource('/api/progress');
  es.onmessage = (ev) => {
    const p = JSON.parse(ev.data);
    if (!p.indexing) {
      es.close();
      refreshCurrentRoot().then(() => {
        showMsg(`Now serving: ${currentRoot} (${p.indexed.toLocaleString()} photos found) — scan the same code above, no new one needed.`, 'ok');
      });
      return;
    }
    showMsg(p.total ? `indexing ${p.done.toLocaleString()} / ${p.total.toLocaleString()}…` : 'scanning folder…', 'busy');
  };
  es.onerror = () => { es.close(); };
}

function submitRoot() {
  const path = rootInput.value.trim();
  if (!path) return;

  rootGo.disabled = true;
  fetch('/api/root?path=' + encodeURIComponent(path), { method: 'POST' })
    .then(async (r) => {
      const data = await r.json().catch(() => ({}));
      if (!r.ok) throw new Error(data.error || `HTTP ${r.status}`);
      watchSwitch();
    })
    .catch((e) => showMsg(e.message, 'error'))
    .finally(() => { rootGo.disabled = false; });
}

rootGo.addEventListener('click', submitRoot);
rootInput.addEventListener('keydown', (e) => {
  if (e.key === 'Enter') submitRoot();
  if (e.key === 'Escape') closeEdit();
});

refreshCurrentRoot();
