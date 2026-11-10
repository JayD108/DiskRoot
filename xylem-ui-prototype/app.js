'use strict';
/* Xylem UI prototype — static demo data, no backend.
   Double-click index.html to open. Nothing here touches the network. */

// ---------------------------------------------------------------------------
// Demo data (sizes in GB; names are generic examples)
// ---------------------------------------------------------------------------
const CATS = {
  ai: { name: 'AI & data', css: '--c-ai' },
  apps: { name: 'Apps & Games', css: '--c-apps' },
  system: { name: 'System', css: '--c-system' },
  other: { name: 'Other', css: '--c-other' },
  temp: { name: 'Temp & caches', css: '--c-temp' },
  dev: { name: 'Dev stuff', css: '--c-dev' },
  docs: { name: 'Documents', css: '--c-docs' },
  down: { name: 'Downloads', css: '--c-down' },
  media: { name: 'Media', css: '--c-media' },
  cloud: { name: 'Cloud-synced', css: '--c-cloud' },
};

const NOTES = {
  'node_modules': ['green', 'JavaScript dependencies of a project. Safe to delete; npm install brings them back.'],
  'Hugging Face cache': ['yellow', 'Downloaded models and datasets. Remove unused ones with huggingface-cli delete-cache.'],
  'pagefile.sys': ['red', 'Virtual memory. Its size is managed in System settings, not by deleting it.'],
  'WinSxS': ['red', 'Windows component store. Much of its size is shared with C:\\Windows (hardlinks). Shrink with DISM cleanup.'],
  'User temp': ['green', 'Temporary files. Anything not in use is safe to delete.'],
  'GPU shader cache': ['green', 'Pre-compiled shaders. Rebuilt automatically.'],
  'Games (Steam)': ['yellow', 'Installed games. Uninstall from Steam so it knows.'],
  'Datasets': ['yellow', 'Data files (.npy, .h5, .parquet…) found by extension. Check before removing.'],
  'Model files': ['yellow', 'Model weights (.safetensors, .gguf, .pt…). Large and often re-downloadable.'],
  'Recycle Bin': ['green', 'Deleted files waiting in the Recycle Bin. Empty it from the bin itself.'],
};

const n = (name, size, kids) => ({ name, size, kids });

const BY_CATEGORY = n('C:\\ by category', 0, [
  { cat: 'ai', ...n('AI & data', 0, [
    n('Datasets', 0, [n('satellite-2019', 182), n('eeg-recordings', 96), n('wavelet-cache', 71.5), n('2077 more places', 260)]),
    n('Model files', 0, [n('checkpoints', 21), n('llm-weights', 12.4), n('837 more places', 4.3)]),
    n('Hugging Face cache', 0, [n('hub', 4.9), n('datasets', 1.4)]),
  ]) },
  { cat: 'apps', ...n('Apps & Games', 0, [
    n('Games (Steam)', 0, [n('Open-world RPG', 96), n('Survival shooter', 72), n('Racing sim', 61), n('more games', 59)]),
    n('Games (Epic)', 0, [n('Action adventure', 151), n('Puzzle game', 26)]),
    n('Installed programs', 0, [n('Program Files', 22.6), n('Program Files (x86)', 11.8)]),
    n('Per-user programs', 0, [n('Programs', 16.3)]),
    n('App data', 0, [n('Local', 10.3), n('Roaming', 2.9)]),
  ]) },
  { cat: 'system', ...n('System', 0, [
    n('pagefile.sys', 0, [n('pagefile.sys', 22)]),
    n('WinSxS', 0, [n('WinSxS', 15.9)]),
    n('Hibernation file', 0, [n('hiberfil.sys', 12.5)]),
    n('Windows', 0, [n('Windows', 13)]),
    n('Restore points', 0, [n('System Volume Information', 10.2)]),
    n('Installer cache', 0, [n('Installer', 3.5)]),
    n('NTFS metadata', 0, [n('$MFT', 1.7)]),
  ]) },
  { cat: 'other', ...n('Other', 0, [
    n('Archives & disk images', 0, [n('research-archive.zip', 36.4), n('iso images', 3.9)]),
    n('Unclassified', 0, [n('LLM', 13.8), n('scratch', 9)]),
    n('Installers', 0, [n('setup files', 12.6)]),
  ]) },
  { cat: 'temp', ...n('Temp & caches', 0, [
    n('User temp', 0, [n('Temp', 33)]),
    n('App caches', 0, [n('492 places', 14.4)]),
    n('Recycle Bin', 0, [n('$Recycle.Bin', 9.4)]),
    n('GPU shader cache', 0, [n('DXCache', 8.1), n('GLCache', 2.2)]),
    n('Crash dumps', 0, [n('CrashDumps', 3)]),
  ]) },
  { cat: 'dev', ...n('Dev stuff', 0, [
    n('Package caches', 0, [n('pip cache', 7), n('npm-cache', 4), n('cargo registry', 2.1)]),
    n('Android emulators', 0, [n('avd', 7.4)]),
    n('Docker', 0, [n('docker_data.vhdx', 7.3)]),
    n('node_modules', 0, [n('web-app/node_modules', 3.2), n('api/node_modules', 2.6), n('140 more', 6.9)]),
    n('Python environments', 0, [n('.venv', 4.4), n('miniconda3', 3.1)]),
    n('Git repositories', 0, [n('.git (12 repos)', 2.3)]),
    n('Gradle', 0, [n('.gradle', 1.6)]),
  ]) },
  { cat: 'docs', ...n('Documents', 0, [
    n('Documents', 0, [n('corpus-english', 19.5), n('corpus-hindi', 8.8), n('papers', 8.4)]),
    n('Desktop', 0, [n('Desktop', 4.3)]),
  ]) },
  { cat: 'down', ...n('Downloads', 0, [n('Downloads', 0, [n('Downloads', 21.4)])]) },
  { cat: 'media', ...n('Media', 0, [n('Photos', 0, [n('Pictures', 9.1), n('Screenshots', 2.4)])]) },
]);

const BY_LOCATION = n('C:\\', 0, [
  n('Users', 0, [
    { cat: 'ai', ...n('you', 0, [
      { cat: 'ai', ...n('Desktop', 0, [{ cat: 'ai', ...n('satellite-2019', 182) }, { cat: 'docs', ...n('notes', 20) }]) },
      { cat: 'temp', ...n('AppData', 0, [{ cat: 'temp', ...n('Temp', 33) }, { cat: 'apps', ...n('Programs', 16.3) }, { cat: 'temp', ...n('Caches', 28) }, { cat: 'dev', ...n('Docker', 7.3) }]) },
      { cat: 'down', ...n('Downloads', 21.4) },
      { cat: 'ai', ...n('.ollama', 4.9) },
      { cat: 'dev', ...n('projects', 0, [{ cat: 'dev', ...n('node_modules', 12.7) }, { cat: 'dev', ...n('.venv', 4.4) }, { cat: 'dev', ...n('src', 1.2) }]) },
    ]) },
  ]),
  { cat: 'ai', ...n('datasets', 0, [{ cat: 'ai', ...n('eeg-recordings', 96) }, { cat: 'ai', ...n('wavelet-cache', 71.5) }, { cat: 'ai', ...n('raw', 140) }]) },
  { cat: 'apps', ...n('Program Files (x86)', 0, [{ cat: 'apps', ...n('Steam', 288) }, { cat: 'apps', ...n('Other apps', 11.8) }]) },
  { cat: 'apps', ...n('Program Files', 0, [{ cat: 'apps', ...n('Epic Games', 177) }, { cat: 'apps', ...n('Other apps', 22.6) }]) },
  { cat: 'system', ...n('Windows', 0, [{ cat: 'system', ...n('WinSxS', 15.9) }, { cat: 'system', ...n('System32', 13.1) }, { cat: 'system', ...n('Installer', 3.5) }]) },
  { cat: 'system', ...n('pagefile.sys', 22) },
  { cat: 'system', ...n('hiberfil.sys', 12.5) },
  { cat: 'other', ...n('archive', 40.3) },
  { cat: 'temp', ...n('$Recycle.Bin', 9.4) },
]);

// Fill folder sizes from children, propagate categories down, link parents.
function prepare(node, parent = null, cat = null) {
  node.parent = parent;
  node.cat = node.cat || cat;
  if (node.kids) {
    node.size = 0;
    for (const k of node.kids) node.size += prepare(k, node, node.cat).size;
    node.kids.sort((a, b) => b.size - a.size);
  }
  return node;
}
prepare(BY_CATEGORY);
prepare(BY_LOCATION);
// In the category tree, every node below a category inherits its colour.
for (const c of BY_CATEGORY.kids) (function tag(x, cat) { x.cat = cat; (x.kids || []).forEach((k) => tag(k, cat)); })(c, c.cat);

// ---------------------------------------------------------------------------
// State + helpers
// ---------------------------------------------------------------------------
const $ = (id) => document.getElementById(id);
const canvas = $('dummy-sunburst');
const ctx = canvas.getContext('2d');
const css = getComputedStyle(document.documentElement);
const cssVar = (v) => css.getPropertyValue(v).trim();
const BG = cssVar('--canvas');
const FONT = cssVar('--font');
const MONO = cssVar('--mono');

const state = {
  group: 'category',
  view: 'sunburst',
  root: BY_CATEGORY,
  shapes: [],       // laid-out shapes for hit-testing
  hover: null,
  selected: null,
  unfurl: 1,        // 0..1 animation progress
  w: 0,
  h: 0,
};

const store = {
  get(k) { try { return localStorage.getItem(k); } catch { return null; } },
  set(k, v) { try { localStorage.setItem(k, v); } catch { /* private mode */ } },
};

function fmtGB(gb) {
  if (gb >= 1024) return `${(gb / 1024).toFixed(2)} TB`;
  if (gb >= 100) return `${gb.toFixed(0)} GB`;
  if (gb >= 10) return `${gb.toFixed(1)} GB`;
  if (gb >= 1) return `${gb.toFixed(2)} GB`;
  return `${(gb * 1024).toFixed(0)} MB`;
}
const pct = (a, b) => { const p = (a / b) * 100; return `${p >= 10 ? p.toFixed(0) : p.toFixed(1)}%`; };
const esc = (s) => s.replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));

function catColor(cat) { return cat ? cssVar(CATS[cat].css) : cssVar('--c-other'); }

// Mix a hex colour toward the canvas (deeper rings get slightly richer/darker).
function shade(hex, depth, sib) {
  const m = /^#(..)(..)(..)$/.exec(hex);
  if (!m) return hex;
  const k = Math.max(0.52, 1 - (depth - 1) * 0.11) - (sib % 3) * 0.04;
  const [r, g, b] = [m[1], m[2], m[3]].map((x) => Math.round(parseInt(x, 16) * k + 9 * (1 - k)));
  return `rgb(${r},${g},${b})`;
}

function fitText(text, maxW) {
  if (maxW < 22) return '';
  if (ctx.measureText(text).width <= maxW) return text;
  let lo = 0;
  let hi = text.length;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (ctx.measureText(text.slice(0, mid) + '…').width <= maxW) lo = mid; else hi = mid - 1;
  }
  return lo > 1 ? text.slice(0, lo) + '…' : '';
}

// ---------------------------------------------------------------------------
// Sunburst: a stem cross-section. Pith in the centre, growth rings outward.
// ---------------------------------------------------------------------------
const TAU = Math.PI * 2;
const ROT = -Math.PI / 2;

function layoutSunburst() {
  const { w, h } = state;
  const cx = w / 2;
  const cy = h / 2;
  const R = Math.min(w, h) / 2 - 18;
  const maxDepth = 3;
  const r0 = Math.max(64, R * 0.24);
  const ring = (R - r0) / maxDepth;
  const shapes = [];
  (function walk(node, a0, a1, depth, sib) {
    if (depth > 0) shapes.push({ kind: 'arc', node, a0, a1, depth, sib, ri: r0 + (depth - 1) * ring + 1, ro: r0 + depth * ring - 1 });
    if (depth >= maxDepth || !node.kids) return;
    let a = a0;
    node.kids.forEach((k, i) => {
      const span = ((a1 - a0) * k.size) / node.size;
      walk(k, a, a + span, depth + 1, i);
      a += span;
    });
  })(state.root, 0, TAU, 0, 0);
  return { cx, cy, r0, ring, R, shapes };
}

function drawSunburst(g) {
  const T = TAU * easeOut(state.unfurl); // the canopy unfurls clockwise
  ctx.lineWidth = 2;
  ctx.strokeStyle = BG; // separation by negative space, never black lines
  for (const s of g.shapes) {
    if (s.a0 >= T) continue;
    const a1 = Math.min(s.a1, T);
    if (a1 - s.a0 < 0.003) continue;
    ctx.beginPath();
    ctx.arc(g.cx, g.cy, s.ro, s.a0 + ROT, a1 + ROT);
    ctx.arc(g.cx, g.cy, s.ri, a1 + ROT, s.a0 + ROT, true);
    ctx.closePath();
    const hot = state.hover === s.node || state.selected === s.node;
    ctx.fillStyle = shade(catColor(s.node.cat), s.depth, s.sib);
    ctx.fill();
    ctx.stroke();
    if (hot) {
      ctx.fillStyle = 'rgba(255,255,255,0.16)';
      ctx.fill();
    }
  }

  // Labels: tangential, flipped on the lower half so they read upright.
  if (state.unfurl >= 1) {
    ctx.textAlign = 'center';
    ctx.textBaseline = 'middle';
    for (const s of g.shapes) {
      const rm = (s.ri + s.ro) / 2;
      const arcLen = (s.a1 - s.a0) * rm;
      if (arcLen < 52 || s.ro - s.ri < 16) continue;
      ctx.font = `${s.depth === 1 ? 600 : 500} ${s.depth === 1 ? 13 : 12}px ${FONT}`;
      const label = fitText(s.node.name, Math.min(arcLen - 14, 230));
      if (!label) continue;
      const mid = (s.a0 + s.a1) / 2 + ROT;
      let rot = mid + Math.PI / 2;
      rot = Math.atan2(Math.sin(rot), Math.cos(rot));
      if (Math.abs(rot) > Math.PI / 2) rot += Math.PI; // bottom half: turn 180°
      ctx.save();
      ctx.translate(g.cx + Math.cos(mid) * rm, g.cy + Math.sin(mid) * rm);
      ctx.rotate(rot);
      ctx.fillStyle = 'rgba(6, 8, 12, 0.86)';
      ctx.fillText(label, 0, 0);
      ctx.restore();
    }
  }

  // The pith: current folder, total, and a growth-ring outline.
  ctx.beginPath();
  ctx.arc(g.cx, g.cy, g.r0 - 6, 0, TAU);
  ctx.fillStyle = state.hover === 'pith' ? '#18181d' : '#111114';
  ctx.fill();
  for (let i = 1; i <= 3; i++) {
    ctx.beginPath();
    ctx.arc(g.cx, g.cy, (g.r0 - 6) * (0.42 + i * 0.16), 0, TAU);
    ctx.strokeStyle = `rgba(255,255,255,${0.025 + i * 0.008})`;
    ctx.lineWidth = 1;
    ctx.stroke();
  }
  ctx.textAlign = 'center';
  ctx.textBaseline = 'middle';
  ctx.fillStyle = cssVar('--muted');
  ctx.font = `500 12px ${FONT}`;
  ctx.fillText(fitText(state.root.name, (g.r0 - 14) * 1.6), g.cx, g.cy - 22);
  ctx.fillStyle = cssVar('--text');
  ctx.font = `600 24px ${MONO}`;
  ctx.fillText(fmtGB(state.root.size), g.cx, g.cy + 4);
  ctx.fillStyle = cssVar('--faint');
  ctx.font = `11px ${FONT}`;
  ctx.fillText(state.root.parent ? '↑ up' : 'tap a ring to go deeper', g.cx, g.cy + 28);
}

function hitSunburst(g, x, y) {
  const dx = x - g.cx;
  const dy = y - g.cy;
  const r = Math.hypot(dx, dy);
  if (r < g.r0 - 6) return 'pith';
  let a = Math.atan2(dy, dx) - ROT;
  if (a < 0) a += TAU;
  for (const s of g.shapes) if (r >= s.ri && r <= s.ro && a >= s.a0 && a < s.a1) return s.node;
  return null;
}

// ---------------------------------------------------------------------------
// Treemap: squarified, two levels, gaps as negative space.
// ---------------------------------------------------------------------------
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
      best = worst;
      sum = s;
      end++;
    }
    if (w >= h) {
      const rw = sum / h;
      let yy = y;
      for (let k = i; k < end; k++) { const rh = (items[k].size * scale) / rw; out.push([items[k], x, yy, rw, rh]); yy += rh; }
      x += rw; w -= rw;
    } else {
      const rh = sum / w;
      let xx = x;
      for (let k = i; k < end; k++) { const rw = (items[k].size * scale) / rh; out.push([items[k], xx, y, rw, rh]); xx += rw; }
      y += rh; h -= rh;
    }
    i = end;
  }
}

function layoutTreemap() {
  const shapes = [];
  const pad = 14;
  const top = [];
  squarify(state.root.kids || [], pad, pad, state.w - pad * 2, state.h - pad * 2, top);
  for (const [node, x, y, w, h] of top) {
    shapes.push({ kind: 'rect', node, x, y, w, h, depth: 1, sib: 0, header: true });
    const inner = [];
    if (node.kids && w > 60 && h > 60) squarify(node.kids, x + 3, y + 24, w - 6, h - 27, inner);
    inner.forEach(([k, kx, ky, kw, kh], i) => shapes.push({ kind: 'rect', node: k, x: kx, y: ky, w: kw, h: kh, depth: 2, sib: i }));
  }
  return { shapes };
}

function drawTreemap(g) {
  const t = easeOut(state.unfurl);
  ctx.textBaseline = 'alphabetic';
  ctx.textAlign = 'left';
  for (const s of g.shapes) {
    const color = catColor(s.node.cat);
    const h = s.h * t;
    if (s.header) {
      ctx.fillStyle = shade(color, 3, 1);
      roundRect(s.x + 1, s.y + 1, s.w - 2, h - 2, 6);
      ctx.fill();
      ctx.fillStyle = shade(color, 1, 0);
      roundRect(s.x + 1, s.y + 1, s.w - 2, Math.min(22, h - 2), 6);
      ctx.fill();
    } else {
      ctx.fillStyle = shade(color, 2, s.sib);
      roundRect(s.x + 1, s.y + 1, s.w - 2, h - 2, 4);
      ctx.fill();
    }
    if (state.hover === s.node || state.selected === s.node) {
      ctx.fillStyle = 'rgba(255,255,255,0.14)';
      roundRect(s.x + 1, s.y + 1, s.w - 2, h - 2, 5);
      ctx.fill();
    }
  }
  if (t < 1) return;
  for (const s of g.shapes) {
    ctx.fillStyle = 'rgba(6, 8, 12, 0.88)';
    if (s.header && s.w > 70) {
      ctx.font = `600 12px ${FONT}`;
      const size = fmtGB(s.node.size);
      ctx.font = `500 11px ${MONO}`;
      const sw = ctx.measureText(size).width;
      ctx.fillText(size, s.x + s.w - sw - 8, s.y + 16);
      ctx.font = `600 12px ${FONT}`;
      ctx.fillText(fitText(s.node.name, s.w - sw - 22), s.x + 8, s.y + 16);
    } else if (!s.header && s.w > 56 && s.h > 34) {
      ctx.font = `500 12px ${FONT}`;
      ctx.fillText(fitText(s.node.name, s.w - 12), s.x + 7, s.y + 18);
      ctx.font = `11px ${MONO}`;
      ctx.fillText(fmtGB(s.node.size), s.x + 7, s.y + 33);
    }
  }
}

function roundRect(x, y, w, h, r) {
  r = Math.max(0, Math.min(r, w / 2, h / 2));
  ctx.beginPath();
  ctx.moveTo(x + r, y);
  ctx.arcTo(x + w, y, x + w, y + h, r);
  ctx.arcTo(x + w, y + h, x, y + h, r);
  ctx.arcTo(x, y + h, x, y, r);
  ctx.arcTo(x, y, x + w, y, r);
  ctx.closePath();
}

function hitTreemap(g, x, y) {
  for (let i = g.shapes.length - 1; i >= 0; i--) {
    const s = g.shapes[i];
    if (x >= s.x && x < s.x + s.w && y >= s.y && y < s.y + s.h) return s.node;
  }
  return null;
}

// ---------------------------------------------------------------------------
// Render loop: draws only when something changed (idle = zero CPU)
// ---------------------------------------------------------------------------
let geo = null;
let frame = 0;
const easeOut = (t) => 1 - Math.pow(1 - Math.min(1, t), 3);

function requestDraw() {
  if (!frame) frame = requestAnimationFrame(() => { frame = 0; draw(); });
}

function draw() {
  ctx.clearRect(0, 0, state.w, state.h);
  if (!geo) geo = state.view === 'sunburst' ? layoutSunburst() : layoutTreemap();
  if (state.view === 'sunburst') drawSunburst(geo); else drawTreemap(geo);
}

function unfurl() {
  // ?still (or the OS "reduce motion" setting) skips the animation.
  const still = new URLSearchParams(location.search).has('still');
  if (still || matchMedia('(prefers-reduced-motion: reduce)').matches) { state.unfurl = 1; requestDraw(); return; }
  const start = performance.now();
  const dur = 750;
  state.unfurl = 0;
  const step = (now) => {
    state.unfurl = Math.min(1, (now - start) / dur);
    draw();
    if (state.unfurl < 1) requestAnimationFrame(step);
  };
  requestAnimationFrame(step);
}

function resize() {
  const r = $('stage').getBoundingClientRect();
  const dpr = window.devicePixelRatio || 1;
  state.w = r.width;
  state.h = r.height;
  canvas.width = Math.round(r.width * dpr);
  canvas.height = Math.round(r.height * dpr);
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  geo = null;
  requestDraw();
}

// ---------------------------------------------------------------------------
// Sidebar, crumbs, inspector, tooltip
// ---------------------------------------------------------------------------
function crumbPath(node) {
  const out = [];
  for (let x = node; x; x = x.parent) out.unshift(x);
  return out;
}

function render() {
  geo = null;
  const root = state.root;
  // Crumbs
  const ol = $('crumbs');
  ol.replaceChildren(...crumbPath(root).map((x) => {
    const li = document.createElement('li');
    const b = document.createElement('button');
    b.type = 'button';
    b.textContent = x.name;
    b.onclick = () => go(x);
    li.append(b);
    return li;
  }));
  // Head
  $('sideTitle').textContent = root.name;
  $('sideSub').textContent = `${fmtGB(root.size)} · ${(root.kids || []).length} items`;
  // Tissue strip: share per category inside the current root
  const byCat = {};
  (function sum(x) { if (!x.kids) { byCat[x.cat] = (byCat[x.cat] || 0) + x.size; return; } x.kids.forEach(sum); })(root);
  $('tissue').replaceChildren(...Object.entries(byCat).sort((a, b) => b[1] - a[1]).map(([cat, v]) => {
    const i = document.createElement('i');
    i.style.flex = String(v);
    i.style.background = catColor(cat);
    i.title = `${CATS[cat] ? CATS[cat].name : cat}: ${fmtGB(v)}`;
    return i;
  }));
  // Ranked list
  $('rank').replaceChildren(...(root.kids || []).map((k) => {
    const li = document.createElement('li');
    const c = catColor(k.cat);
    li.innerHTML =
      `<span class="dot" style="background:${c}"></span>` +
      `<span class="name">${esc(k.name)}</span>` +
      `<span class="size num">${fmtGB(k.size)}<span class="pct">${pct(k.size, root.size)}</span></span>` +
      `<span class="bar"><i style="width:${((k.size / root.size) * 100).toFixed(2)}%;background:${c}"></i></span>`;
    li._node = k;
    li.onclick = () => (k.kids ? go(k) : select(k));
    li.onmouseenter = () => { state.hover = k; requestDraw(); };
    li.onmouseleave = () => { state.hover = null; requestDraw(); };
    if (state.selected === k) li.classList.add('sel');
    return li;
  }));
  inspect(state.selected);
  requestDraw();
}

function go(node) {
  if (!node || !node.kids) return;
  state.root = node;
  state.hover = null;
  render();
  unfurl();
}

function select(node) {
  state.selected = node;
  for (const li of $('rank').children) li.classList.toggle('sel', li._node === node);
  inspect(node);
  requestDraw();
}

function noteFor(node) {
  for (let x = node; x; x = x.parent) if (NOTES[x.name]) return NOTES[x.name];
  return null;
}

function kindOf(node) {
  const path = crumbPath(node).slice(1).map((x) => x.name);
  const cat = CATS[node.cat] ? CATS[node.cat].name : 'Other';
  return state.group === 'category' ? path.slice(0, 2).join(' › ') : cat;
}

function inspect(node) {
  const el = $('inspector');
  if (!node) { el.innerHTML = '<div class="insp-empty">Select something to see what it is and whether it\'s safe to remove.</div>'; return; }
  const note = noteFor(node);
  const [level, text] = note || ['yellow', 'Xylem has no rule for this yet. Check what it is before removing it.'];
  const label = { green: 'Safe to clear', yellow: 'Check first', red: "Don't delete by hand" }[level];
  el.innerHTML =
    `<div class="insp-title"><span class="dot" style="background:${catColor(node.cat)}"></span>${esc(node.name)}</div>` +
    `<div class="insp-kind">${esc(kindOf(node))} · <span class="num">${fmtGB(node.size)}</span></div>` +
    `<div class="insp-note">${esc(text)}</div>` +
    `<div class="insp-row"><span class="badge ${level}">${label}</span>` +
    `<span><button class="btn ghost mini" type="button">Show in Explorer</button> ` +
    `<button class="btn mini danger" type="button" ${level === 'red' ? 'disabled title="Locked: system item"' : ''}>Move to Recycle Bin</button></span></div>`;
}

function showTip(node, x, y) {
  const tip = $('tip');
  if (!node) { tip.classList.add('hidden'); return; }
  if (node === 'pith') {
    if (!state.root.parent) { tip.classList.add('hidden'); return; }
    tip.innerHTML = `<div class="t1">${esc(state.root.parent.name)}</div><div class="t2">Click to go up</div>`;
  } else {
    tip.innerHTML =
      `<div class="t1"><span class="dot" style="background:${catColor(node.cat)}"></span>${esc(node.name)}</div>` +
      `<div class="t2">${esc(kindOf(node))}</div>` +
      `<div class="row"><span>On disk</span><b class="num">${fmtGB(node.size)}</b></div>` +
      `<div class="row"><span>Share of ${esc(state.root.name)}</span><b class="num">${pct(node.size, state.root.size)}</b></div>`;
  }
  tip.classList.remove('hidden');
  const r = tip.getBoundingClientRect();
  tip.style.left = `${Math.min(x + 16, innerWidth - r.width - 8)}px`;
  tip.style.top = `${Math.min(y + 16, innerHeight - r.height - 8)}px`;
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------
function hit(ev) {
  if (!geo) return null;
  const r = canvas.getBoundingClientRect();
  const x = ev.clientX - r.left;
  const y = ev.clientY - r.top;
  return state.view === 'sunburst' ? hitSunburst(geo, x, y) : hitTreemap(geo, x, y);
}

canvas.addEventListener('mousemove', (ev) => {
  const h = hit(ev);
  if (h !== state.hover) {
    state.hover = h;
    canvas.style.cursor = h ? 'pointer' : 'default';
    for (const li of $('rank').children) li.classList.toggle('hot', li._node === h);
    requestDraw();
  }
  showTip(h, ev.clientX, ev.clientY);
});
canvas.addEventListener('mouseleave', () => { state.hover = null; showTip(null); requestDraw(); });
canvas.addEventListener('click', (ev) => {
  const h = hit(ev);
  if (h === 'pith') { if (state.root.parent) go(state.root.parent); return; }
  if (!h) return;
  if (h.kids) go(h); else select(h);
});

for (const b of document.querySelectorAll('[data-group]')) {
  b.onclick = () => {
    state.group = b.dataset.group;
    document.querySelectorAll('[data-group]').forEach((x) => x.setAttribute('aria-pressed', String(x === b)));
    state.selected = null;
    go(state.group === 'category' ? BY_CATEGORY : BY_LOCATION);
  };
}
for (const b of document.querySelectorAll('[data-view]')) {
  b.onclick = () => {
    state.view = b.dataset.view;
    document.querySelectorAll('[data-view]').forEach((x) => x.setAttribute('aria-pressed', String(x === b)));
    geo = null;
    unfurl();
  };
}

// Fake fast scan: the sap rises, then the canopy unfurls again.
function demoScan(fast) {
  const overlay = $('scanOverlay');
  const sap = $('sap');
  $('scanTitle').textContent = fast ? 'Drawing up from the roots…' : 'Walking the folders…';
  $('scanSub').textContent = fast ? 'reading Master File Table · read-only' : 'no admin needed · slower';
  overlay.classList.remove('hidden');
  const start = performance.now();
  const dur = fast ? 1480 : 3200;
  const step = (now) => {
    const t = Math.min(1, (now - start) / dur);
    sap.style.height = `${(easeOut(t) * 100).toFixed(1)}%`;
    if (t < 1) { requestAnimationFrame(step); return; }
    overlay.classList.add('hidden');
    sap.style.height = '0%';
    unfurl();
  };
  requestAnimationFrame(step);
}
$('fastBtn').onclick = () => demoScan(true);
$('scanBtn').onclick = () => demoScan(false);

// Modals
const open = (id) => { $(id).classList.remove('hidden'); $(id).querySelector('button').focus(); };
const close = (id) => $(id).classList.add('hidden');
$('shieldBtn').onclick = () => open('shield');
$('shieldClose').onclick = () => close('shield');
$('consentOk').onclick = () => { store.set('xylem-consent', '1'); close('consent'); unfurl(); };
$('welcomeLink').onclick = () => open('consent');
window.addEventListener('keydown', (ev) => {
  if (ev.key === 'Escape') { close('shield'); }
  if (ev.key === 'Backspace' && state.root.parent && document.querySelector('.modal:not(.hidden)') === null) go(state.root.parent);
});

// ---------------------------------------------------------------------------
// Start
// ---------------------------------------------------------------------------
new ResizeObserver(resize).observe($('stage'));
render();
// ?nowelcome skips the first-launch screen (handy for screenshots).
const skipWelcome = new URLSearchParams(location.search).has('nowelcome');
if (!store.get('xylem-consent') && !skipWelcome) open('consent'); else unfurl();
