'use strict';

// ---------------------------------------------------------------------------
// Backend bridge
// ---------------------------------------------------------------------------
const tauri = window.__TAURI__;
const invoke = (cmd, args) =>
  tauri ? tauri.core.invoke(cmd, args) : Promise.reject(new Error('Open this page inside the Xylem app.'));

const $ = (id) => document.getElementById(id);
const canvas = $('canvas');
const ctx = canvas.getContext('2d');

const store = {
  get(k) { try { return localStorage.getItem(k); } catch { return null; } },
  set(k, v) { try { localStorage.setItem(k, v); } catch { /* storage unavailable */ } },
};

const state = {
  summary: null,
  root: 0,          // index node id at the centre of the view
  crumbs: [],
  view: [],         // pruned nodes from the backend (parents before children)
  kids: [],         // view index -> child view indices
  branch: [],       // view index -> top-level branch number (for colour)
  sib: [],          // view index -> position among its siblings
  byDepth: [],      // depth -> view indices
  list: [],
  listTotal: 0,
  mode: store.get('mode') === 'treemap' ? 'treemap' : 'sunburst',
  cmode: store.get('cmode') === 'category' ? 'category' : 'location', // group by
  catKey: '',       // by-category view root ('' = everything)
  catCrumbs: [],
  breakdown: [],    // [category, bytes] inside the current folder
  tax: null,        // category / sub-type / rule names from the backend
  hoverKey: null,   // category key hovered in the side list
  hover: -1,        // view index under the mouse, -2 = centre, -1 = none
  hoverId: null,    // index id hovered in the side list
  geo: null,
  busy: false,
  size: { w: 0, h: 0 },
};

// ---------------------------------------------------------------------------
// Formatting
// ---------------------------------------------------------------------------
function fmtBytes(n) {
  const units = ['B', 'KB', 'MB', 'GB', 'TB', 'PB'];
  if (n < 1024) return `${n} B`;
  let v = n;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) { v /= 1024; i++; }
  const s = v >= 100 ? v.toFixed(0) : v >= 10 ? v.toFixed(1) : v.toFixed(2);
  return `${s} ${units[i]}`;
}
const fmtCount = (n) => n.toLocaleString('en-US');
const fmtFiles = (n) => `${fmtCount(n)} ${n === 1 ? 'file' : 'files'}`;
function fmtPct(part, whole) {
  if (!whole) return '0%';
  const p = (part / whole) * 100;
  return `${p >= 10 ? p.toFixed(0) : p >= 1 ? p.toFixed(1) : p.toFixed(2)}%`;
}
function escapeHtml(s) {
  return s.replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
}

// ---------------------------------------------------------------------------
// Theme + colours
// ---------------------------------------------------------------------------
const darkQuery = matchMedia('(prefers-color-scheme: dark)');
let theme = {};
function readTheme() {
  const cs = getComputedStyle(document.documentElement);
  const v = (name) => cs.getPropertyValue(name).trim();
  theme = {
    dark: darkQuery.matches,
    bg: v('--bg'),
    surface: v('--surface'),
    surface2: v('--surface-2'),
    border: v('--border'),
    text: v('--text'),
    muted: v('--muted'),
    font: v('--font'),
  };
}

// One fixed hue per category, so colours mean the same thing everywhere.
// Order matches the backend's CATEGORIES list.
const CAT_HUES = [212, 312, 174, 138, 266, 222, 28, 48, 194, 220];
const CAT_SAT = [62, 58, 52, 52, 58, 12, 70, 72, 60, 8];

function catSwatch(cat) {
  if (cat === undefined || cat === null || cat === 255) return theme.dark ? 'hsl(220 8% 40%)' : 'hsl(220 10% 72%)';
  return `hsl(${CAT_HUES[cat]} ${CAT_SAT[cat]}% ${theme.dark ? 58 : 50}%)`;
}

function colorOf(i) {
  const n = state.view[i];
  if (n.id === -1 && n.cat === 255) return theme.dark ? 'hsl(220 8% 33%)' : 'hsl(220 12% 82%)';
  const d = Math.max(1, n.depth);
  const sib = state.sib[i];
  const cat = n.cat === undefined || n.cat === 255 ? 9 : n.cat;
  // Category picks the hue; depth and siblings shift shade slightly so
  // neighbours stay distinguishable. Groups are drawn muted.
  const hue = (CAT_HUES[cat] + ((sib % 5) - 2) * 5 + 360) % 360;
  const sat = Math.max(6, CAT_SAT[cat] - d * 4 - (n.id === -1 ? 30 : 0));
  const base = theme.dark ? 44 + d * 5 : 46 + d * 6;
  const light = Math.min(theme.dark ? 78 : 86, base + (sib % 3) * 3);
  return `hsl(${hue.toFixed(1)} ${sat}% ${light}%)`;
}

const LABEL_ON_FILL = 'rgba(12, 15, 22, 0.88)';

function fitText(text, maxWidth) {
  if (maxWidth < 18) return '';
  if (ctx.measureText(text).width <= maxWidth) return text;
  let lo = 0;
  let hi = text.length;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (ctx.measureText(text.slice(0, mid) + '…').width <= maxWidth) lo = mid; else hi = mid - 1;
  }
  return lo > 0 ? text.slice(0, lo) + '…' : '';
}

// ---------------------------------------------------------------------------
// View preparation
// ---------------------------------------------------------------------------
function prepareView(nodes) {
  state.view = nodes;
  state.kids = nodes.map(() => []);
  state.branch = new Array(nodes.length).fill(0);
  state.sib = new Array(nodes.length).fill(0);
  state.byDepth = [];
  let b = 0;
  for (let i = 1; i < nodes.length; i++) {
    const n = nodes[i];
    state.sib[i] = state.kids[n.parent].length;
    state.kids[n.parent].push(i);
    state.branch[i] = n.depth === 1 ? b++ : state.branch[n.parent];
    (state.byDepth[n.depth] ||= []).push(i);
  }
  state.hover = -1;
}

const isHot = (i) => {
  if (i === state.hover) return true;
  const n = state.view[i];
  if (!n) return false;
  if (state.hoverKey) return n.key === state.hoverKey;
  return state.hoverId !== null && !n.key && n.id === state.hoverId;
};

// ---------------------------------------------------------------------------
// Sunburst
// ---------------------------------------------------------------------------
const ROT = -Math.PI / 2;
const TAU = Math.PI * 2;

function layoutSunburst(w, h) {
  const nodes = state.view;
  let maxDepth = 1;
  for (const n of nodes) maxDepth = Math.max(maxDepth, n.depth);
  const cx = w / 2;
  const cy = h / 2;
  const R = Math.max(80, Math.min(w, h) / 2 - 14);
  const r0 = Math.max(58, R * 0.2);
  const ring = (R - r0) / maxDepth;
  const a0 = new Float64Array(nodes.length);
  const a1 = new Float64Array(nodes.length);
  a1[0] = TAU;
  for (let i = 0; i < nodes.length; i++) {
    const total = nodes[i].size;
    const span = a1[i] - a0[i];
    let a = a0[i];
    for (const k of state.kids[i]) {
      const s = total > 0 ? (span * nodes[k].size) / total : 0;
      a0[k] = a;
      a1[k] = a + s;
      a += s;
    }
  }
  return { kind: 'sun', cx, cy, R, r0, ring, maxDepth, a0, a1 };
}

function ringPath(g, i) {
  const d = state.view[i].depth;
  const ri = g.r0 + (d - 1) * g.ring;
  const ro = ri + g.ring - 1.5;
  ctx.beginPath();
  ctx.arc(g.cx, g.cy, ro, g.a0[i] + ROT, g.a1[i] + ROT);
  ctx.arc(g.cx, g.cy, ri, g.a1[i] + ROT, g.a0[i] + ROT, true);
  ctx.closePath();
}

function drawSunburst(g) {
  const nodes = state.view;
  ctx.lineWidth = 1;
  ctx.strokeStyle = theme.bg;
  for (let i = 1; i < nodes.length; i++) {
    const span = g.a1[i] - g.a0[i];
    if (span < 0.002) continue;
    ringPath(g, i);
    ctx.fillStyle = colorOf(i);
    ctx.fill();
    // Separator lines between segments; a full ring has no neighbours.
    if (span > 0.008 && span < TAU - 0.001) ctx.stroke();
  }

  // Labels along each arc where they fit.
  ctx.font = `12px ${theme.font}`;
  ctx.textAlign = 'center';
  ctx.textBaseline = 'middle';
  ctx.fillStyle = LABEL_ON_FILL;
  for (let i = 1; i < nodes.length; i++) {
    const span = g.a1[i] - g.a0[i];
    const rm = g.r0 + (nodes[i].depth - 0.5) * g.ring;
    const arcLen = span * rm;
    if (arcLen < 44 || g.ring < 15) continue;
    const label = fitText(nodes[i].name, Math.min(arcLen - 10, 220));
    if (!label) continue;
    const mid = (g.a0[i] + g.a1[i]) / 2 + ROT;
    let rot = mid + Math.PI / 2;
    rot = Math.atan2(Math.sin(rot), Math.cos(rot));
    if (Math.abs(rot) > Math.PI / 2) rot += Math.PI;
    ctx.save();
    ctx.translate(g.cx + Math.cos(mid) * rm, g.cy + Math.sin(mid) * rm);
    ctx.rotate(rot);
    ctx.fillText(label, 0, 0);
    ctx.restore();
  }

  // Highlight.
  for (let i = 1; i < nodes.length; i++) {
    if (!isHot(i)) continue;
    ringPath(g, i);
    ctx.fillStyle = theme.dark ? 'rgba(255,255,255,0.22)' : 'rgba(255,255,255,0.35)';
    ctx.fill();
    ctx.lineWidth = 2;
    ctx.strokeStyle = theme.text;
    ctx.stroke();
  }

  // Centre disc: current folder.
  const root = nodes[0];
  ctx.beginPath();
  ctx.arc(g.cx, g.cy, g.r0 - 4, 0, TAU);
  ctx.fillStyle = state.hover === -2 && canGoUp() ? theme.surface2 : theme.surface;
  ctx.fill();
  ctx.lineWidth = 1;
  ctx.strokeStyle = theme.border;
  ctx.stroke();
  const maxW = (g.r0 - 10) * 1.7;
  ctx.fillStyle = theme.text;
  ctx.font = `600 13px ${theme.font}`;
  ctx.fillText(fitText(root.name, maxW), g.cx, g.cy - 18);
  ctx.font = `600 ${g.r0 > 80 ? 22 : 18}px ${theme.font}`;
  ctx.fillText(fmtBytes(root.size), g.cx, g.cy + 4);
  ctx.font = `11px ${theme.font}`;
  ctx.fillStyle = theme.muted;
  ctx.fillText(`${fmtFiles(root.files)}`, g.cx, g.cy + 24);
  if (canGoUp()) ctx.fillText('↑ up', g.cx, g.cy + 40);
}

function hitSunburst(g, x, y) {
  const dx = x - g.cx;
  const dy = y - g.cy;
  const r = Math.hypot(dx, dy);
  if (r < g.r0 - 4) return -2;
  const d = Math.floor((r - g.r0) / g.ring) + 1;
  if (d < 1 || d > g.maxDepth) return -1;
  let a = Math.atan2(dy, dx) - ROT;
  if (a < 0) a += TAU;
  if (a >= TAU) a -= TAU;
  for (const i of state.byDepth[d] || []) {
    if (a >= g.a0[i] && a < g.a1[i]) return i;
  }
  return -1;
}

// ---------------------------------------------------------------------------
// Treemap (squarified, nested)
// ---------------------------------------------------------------------------
function squarify(items, x, y, w, h, place) {
  const size = (k) => state.view[k].size;
  items.sort((p, q) => size(q) - size(p));
  const total = items.reduce((s, k) => s + size(k), 0);
  if (total <= 0 || w <= 0 || h <= 0) return;
  const scale = (w * h) / total;
  let i = 0;
  while (i < items.length) {
    const short = Math.min(w, h);
    const maxA = size(items[i]) * scale;
    let end = i;
    let sum = 0;
    let best = Infinity;
    while (end < items.length) {
      const a = size(items[end]) * scale;
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
      for (let k = i; k < end; k++) {
        const rh = (size(items[k]) * scale) / rw;
        place(items[k], x, yy, rw, rh);
        yy += rh;
      }
      x += rw;
      w -= rw;
    } else {
      const rh = sum / w;
      let xx = x;
      for (let k = i; k < end; k++) {
        const rw = (size(items[k]) * scale) / rh;
        place(items[k], xx, y, rw, rh);
        xx += rw;
      }
      y += rh;
      h -= rh;
    }
    i = end;
  }
}

const HEADER = 18;
const PAD = 2;

function layoutTreemap(w, h) {
  const rects = [];
  const place = (i, x, y, rw, rh) => {
    const n = state.view[i];
    const kids = state.kids[i].filter((k) => state.view[k].size > 0);
    const nest = i === 0 || (n.dir && kids.length > 0 && rw > 44 && rh > 44);
    if (i !== 0) rects.push({ i, x, y, w: rw, h: rh, leaf: !nest });
    if (!nest) return;
    let ix = x + PAD;
    let iy = y + PAD;
    let iw = rw - PAD * 2;
    let ih = rh - PAD * 2;
    if (i !== 0) { iy += HEADER; ih -= HEADER; }
    if (iw < 6 || ih < 6) {
      if (i !== 0) rects[rects.length - 1].leaf = true;
      return;
    }
    squarify(kids, ix, iy, iw, ih, place);
  };
  place(0, 0, 0, w, h);
  return { kind: 'tree', rects };
}

function drawTreemap(g) {
  ctx.lineWidth = 1;
  for (const r of g.rects) {
    const color = colorOf(r.i);
    if (r.leaf) {
      ctx.fillStyle = color;
      ctx.fillRect(r.x, r.y, r.w, r.h);
      if (r.w > 8 && r.h > 8) {
        const grad = ctx.createLinearGradient(r.x, r.y, r.x + r.w, r.y + r.h);
        grad.addColorStop(0, 'rgba(255,255,255,0.20)');
        grad.addColorStop(1, 'rgba(0,0,0,0.10)');
        ctx.fillStyle = grad;
        ctx.fillRect(r.x, r.y, r.w, r.h);
      }
    } else {
      ctx.fillStyle = theme.dark ? 'rgba(255,255,255,0.04)' : 'rgba(0,0,0,0.03)';
      ctx.fillRect(r.x, r.y, r.w, r.h);
      ctx.fillStyle = color;
      ctx.fillRect(r.x, r.y, r.w, Math.min(HEADER, r.h));
    }
    ctx.strokeStyle = theme.bg;
    ctx.strokeRect(r.x + 0.5, r.y + 0.5, r.w - 1, r.h - 1);
  }

  ctx.textAlign = 'left';
  ctx.textBaseline = 'alphabetic';
  for (const r of g.rects) {
    const n = state.view[r.i];
    ctx.fillStyle = LABEL_ON_FILL;
    if (!r.leaf) {
      if (r.w < 50) continue;
      ctx.font = `600 11px ${theme.font}`;
      const size = fmtBytes(n.size);
      const sw = ctx.measureText(size).width;
      const name = fitText(n.name, r.w - sw - 18);
      ctx.fillText(name, r.x + 6, r.y + 13);
      if (name) ctx.fillText(size, r.x + r.w - sw - 6, r.y + 13);
    } else if (r.w > 46 && r.h > 18) {
      ctx.font = `12px ${theme.font}`;
      ctx.fillText(fitText(n.name, r.w - 10), r.x + 5, r.y + 15);
      if (r.h > 34) {
        ctx.font = `11px ${theme.font}`;
        ctx.fillText(fitText(fmtBytes(n.size), r.w - 10), r.x + 5, r.y + 30);
      }
    }
  }

  for (const r of g.rects) {
    if (!isHot(r.i)) continue;
    ctx.fillStyle = theme.dark ? 'rgba(255,255,255,0.18)' : 'rgba(255,255,255,0.3)';
    ctx.fillRect(r.x, r.y, r.w, r.leaf ? r.h : Math.min(HEADER, r.h));
    ctx.lineWidth = 2;
    ctx.strokeStyle = theme.text;
    ctx.strokeRect(r.x + 1, r.y + 1, r.w - 2, r.h - 2);
  }
}

function hitTreemap(g, x, y) {
  for (let k = g.rects.length - 1; k >= 0; k--) {
    const r = g.rects[k];
    if (x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h) return r.i;
  }
  return -1;
}

// ---------------------------------------------------------------------------
// Drawing loop
// ---------------------------------------------------------------------------
let frame = 0;
function requestDraw() {
  if (!frame) frame = requestAnimationFrame(() => { frame = 0; draw(); });
}

function resize() {
  const rect = $('chart').getBoundingClientRect();
  const dpr = window.devicePixelRatio || 1;
  state.size = { w: rect.width, h: rect.height };
  canvas.width = Math.max(1, Math.round(rect.width * dpr));
  canvas.height = Math.max(1, Math.round(rect.height * dpr));
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  state.geo = null;
  requestDraw();
}

function draw() {
  const { w, h } = state.size;
  ctx.clearRect(0, 0, w, h);
  if (!state.view.length) return;
  if (!state.geo) {
    state.geo = state.mode === 'sunburst' ? layoutSunburst(w, h) : layoutTreemap(w - 1, h - 1);
  }
  if (state.geo.kind === 'sun') drawSunburst(state.geo); else drawTreemap(state.geo);
}

// ---------------------------------------------------------------------------
// Side list, legend, breadcrumbs, stats
// ---------------------------------------------------------------------------
const ICON_DIR = '<svg class="ico dir" viewBox="0 0 16 16"><path fill="currentColor" d="M1.5 3.5A1.5 1.5 0 0 1 3 2h3.2l1.6 1.6H13A1.5 1.5 0 0 1 14.5 5v7.5A1.5 1.5 0 0 1 13 14H3a1.5 1.5 0 0 1-1.5-1.5z"/></svg>';
const ICON_FILE = '<svg class="ico" viewBox="0 0 16 16"><path fill="currentColor" d="M4 1.5h5l3.5 3.5v8.5A1.5 1.5 0 0 1 11 15H4a1.5 1.5 0 0 1-1.5-1.5v-10A1.5 1.5 0 0 1 4 1.5zm4.5.9V5.5h3.1z"/></svg>';
const ICON_CAT = '<svg class="ico" viewBox="0 0 16 16"><path fill="currentColor" d="M2 2h5v5H2zm7 0h5v5H9zM2 9h5v5H2zm7 0h5v5H9z"/></svg>';
const ICON_REVEAL = '<svg viewBox="0 0 16 16"><path fill="currentColor" d="M9 2h5v5h-1.5V4.6L7.3 9.8 6.2 8.7l5.2-5.2H9zM3 4h4v1.5H3.5v7h7V9H12v4a1 1 0 0 1-1 1H3a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1z"/></svg>';
const ICON_TRASH = '<svg viewBox="0 0 16 16"><path fill="currentColor" d="M6 1.5h4l.5 1H14V4H2V2.5h3.5zM3 5h10l-.8 9.1a1 1 0 0 1-1 .9H4.8a1 1 0 0 1-1-.9zm3 1.5v6.5h1.2V6.5zm2.8 0v6.5H10V6.5z"/></svg>';

function catDot(cat) {
  return `<span class="dot" style="background:${catSwatch(cat)}"></span>`;
}

function catLabel(cat, sub) {
  const c = state.tax && state.tax.cats[cat];
  const s = state.tax && state.tax.subs[sub];
  if (!c) return '';
  return s && s.name !== c.name ? `${c.name} › ${s.name}` : c.name;
}

function ruleOf(r) {
  return state.tax && r !== undefined && r !== 65535 ? state.tax.rules[r] : null;
}

const SAFETY = { green: 'Safe to clear', yellow: 'Check first', red: "Don't delete by hand" };
function safetyBadge(r) {
  const rule = ruleOf(r);
  return rule && rule.safety ? `<span class="badge ${rule.safety}">${SAFETY[rule.safety]}</span>` : '';
}

/** Rows for the side list: real items (location) or view nodes (category). */
function listRows() {
  if (state.cmode === 'location') return state.list;
  return state.view
    .map((n, i) => ({ n, i }))
    .filter(({ n }) => n.depth === 1)
    .map(({ n }) => ({
      id: n.id, key: n.key || '', name: n.name, on_disk: n.size, size: n.size, files: n.files,
      dir: n.dir, cat: n.cat, sub: n.sub, rule: n.rule, group: n.id === -1, hint: n.hint || '',
      protected: true, // decided by the backend when the dialog opens
    }));
}

function renderList() {
  const root = state.view[0];
  $('sideTitle').textContent = root ? root.name : 'Nothing scanned yet';
  $('sideSub').textContent = root ? `${fmtBytes(root.size)} on disk · ${fmtFiles(root.files)}` : '';
  const total = root ? root.size : 0;
  const ol = $('list');
  ol.replaceChildren();
  ol.scrollTop = 0; // new folder: start at its biggest items
  const rows = listRows();
  for (const it of rows) {
    const li = document.createElement('li');
    if (it.key) li.dataset.key = it.key; else if (it.id >= 0) li.dataset.id = it.id;
    const opens = it.key || it.dir || state.cmode === 'category';
    li.className = opens && !it.group ? 'dir' : '';
    const tags = (it.link ? '<span class="tag">link</span>' : '') + (it.cloud ? '<span class="tag">cloud</span>' : '');
    const icon = it.key ? ICON_CAT : it.dir ? ICON_DIR : ICON_FILE;
    const sub = state.cmode === 'category' && it.hint ? escapeHtml(it.hint) : `${fmtPct(it.on_disk, total)}${it.dir || it.key ? ` · ${fmtFiles(it.files)}` : ''}`;
    const canDelete = it.id >= 0 && !it.key && !it.group && (state.cmode === 'category' || !it.protected);
    li.innerHTML =
      `<span class="cdot">${icon}${it.cat !== undefined && it.cat !== 255 ? catDot(it.cat) : ''}</span>` +
      `<div class="nm"><div class="t" title="${escapeHtml(it.hint || it.name)}">${escapeHtml(it.name)}${tags}</div>` +
      `<div class="barw"><i style="width:${total ? Math.max(0.5, (it.on_disk / total) * 100).toFixed(2) : 0}%;background:${catSwatch(it.cat)}"></i></div></div>` +
      `<div class="sz">${fmtBytes(it.on_disk)}<small title="${sub}">${sub.length > 46 ? '…' + sub.slice(-45) : sub}</small></div>` +
      (it.id >= 0 && !it.group ? `<button class="rv" title="Show in Explorer" aria-label="Show ${escapeHtml(it.name)} in Explorer">${ICON_REVEAL}</button>` : '<span></span>') +
      (canDelete ? `<button class="rv rm" title="Move to Recycle Bin…" aria-label="Move ${escapeHtml(it.name)} to the Recycle Bin">${ICON_TRASH}</button>` : '<span></span>');
    ol.append(li);
  }
  if (state.cmode === 'location' && state.listTotal > state.list.length) {
    const li = document.createElement('li');
    li.className = 'more';
    li.textContent = `+ ${fmtCount(state.listTotal - state.list.length)} smaller items not listed`;
    ol.append(li);
  }
  if (root && rows.length === 0) {
    const li = document.createElement('li');
    li.className = 'more';
    li.textContent = state.cmode === 'location' ? 'This folder is empty.' : 'Nothing here.';
    ol.append(li);
  }
  renderLegend();
}

function renderLegend() {
  const el = $('legend');
  el.replaceChildren();
  if (state.cmode !== 'location' || !state.tax || !state.breakdown.length) return;
  const total = state.breakdown.reduce((s, [, v]) => s + v, 0);
  for (const [cat, bytes] of state.breakdown) {
    if (bytes / total < 0.005) continue;
    const b = document.createElement('button');
    b.type = 'button';
    b.className = 'chip';
    b.title = `${state.tax.cats[cat].name}: ${fmtBytes(bytes)} here. Click to see it by category.`;
    b.innerHTML = `${catDot(cat)}${escapeHtml(state.tax.cats[cat].name)} <b>${fmtPct(bytes, total)}</b>`;
    b.onclick = () => { setCmode('category'); navigateCat(`cat:${cat}`); };
    el.append(b);
  }
}

function renderCrumbs() {
  const ol = $('crumbs');
  ol.replaceChildren();
  const items = state.cmode === 'category'
    ? state.catCrumbs.map((c) => ({ label: c.name, go: () => navigateCat(c.key) }))
    : state.crumbs.map((c) => ({ label: c.name, go: () => navigate(c.id) }));
  for (const c of items) {
    const li = document.createElement('li');
    const b = document.createElement('button');
    b.type = 'button';
    b.textContent = c.label;
    b.title = c.label;
    b.onclick = c.go;
    li.append(b);
    ol.append(li);
  }
}

function renderStats() {
  const s = state.summary;
  if (!s) { $('stats').innerHTML = ''; return; }
  const secs = s.elapsed_ms >= 10000 ? (s.elapsed_ms / 1000).toFixed(0) : (s.elapsed_ms / 1000).toFixed(2);
  let html =
    `<div><b>${fmtBytes(s.on_disk)}</b> on disk · <b>${fmtCount(s.files)}</b> files · <b>${fmtCount(s.dirs)}</b> folders</div>` +
    `<div>Scanned in <b>${secs} s</b> · ${escapeHtml(s.mode)} · <b>${Math.round(s.categorized * 100)}%</b> categorized</div>`;
  if (s.unreadable > 0) {
    html += `<div class="hint">${fmtCount(s.unreadable)} folders couldn't be read${s.admin_hint ? ' · try Fast scan (admin)' : ''}</div>`;
  }
  $('stats').innerHTML = html;
}

function renderMode() {
  for (const b of document.querySelectorAll('#modeSeg button')) {
    b.setAttribute('aria-pressed', String(b.dataset.mode === state.mode));
  }
  for (const b of document.querySelectorAll('#cmodeSeg button')) {
    b.setAttribute('aria-pressed', String(b.dataset.cmode === state.cmode));
  }
}

// ---------------------------------------------------------------------------
// Tooltip + toast
// ---------------------------------------------------------------------------
function showTip(i, x, y) {
  const tip = $('tip');
  if (i === -1 || (i === -2 && !canGoUp())) { tip.classList.add('hidden'); return; }
  const root = state.view[0];
  if (i === -2) {
    tip.innerHTML = `<div class="tn">${escapeHtml(upLabel())}</div><div class="tk">Click to go up one level</div>`;
  } else {
    const n = state.view[i];
    let kind;
    if (n.key) kind = 'Click to open';
    else if (n.id === -1) kind = `${fmtCount(n.merged)} items too small to draw separately`;
    else if (state.cmode === 'category') kind = 'Click to open this place by location';
    else kind = n.dir ? 'Folder · click to open' : 'File · double-click to show in Explorer';
    const cat = n.own_cat !== undefined && n.own_cat !== 255 && !n.key ? n.own_cat : n.cat;
    const rule = ruleOf(n.rule);
    tip.innerHTML =
      `<div class="tn">${escapeHtml(n.name)}</div>` +
      (cat !== undefined && cat !== 255 ? `<div class="tc">${catDot(cat)}${escapeHtml(catLabel(cat, n.sub))}${safetyBadge(n.rule)}</div>` : '') +
      `<div class="tr"><span>On disk</span><b>${fmtBytes(n.size)}</b></div>` +
      `<div class="tr"><span>Share of ${escapeHtml(root.name)}</span><b>${fmtPct(n.size, root.size)}</b></div>` +
      (n.dir || n.id < 0 || n.key ? `<div class="tr"><span>Files</span><b>${fmtCount(n.files)}</b></div>` : '') +
      (n.hint ? `<div class="tk">${escapeHtml(n.hint)}</div>` : '') +
      (rule && rule.note && !n.key ? `<div class="note">${escapeHtml(rule.note)}</div>` : '') +
      `<div class="tk">${kind}</div>`;
  }
  tip.classList.remove('hidden');
  const r = tip.getBoundingClientRect();
  const left = Math.min(x + 16, innerWidth - r.width - 8);
  const top = Math.min(y + 16, innerHeight - r.height - 8);
  tip.style.left = `${Math.max(8, left)}px`;
  tip.style.top = `${Math.max(8, top)}px`;
}

let toastTimer = 0;
function toast(msg, ok = false) {
  const t = $('toast');
  t.textContent = msg;
  t.classList.toggle('ok', ok);
  t.classList.remove('hidden');
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => t.classList.add('hidden'), ok ? 4000 : 7000);
}

// ---------------------------------------------------------------------------
// Navigation
// ---------------------------------------------------------------------------
let navSeq = 0;

function afterViewLoaded() {
  state.geo = null;
  $('tip').classList.add('hidden');
  renderCrumbs();
  renderList();
  requestDraw();
}

async function navigate(id) {
  const seq = ++navSeq;
  try {
    const depth = state.mode === 'sunburst' ? 6 : 3;
    const [view, list, crumbs] = await Promise.all([
      invoke('view', { node: id, depth }),
      invoke('list', { node: id, limit: 400 }),
      invoke('crumbs', { node: id }),
    ]);
    if (seq !== navSeq) return; // a newer navigation started meanwhile
    state.root = id;
    state.crumbs = crumbs;
    state.list = list.items;
    state.listTotal = list.total;
    state.breakdown = list.breakdown;
    prepareView(view);
    afterViewLoaded();
  } catch (e) {
    toast(String(e));
  }
}

async function navigateCat(key) {
  const seq = ++navSeq;
  try {
    const out = await invoke('category_view', { key });
    if (seq !== navSeq) return;
    state.catKey = key;
    state.catCrumbs = out.crumbs;
    prepareView(out.nodes);
    afterViewLoaded();
  } catch (e) {
    toast(String(e));
  }
}

function refresh() {
  return state.cmode === 'category' ? navigateCat(state.catKey) : navigate(state.root);
}

function canGoUp() {
  return state.cmode === 'category' ? state.catCrumbs.length > 1 : state.crumbs.length > 1;
}

function upLabel() {
  const c = state.cmode === 'category' ? state.catCrumbs : state.crumbs;
  return c.length > 1 ? c[c.length - 2].name : '';
}

function goUp() {
  if (!canGoUp()) return;
  if (state.cmode === 'category') navigateCat(state.catCrumbs[state.catCrumbs.length - 2].key);
  else navigate(state.crumbs[state.crumbs.length - 2].id);
}

function setCmode(m) {
  if (state.cmode === m) return;
  state.cmode = m;
  store.set('cmode', m);
  renderMode();
}

/** Opens a real item (from the category view) in the location view. */
async function openPlace(id) {
  try {
    const info = await invoke('info', { node: id });
    setCmode('location');
    await navigate(info.folder);
  } catch (e) {
    toast(String(e));
  }
}

function openViewNode(n) {
  if (n.key) { navigateCat(n.key); return; }
  if (n.id < 0) return;
  if (state.cmode === 'category') openPlace(n.id);
  else if (n.dir) navigate(n.id);
}

// ---------------------------------------------------------------------------
// Scanning
// ---------------------------------------------------------------------------
function setBusy(on, label = '') {
  state.busy = on;
  $('busy').classList.toggle('hidden', !on);
  $('scanBtn').disabled = on;
  $('adminBtn').disabled = on;
  clearInterval(setBusy.timer);
  if (on) {
    const start = performance.now();
    const tick = () => { $('busyText').textContent = `${label} · ${((performance.now() - start) / 1000).toFixed(0)} s`; };
    tick();
    setBusy.timer = setInterval(tick, 500);
  }
}

async function runScan(kind) {
  const path = $('path').value.trim();
  if (!path) { toast('Type a drive like C:\\ or a folder path first.'); return; }
  $('empty').classList.add('hidden');
  setBusy(true, kind === 'admin' ? `Fast scan of ${path} (approve the admin prompt)` : `Scanning ${path}`);
  try {
    state.summary = kind === 'admin' ? await invoke('scan_admin', { drive: path }) : await invoke('scan', { path });
    renderStats();
    state.root = 0;
    state.catKey = '';
    await refresh();
  } catch (e) {
    toast(String(e));
    if (!state.view.length) $('empty').classList.remove('hidden');
  } finally {
    setBusy(false);
  }
}

// ---------------------------------------------------------------------------
// Safe delete
// ---------------------------------------------------------------------------
let pendingDelete = null;

async function askDelete(id) {
  let info;
  try {
    info = await invoke('info', { node: id });
  } catch (e) {
    toast(String(e));
    return;
  }
  pendingDelete = info.protected ? null : id;
  const rule = ruleOf(info.rule);
  $('mTitle').textContent = `Move "${info.name}" to the Recycle Bin?`;
  $('mBody').innerHTML =
    `<div class="path">${escapeHtml(info.path)}</div>` +
    `<div class="facts">` +
    `<span>Frees</span><b>${fmtBytes(info.on_disk)}</b>` +
    (info.dir ? `<span>Contains</span><b>${fmtFiles(info.files)}</b>` : '') +
    `<span>What it is</span><b>${catDot(info.cat)} ${escapeHtml(catLabel(info.cat, info.sub))}${safetyBadge(info.rule)}</b>` +
    `</div>` +
    (rule && rule.note ? `<div class="note">${escapeHtml(rule.note)}</div>` : '') +
    (info.protected
      ? `<div class="warn">Xylem won't delete this. ${escapeHtml(info.protected)}</div>`
      : `<div class="note">It goes to the Recycle Bin, so you can restore it. If Windows can't recycle it (too big for the bin), Windows will ask before deleting it permanently.</div>`);
  $('mOk').classList.toggle('hidden', !!info.protected);
  $('mCancel').textContent = info.protected ? 'Close' : 'Cancel';
  $('modal').classList.remove('hidden');
  (info.protected ? $('mCancel') : $('mOk')).focus();
}

function closeModal() {
  $('modal').classList.add('hidden');
  pendingDelete = null;
}

async function confirmDelete() {
  const id = pendingDelete;
  closeModal();
  if (id === null) return;
  setBusy(true, 'Moving to the Recycle Bin');
  try {
    const out = await invoke('delete', { node: id });
    if (out.cancelled) {
      toast('Cancelled. Nothing was deleted.', true);
    } else {
      toast(`Moved "${out.name}" to the Recycle Bin · ${fmtBytes(out.freed)} freed`, true);
      state.summary = await invoke('summary_now');
      renderStats();
      await refresh();
    }
  } catch (e) {
    toast(String(e));
  } finally {
    setBusy(false);
  }
}

$('mCancel').addEventListener('click', closeModal);
$('mOk').addEventListener('click', confirmDelete);
$('modal').addEventListener('click', (ev) => { if (ev.target === $('modal')) closeModal(); });

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------
function pointerHit(ev) {
  if (!state.geo) return -1;
  const r = canvas.getBoundingClientRect();
  const x = ev.clientX - r.left;
  const y = ev.clientY - r.top;
  return state.geo.kind === 'sun' ? hitSunburst(state.geo, x, y) : hitTreemap(state.geo, x, y);
}

function rowMatches(li, n) {
  if (!n) return false;
  if (n.key) return li.dataset.key === n.key;
  return n.id >= 0 && li.dataset.id !== undefined && Number(li.dataset.id) === n.id;
}

function syncListHot() {
  const n = state.hover >= 0 ? state.view[state.hover] : null;
  for (const li of $('list').children) li.classList.toggle('hot', rowMatches(li, n));
}

canvas.addEventListener('mousemove', (ev) => {
  const i = pointerHit(ev);
  if (i !== state.hover) {
    state.hover = i;
    const n = i > 0 ? state.view[i] : null;
    const clickable = (i === -2 && canGoUp()) || (n && (n.key || (n.id >= 0 && (n.dir || state.cmode === 'category'))));
    canvas.style.cursor = clickable ? 'pointer' : 'default';
    syncListHot();
    requestDraw();
  }
  showTip(i === 0 ? -1 : i, ev.clientX, ev.clientY);
});

canvas.addEventListener('mouseleave', () => {
  state.hover = -1;
  syncListHot();
  $('tip').classList.add('hidden');
  requestDraw();
});

canvas.addEventListener('click', (ev) => {
  const i = pointerHit(ev);
  if (i === -2) { goUp(); return; }
  if (i > 0) openViewNode(state.view[i]);
});

canvas.addEventListener('dblclick', (ev) => {
  const i = pointerHit(ev);
  const n = i > 0 ? state.view[i] : null;
  if (n && !n.key && n.id >= 0 && !n.dir && state.cmode === 'location') {
    invoke('reveal', { node: n.id }).catch((e) => toast(String(e)));
  }
});

canvas.addEventListener('contextmenu', (ev) => {
  const i = pointerHit(ev);
  const n = i > 0 ? state.view[i] : null;
  if (n && !n.key && n.id >= 0) { ev.preventDefault(); askDelete(n.id); }
});

$('list').addEventListener('click', (ev) => {
  const li = ev.target.closest('li');
  if (!li || (!li.dataset.id && !li.dataset.key)) return;
  if (li.dataset.key) { navigateCat(li.dataset.key); return; }
  const id = Number(li.dataset.id);
  if (ev.target.closest('.rm')) { askDelete(id); return; }
  if (ev.target.closest('.rv')) { invoke('reveal', { node: id }).catch((e) => toast(String(e))); return; }
  if (state.cmode === 'category') { openPlace(id); return; }
  const item = state.list.find((it) => it.id === id);
  if (item && item.dir) navigate(id);
});

$('list').addEventListener('mouseover', (ev) => {
  const li = ev.target.closest('li');
  const key = li ? li.dataset.key || null : null;
  const id = li && li.dataset.id !== undefined ? Number(li.dataset.id) : null;
  if (id !== state.hoverId || key !== state.hoverKey) {
    state.hoverId = id;
    state.hoverKey = key;
    requestDraw();
  }
});
$('list').addEventListener('mouseleave', () => { state.hoverId = null; state.hoverKey = null; requestDraw(); });

$('scanForm').addEventListener('submit', (ev) => { ev.preventDefault(); runScan('normal'); });
$('adminBtn').addEventListener('click', () => runScan('admin'));

for (const b of document.querySelectorAll('#modeSeg button')) {
  b.addEventListener('click', () => {
    if (state.mode === b.dataset.mode) return;
    state.mode = b.dataset.mode;
    store.set('mode', state.mode);
    renderMode();
    if (state.summary) refresh();
  });
}

for (const b of document.querySelectorAll('#cmodeSeg button')) {
  b.addEventListener('click', () => {
    if (state.cmode === b.dataset.cmode) return;
    setCmode(b.dataset.cmode);
    if (state.summary) refresh();
  });
}

window.addEventListener('keydown', (ev) => {
  if (!$('modal').classList.contains('hidden')) {
    if (ev.key === 'Escape') closeModal();
    return;
  }
  if (ev.target instanceof HTMLInputElement) return;
  if (ev.key === 'Backspace' || (ev.altKey && ev.key === 'ArrowLeft')) { ev.preventDefault(); goUp(); }
});

new ResizeObserver(resize).observe($('chart'));
darkQuery.addEventListener('change', () => { readTheme(); requestDraw(); });

// ---------------------------------------------------------------------------
// Start
// ---------------------------------------------------------------------------
(async function start() {
  readTheme();
  renderMode();
  try {
    state.tax = await invoke('taxonomy');
    const drives = await invoke('list_drives');
    $('drives').replaceChildren(...drives.map((d) => Object.assign(document.createElement('option'), { value: d })));
    $('path').value = drives.includes('C:\\') ? 'C:\\' : drives[0] || '';
    const initial = await invoke('initial_path');
    if (initial) {
      $('path').value = initial;
      runScan('normal');
    }
  } catch (e) {
    toast(String(e));
  }
})();
