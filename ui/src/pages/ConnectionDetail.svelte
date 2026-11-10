<script lang="ts">
  import { onMount } from 'svelte';
  import { api, toInput, type ToolInfo } from '../lib/api';
  import { aliasError, catalog, normalizeAlias } from '../lib/catalog';
  import { transportSummary } from '../lib/format';
  import { connectionById, reloadConnections } from '../lib/hub.svelte';
  import { navigate, route } from '../lib/router.svelte';
  import ConfirmButton from '../lib/ConfirmButton.svelte';
  import ScopeEditor from '../lib/ScopeEditor.svelte';
  import StatusDot from '../lib/StatusDot.svelte';
  import TransportEditor from '../lib/TransportEditor.svelte';

  let { id }: { id: string } = $props();

  const c = $derived(connectionById(id));
  let tab = $state<'tools' | 'settings'>('tools');
  let tools = $state<ToolInfo[]>([]);
  let filter = $state('');
  let busy = $state('');
  let notice = $state('');
  let error = $state('');

  // Settings form
  let name = $state('');
  let alias = $state('');
  let idle = $state(10);
  let editor = $state<ReturnType<typeof TransportEditor>>();

  async function loadTools() {
    tools = await api.connectionTools(id);
  }

  function resetForm() {
    if (!c) return;
    name = c.name;
    alias = c.alias;
    idle = Math.round(c.idle_timeout_secs / 60);
  }

  onMount(() => {
    resetForm();
    loadTools();
    if (route.path.includes('test=1')) test();
  });

  // Tool list changes when the upstream (re)announces its tools.
  $effect(() => {
    void c?.tool_count;
    loadTools();
  });

  async function run(label: string, fn: () => Promise<void>) {
    busy = label;
    error = '';
    notice = '';
    try {
      await fn();
    } catch (e) {
      error = String(e);
    } finally {
      busy = '';
    }
  }

  const test = () =>
    run('test', async () => {
      const n = await api.testConnection(id);
      notice = `Connected. ${n} tool${n === 1 ? '' : 's'} available.`;
      await reloadConnections();
      await loadTools();
    });

  const stop = () => run('stop', () => api.stopConnection(id));

  const signIn = () =>
    run('signin', async () => {
      // A remote server answering 401 without OAuth configured: switch it over.
      if (c!.transport.kind === 'http' && c!.transport.auth !== 'oauth') {
        await api.updateConnection(id, { ...toInput(c!), transport: { ...c!.transport, auth: 'oauth' } });
        await reloadConnections();
      }
      await api.startOauth(id);
      notice = 'Finish signing in in your browser. Plexus connects as soon as you approve.';
    });

  const signOut = () =>
    run('signout', async () => {
      await api.signOut(id);
      await reloadConnections();
    });

  const toggle = () =>
    run('toggle', async () => {
      await api.updateConnection(id, { ...toInput(c!), enabled: !c!.enabled });
      await reloadConnections();
    });

  const remove = () =>
    run('delete', async () => {
      await api.deleteConnection(id);
      await reloadConnections();
      navigate('/connections');
    });

  const save = (e: SubmitEvent) => {
    e.preventDefault();
    return run('save', async () => {
      alias = alias.replace(/_+$/, '');
      const problem = aliasError(alias);
      if (problem) throw problem;
      const built = editor!.build();
      await api.updateConnection(id, {
        ...toInput(c!),
        name: name.trim(),
        alias: alias.trim(),
        idle_timeout_secs: Math.max(1, idle) * 60,
        ...built,
      });
      await reloadConnections();
      notice = 'Saved.';
      tab = 'tools';
    });
  };

  const shown = $derived(
    tools.filter(
      (t) =>
        !filter ||
        t.name.toLowerCase().includes(filter.toLowerCase()) ||
        t.description?.toLowerCase().includes(filter.toLowerCase()),
    ),
  );
  const template = $derived(catalog.find((t) => t.id === c?.template_id));
  const isOauth = $derived(
    c?.transport.kind === 'http' &&
      (c.transport.auth === 'oauth' || (c.transport.auth === 'none' && c.status.state === 'needs_auth')),
  );
  const signedIn = $derived(!!c?.secrets_present.includes('oauth'));
</script>

{#if !c}
  <p class="pt-6 text-muted">This connection no longer exists. <a href="#/connections" class="underline">Back to connections</a></p>
{:else}
  <header class="pb-5 pt-2">
    <a href="#/connections" class="text-[12px] text-muted hover:text-ink">Connections</a>
    <div class="mt-1 flex items-start justify-between gap-6">
      <div class="min-w-0">
        <h1 class="page-title truncate">{c.name}</h1>
        <div class="mt-1.5 flex flex-wrap items-center gap-x-4 gap-y-1 text-[12px] text-muted">
          <StatusDot status={c.status} label />
          <span class="selectable font-mono text-violet">{c.alias}</span>
          <span class="selectable truncate font-mono">{transportSummary(c.transport)}</span>
          {#if template}<span>{template.name}</span>{/if}
        </div>
      </div>
      <div class="flex shrink-0 gap-2">
        {#if c.status.state === 'running'}
          <button class="btn" onclick={stop} disabled={!!busy}>Stop</button>
        {/if}
        {#if isOauth && signedIn}
          <button class="btn" onclick={signOut} disabled={!!busy}>Sign out</button>
        {/if}
        <button class="btn" onclick={toggle} disabled={!!busy}>{c.enabled ? 'Disable' : 'Enable'}</button>
        {#if isOauth && (!signedIn || c.status.state === 'needs_auth')}
          <button class="btn btn-primary" onclick={signIn} disabled={!!busy || !c.enabled}>
            {signedIn ? 'Sign in again' : 'Sign in'}
          </button>
        {:else}
          <button class="btn btn-primary" onclick={test} disabled={!!busy || !c.enabled}>
            {busy === 'test' ? 'Connecting…' : 'Test connection'}
          </button>
        {/if}
      </div>
    </div>
  </header>

  {#if c.status.state === 'error'}
    <pre
      class="selectable mb-5 max-h-48 overflow-auto whitespace-pre-wrap rounded-lg border border-fail/40 bg-fail/5 px-3.5 py-2.5 font-mono text-[11.5px] leading-relaxed text-fail">{c.status.message}</pre>
  {:else if c.status.state === 'needs_auth'}
    <div class="mb-5 rounded-lg border border-warn/40 bg-warn/5 px-3.5 py-2.5 text-[12.5px]">
      This server needs you to sign in before Plexus can use it. Signed in to a different account of this service in your
      browser? Sign out there first, or copy the sign-in link into a private window.
    </div>
  {/if}
  {#if error}<p class="mb-5 rounded-lg border border-fail/40 px-3.5 py-2.5 text-[12.5px] text-fail">{error}</p>{/if}
  {#if notice}<p class="mb-5 text-[12.5px] text-live">{notice}</p>{/if}

  <section class="mb-6 rounded-xl border border-grid bg-surface px-4 py-3.5">
    <h2 class="mb-2 text-[12px] font-medium text-muted">Available to clients working in</h2>
    <ScopeEditor connection={c} />
  </section>

  <div class="mb-4 flex items-center gap-5 border-b border-grid text-[13px] font-medium">
    {#each [['tools', `Tools (${tools.length})`], ['settings', 'Settings']] as [k, label] (k)}
      <button
        class="-mb-px border-b-2 pb-2 {tab === k ? 'border-violet text-ink' : 'border-transparent text-muted hover:text-ink'}"
        onclick={() => {
          tab = k as 'tools' | 'settings';
          if (k === 'settings') resetForm();
        }}>{label}</button
      >
    {/each}
  </div>

  {#if tab === 'tools'}
    {#if tools.length === 0}
      <p class="py-8 text-center text-muted">
        {c.status.state === 'error' ? 'Fix the error above, then test the connection.' : 'No tools discovered yet. Test the connection to load them.'}
      </p>
    {:else}
      {#if tools.length > 8}
        <input class="field mb-3 max-w-[280px]" placeholder="Filter tools" bind:value={filter} />
      {/if}
      <ul class="divide-y divide-grid rounded-xl border border-grid bg-surface">
        {#each shown as t (t.name)}
          <li class="px-4 py-2.5">
            <div class="flex items-center gap-2">
              <span class="selectable font-mono text-[12.5px]">{t.exposed_name}</span>
              {#if t.destructive}<span class="rounded bg-fail/10 px-1.5 text-[11px] text-fail">destructive</span>{/if}
              {#if t.read_only}<span class="rounded bg-live/10 px-1.5 text-[11px] text-live">read-only</span>{/if}
            </div>
            {#if t.description}
              <p class="mt-0.5 line-clamp-2 max-w-[78ch] text-[12px] text-muted">{t.description}</p>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}
  {:else}
    <form class="flex max-w-[560px] flex-col gap-5" onsubmit={save}>
      <div class="grid grid-cols-[1.4fr_1fr_auto] gap-3">
        <label><span class="label">Name</span><input class="field" bind:value={name} required /></label>
        <label>
          <span class="label">Alias</span>
          <input
            class="field font-mono"
            value={alias}
            oninput={(e) => (alias = e.currentTarget.value = normalizeAlias(e.currentTarget.value))}
            maxlength="24"
            autocapitalize="off"
            autocomplete="off"
            spellcheck="false"
            {...{ autocorrect: 'off' }}
          />
        </label>
        <label>
          <span class="label">Stop after idle</span>
          <span class="flex items-center gap-1.5"
            ><input class="field w-16 tabular-nums" type="number" min="1" bind:value={idle} /> min</span
          >
        </label>
      </div>
      <TransportEditor bind:this={editor} initial={c.transport} secretsPresent={c.secrets_present} />
      <div class="flex items-center gap-2">
        <button class="btn btn-primary" disabled={!!busy}>Save changes</button>
        <span class="ml-auto">
          <ConfirmButton
            label="Delete connection"
            confirmLabel="Delete"
            warning="Also removes its secrets from the Keychain."
            onconfirm={remove}
            disabled={!!busy}
          />
        </span>
      </div>
    </form>
  {/if}
{/if}
