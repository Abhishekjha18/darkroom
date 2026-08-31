// darkroom web client. No framework, no build step, no npm.
//
// Tiles point at /thumb/{id} — PNG bytes darkroom decoded, resampled and
// re-encoded itself, served straight from the index without touching the
// filesystem. /orig/{id} is only fetched when a photo is opened.

'use strict';

const $ = (id) => document.getElementById(id);

const statsEl = $('stats');
const emptyEl = $('empty');
const timelineEl = $('view-timeline');
const clustersEl = $('view-clusters');
const dupeCount = $('dupe-count');
const progressEl = $('progress');
const progressFill = $('progress-fill');
const progressText = $('progress-text');
const viewer = $('viewer');
const viewerImg = $('viewer-img');
const viewerInfo = $('viewer-info');

let photos = [];
let byId = new Map();

function bytes(n) {
  if (n < 1024) return n + ' B';
  const units = ['KB', 'MB', 'GB', 'TB'];
  let v = n / 1024, i = 0;
  while (v >= 1024 && i < units.length - 1) { v /= 1024; i++; }
  return (v < 10 ? v.toFixed(1) : Math.round(v)) + ' ' + units[i];
}

// The server already decided which day each photo belongs to; this only
// makes that label human.
function prettyDay(day) {
  const [y, m, d] = day.split('-').map(Number);
  const months = ['January', 'February', 'March', 'April', 'May', 'June',
    'July', 'August', 'September', 'October', 'November', 'December'];
  return `${d} ${months[m - 1]} ${y}`;
}

function tile(p) {
  const el = document.createElement('button');
  el.className = 'tile';
  el.type = 'button';
  el.addEventListener('click', () => open(p));

  if (!p.thumb) {
    el.classList.add('nopreview');
    const fmt = document.createElement('span');
    fmt.className = 'fmt';
    fmt.textContent = p.reason || p.format;
    const why = document.createElement('span');
    why.className = 'why';
    why.textContent = p.state === 'failed' ? 'could not decode' : 'preview unavailable';
    el.append(fmt, why);
    return el;
  }

  const img = document.createElement('img');
  img.loading = 'lazy';
  img.decoding = 'async';
  img.alt = p.name;
  img.src = '/thumb/' + p.id;
  img.addEventListener('load', () => img.classList.add('loaded'));
  el.appendChild(img);
  return el;
}

function renderTimeline() {
  timelineEl.textContent = '';
  const frag = document.createDocumentFragment();
  let currentDay = null;
  let grid = null;

  for (const p of photos) {
    if (p.day !== currentDay) {
      currentDay = p.day;
      const h = document.createElement('h2');
      h.className = 'day';
      h.textContent = prettyDay(p.day);
      if (p.dateSource === 'mtime') {
        const g = document.createElement('span');
        g.className = 'guess';
        // "This date is a guess" is information the user wants.
        g.textContent = 'from file time';
        h.appendChild(g);
      }
      frag.appendChild(h);
      grid = document.createElement('div');
      grid.className = 'grid';
      frag.appendChild(grid);
    }
    grid.appendChild(tile(p));
  }
  timelineEl.appendChild(frag);
}

function renderClusters(data) {
  clustersEl.textContent = '';
  dupeCount.hidden = data.count === 0;
  dupeCount.textContent = data.count;

  if (!data.count) {
    const p = document.createElement('p');
    p.className = 'empty';
    p.textContent = 'No near-duplicates found.';
    clustersEl.appendChild(p);
    return;
  }

  const head = document.createElement('h2');
  head.className = 'day';
  head.textContent = `${data.count} clusters · ${bytes(data.wasted)} reclaimable`;
  clustersEl.appendChild(head);

  for (const c of data.clusters) {
    const box = document.createElement('div');
    box.className = 'cluster';

    const label = document.createElement('div');
    label.className = 'cluster-head';
    label.textContent = `${c.ids.length} copies · ${bytes(c.wasted)} reclaimable`;
    box.appendChild(label);

    const grid = document.createElement('div');
    grid.className = 'grid';
    for (const id of c.ids) {
      const p = byId.get(id);
      if (!p) continue;
      const t = tile(p);
      if (id === c.best) {
        t.classList.add('keeper');
        const k = document.createElement('span');
        k.className = 'keeper-tag';
        k.textContent = 'keep';
        t.appendChild(k);
      }
      grid.appendChild(t);
    }
    box.appendChild(grid);
    clustersEl.appendChild(box);
  }
}

function open(p) {
  viewerImg.src = p.thumb ? '/orig/' + p.id : '';
  viewerImg.alt = p.name;
  viewerInfo.textContent = '';

  const rows = [['name', p.rel]];
  if (p.w) rows.push(['size', `${p.w} x ${p.h} · ${bytes(p.bytes)}`]);
  else rows.push(['size', bytes(p.bytes)]);
  rows.push(['taken', prettyDay(p.day) + (p.dateSource === 'mtime' ? ' (file time)' : '')]);
  if (p.camera) rows.push(['camera', p.camera]);
  if (p.lens) rows.push(['lens', p.lens]);
  const shot = [p.exposure, p.aperture, p.iso ? 'ISO ' + p.iso : null].filter(Boolean);
  if (shot.length) rows.push(['exposure', shot.join(' · ')]);
  if (p.gps) rows.push(['gps', p.gps.join(', ')]);
  if (p.state !== 'ok') rows.push(['note', (p.reason || '') + ' — no preview']);

  for (const [k, v] of rows) {
    if (!v) continue;
    const d = document.createElement('div');
    if (k === 'name') {
      d.className = 'name';
      d.textContent = v;
    } else {
      d.textContent = k + ': ' + v;
    }
    viewerInfo.appendChild(d);
  }
  viewer.hidden = false;
}

function close() {
  viewer.hidden = true;
  viewerImg.src = '';
}

$('close').addEventListener('click', close);
viewer.addEventListener('click', (e) => { if (e.target === viewer) close(); });
document.addEventListener('keydown', (e) => { if (e.key === 'Escape') close(); });

for (const tab of document.querySelectorAll('.tab')) {
  tab.addEventListener('click', () => {
    for (const t of document.querySelectorAll('.tab')) t.classList.remove('is-on');
    tab.classList.add('is-on');
    const wantClusters = tab.dataset.view === 'clusters';
    timelineEl.hidden = wantClusters;
    clustersEl.hidden = !wantClusters;
  });
}

function loadCatalog() {
  return fetch('/api/photos')
    .then((r) => r.json())
    .then((data) => {
      photos = data.photos;
      byId = new Map(photos.map((p) => [p.id, p]));

      const parts = [data.count.toLocaleString() + ' photos', bytes(data.bytes)];
      if (data.clusters) parts.push(bytes(data.wasted) + ' reclaimable');
      statsEl.textContent = parts.join(' · ');

      emptyEl.hidden = photos.length > 0 || data.indexing;
      renderTimeline();
      return fetch('/api/clusters').then((r) => r.json()).then(renderClusters);
    })
    .catch((e) => {
      statsEl.textContent = 'failed to load catalog';
      console.error(e);
    });
}

// Live indexing progress. The counter climbing is the cheapest motion
// available, and the stream closes itself when indexing finishes — so a
// second index (a folder switch) needs its own fresh EventSource, not a
// re-send on the one that already closed.
function watchProgress() {
  const es = new EventSource('/api/progress');
  es.onmessage = (ev) => {
    const p = JSON.parse(ev.data);
    if (!p.indexing) {
      es.close();
      progressEl.hidden = true;
      loadCatalog();
      return;
    }
    progressEl.hidden = false;
    const pct = p.total ? Math.round((p.done / p.total) * 100) : 0;
    progressFill.style.width = pct + '%';
    progressText.textContent = p.total
      ? `indexing ${p.done.toLocaleString()} / ${p.total.toLocaleString()}`
      : 'scanning folder…';
  };
  es.onerror = () => { es.close(); progressEl.hidden = true; loadCatalog(); };
}

loadCatalog();
watchProgress();
