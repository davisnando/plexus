<script lang="ts">
  import { api, profileInput, type ConnectionInput, type Transport } from '../lib/api';
  import { aliasError, catalog, fromTemplate, normalizeAlias, slugify, suggestAlias, type Template } from '../lib/catalog';
  import { hub, reloadConnections } from '../lib/hub.svelte';
  import { navigate } from '../lib/router.svelte';
  import { searchRegistry, type RegistryResult } from '../lib/registry';
  import TransportEditor from '../lib/TransportEditor.svelte';

  const customStdio: Transport = { kind: 'stdio', command: '', args: [], env: {}, secret_env: [] };

  let template = $state<Template | null>(null);
  let custom = $state(false);
  let name = $state('');
  let alias = $state('');
  let aliasTouched = $state(false);
  let checkAlias = $state(false);
  let values = $state<Record<string, string>>({});
  let profileIds = $state<string[]>(hub.profiles.length === 1 ? [hub.profiles[0].id] : []);
  let error = $state('');
  let saving = $state(false);
  let editor = $state<ReturnType<typeof TransportEditor>>();

  const taken = $derived(hub.connections.map((c) => c.alias));

  let query = $state('');
  let results = $state<RegistryResult[]>([]);
  let searching = $state(false);
  let searchError = $state('');

  // Debounced registry search; aborts superseded requests.
  $effect(() => {
    const q = query.trim();
    if (q.length < 2) {
      results = [];
      return;
    }
    const ctrl = new AbortController();
    const timer = setTimeout(async () => {
      searching = true;
      searchError = '';
      try {
        results = await searchRegistry(q, ctrl.signal);
      } catch (e) {
        if (!ctrl.signal.aborted) searchError = `Couldn't reach the MCP Registry: ${e}`;
      } finally {
        if (!ctrl.signal.aborted) searching = false;
      }
    }, 300);
    return () => {
      clearTimeout(timer);
      ctrl.abort();
    };
  });
  const existing = (t: Template) => hub.connections.filter((c) => c.template_id === t.id).length;

  function pick(t: Template | null) {
    template = t;
    custom = !t;
    values = Object.fromEntries((t?.fields ?? []).filter((f) => f.default).map((f) => [f.key, f.default!]));
    error = '';
    name = t ? (existing(t) ? `${t.name} ${existing(t) + 1}` : t.name) : '';
    alias = t ? suggestAlias(t.alias, taken) : '';
    aliasTouched = false;
  }

  function onName() {
    if (!aliasTouched && custom) alias = suggestAlias(slugify(name) || 'server', taken);
  }

  async function save(e: SubmitEvent) {
    e.preventDefault();
    error = '';
    alias = alias.replace(/_+$/, '');
    checkAlias = true;
    if (aliasError(alias)) return;
    let input: ConnectionInput;
    if (template) {
      const missing = template.fields.filter((f) => !f.optional && !values[f.key]?.trim());
      if (missing.length) {
        error = `Fill in ${missing.map((f) => f.label).join(', ')}.`;
        return;
      }
      input = fromTemplate(template, values, name.trim(), alias.trim());
    } else {
      const built = editor!.build();
      input = { alias: alias.trim(), name: name.trim(), template_id: null, enabled: true, ...built };
    }
    saving = true;
    try {
      const created = await api.createConnection(input);
      for (const id of profileIds) {
        const p = hub.profiles.find((p) => p.id === id)!;
        const next = profileInput(p);
        next.connection_ids.push(created.id);
        await api.updateProfile(id, next);
      }
      await reloadConnections();
      if (input.transport.kind === 'http' && input.transport.auth === 'oauth') {
        // Go straight to the browser; discovery runs once sign-in completes.
        await api.startOauth(created.id).catch(() => {});
        navigate(`/connections/${created.id}`);
      } else {
        navigate(`/connections/${created.id}?test=1`);
      }
    } catch (err) {
      error = String(err);
    } finally {
      saving = false;
    }
  }
</script>

<header class="pb-6 pt-2">
  <a href="#/connections" class="text-[12px] text-muted hover:text-ink">Connections</a>
  <h1 class="page-title mt-1">{template ? `Add ${template.name}` : custom ? 'Add a custom server' : 'Add a connection'}</h1>
</header>

{#if !template && !custom}
  <div class="grid grid-cols-2 gap-2.5 lg:grid-cols-3">
    {#each catalog as t (t.id)}
      <button
        class="group flex items-start gap-3 rounded-xl border border-grid bg-surface p-3.5 text-left transition-colors hover:border-violet/60"
        onclick={() => pick(t)}
      >
        <span
          class="grid size-9 shrink-0 place-items-center rounded-lg bg-violet-soft font-display text-[13px] font-semibold text-violet"
          >{t.mark}</span
        >
        <span class="min-w-0">
          <span class="flex items-baseline gap-2">
            <span class="font-medium">{t.name}</span>
            {#if existing(t)}<span class="text-[11px] text-muted">{existing(t)} added</span>{/if}
          </span>
          <span class="mt-0.5 block text-[12px] leading-snug text-muted">{t.description}</span>
        </span>
      </button>
    {/each}
    <button
      class="flex items-start gap-3 rounded-xl border border-dashed border-grid p-3.5 text-left transition-colors hover:border-violet/60"
      onclick={() => pick(null)}
    >
      <span class="grid size-9 shrink-0 place-items-center rounded-lg border border-grid font-mono text-[13px] text-muted">{'{}'}</span>
      <span>
        <span class="font-medium">Any MCP server</span>
        <span class="mt-0.5 block text-[12px] leading-snug text-muted">A local command (npx, uvx, a binary) or a remote URL.</span>
      </span>
    </button>
  </div>

  <section class="mt-10">
    <h2 class="font-display text-[15px] font-semibold">Search the MCP Registry</h2>
    <p class="mt-0.5 text-[12px] text-muted">Thousands of community and vendor servers from registry.modelcontextprotocol.io.</p>
    <input class="field mt-3 max-w-[420px]" placeholder="Try “postgres”, “slack” or “kubernetes”" bind:value={query} />
    {#if searchError}<p class="mt-2 text-[12.5px] text-fail">{searchError}</p>{/if}
    {#if searching && !results.length}<p class="mt-3 text-muted">Searching…</p>{/if}
    {#if results.length}
      <ul class="mt-3 divide-y divide-grid rounded-xl border border-grid bg-surface">
        {#each results as r (r.name)}
          <li class="flex items-start gap-4 px-4 py-3">
            <div class="min-w-0 flex-1">
              <div class="flex items-baseline gap-2">
                <span class="font-medium">{r.title}</span>
                <span class="truncate font-mono text-[11px] text-muted">{r.name}</span>
              </div>
              {#if r.description}<p class="mt-0.5 line-clamp-2 text-[12px] text-muted">{r.description}</p>{/if}
            </div>
            <div class="flex shrink-0 gap-1.5">
              {#each r.options as o, i (i)}
                <button class="btn py-1 text-[12px]" onclick={() => pick(o.template)}>{o.label}{r.options.filter((x) => x.label === o.label).length > 1 ? ' ' + (r.options.filter((x, j) => x.label === o.label && j <= i).length) : ''}</button>
              {/each}
            </div>
          </li>
        {/each}
      </ul>
    {:else if query.trim().length >= 2 && !searching && !searchError}
      <p class="mt-3 text-muted">No servers match “{query}”.</p>
    {/if}
  </section>
{:else}
  <form class="flex max-w-[560px] flex-col gap-5" onsubmit={save}>
    <div class="grid grid-cols-[1.4fr_1fr] gap-3">
      <label>
        <span class="label">Name</span>
        <input class="field" bind:value={name} oninput={onName} placeholder="Jira Acme" required />
      </label>
      <label>
        <span class="label">Alias</span>
        <input
          class="field font-mono"
          value={alias}
          oninput={(e) => {
            aliasTouched = true;
            alias = e.currentTarget.value = normalizeAlias(e.currentTarget.value);
          }}
          maxlength="24"
          autocapitalize="off"
          autocomplete="off"
          spellcheck="false"
          {...{ autocorrect: 'off' }}
        />
      </label>
    </div>
    {#if checkAlias && aliasError(alias)}<p class="-mt-3 text-[12px] text-fail">{aliasError(alias)}</p>{/if}
    <p class="-mt-3 text-[12px] text-muted">
      Tools appear as <span class="font-mono text-ink">{alias || 'alias'}__tool_name</span>. Use something that tells
      accounts apart, like <span class="font-mono">jira_acme</span>.
    </p>

    {#if template}
      {#each template.fields as f (f.key)}
        <label>
          <span class="label">{f.label}{f.optional ? ' (optional)' : ''}</span>
          <input
            class="field {f.target === 'arg' || f.target === 'env' ? '' : 'font-mono'}"
            type={f.target === 'secret_env' || f.target === 'bearer' ? 'password' : 'text'}
            bind:value={values[f.key]}
            placeholder={f.placeholder}
            spellcheck="false"
          />
          {#if f.help}<span class="mt-1 block text-[12px] text-muted">{f.help}</span>{/if}
        </label>
      {/each}
      {#if template.transport.kind === 'http' && template.transport.auth === 'oauth'}
        <p class="rounded-lg bg-violet-soft px-3 py-2.5 text-[12.5px] text-ink/80">
          After saving, use <b>Sign in</b> to open the browser. Already signed in to another {template.name.split(' ')[0]} account?
          Sign out there first or copy the link into a private window.
        </p>
      {/if}
    {:else}
      <TransportEditor bind:this={editor} initial={customStdio} />
    {/if}

    {#if hub.profiles.length}
      <fieldset>
        <span class="label">Add to profiles</span>
        <div class="flex flex-wrap gap-x-4 gap-y-1.5">
          {#each hub.profiles as p (p.id)}
            <label class="flex items-center gap-1.5">
              <input type="checkbox" class="accent-violet" value={p.id} bind:group={profileIds} />
              {p.name}
            </label>
          {/each}
        </div>
      </fieldset>
    {/if}

    {#if error}<p class="rounded-lg border border-fail/40 px-3 py-2 text-[12.5px] text-fail">{error}</p>{/if}

    <div class="flex gap-2">
      <button class="btn btn-primary" disabled={saving}>{saving ? 'Adding…' : 'Add connection'}</button>
      <button type="button" class="btn btn-ghost" onclick={() => ((template = null), (custom = false))}>Back</button>
    </div>
  </form>
{/if}
