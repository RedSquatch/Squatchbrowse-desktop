// SquatchBrowse page script. Injected into the top frame of every page.
// Provides: Reader view, Unstick (overlay remover), Zap (element remover),
// video discovery, and keyboard shortcuts. Talks to Rust via window.ipc.
(() => {
  if (window.__squatch || window.top !== window) return;
  const post = (o) => { try { window.ipc.postMessage(JSON.stringify(o)); } catch (_) {} };
  const toast = (msg) => post({ evt: 'toast', msg });
  const S = (window.__squatch = {});
  const OWN = 'data-squatch';

  // ---------- page state ----------
  let lastTitle = null, lastUrl = null, lastVids = -1;
  function report(force) {
    const vids = findVideos().length;
    if (!force && document.title === lastTitle && location.href === lastUrl && vids === lastVids) return;
    lastTitle = document.title; lastUrl = location.href; lastVids = vids;
    post({ evt: 'page', url: location.href, title: document.title, videos: vids });
  }
  document.addEventListener('DOMContentLoaded', () => report(true));
  window.addEventListener('load', () => report(true));
  setInterval(() => report(false), 2000); // catches SPA navigation and late videos

  // ---------- element hiding (Windows) ----------
  // On Linux WebKit hides ad boxes natively. On Windows we send the page's class and
  // id names to the app, which answers with the selectors to hide (S.hide).
  if (window.__sqCosmetic) {
    const seenC = new Set(), seenI = new Set();
    let hideStyle = null;
    const askCosmetic = () => {
      const classes = [], ids = [];
      document.querySelectorAll('[class],[id]').forEach((el) => {
        if (el.id && !seenI.has(el.id)) { seenI.add(el.id); ids.push(el.id); }
        if (el.classList) el.classList.forEach((c) => { if (!seenC.has(c)) { seenC.add(c); classes.push(c); } });
      });
      if (classes.length || ids.length || !hideStyle) post({ evt: 'cosmetic', classes, ids });
    };
    S.hide = (sels) => {
      if (!sels || !sels.length) return;
      if (!hideStyle) {
        hideStyle = document.createElement('style');
        hideStyle.setAttribute(OWN, '');
        (document.head || document.documentElement).appendChild(hideStyle);
      }
      // One bad selector would void a whole rule, so group them in small batches.
      const out = [];
      for (let i = 0; i < sels.length; i += 20) out.push(sels.slice(i, i + 20).join(',') + '{display:none!important}');
      hideStyle.textContent += out.join('\n') + '\n';
    };
    document.addEventListener('DOMContentLoaded', askCosmetic);
    window.addEventListener('load', () => { askCosmetic(); setTimeout(askCosmetic, 2500); });
  }

  // ---------- keyboard ----------
  window.addEventListener('keydown', (e) => {
    const k = e.key, lk = k.toLowerCase();
    let key = null;
    if ((e.ctrlKey && lk === 'l') || k === 'F6') key = 'focus-url';
    else if (e.altKey && k === 'ArrowLeft') key = 'back';
    else if (e.altKey && k === 'ArrowRight') key = 'forward';
    else if (k === 'F5' || (e.ctrlKey && lk === 'r')) key = 'reload';
    else if (k === 'F9') key = 'reader';
    else if (k === 'F12') key = 'devtools';
    else if (e.ctrlKey && !e.shiftKey && lk === 't') key = 'new-tab';
    else if (e.ctrlKey && lk === 'w') key = 'close-tab';
    else if (e.ctrlKey && k === 'Tab') key = e.shiftKey ? 'prev-tab' : 'next-tab';
    else if (e.ctrlKey && k === 'PageDown') key = 'next-tab';
    else if (e.ctrlKey && k === 'PageUp') key = 'prev-tab';
    else if (e.ctrlKey && /^[1-9]$/.test(k)) key = 'tab-' + k;
    else if (e.ctrlKey && !e.shiftKey && lk === 'd') key = 'bookmark';
    else if (e.ctrlKey && e.shiftKey && lk === 'o') key = 'bookmarks';
    if (key) { e.preventDefault(); e.stopPropagation(); post({ evt: 'key', key }); }
  }, true);

  // ---------- links that should open a new tab ----------
  // Only real user clicks count (isTrusted), so ad scripts can't open tabs.
  function linkFrom(e) {
    const a = e.target && e.target.closest ? e.target.closest('a[href]') : null;
    if (!a || !/^https?:/i.test(a.href)) return null;
    return a;
  }
  document.addEventListener('click', (e) => {
    if (!e.isTrusted || S._zapStop || e.button !== 0) return;
    const a = linkFrom(e);
    if (!a) return;
    const t = (a.getAttribute('target') || '').toLowerCase();
    const newTab = e.ctrlKey || e.metaKey || (t && t !== '_self' && t !== '_top' && t !== '_parent');
    if (!newTab) return;
    e.preventDefault(); e.stopPropagation();
    post({ evt: 'open-tab', url: a.href, background: e.ctrlKey || e.metaKey });
  }, true);
  document.addEventListener('auxclick', (e) => {
    if (!e.isTrusted || e.button !== 1 || S._zapStop) return;
    const a = linkFrom(e);
    if (!a) return;
    e.preventDefault(); e.stopPropagation();
    post({ evt: 'open-tab', url: a.href, background: true });
  }, true);

  // ---------- videos ----------
  const PLAYER_HOSTS = /(youtube(-nocookie)?\.com|youtu\.be|vimeo\.com|dailymotion\.com|twitch\.tv|streamable\.com|rumble\.com|odysee\.com|bitchute\.com|player\.|\/embed\/)/i;
  function findVideos() {
    const out = new Set();
    document.querySelectorAll('video').forEach((v) => {
      const src = v.currentSrc || v.src;
      if (src && !src.startsWith('blob:')) out.add(src);
      v.querySelectorAll('source[src]').forEach((s) => s.src && out.add(s.src));
    });
    document.querySelectorAll('iframe[src]').forEach((f) => {
      if (PLAYER_HOSTS.test(f.src)) out.add(f.src);
    });
    // Streams played through MediaSource show up as blob: URLs; yt-dlp can
    // usually still find them from the page URL, so count the element itself.
    const blobOnly = [...document.querySelectorAll('video')].some((v) => (v.currentSrc || '').startsWith('blob:'));
    const list = [...out];
    if (blobOnly && list.length === 0) list.push('');
    return list;
  }
  S.videos = (mode) => post({ evt: 'videos', mode: mode || 'mp4', page: location.href, list: findVideos().filter(Boolean) });

  // Small chooser that drops down under the toolbar's Save button.
  S.saveMenu = () => {
    const old = document.getElementById('squatch-save');
    if (old) { old.remove(); return; }
    const host = document.createElement('div');
    host.id = 'squatch-save';
    host.setAttribute(OWN, '');
    host.style.cssText = 'position:fixed;top:8px;right:120px;z-index:2147483647;';
    const root = host.attachShadow({ mode: 'open' });
    const opts = [
      ['mp4', 'Video · 1080p MP4', 'Plays everywhere'],
      ['best', 'Video · up to 4K', 'Best quality, WebM or MKV'],
      ['audio', 'Audio · best quality', 'Original stream, no re-encoding'],
      ['mp3', 'Audio · MP3 320 kbps', 'With title and cover art'],
    ];
    root.innerHTML = '<style>' +
      '.m{background:#25282c;color:#e4e1da;border:1px solid #3a3f45;border-radius:10px;box-shadow:0 8px 28px #000a;padding:6px;min-width:250px;font:13px system-ui,sans-serif}' +
      'button{all:unset;display:block;width:100%;box-sizing:border-box;padding:8px 12px;border-radius:7px;cursor:default}' +
      'button:hover,button:focus{background:#353a40;outline:none}' +
      'b{font-weight:600;display:block}span{color:#8f8b84;font-size:12px}' +
      '.sep{height:1px;background:#3a3f45;margin:5px 6px}</style><div class="m">' +
      opts.map((o, i) => (i === 2 ? '<div class="sep"></div>' : '') +
        '<button data-m="' + o[0] + '"><b>' + o[1] + '</b><span>' + o[2] + '</span></button>').join('') + '</div>';
    const close = () => { host.remove(); document.removeEventListener('mousedown', outside, true); document.removeEventListener('keydown', esc, true); };
    const outside = (e) => { if (!host.contains(e.target)) close(); };
    const esc = (e) => { if (e.key === 'Escape') close(); };
    root.addEventListener('click', (e) => {
      const b = e.target.closest('button');
      if (b) { close(); S.videos(b.dataset.m); }
    });
    document.addEventListener('mousedown', outside, true);
    document.addEventListener('keydown', esc, true);
    document.documentElement.appendChild(host);
    const first = root.querySelector('button'); if (first) first.focus();
  };

  // ---------- unstick: remove overlays and unlock scrolling ----------
  S.unstick = () => {
    const vw = innerWidth, vh = innerHeight;
    let removed = 0;
    for (const el of document.querySelectorAll('body *')) {
      if (el.hasAttribute(OWN) || el.closest('[' + OWN + ']')) continue;
      const cs = getComputedStyle(el);
      if (cs.position !== 'fixed' && cs.position !== 'sticky') continue;
      const r = el.getBoundingClientRect();
      if (r.width === 0 || r.height === 0) continue;
      const area = (Math.min(r.width, vw) * Math.min(r.height, vh)) / (vw * vh);
      const z = parseInt(cs.zIndex, 10) || 0;
      const isHeaderBar = r.top <= 2 && r.height < 120 && area < 0.2; // keep slim site navbars
      if (area > 0.3 || (z >= 1000 && area > 0.04 && !isHeaderBar) || (cs.position === 'fixed' && r.top > vh * 0.6 && area > 0.06)) {
        el.remove();
        removed++;
      }
    }
    // Undo scroll locks.
    for (const el of [document.documentElement, document.body]) {
      if (!el) continue;
      el.style.setProperty('overflow', 'auto', 'important');
      el.style.setProperty('height', 'auto', 'important');
      if (getComputedStyle(el).position === 'fixed') el.style.setProperty('position', 'static', 'important');
      el.classList.remove('modal-open', 'no-scroll', 'noscroll', 'overflow-hidden', 'scroll-lock', 'is-locked');
    }
    // Undo blur and dead clicks left behind by modal scripts.
    document.querySelectorAll('body > *, main, article, #content, .content').forEach((el) => {
      const cs = getComputedStyle(el);
      if (cs.filter && cs.filter !== 'none') el.style.setProperty('filter', 'none', 'important');
      if (cs.pointerEvents === 'none') el.style.setProperty('pointer-events', 'auto', 'important');
      if (cs.userSelect === 'none') el.style.setProperty('user-select', 'auto', 'important');
    });
    toast(removed ? `Removed ${removed} overlay${removed > 1 ? 's' : ''} and unlocked scrolling` : 'No overlays found; scrolling unlocked');
  };

  // ---------- zap: click to remove, shift+click to remove forever ----------
  function cssEsc(s) { return window.CSS && CSS.escape ? CSS.escape(s) : s.replace(/[^\w-]/g, '\\$&'); }
  function stableClass(c) { return /^[a-zA-Z][\w-]{1,40}$/.test(c) && !/\d{3,}/.test(c) && !/^(css|sc|jsx|svelte)-/.test(c); }
  function part(el) {
    if (el.id && /^[a-zA-Z][\w-]{1,60}$/.test(el.id) && !/\d{4,}/.test(el.id)) return '#' + cssEsc(el.id);
    let s = el.tagName.toLowerCase();
    const cls = [...el.classList].filter(stableClass).slice(0, 3);
    if (cls.length) s += '.' + cls.map(cssEsc).join('.');
    return s;
  }
  function selectorFor(el) {
    let sel = part(el), cur = el;
    for (let i = 0; i < 4; i++) {
      try { if (sel.startsWith('#') || document.querySelectorAll(sel).length <= 2) break; } catch (_) { break; }
      cur = cur.parentElement;
      if (!cur || cur === document.body) break;
      sel = part(cur) + ' > ' + sel;
    }
    return sel;
  }
  S.zap = () => {
    if (S._zapStop) return S._zapStop();
    const box = document.createElement('div');
    box.setAttribute(OWN, '');
    box.style.cssText = 'position:fixed;pointer-events:none;z-index:2147483647;border:2px solid #f0a020;background:rgba(240,160,32,.18);border-radius:3px;transition:all .05s';
    document.documentElement.appendChild(box);
    let target = null;
    const over = (e) => {
      target = e.target;
      if (!target || target.hasAttribute(OWN)) return;
      const r = target.getBoundingClientRect();
      Object.assign(box.style, { left: r.left + 'px', top: r.top + 'px', width: r.width + 'px', height: r.height + 'px' });
    };
    const click = (e) => {
      e.preventDefault(); e.stopPropagation(); e.stopImmediatePropagation();
      const el = e.target;
      if (!el || el === document.body || el === document.documentElement) return;
      if (e.shiftKey) post({ evt: 'remember', selector: selectorFor(el) });
      el.remove();
    };
    const key = (e) => { if (e.key === 'Escape') { e.preventDefault(); stop(); } };
    const swallow = (e) => { e.preventDefault(); e.stopPropagation(); };
    function stop() {
      document.removeEventListener('mouseover', over, true);
      document.removeEventListener('click', click, true);
      document.removeEventListener('mousedown', swallow, true);
      document.removeEventListener('mouseup', swallow, true);
      document.removeEventListener('keydown', key, true);
      box.remove();
      S._zapStop = null;
      toast('Zap off');
    }
    document.addEventListener('mouseover', over, true);
    document.addEventListener('click', click, true);
    document.addEventListener('mousedown', swallow, true);
    document.addEventListener('mouseup', swallow, true);
    document.addEventListener('keydown', key, true);
    S._zapStop = stop;
    toast('Zap: click removes, Shift+click removes on every visit, Esc to finish');
  };

  // ---------- reader view ----------
  const PREF_KEY = 'squatch-reader-prefs';
  function prefs() {
    try { return Object.assign({ size: 20, theme: 'sepia', width: 680, font: 'serif' }, JSON.parse(localStorage.getItem(PREF_KEY) || '{}')); }
    catch (_) { return { size: 20, theme: 'sepia', width: 680, font: 'serif' }; }
  }
  function savePrefs(p) { try { localStorage.setItem(PREF_KEY, JSON.stringify(p)); } catch (_) {} }
  const THEMES = {
    light: { bg: '#fbfbf8', fg: '#1d1d1b', muted: '#6b6b66', link: '#1d5fa8' },
    sepia: { bg: '#f4ecd8', fg: '#3b2f22', muted: '#7a6a55', link: '#8a4b12' },
    dark:  { bg: '#16181b', fg: '#d9d6cf', muted: '#8d8a83', link: '#e0a85a' },
  };

  S.reader = () => {
    const existing = document.getElementById('squatch-reader');
    if (existing) {
      existing.remove();
      document.documentElement.classList.remove('squatch-reading');
      document.documentElement.style.removeProperty('overflow');
      return;
    }
    let art = null;
    try { art = new Readability(document.cloneNode(true), { charThreshold: 250 }).parse(); } catch (e) { console.warn(e); }
    if (!art || !art.content || (art.length || 0) < 200) {
      toast('No article found here. Try Unstick or Zap instead.');
      return;
    }
    document.querySelectorAll('video,audio').forEach((m) => { try { m.pause(); } catch (_) {} });

    const host = document.createElement('div');
    host.id = 'squatch-reader';
    host.setAttribute(OWN, '');
    host.style.cssText = 'position:fixed;inset:0;z-index:2147483646;overflow:auto;';
    const root = host.attachShadow({ mode: 'open' });
    const p = prefs();
    const minutes = Math.max(1, Math.round((art.length || 0) / 1100));

    root.innerHTML = `
      <style>
        :host { all: initial; }
        .wrap { min-height: 100%; background: var(--bg); color: var(--fg); transition: background .2s, color .2s; }
        .bar { position: sticky; top: 0; display: flex; gap: 6px; justify-content: flex-end; padding: 8px 14px;
               background: var(--bg); border-bottom: 1px solid color-mix(in srgb, var(--fg) 10%, transparent); font: 13px system-ui, sans-serif; }
        .bar button { font: inherit; color: var(--fg); background: transparent; border: 1px solid color-mix(in srgb, var(--fg) 22%, transparent);
               border-radius: 6px; padding: 4px 10px; cursor: pointer; }
        .bar button:hover { background: color-mix(in srgb, var(--fg) 8%, transparent); }
        .bar .sp { flex: 1; color: var(--muted); align-self: center; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
        .sr { max-width: var(--w); margin: 0 auto; padding: 36px 22px 120px; font-size: var(--size); line-height: 1.65; font-family: var(--font); }
        h1.title { font-size: 1.7em; line-height: 1.2; margin: 0 0 .3em; }
        .meta { color: var(--muted); font: 14px system-ui, sans-serif; margin-bottom: 2em; }
        a { color: var(--link); }
        img, video, figure, iframe { max-width: 100%; height: auto; }
        figure { margin: 1.5em 0; } figcaption { color: var(--muted); font-size: .8em; }
        pre, code { font-family: ui-monospace, monospace; font-size: .85em; }
        pre { overflow: auto; padding: 12px; background: color-mix(in srgb, var(--fg) 6%, transparent); border-radius: 6px; }
        blockquote { margin: 1em 0; padding-left: 1em; border-left: 3px solid color-mix(in srgb, var(--fg) 25%, transparent); color: var(--muted); }
        p { margin: 0 0 1.1em; }
        .body article, .body section, .body div { margin: 0; padding: 0; }
      </style>
      <div class="wrap">
        <div class="bar">
          <span class="sp">${(art.siteName || location.hostname).replace(/</g, '&lt;')}</span>
          <button data-a="smaller" title="Smaller text">A−</button>
          <button data-a="bigger" title="Bigger text">A+</button>
          <button data-a="narrow" title="Narrower column">⇥⇤</button>
          <button data-a="wide" title="Wider column">⇤⇥</button>
          <button data-a="font" title="Serif / sans">Aa</button>
          <button data-a="theme" title="Light / sepia / dark">◐</button>
          <button data-a="close" title="Close reader (F9)">✕</button>
        </div>
        <main class="sr">
          <h1 class="title"></h1>
          <div class="meta"></div>
          <div class="body"></div>
        </main>
      </div>`;
    root.querySelector('h1.title').textContent = art.title || document.title;
    root.querySelector('.meta').textContent = [art.byline, `${minutes} min read`].filter(Boolean).join(' · ');
    root.querySelector('.body').innerHTML = art.content;
    root.querySelectorAll('.body a[href]').forEach((a) => { a.target = '_self'; });

    const wrap = root.querySelector('.wrap');
    function apply() {
      const t = THEMES[p.theme] || THEMES.sepia;
      wrap.style.setProperty('--bg', t.bg); wrap.style.setProperty('--fg', t.fg);
      wrap.style.setProperty('--muted', t.muted); wrap.style.setProperty('--link', t.link);
      wrap.style.setProperty('--size', p.size + 'px'); wrap.style.setProperty('--w', p.width + 'px');
      wrap.style.setProperty('--font', p.font === 'serif' ? 'Charter, "Bitstream Charter", Georgia, "DejaVu Serif", serif' : 'system-ui, "Inter", "DejaVu Sans", sans-serif');
      host.style.background = t.bg;
      savePrefs(p);
    }
    root.querySelector('.bar').addEventListener('click', (e) => {
      const a = e.target.closest('button') && e.target.closest('button').dataset.a;
      if (a === 'smaller') p.size = Math.max(14, p.size - 1);
      if (a === 'bigger') p.size = Math.min(32, p.size + 1);
      if (a === 'narrow') p.width = Math.max(480, p.width - 60);
      if (a === 'wide') p.width = Math.min(1100, p.width + 60);
      if (a === 'font') p.font = p.font === 'serif' ? 'sans' : 'serif';
      if (a === 'theme') p.theme = { light: 'sepia', sepia: 'dark', dark: 'light' }[p.theme] || 'sepia';
      if (a === 'close') return S.reader();
      apply();
    });
    apply();
    document.documentElement.classList.add('squatch-reading');
    document.documentElement.style.setProperty('overflow', 'hidden', 'important');
    document.documentElement.appendChild(host);
    host.scrollTop = 0;
    host.tabIndex = -1; host.focus();
  };
})();
