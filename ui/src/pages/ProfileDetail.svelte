<script lang="ts">
  import { onMount } from 'svelte';
  import { api, profileInput, type ProfileMode, type ProfileTool, type ToolPolicy } from '../lib/api';
  import ConfirmButton from '../lib/ConfirmButton.svelte';
  import CopyButton from '../lib/CopyButton.svelte';
  import { hub, profileById, reloadProfiles } from '../lib/hub.svelte';
  import { navigate } from '../lib/router.svelte';

  let { id }: { id: string } = $props();

  const p = $derived(profileById(id));
  let token = $state('');
  let reveal = $state(false);
  let tools = $state<ProfileTool[]>([]);
  let client = $state<'claude-code' | 'cursor' | 'vscode' | 'claude-desktop'>('claude-code');
  let editing = $state(false);
  let name = $state('');
  let slug = $state('');
  let error = $state('');

  onMount(async () => {
    token = await api.profileToken(id);
    tools = await api.profileTools(id);
  });

  // Refresh tool rows when membership or connections change.
  $effect(() => {
    void p?.connection_ids.length;
    void hub.connections.map((c) => c.tool_count).join();
    api.profileTools(id).then((t) => (tools = t));
  });

  const masked = $derived(reveal ? token : token.slice(0, 4) + '•'.repeat(18));
  const serverName = $derived(`plexus-${p?.slug}`);

  function snippet(tok: string): string {
    if (!p) return '';
    const auth = `Bearer ${tok}`;
    switch (client) {
      case 'claude-code':
        return `claude mcp add --transport http --scope user ${serverName} ${p.url} \\\n  --header "Authorization: ${auth}"`;
      case 'cursor':
        return JSON.stringify({ mcpServers: { [serverName]: { url: p.url, headers: { Authorization: auth } } } }, null, 2);
      case 'vscode':
        return JSON.stringify(
          { servers: { [serverName]: { type: 'http', url: p.url, headers: { Authorization: auth } } } },
          null,
          2,
        );
      case 'claude-desktop':
        return JSON.stringify(
          {
            mcpServers: {
              [serverName]: {
                command: 'npx',
                args: ['-y', 'mcp-remote', p.url, '--header', 'Authorization:${PLEXUS_AUTH}'],
                env: { PLEXUS_AUTH: auth },
              },
            },
          },
          null,
          2,
        );
    }
  }

  const clients = [
    ['claude-code', 'Claude Code', 'Run in a terminal.'],
    ['cursor', 'Cursor', 'Merge into ~/.cursor/mcp.json.'],
    ['vscode', 'VS Code', 'Merge into your user mcp.json (Command Palette → MCP: Open User Configuration).'],
    ['claude-desktop', 'Claude Desktop', 'Merge into claude_desktop_config.json. Needs Node.js, since Desktop bridges HTTP through mcp-remote.'],
  ] as const;

  let installing = $state(false);
  let installMsg = $state<{ ok: boolean; text: string } | null>(null);

  async function install() {
    installing = true;
    installMsg = null;
    try {
      installMsg = { ok: true, text: await api.installClient(id, client) };
    } catch (e) {
      installMsg = { ok: false, text: String(e) };
    } finally {
      installing = false;
    }
  }

  async function setMode(mode: ProfileMode) {
    if (!p) return;
    await api.updateProfile(id, { ...profileInput(p), mode });
    await reloadProfiles();
  }

  async function setPolicy(t: ProfileTool, policy: ToolPolicy) {
    await api.setToolPolicy(id, t.connection_id, t.tool, policy);
    t.policy = policy;
  }

  async function setAll(connectionId: string, policy: ToolPolicy) {
    for (const t of tools.filter((t) => t.connection_id === connectionId)) await setPolicy(t, policy);
  }

  async function rotate() {
    token = await api.rotateProfileToken(id);
    reveal = true;
  }

  async function saveMeta(e: SubmitEvent) {
    e.preventDefault();
    error = '';
    try {
      await api.updateProfile(id, { ...profileInput(p!), name: name.trim(), slug: slug.trim() });
      await reloadProfiles();
      editing = false;
    } catch (err) {
      error = String(err);
    }
  }

  async function remove() {
    await api.deleteProfile(id);
    await reloadProfiles();
    navigate('/profiles');
  }

  const groups = $derived.by(() => {
    const m = new Map<string, ProfileTool[]>();
    for (const t of tools) m.set(t.connection_id, [...(m.get(t.connection_id) ?? []), t]);
    return [...m.values()];
  });

  const policies: [ToolPolicy, string][] = [
    ['allow', 'Allow'],
    ['approval', 'Ask'],
    ['hidden', 'Hide'],
  ];
</script>

{#if !p}
  <p class="pt-6 text-muted">This profile no longer exists. <a href="#/profiles" class="underline">Back to profiles</a></p>
{:else}
  <header class="pb-6 pt-2">
    <a href="#/profiles" class="text-[12px] text-muted hover:text-ink">Profiles</a>
    {#if editing}
      <form class="mt-1 flex items-end gap-3" onsubmit={saveMeta}>
        <label><span class="label">Name</span><input class="field" bind:value={name} required /></label>
        <label><span class="label">URL slug</span><input class="field font-mono" bind:value={slug} required /></label>
        <button class="btn btn-primary">Save</button>
        <button type="button" class="btn btn-ghost" onclick={() => (editing = false)}>Cancel</button>
      </form>
      {#if error}<p class="mt-2 text-[12.5px] text-fail">{error}</p>{/if}
    {:else}
      <div class="mt-1 flex items-end justify-between">
        <h1 class="page-title">{p.name}</h1>
        <div class="flex gap-2">
          <button class="btn" onclick={() => ((name = p.name), (slug = p.slug), (editing = true))}>Rename</button>
          <ConfirmButton label="Delete" confirmLabel="Delete profile" warning="Clients using this endpoint disconnect." onconfirm={remove} />
        </div>
      </div>
    {/if}
  </header>

  <section class="rounded-xl border border-grid bg-surface p-5">
    <div class="grid grid-cols-[88px_1fr_auto] items-center gap-x-4 gap-y-3">
      <span class="text-[12px] text-muted">Endpoint</span>
      <span class="selectable truncate font-mono text-[12.5px]">{p.url}</span>
      <div><CopyButton value={p.url} /></div>
      <span class="text-[12px] text-muted">Token</span>
      <button class="selectable truncate text-left font-mono text-[12.5px]" onclick={() => (reveal = !reveal)} title="Click to {reveal ? 'hide' : 'show'}"
        >{masked}</button
      >
      <div class="flex gap-2">
        <CopyButton value={token} />
        <ConfirmButton
          class="btn btn-ghost"
          label="Rotate"
          confirmLabel="Rotate token"
          warning="Clients need the new token."
          onconfirm={rotate}
        />
      </div>
    </div>
  </section>

  <section class="mt-8">
    <h2 class="font-display text-[15px] font-semibold">Connect a client</h2>
    <div class="mt-3 flex gap-1 text-[12.5px] font-medium">
      {#each clients as [k, label] (k)}
        <button
          class="rounded-md px-2.5 py-1 {client === k ? 'bg-violet-soft text-violet' : 'text-muted hover:text-ink'}"
          onclick={() => ((client = k), (installMsg = null))}>{label}</button
        >
      {/each}
    </div>
    <p class="mt-2 text-[12px] text-muted">{clients.find((c) => c[0] === client)?.[2]}</p>
    <div class="mt-2 flex items-start gap-3">
      <pre class="selectable flex-1 overflow-x-auto rounded-lg border border-grid bg-paper px-3.5 py-3 font-mono text-[11.5px] leading-relaxed">{snippet(
          reveal ? token : masked,
        )}</pre>
      <div class="flex flex-col gap-2">
        <button class="btn btn-primary" onclick={install} disabled={installing}>
          {installing ? 'Adding…' : `Add to ${clients.find((c) => c[0] === client)?.[1]}`}
        </button>
        <CopyButton value={async () => snippet(token)} />
      </div>
    </div>
    {#if installMsg}
      <p class="mt-2 text-[12.5px] {installMsg.ok ? 'text-live' : 'text-fail'}">{installMsg.text}</p>
    {/if}
  </section>

  <section class="mt-8">
    <h2 class="font-display text-[15px] font-semibold">How tools are offered</h2>
    <div class="mt-3 grid grid-cols-2 gap-2.5">
      {#each [['direct', 'Every tool', 'The client sees each tool directly, prefixed with its alias. Best with a few connections.'], ['meta', 'Search & call', 'The client gets four tools (list, search, describe, call) and looks tools up on demand. Keeps context small with many connections.']] as [m, title, desc] (m)}
        <button
          class="rounded-xl border p-3.5 text-left transition-colors {p.mode === m
            ? 'border-violet bg-violet-soft/50'
            : 'border-grid bg-surface hover:border-violet/50'}"
          onclick={() => setMode(m as ProfileMode)}
        >
          <span class="font-medium">{title}</span>
          <span class="mt-0.5 block text-[12px] leading-snug text-muted">{desc}</span>
        </button>
      {/each}
    </div>
  </section>

  <section class="mt-8">
    <h2 class="font-display text-[15px] font-semibold">Tool access</h2>
    <p class="mt-1 text-[12px] text-muted">
      Ask opens a prompt on this Mac before the call runs. Hidden tools are invisible to the client.
    </p>
    {#if groups.length === 0}
      <p class="mt-4 text-muted">
        No tools yet. Route connections into this profile on the <a href="#/profiles" class="text-violet hover:underline">Profiles</a> page, then test them.
      </p>
    {/if}
    {#each groups as g (g[0].connection_id)}
      <div class="mt-4 overflow-hidden rounded-xl border border-grid bg-surface">
        <div class="flex items-center justify-between border-b border-grid bg-paper/50 px-4 py-2">
          <span class="font-medium">{g[0].connection_name} <span class="font-mono text-[11.5px] font-normal text-muted">{g[0].connection_alias}</span></span>
          <select class="field w-auto py-0.5 text-[12px]" onchange={(e) => setAll(g[0].connection_id, e.currentTarget.value as ToolPolicy)}>
            <option value="" selected disabled>Set all…</option>
            {#each policies as [v, label] (v)}<option value={v}>{label}</option>{/each}
          </select>
        </div>
        <ul class="divide-y divide-grid">
          {#each g as t (t.tool)}
            <li class="flex items-center gap-4 px-4 py-2 {t.policy === 'hidden' ? 'opacity-55' : ''}">
              <div class="min-w-0 flex-1">
                <div class="selectable truncate font-mono text-[12px]">{t.tool}</div>
                {#if t.description}<div class="truncate text-[11.5px] text-muted">{t.description}</div>{/if}
              </div>
              <div class="inline-flex shrink-0 rounded-md border border-grid bg-paper p-0.5 text-[11.5px] font-medium">
                {#each policies as [v, label] (v)}
                  <button
                    class="rounded px-2 py-0.5 {t.policy === v
                      ? v === 'hidden'
                        ? 'bg-surface text-muted shadow-sm'
                        : v === 'approval'
                          ? 'bg-surface text-warn shadow-sm'
                          : 'bg-surface text-ink shadow-sm'
                      : 'text-muted hover:text-ink'}"
                    onclick={() => setPolicy(t, v)}>{label}</button
                  >
                {/each}
              </div>
            </li>
          {/each}
        </ul>
      </div>
    {/each}
  </section>
{/if}
