import type { Scope, Transport } from './api';

export function transportSummary(t: Transport): string {
  if (t.kind === 'stdio') return [t.command, ...t.args].join(' ');
  try {
    const u = new URL(t.url);
    return u.host + (u.pathname === '/' ? '' : u.pathname);
  } catch {
    return t.url;
  }
}

export function timeAgo(ts: number): string {
  const s = Math.max(0, Math.floor(Date.now() / 1000 - ts));
  if (s < 60) return `${s}s ago`;
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  return new Date(ts * 1000).toLocaleDateString();
}

export function clock(ts: number): string {
  return new Date(ts * 1000).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });
}

export function scopeLabel(scope: Scope): string {
  if (scope.kind === 'everywhere') return 'everywhere';
  const n = scope.folders.length;
  return n ? `${n} folder${n > 1 ? 's' : ''}` : 'no folders';
}
