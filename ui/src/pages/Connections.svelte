<script lang="ts">
  import { hub } from '../lib/hub.svelte';
  import { catalog } from '../lib/catalog';
  import { transportSummary } from '../lib/format';
  import StatusDot from '../lib/StatusDot.svelte';

  // Group instances of the same service so "3 × Jira" reads as one family.
  const groups = $derived.by(() => {
    const byKey = new Map<string, typeof hub.connections>();
    for (const c of hub.connections) {
      const key = c.template_id ?? `custom:${c.id}`;
      byKey.set(key, [...(byKey.get(key) ?? []), c]);
    }
    return [...byKey.entries()].map(([key, items]) => ({
      key,
      template: catalog.find((t) => t.id === key),
      items,
    }));
  });

  const profileCount = (id: string) => hub.profiles.filter((p) => p.connection_ids.includes(id)).length;
</script>

<header class="flex items-end justify-between pb-6 pt-2">
  <div>
    <h1 class="page-title">Connections</h1>
    <p class="mt-1 text-muted">MCP servers Plexus can reach. Add the same service more than once for different accounts.</p>
  </div>
  <a href="#/connections/new" class="btn btn-primary">Add connection</a>
</header>

{#if hub.connections.length === 0}
  <div class="rounded-xl border border-dashed border-grid px-8 py-14 text-center">
    <p class="font-display text-[17px] font-semibold">Nothing connected yet</p>
    <p class="mx-auto mt-1.5 max-w-[46ch] text-muted">
      Start with Jira, Sentry, GitHub or any MCP server. Each connection gets an alias, so two Jira sites show up as
      <span class="font-mono text-ink">jira_acme__…</span> and <span class="font-mono text-ink">jira_client__…</span>.
    </p>
    <a href="#/connections/new" class="btn btn-primary mt-5">Add your first connection</a>
  </div>
{:else}
  <div class="flex flex-col gap-5">
    {#each groups as g (g.key)}
      <section>
        {#if g.template && g.items.length > 1}
          <h2 class="mb-1.5 px-1 text-[12px] font-medium text-muted">
            {g.template.name} · {g.items.length} connections
          </h2>
        {/if}
        <ul class="overflow-hidden rounded-xl border border-grid bg-surface">
          {#each g.items as c (c.id)}
            <li class="border-b border-grid last:border-b-0">
              <a href="#/connections/{c.id}" class="grid grid-cols-[16px_1fr_auto] items-center gap-3 px-4 py-3 hover:bg-paper/60">
                <StatusDot status={c.status} />
                <div class="min-w-0">
                  <div class="flex items-baseline gap-2">
                    <span class="truncate font-medium">{c.name}</span>
                    <span class="font-mono text-[12px] text-violet">{c.alias}</span>
                  </div>
                  <div class="mt-0.5 truncate font-mono text-[11.5px] text-muted">{transportSummary(c.transport)}</div>
                  {#if c.status.state === 'error'}
                    <div class="mt-1 truncate text-[12px] text-fail">{c.status.message.split('\n')[0]}</div>
                  {:else if c.status.state === 'needs_auth'}
                    <div class="mt-1 text-[12px] text-warn">Sign in required</div>
                  {/if}
                </div>
                <div class="flex items-center gap-5 text-[12px] text-muted tabular-nums">
                  <span>{c.tool_count} tools</span>
                  <span class="w-[72px] text-right">
                    {#if profileCount(c.id)}in {profileCount(c.id)} profile{profileCount(c.id) > 1 ? 's' : ''}{:else}<span class="text-warn">no profile</span>{/if}
                  </span>
                </div>
              </a>
            </li>
          {/each}
        </ul>
      </section>
    {/each}
  </div>
{/if}
