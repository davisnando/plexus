<script lang="ts">
  import { clock, timeAgo } from '../lib/format';
  import { hub } from '../lib/hub.svelte';

  let status = $state<'all' | 'problems'>('all');
  let profile = $state('');

  const conn = (id: string) => hub.connections.find((c) => c.id === id);
  const prof = (id: string) => hub.profiles.find((p) => p.id === id);
  const rows = $derived(
    hub.calls.filter(
      (c) => (status === 'all' || c.status !== 'ok') && (!profile || c.profile_id === profile),
    ),
  );
  const tone = { ok: 'text-live', error: 'text-fail', denied: 'text-warn', timeout: 'text-warn' };
  const word = { ok: 'OK', error: 'Failed', denied: 'Denied', timeout: 'No answer' };
</script>

<header class="flex items-end justify-between pb-6 pt-2">
  <div>
    <h1 class="page-title">Activity</h1>
    <p class="mt-1 text-muted">Tool calls made through Plexus. Only names and timings are kept, never arguments or results.</p>
  </div>
  <div class="flex gap-2">
    <select class="field w-auto" bind:value={profile}>
      <option value="">All profiles</option>
      {#each hub.profiles as p (p.id)}<option value={p.id}>{p.name}</option>{/each}
    </select>
    <select class="field w-auto" bind:value={status}>
      <option value="all">All calls</option>
      <option value="problems">Problems only</option>
    </select>
  </div>
</header>

{#if rows.length === 0}
  <p class="rounded-xl border border-dashed border-grid px-8 py-12 text-center text-muted">
    {hub.calls.length ? 'No calls match these filters.' : 'No tool calls yet. Calls appear here live once a client uses a profile.'}
  </p>
{:else}
  <div class="overflow-hidden rounded-xl border border-grid bg-surface">
    <table class="w-full text-left text-[12.5px]">
      <thead class="border-b border-grid text-[11.5px] text-muted">
        <tr>
          <th class="px-4 py-2 font-medium">Time</th>
          <th class="px-2 py-2 font-medium">Tool</th>
          <th class="px-2 py-2 font-medium">Profile</th>
          <th class="px-2 py-2 text-right font-medium">Duration</th>
          <th class="px-4 py-2 text-right font-medium">Result</th>
        </tr>
      </thead>
      <tbody class="divide-y divide-grid">
        {#each rows as c (c.id)}
          <tr class="align-top">
            <td class="whitespace-nowrap px-4 py-2 tabular-nums text-muted" title={timeAgo(c.ts)}>{clock(c.ts)}</td>
            <td class="px-2 py-2">
              <span class="font-mono text-violet">{conn(c.connection_id)?.alias ?? 'deleted'}</span><span class="font-mono"
                >__{c.tool}</span
              >
              {#if c.error}<div class="selectable mt-0.5 line-clamp-2 max-w-[60ch] text-[11.5px] text-muted">{c.error}</div>{/if}
            </td>
            <td class="px-2 py-2 text-muted">{prof(c.profile_id)?.name ?? '—'}</td>
            <td class="px-2 py-2 text-right tabular-nums text-muted">{c.duration_ms} ms</td>
            <td class="px-4 py-2 text-right font-medium {tone[c.status]}">{word[c.status]}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
{/if}
