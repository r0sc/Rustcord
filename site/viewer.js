// The "A look inside" viewer: tabs like the app's settings nav (docs: site-spec §5.4).
// tabStep is pure, so node can test it (see test.mjs).

// Arrow keys move one tab and wrap; Home and End jump. -1 for any other key.
export function tabStep(key, i, n) {
  if (key === 'Home') return 0;
  if (key === 'End') return n - 1;
  const d = { ArrowDown: 1, ArrowRight: 1, ArrowUp: -1, ArrowLeft: -1 }[key];
  return d ? (i + d + n) % n : -1;
}

export function initViewer(root, start) {
  const tabs = [...root.querySelectorAll('[role="tab"]')];
  const panels = tabs.map(t => document.getElementById(t.getAttribute('aria-controls')));
  let cur = -1;
  const pick = (i, user) => {
    if (i === cur) return;
    if (user) root.classList.add('touched');
    panels.forEach((p, k) => {
      p.classList.toggle('is-current', k === i); p.classList.toggle('is-prev', k === cur);
      p.inert = k !== i; p.tabIndex = k === i ? 0 : -1;
    });
    tabs.forEach((t, k) => { t.setAttribute('aria-selected', String(k === i)); t.tabIndex = k === i ? 0 : -1; });
    root.dataset.current = tabs[i].dataset.view;
    cur = i;
  };
  panels.forEach((p, k) => { p.setAttribute('role', 'tabpanel'); p.setAttribute('aria-labelledby', tabs[k].id); });
  tabs.forEach((t, i) => {
    t.addEventListener('click', () => pick(i, true));
    t.addEventListener('keydown', e => {
      const j = tabStep(e.key, i, tabs.length);
      if (j < 0) return;
      e.preventDefault(); pick(j, true); tabs[j].focus();
    });
  });
  root.querySelector('[role="tablist"]').hidden = false;
  root.classList.add('is-tabs');
  pick(Math.max(0, tabs.findIndex(t => t.dataset.view === start)), false);
}
