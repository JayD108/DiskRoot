'use strict';
/* Xylem UI prototype v2: "classic utility, modern finish".
   Static demo data, no backend, no network. Open index.html directly.
   URL switches: ?dark  ?category  ?treemap  ?about  (for screenshots) */

// ---------------------------------------------------------------------------
// Demo data. Sizes in GB, generic names.
// ---------------------------------------------------------------------------
const CATS = {
  apps: 'Apps & Games', ai: 'AI & data', dev: 'Dev stuff', system: 'System', temp: 'Temp & caches',
  docs: 'Documents', down: 'Downloads', media: 'Media', cloud: 'Cloud-synced', other: 'Other',
};

// [safety, note] by sub-type / known name
const RULES = {
  'node_modules': ['green', 'JavaScript packages of a project. Safe to delete; npm install brings them back.'],
  'Temp': ['green', 'Temporary files. Anything not in use is safe to delete.'],
  'DXCache': ['green', 'GPU shader cache. Rebuilt automatically.'],
  'pip-cache': ['green', 'pip download cache. Clear with: pip cache purge'],
  '.venv': ['green', 'Python virtual environment. Recreate it from requirements.txt.'],
  'Steam': ['yellow', 'Installed games. Uninstall from Steam so it knows.'],
  'Epic Games': ['yellow', 'Installed games. Uninstall from the Epic launcher.'],
  'WinSxS': ['red', 'Windows component store. Much of it is shared with C:\\Windows. Shrink with DISM cleanup.'],
  'Windows': ['red', 'Windows itself. Use Disk Cleanup or Storage settings instead.'],
  'pagefile.sys': ['red', 'Virtual memory, managed by Windows.'],
  'hiberfil.sys': ['red', 'Hibernation file. "powercfg /h off" removes it.'],
  '$Recycle.Bin': ['green', 'Deleted files in the Recycle Bin. Empty it from the bin.'],
  'datasets': ['yellow', 'Data files (.npy, .h5, .parquet). Check before removing.'],
  'models': ['yellow', 'Model weights. Large, often re-downloadable.'],
  'Docker': ['yellow', 'Docker data. Clean with: docker system prune'],
  'Downloads': ['yellow', 'Your downloads. Old installers are usually safe to remove.'],
};

function D(name, cat, kids) { return { name, cat, kids }; }
function F(name, cat, size, files) { return { name, cat, size, files: files ?? Math.max(1, Math.round(size * 37)) }; }

const LOCATION = D('C:\\', 'other', [
  D('Program Files (x86)', 'apps', [
    D('Steam', 'apps', [F('Open-world RPG', 'apps', 96, 2104), F('Survival shooter', 'apps', 72, 911), F('Racing sim', 'apps', 61, 4410), F('9 more games', 'apps', 59, 7830)]),
    F('Common Files', 'apps', 6.3), F('Other programs', 'apps', 5.5),
  ]),
  D('datasets', 'ai', [F('satellite-2019', 'ai', 182, 884), F('eeg-recordings', 'ai', 96, 412), F('wavelet-cache', 'ai', 71.5, 96), F('raw', 'ai', 140, 1300)]),
  D('Users', 'other', [
    D('you', 'other', [
      D('AppData', 'apps', [
        D('Local', 'apps', [F('Temp', 'temp', 33, 98000), F('Programs', 'apps', 16.3, 41000), F('DXCache', 'temp', 8.1, 2400), F('pip-cache', 'dev', 7, 14000), F('Docker', 'dev', 7.3, 12)]),
        F('Roaming', 'apps', 2.9, 9100),
      ]),
      D('projects', 'dev', [
        D('web-app', 'dev', [F('node_modules', 'dev', 3.2, 61000), F('src', 'dev', 0.2, 900)]),
        D('ml-lab', 'dev', [F('.venv', 'dev', 4.4, 38000), F('models', 'ai', 21, 40), F('notebooks', 'docs', 0.4, 120)]),
      ]),
      F('Downloads', 'down', 21.4, 314),
      D('Documents', 'docs', [F('corpora', 'docs', 28.3, 410), F('papers', 'docs', 8.4, 2900)]),
      F('Pictures', 'media', 11.5, 51000),
      F('OneDrive', 'cloud', 0.01, 2300),
    ]),
  ]),
  D('Program Files', 'apps', [F('Epic Games', 'apps', 177, 15300), F('Other programs', 'apps', 22.6, 54000)]),
  D('Windows', 'system', [F('WinSxS', 'system', 15.9, 98000), F('System32', 'system', 13.1, 21000), F('Installer', 'system', 3.5, 600)]),
  F('pagefile.sys', 'system', 22, 1),
  F('hiberfil.sys', 'system', 12.5, 1),
  F('archive', 'other', 40.3, 210),
  F('$Recycle.Bin', 'temp', 9.4, 3100),
]);

// Finish the tree: folder totals, parents, paths.
(function finish(node, parent) {
  node.parent = parent;
  node.path = parent ? (parent.parent ? `${parent.path}\\${node.name}` : `${parent.path}${node.name}`) : node.name;
  if (node.kids) {
    node.size = 0;
    node.files = 0;
    for (const k of node.kids) { finish(k, node); node.size += k.size; node.files += k.files; }
    node.kids.sort((a, b) => b.size - a.size);
  }
})(LOCATION, null);

// By-category tree built from the leaves of the location tree.
function buildCategoryTree() {
  const root = { name: 'C:\\ by category', cat: 'other', kids: [], parent: null, path: 'C:\\ by category' };
  const byCat = {};
  (function walk(n) {
    if (n.kids) { n.kids.forEach(walk); return; }
    (byCat[n.cat] ||= []).push(n);
  })(LOCATION);
  for (const [cat, leaves] of Object.entries(byCat)) {
    const c = { name: CATS[cat], cat, parent: root, path: CATS[cat], kids: [], size: 0, files: 0 };
    for (const l of leaves) {
      const place = { name: l.name, cat, size: l.size, files: l.files, parent: c, path: l.path, ref: l };
      c.kids.push(place);
      c.size += l.size;
      c.files += l.files;
    }
    c.kids.sort((a, b) => b.size - a.size);
    root.kids.push(c);
  }
  root.kids.sort((a, b) => b.size - a.size);
  root.size = root.kids.reduce((s, k) => s + k.size, 0);
  root.files = root.kids.reduce((s, k) => s + k.files, 0);
  return root;
}
const CATEGORY = buildCategoryTree();

// ---------------------------------------------------------------------------
// State + helpers
// ---------------------------------------------------------------------------
const $ = (id) => document.getElementById(id);
const params = new URLSearchParams(location.search);
const store = {
  get(k) { try { return localStorage.getItem(k); } catch { return null; } },
  set(k, v) { try { localStorage.setItem(k, v); } catch { /* private mode */ } },
};

const state = {
  theme: params.has('dark') ? 'dark' : store.get('xylem-theme') || 'light',
  view: params.has('treemap') ? 'treemap' : 'sunburst',
  group: params.has('category') ? 'category' : 'location',
  tree: null,              // LOCATION or CATEGORY
  selected: null,
  expanded: new Set(),
  hover: null,
  filter: '',
};

const cssVar = (v) => getComputedStyle(document.documentElement).getPropertyValue(v).trim();
const catVar = { apps: '--c-apps', ai: '--c-ai', dev: '--c-dev', system: '--c-system', temp: '--c-temp', docs: '--c-docs', down: '--c-down', media: '--c-media', cloud: '--c-cloud', other: '--c-other' };
const catColor = (cat) => cssVar(catVar[cat] || '--c-other');

function fmtGB(gb) {
  if (gb >= 1024) return `${(gb / 1024).toFixed(2)} TB`;
  if (gb >= 100) return `${gb.toFixed(0)} GB`;
  if (gb >= 10) return `${gb.toFixed(1)} GB`;
  if (gb >= 1) return `${gb.toFixed(2)} GB`;
  if (gb >= 0.001) return `${(gb * 1024).toFixed(0)} MB`;
  return `${(gb * 1024 * 1024).toFixed(0)} KB`;
}
const fmtN = (n) => n.toLocaleString('en-US');
const pctOf = (a, b) => (b ? (a / b) * 100 : 0);
const fmtPct = (p) => `${p >= 10 ? p.toFixed(0) : p.toFixed(1)}%`;
const esc = (s) => String(s).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));

function ruleFor(node) {
  const n = node.ref || node;
  for (let x = n; x; x = x.parent) if (RULES[x.name]) return RULES[x.name];
  return null;
}

// Blend a hex colour toward a target (lighter for deeper rings in light theme).
function tint(hex, depth) {
  const m = /^#(..)(..)(..)$/.exec(hex);
  if (!m) return hex;
  const dark = state.theme === 'dark';
  const t = Math.min(0.55, (depth - 1) * 0.16);
  const target = dark ? [24, 25, 22] : [252, 251, 248];
  const c = [m[1], m[2], m[3]].map((h, i) => Math.round(parseInt(h, 16) * (1 - t) + target[i] * t));
  return `rgb(${c.join(',')})`;
}

// ---------------------------------------------------------------------------
// Menus, toolbar, theme
// ---------------------------------------------------------------------------
function applyTheme() {
  document.documentElement.dataset.theme = state.theme;
  store.set('xylem-theme', state.theme);
}

function syncChecks() {
  for (const el of document.querySelectorAll('[data-set]')) {
    const on = state[el.dataset.set] === el.dataset.val;
    if (el.closest('.dropdown')) el.classList.toggle('checked', on);
    else el.setAttribute('aria-pressed', String(on));
  }
}

function setOption(key, val) {
  if (state[key] === val) return;
  state[key] = val;
  if (key === 'theme') { applyTheme(); renderAll(); }
  if (key === 'group') { useTree(val === 'category' ? CATEGORY : LOCATION); }
  if (key === 'view') { geo = null; requestDraw(); }
  syncChecks();
}

document.addEventListener('click', (ev) => {
  const menu = ev.target.closest('.menu');
  const opened = document.querySelector('.menu.open');
  if (menu && ev.target.parentElement === menu) {
    const was = menu.classList.contains('open');
    opened?.classList.remove('open');
    if (!was) menu.classList.add('open');
    return;
  }
  if (opened && !ev.target.closest('.dropdown')) opened.classList.remove('open');
  const set = ev.target.closest('[data-set]');
  if (set) { setOption(set.dataset.set, set.dataset.val); opened?.classList.remove('open'); return; }
  const act = ev.target.closest('[data-act]');
  if (act) { opened?.classList.remove('open'); action(act.dataset.act); }
  if (ev.target.closest('[data-close]')) ev.target.closest('.modal').classList.add('hidden');
});

// Hovering across the menu bar while one menu is open switches menus (classic behaviour).
for (const m of document.querySelectorAll('.menu')) {
  m.addEventListener('mouseenter', () => {
    const opened = document.querySelector('.menu.open');
    if (opened && opened !== m) { opened.classList.remove('open'); m.classList.add('open'); }
  });
}

function action(a) {
  if (a === 'privacy') $('dlgPrivacy').classList.remove('hidden');
  else if (a === 'about') $('dlgAbout').classList.remove('hidden');
  else if (a === 'up') { const p = mapRoot().parent; if (p) select(p, true); }
  else if (a === 'scan' || a === 'fast') fakeScan(a === 'fast');
  else status(`"${a}" isn't part of the prototype.`);
}

let statusTimer = 0;
function status(msg) {
  $('stMsg').textContent = msg;
  clearTimeout(statusTimer);
  statusTimer = setTimeout(() => { $('stMsg').textContent = 'Ready'; }, 4000);
}

function fakeScan(fast) {
  const dlg = $('dlgScan');
  $('scanStep').textContent = fast ? 'Reading the Master File Table (read-only)…' : 'Walking folders (no admin needed)…';
  dlg.classList.remove('hidden');
  const total = 1799936;
  const dur = fast ? 1480 : 3260;
  const t0 = performance.now();
  const tick = (now) => {
    const t = Math.min(1, (now - t0) / dur);
    $('scanBar').style.width = `${(t * 100).toFixed(1)}%`;
    $('scanCount').textContent = `${fmtN(Math.round(total * t))} records`;
    if (t < 1) { requestAnimationFrame(tick); return; }
    dlg.classList.add('hidden');
    status(`Scanned C:\\ in ${(dur / 1000).toFixed(2)} s (${fast ? 'MFT' : 'folder walk'})`);
  };
  requestAnimationFrame(tick);
}

// ---------------------------------------------------------------------------
// Tree-table
// ---------------------------------------------------------------------------
const ICON_DIR = '<svg class="ico" viewBox="0 0 16 16"><path d="M1.5 4A1.5 1.5 0 0 1 3 2.5h3l1.5 1.5H13A1.5 1.5 0 0 1 14.5 5.5v6A1.5 1.5 0 0 1 13 13H3a1.5 1.5 0 0 1-1.5-1.5z" fill="none" stroke="currentColor" stroke-width="1.1"/></svg>';
const ICON_FILE = '<svg class="ico" viewBox="0 0 16 16"><path d="M4 1.5h5l3 3v10H4z M9 1.5v3h3" fill="none" stroke="currentColor" stroke-width="1.1"/></svg>';

function useTree(tree) {
  state.tree = tree;
  state.expanded = new Set([tree]);
  if (tree.kids && tree.kids[0]) state.expanded.add(tree.kids[0]);
  state.selected = tree;
  renderAll();
}

function visibleRows() {
  const out = [];
  const f = state.filter.toLowerCase();
  const matches = (n) => !f || n.name.toLowerCase().includes(f) || (n.kids || []).some(matches);
  (function walk(n, depth) {
    if (!matches(n)) return;
    out.push({ n, depth });
    if (n.kids && (state.expanded.has(n) || f)) n.kids.forEach((k) => walk(k, depth + 1));
  })(state.tree, 0);
  return out;
}

function renderRows() {
  const rows = visibleRows();
  const frag = document.createDocumentFragment();
  for (const { n, depth } of rows) {
    const row = document.createElement('div');
    row.className = 'row' + (n === state.selected ? ' sel' : '');
    row.setAttribute('role', 'treeitem');
    if (n.kids) row.setAttribute('aria-expanded', String(state.expanded.has(n)));
    row._n = n;
    const parentSize = n.parent ? n.parent.size : n.size;
    const p = pctOf(n.size, parentSize);
    const twisty = n.kids ? (state.expanded.has(n) ? '▼' : '▶') : '';
    row.innerHTML =
      `<span class="name" style="padding-left:${depth * 16}px"><span class="twisty">${twisty}</span>` +
      `<span class="sw" style="background:${catColor(n.cat)}"></span>${n.kids ? ICON_DIR : ICON_FILE}` +
      `<span class="label">${esc(n.name)}</span></span>` +
      `<span class="size num">${fmtGB(n.size)}</span>` +
      `<span class="bar"><span class="track"><span class="fill" style="width:${p.toFixed(2)}%;background:${catColor(n.cat)}"></span></span><span class="pct num">${fmtPct(p)}</span></span>` +
      `<span class="files num">${fmtN(n.files)}</span>` +
      `<span class="cat">${esc(n.kids && state.group === 'location' ? '' : CATS[n.cat])}</span>`;
    frag.append(row);
  }
  $('rows').replaceChildren(frag);
}

$('rows').addEventListener('click', (ev) => {
  const row = ev.target.closest('.row');
  if (!row) return;
  const n = row._n;
  if (ev.target.closest('.twisty') && n.kids) { toggle(n); return; }
  select(n, false);
});
$('rows').addEventListener('dblclick', (ev) => {
  const row = ev.target.closest('.row');
  if (row && row._n.kids) toggle(row._n);
});
$('rows').addEventListener('mouseover', (ev) => {
  const row = ev.target.closest('.row');
  const n = row ? row._n : null;
  if (n !== state.hover) { state.hover = n; requestDraw(); }
});
$('rows').addEventListener('mouseleave', () => { state.hover = null; requestDraw(); });

$('rows').addEventListener('keydown', (ev) => {
  const rows = visibleRows();
  const i = rows.findIndex((r) => r.n === state.selected);
  const n = state.selected;
  if (ev.key === 'ArrowDown' && i < rows.length - 1) select(rows[i + 1].n, false);
  else if (ev.key === 'ArrowUp' && i > 0) select(rows[i - 1].n, false);
  else if (ev.key === 'ArrowRight' && n.kids) { if (!state.expanded.has(n)) toggle(n); else select(n.kids[0], false); }
  else if (ev.key === 'ArrowLeft') { if (n.kids && state.expanded.has(n)) toggle(n); else if (n.parent) select(n.parent, false); }
  else if (ev.key === 'Enter' && n.kids) toggle(n);
  else return;
  ev.preventDefault();
});

function toggle(n) {
  if (state.expanded.has(n)) state.expanded.delete(n); else state.expanded.add(n);
  renderRows();
}

function select(n, reveal) {
  state.selected = n;
  if (reveal || true) for (let p = n.parent; p; p = p.parent) state.expanded.add(p);
  renderRows();
  const row = [...$('rows').children].find((r) => r._n === n);
  row?.scrollIntoView({ block: 'nearest' });
  geo = null;
  renderMapHeader();
  renderLegend();
  renderDetails();
  requestDraw();
}

$('filter').addEventListener('input', (ev) => { state.filter = ev.target.value; renderRows(); });

// ---------------------------------------------------------------------------
// Map: the selected folder as a stem cross-section (or a treemap)
// ---------------------------------------------------------------------------
const canvas = $('dummy-sunburst');
const ctx = canvas.getContext('2d');
let geo = null;
let frame = 0;
let size = { w: 0, h: 0 };
const TAU = Math.PI * 2;
const ROT = -Math.PI / 2;

const mapRoot = () => (state.selected && state.selected.kids ? state.selected : state.selected?.parent || state.tree);

function requestDraw() {
  if (!frame) frame = requestAnimationFrame(() => { frame = 0; draw(); });
}

function layoutSunburst() {
  const root = mapRoot();
  const cx = size.w / 2;
  const cy = size.h / 2;
  const R = Math.min(size.w, size.h) / 2 - 10;
  const maxDepth = 3;
  const r0 = Math.max(46, R * 0.27);
  const ring = (R - r0) / maxDepth;
  const shapes = [];
  (function walk(n, a0, a1, d) {
    if (d > 0) shapes.push({ n, a0, a1, d, ri: r0 + (d - 1) * ring, ro: r0 + d * ring });
    if (!n.kids || d >= maxDepth) return;
    let a = a0;
    for (const k of n.kids) { const s = ((a1 - a0) * k.size) / n.size; walk(k, a, a + s, d + 1); a += s; }
  })(root, 0, TAU, 0);
  return { kind: 'sun', root, cx, cy, r0, shapes };
}

function drawSunburst(g) {
  const gap = cssVar('--gap');
  for (const s of g.shapes) {
    if (s.a1 - s.a0 < 0.002) continue;
    ctx.beginPath();
    ctx.arc(g.cx, g.cy, s.ro, s.a0 + ROT, s.a1 + ROT);
    ctx.arc(g.cx, g.cy, s.ri, s.a1 + ROT, s.a0 + ROT, true);
    ctx.closePath();
    ctx.fillStyle = tint(catColor(s.n.cat), s.d);
    ctx.fill();
    ctx.lineWidth = 1.5;
    ctx.strokeStyle = gap;          // negative space between segments
    ctx.stroke();
    if (s.n === state.hover || s.n === state.selected) {
      ctx.lineWidth = 2;
      ctx.strokeStyle = cssVar('--text');
      ctx.stroke();
    }
  }
  // Labels only where they fit; tangential and upright.
  ctx.font = `11.5px ${cssVar('--font')}`;
  ctx.textAlign = 'center';
  ctx.textBaseline = 'middle';
  for (const s of g.shapes) {
    const rm = (s.ri + s.ro) / 2;
    const len = (s.a1 - s.a0) * rm;
    if (len < 60 || s.ro - s.ri < 15) continue;
    const label = fit(s.n.name, Math.min(len - 12, 180));
    if (!label) continue;
    const mid = (s.a0 + s.a1) / 2 + ROT;
    let rot = Math.atan2(Math.sin(mid + Math.PI / 2), Math.cos(mid + Math.PI / 2));
    if (Math.abs(rot) > Math.PI / 2) rot += Math.PI;
    ctx.save();
    ctx.translate(g.cx + Math.cos(mid) * rm, g.cy + Math.sin(mid) * rm);
    ctx.rotate(rot);
    ctx.fillStyle = inkFor(s.d);
    ctx.fillText(label, 0, 0);
    ctx.restore();
  }
  // Pith with faint growth rings
  ctx.beginPath();
  ctx.arc(g.cx, g.cy, g.r0 - 3, 0, TAU);
  ctx.fillStyle = cssVar('--panel');
  ctx.fill();
  ctx.lineWidth = 1;
  for (let i = 1; i <= 3; i++) {
    ctx.beginPath();
    ctx.arc(g.cx, g.cy, (g.r0 - 3) * (0.35 + i * 0.2), 0, TAU);
    ctx.strokeStyle = cssVar('--line');
    ctx.stroke();
  }
  ctx.fillStyle = cssVar('--text');
  ctx.font = `600 15px ${cssVar('--font')}`;
  ctx.fillText(fmtGB(g.root.size), g.cx, g.cy - 2);
  ctx.fillStyle = cssVar('--muted');
  ctx.font = `11px ${cssVar('--font')}`;
  ctx.fillText(fit(g.root.name, (g.r0 - 10) * 1.6), g.cx, g.cy + 16);
}

// Readable label colour for a segment: white on strong fills and in the
// dark theme; dark ink on the lighter tints of the light theme.
function inkFor(depth) {
  if (state.theme === 'dark' || depth <= 1) return 'rgba(255,255,255,0.92)';
  return 'rgba(20,20,18,0.86)';
}

function fit(text, w) {
  if (w < 20) return '';
  if (ctx.measureText(text).width <= w) return text;
  let lo = 0;
  let hi = text.length;
  while (lo < hi) {
    const m = (lo + hi + 1) >> 1;
    if (ctx.measureText(text.slice(0, m) + '…').width <= w) lo = m; else hi = m - 1;
  }
  return lo > 1 ? text.slice(0, lo) + '…' : '';
}

function squarify(items, x, y, w, h, out) {
  const total = items.reduce((s, k) => s + k.size, 0);
  if (!total || w <= 0 || h <= 0) return;
  const scale = (w * h) / total;
  let i = 0;
  while (i < items.length) {
    const short = Math.min(w, h);
    const maxA = items[i].size * scale;
    let end = i;
    let sum = 0;
    let best = Infinity;
    while (end < items.length) {
      const a = items[end].size * scale;
      const s = sum + a;
      const worst = Math.max((short * short * maxA) / (s * s), (s * s) / (short * short * a));
      if (worst > best) break;
      best = worst; sum = s; end++;
    }
    if (w >= h) {
      const rw = sum / h; let yy = y;
      for (let k = i; k < end; k++) { const rh = (items[k].size * scale) / rw; out.push([items[k], x, yy, rw, rh]); yy += rh; }
      x += rw; w -= rw;
    } else {
      const rh = sum / w; let xx = x;
      for (let k = i; k < end; k++) { const rw = (items[k].size * scale) / rh; out.push([items[k], xx, y, rw, rh]); xx += rw; }
      y += rh; h -= rh;
    }
    i = end;
  }
}

function layoutTreemap() {
  const root = mapRoot();
  const shapes = [];
  const top = [];
  squarify((root.kids || []).filter((k) => k.size > 0), 6, 6, size.w - 12, size.h - 12, top);
  for (const [n, x, y, w, h] of top) {
    shapes.push({ n, x, y, w, h, head: !!n.kids });
    if (n.kids && w > 50 && h > 44) {
      const inner = [];
      squarify(n.kids.filter((k) => k.size > 0), x + 2, y + 20, w - 4, h - 22, inner);
      for (const [k, kx, ky, kw, kh] of inner) shapes.push({ n: k, x: kx, y: ky, w: kw, h: kh, inner: true });
    }
  }
  return { kind: 'tree', root, shapes };
}

function drawTreemap(g) {
  const gap = cssVar('--gap');
  ctx.textAlign = 'left';
  ctx.textBaseline = 'alphabetic';
  for (const s of g.shapes) {
    ctx.fillStyle = tint(catColor(s.n.cat), s.inner ? 2 : 1);
    ctx.fillRect(s.x, s.y, s.w, s.h);
    ctx.lineWidth = 2;
    ctx.strokeStyle = gap;
    ctx.strokeRect(s.x, s.y, s.w, s.h);
    if (s.n === state.hover || s.n === state.selected) {
      ctx.lineWidth = 2;
      ctx.strokeStyle = cssVar('--text');
      ctx.strokeRect(s.x + 1.5, s.y + 1.5, s.w - 3, s.h - 3);
    }
  }
  for (const s of g.shapes) {
    if (s.w < 54 || s.h < 18) continue;
    ctx.fillStyle = inkFor(s.inner ? 2 : 1);
    ctx.font = `${s.inner ? 400 : 600} 11.5px ${cssVar('--font')}`;
    ctx.fillText(fit(s.n.name, s.w - 10), s.x + 6, s.y + 14);
    if (!s.head && s.h > 34) {
      ctx.font = `11px ${cssVar('--font')}`;
      ctx.fillText(fmtGB(s.n.size), s.x + 6, s.y + 29);
    }
  }
}

function draw() {
  ctx.clearRect(0, 0, size.w, size.h);
  if (!state.tree) return;
  if (!geo) geo = state.view === 'sunburst' ? layoutSunburst() : layoutTreemap();
  if (geo.kind === 'sun') drawSunburst(geo); else drawTreemap(geo);
}

function hit(ev) {
  if (!geo) return null;
  const r = canvas.getBoundingClientRect();
  const x = ev.clientX - r.left;
  const y = ev.clientY - r.top;
  if (geo.kind === 'sun') {
    const dx = x - geo.cx;
    const dy = y - geo.cy;
    const rr = Math.hypot(dx, dy);
    if (rr < geo.r0 - 3) return geo.root.parent ? { up: true } : null;
    let a = Math.atan2(dy, dx) - ROT;
    if (a < 0) a += TAU;
    const s = geo.shapes.find((q) => rr >= q.ri && rr < q.ro && a >= q.a0 && a < q.a1);
    return s ? s.n : null;
  }
  for (let i = geo.shapes.length - 1; i >= 0; i--) {
    const s = geo.shapes[i];
    if (x >= s.x && x < s.x + s.w && y >= s.y && y < s.y + s.h) return s.n;
  }
  return null;
}

canvas.addEventListener('mousemove', (ev) => {
  const h = hit(ev);
  const n = h && !h.up ? h : null;
  if (n !== state.hover) {
    state.hover = n;
    for (const row of $('rows').children) row.style.background = row._n === n ? 'var(--hover)' : '';
    requestDraw();
  }
  canvas.style.cursor = h ? 'pointer' : 'default';
  const tip = $('tip');
  if (!h) { tip.classList.add('hidden'); return; }
  tip.innerHTML = h.up
    ? `<b>Up to ${esc(geo.root.parent.name)}</b>`
    : `<b>${esc(n.name)}</b> <span class="num">${fmtGB(n.size)}</span><div class="sub">${esc(CATS[n.cat])} · ${fmtPct(pctOf(n.size, geo.root.size))} of ${esc(geo.root.name)}</div>`;
  tip.classList.remove('hidden');
  tip.style.left = `${Math.min(ev.clientX + 14, innerWidth - tip.offsetWidth - 8)}px`;
  tip.style.top = `${Math.min(ev.clientY + 16, innerHeight - tip.offsetHeight - 8)}px`;
});
canvas.addEventListener('mouseleave', () => {
  state.hover = null;
  $('tip').classList.add('hidden');
  for (const row of $('rows').children) row.style.background = '';
  requestDraw();
});
canvas.addEventListener('click', (ev) => {
  const h = hit(ev);
  if (!h) return;
  if (h.up) select(geo.root.parent, true); else select(h, true);
});

new ResizeObserver(() => {
  const r = $('map').getBoundingClientRect();
  const dpr = window.devicePixelRatio || 1;
  size = { w: r.width, h: r.height };
  canvas.width = Math.round(r.width * dpr);
  canvas.height = Math.round(r.height * dpr);
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  geo = null;
  requestDraw();
}).observe($('map'));

// ---------------------------------------------------------------------------
// Map header, legend, details
// ---------------------------------------------------------------------------
function renderMapHeader() {
  const root = mapRoot();
  $('mapTitle').textContent = root.name;
  $('mapSub').textContent = `${fmtGB(root.size)} · ${fmtN(root.files)} files`;
  $('winTitle').textContent = `Xylem: ${root.path}`;
}

function renderLegend() {
  const root = mapRoot();
  const sums = {};
  (function walk(n) { if (n.kids) n.kids.forEach(walk); else sums[n.cat] = (sums[n.cat] || 0) + n.size; })(root);
  const entries = Object.entries(sums).sort((a, b) => b[1] - a[1]).slice(0, 8);
  $('legend').innerHTML = entries.map(([cat, v]) =>
    `<div class="li" data-cat="${cat}"><span class="sw" style="background:${catColor(cat)}"></span>` +
    `<span class="nm">${esc(CATS[cat])}</span><span class="v num">${fmtGB(v)} · ${fmtPct(pctOf(v, root.size))}</span></div>`).join('');
}

$('legend').addEventListener('click', (ev) => {
  const li = ev.target.closest('.li');
  if (!li) return;
  setOption('group', 'category');
  const c = CATEGORY.kids.find((k) => k.cat === li.dataset.cat);
  if (c) select(c, true);
});

function renderDetails() {
  const n = state.selected;
  const el = $('details');
  if (!n || !n.parent) {
    el.innerHTML = '<div class="empty">Select a folder or file to see what it is and whether it\'s safe to remove.</div>';
    return;
  }
  const rule = ruleFor(n);
  const [level, note] = rule || [null, 'No rule for this yet. Check what it is before removing it.'];
  const label = { green: 'Safe to clear', yellow: 'Check first', red: "Don't delete by hand" }[level] || 'Unknown';
  const locked = level === 'red';
  el.innerHTML =
    `<dl class="props">` +
    `<dt>Name</dt><dd><b>${esc(n.name)}</b></dd>` +
    `<dt>Location</dt><dd title="${esc(n.path)}">${esc(n.ref ? n.ref.path : n.path)}</dd>` +
    `<dt>Size on disk</dt><dd class="num">${fmtGB(n.size)} · ${fmtN(n.files)} files · ${fmtPct(pctOf(n.size, n.parent.size))} of ${esc(n.parent.name)}</dd>` +
    `<dt>Category</dt><dd><span class="sw" style="background:${catColor(n.cat)}"></span>${esc(CATS[n.cat])}</dd>` +
    `<dt>Safe to delete?</dt><dd><span class="safety ${level || ''}">${label}</span> <span class="muted">· ${esc(note)}</span></dd>` +
    `</dl>` +
    `<div class="actions">` +
    `<button type="button" class="btn" onclick="status('Would open Explorer here')">Show in Explorer</button>` +
    `<button type="button" class="btn danger" ${locked ? 'disabled title="Locked: system item"' : ''} onclick="status('Would ask, then move to the Recycle Bin')">Move to Recycle Bin…</button>` +
    `</div>`;
}

// ---------------------------------------------------------------------------
// Start
// ---------------------------------------------------------------------------
function renderAll() {
  renderRows();
  renderMapHeader();
  renderLegend();
  renderDetails();
  geo = null;
  requestDraw();
}

applyTheme();
syncChecks();
useTree(state.group === 'category' ? CATEGORY : LOCATION);
// Start with something selected so every panel shows content.
const users = LOCATION.kids.find((k) => k.name === 'Users');
if (state.group === 'location' && users) {
  const you = users.kids[0];
  state.expanded.add(users);
  state.expanded.add(you);
  select(you, true);
}
if (params.has('about')) $('dlgAbout').classList.remove('hidden');
window.addEventListener('keydown', (ev) => {
  if (ev.key === 'Escape') document.querySelectorAll('.modal').forEach((m) => m.classList.add('hidden'));
  if (ev.key === 'F5') { ev.preventDefault(); fakeScan(ev.ctrlKey); }
  if (ev.key === 'Backspace' && document.activeElement !== $('filter')) action('up');
});
