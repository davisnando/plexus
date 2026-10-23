// Minimal hash router: `#/connections/abc` -> path `/connections/abc`.
function current(): string {
  return location.hash.replace(/^#/, '') || '/connections';
}

export const route = $state({ path: current() });

window.addEventListener('hashchange', () => {
  route.path = current();
});

export function navigate(path: string) {
  location.hash = path;
}

/** Match `/connections/:id` style patterns; returns params or null. */
export function match(pattern: string, path = route.path): Record<string, string> | null {
  const p = pattern.split('/');
  const s = path.split('?')[0].split('/');
  if (p.length !== s.length) return null;
  const params: Record<string, string> = {};
  for (let i = 0; i < p.length; i++) {
    if (p[i].startsWith(':')) params[p[i].slice(1)] = decodeURIComponent(s[i]);
    else if (p[i] !== s[i]) return null;
  }
  return params;
}
