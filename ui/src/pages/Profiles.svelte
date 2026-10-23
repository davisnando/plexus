<script lang="ts">
  import { api, profileInput, type Profile, type ProfileMode } from '../lib/api';
  import { normalizeSlug, slugify } from '../lib/catalog';
  import { hub, reloadProfiles } from '../lib/hub.svelte';
  import { navigate } from '../lib/router.svelte';
  import StatusDot from '../lib/StatusDot.svelte';

  let creating = $state(false);
  let name = $state('');
  let slug = $state('');
  let slugTouched = $state(false);
  let mode = $state<ProfileMode>('direct');
  let error = $state('');
  let hover = $state<{ row: string | null; col: string | null }>({ row: null, col: null });
  let pending = $state<string | null>(null);

  async function create(e: SubmitEvent) {
    e.preventDefault();
    error = '';
    try {
      const p = await api.createProfile({
        name: name.trim(),
        slug: slug.trim(),
        mode,
        connection_ids: hub.connections.map((c) => c.id),
      });
      await reloadProfiles();
      creating = false;
      navigate(`/profiles/${p.id}`);
    } catch (err) {
      error = String(err);
    }
  }

  async function toggle(p: Profile, connectionId: string) {
    const key = `${p.id}:${connectionId}`;
    pending = key;
    const next = profileInput(p);
    next.connection_ids = next.connection_ids.includes(connectionId)
      ? next.connection_ids.filter((id) => id !== connectionId)
      : [...next.connection_ids, connectionId];
    try {
      await api.updateProfile(p.id, next);
      await reloadProfiles();
    } finally {
      pending = null;
    }
  }

  function startCreate() {
    creating = true;
    name = hub.profiles.length ? '' : 'Work';
    slug = hub.profiles.length ? '' : 'work';
    slugTouched = false;
    mode = 'direct';
  }
</script>

<header class="flex items-end justify-between pb-6 pt-2">
  <div>
    <h1 class="page-title">Profiles</h1>
    <p class="mt-1 max-w-[62ch] text-muted">
      Each profile is its own endpoint for an AI client. Click where a connection meets a profile to route it there.
    </p>
  </div>
  {#if !creating}<button class="btn btn-primary" onclick={startCreate}>New profile</button>{/if}
</header>

{#if creating}
  <form class="mb-8 grid grid-cols-[1.3fr_1fr_auto_auto] items-end gap-3 rounded-xl border border-grid bg-surface p-4" onsubmit={create}>
    <label>
      <span class="label">Name</span>
      <!-- svelte-ignore a11y_autofocus -->
      <input
        class="field"
        bind:value={name}
        oninput={() => !slugTouched && (slug = slugify(name, '-'))}
        placeholder="Work"
        required
        autofocus
      />
    </label>
    <label>
      <span class="label">URL slug</span>
      <input
        class="field font-mono"
        value={slug}
        oninput={(e) => {
          slugTouched = true;
          slug = e.currentTarget.value = normalizeSlug(e.currentTarget.value);
        }}
        autocapitalize="off"
        autocomplete="off"
        spellcheck="false"
        {...{ autocorrect: 'off' }}
        required
      />
    </label>
    <label>
      <span class="label">Tools shown as</span>
      <select class="field" bind:value={mode}>
        <option value="direct">Every tool</option>
        <option value="meta">Search &amp; call</option>
      </select>
    </label>
    <div class="flex gap-2">
      <button class="btn btn-primary">Create</button>
      <button type="button" class="btn btn-ghost" onclick={() => (creating = false)}>Cancel</button>
    </div>
    <p class="col-span-4 text-[12px] text-muted">
      Endpoint: <span class="font-mono text-ink">http://127.0.0.1:{hub.server.port}/mcp/{slug || 'slug'}</span>. New profiles
      start with every connection routed in.
    </p>
    {#if error}<p class="col-span-4 text-[12.5px] text-fail">{error}</p>{/if}
  </form>
{/if}

{#if hub.profiles.length === 0 && !creating}
  <div class="rounded-xl border border-dashed border-grid px-8 py-14 text-center">
    <p class="font-display text-[17px] font-semibold">No profiles yet</p>
    <p class="mx-auto mt-1.5 max-w-[48ch] text-muted">
      A profile is what you plug into Claude Code, Cursor or VS Code. Make one for work and one for side projects, each
      with its own set of connections.
    </p>
    <button class="btn btn-primary mt-5" onclick={startCreate}>Create a profile</button>
  </div>
{:else if hub.profiles.length}
  <div class="overflow-x-auto rounded-xl border border-grid bg-surface">
    <table class="w-full border-collapse text-left" onmouseleave={() => (hover = { row: null, col: null })}>
      <thead>
        <tr>
          <th class="w-full border-b border-grid px-4 py-3 align-bottom text-[12px] font-medium text-muted">Connection</th>
          {#each hub.profiles as p (p.id)}
            <th
              class="min-w-[112px] border-b border-l border-grid px-3 py-3 text-center align-bottom transition-colors
                {hover.col === p.id ? 'bg-violet-soft/60' : ''}"
            >
              <a href="#/profiles/{p.id}" class="group block">
                <span class="block font-medium group-hover:text-violet">{p.name}</span>
                <span class="block font-mono text-[11px] font-normal text-muted">/{p.slug}</span>
                <span class="mt-1 inline-block rounded-full border border-grid px-1.5 text-[10.5px] font-normal text-muted">
                  {p.mode === 'meta' ? 'search & call' : 'every tool'}
                </span>
              </a>
            </th>
          {/each}
        </tr>
      </thead>
      <tbody>
        {#each hub.connections as c (c.id)}
          {@const last = hub.profiles.findLastIndex((p) => p.connection_ids.includes(c.id))}
          <tr class="transition-colors {hover.row === c.id ? 'bg-violet-soft/40' : ''}">
            <td class="border-b border-grid px-4 py-2.5">
              <a href="#/connections/{c.id}" class="flex items-center gap-2.5">
                <StatusDot status={c.status} />
                <span class="font-medium">{c.name}</span>
                <span class="font-mono text-[11.5px] text-muted">{c.alias}</span>
              </a>
            </td>
            {#each hub.profiles as p, j (p.id)}
              {@const on = p.connection_ids.includes(c.id)}
              <td
                class="relative border-b border-l border-grid p-0 text-center transition-colors {hover.col === p.id ? 'bg-violet-soft/40' : ''}"
                onmouseenter={() => (hover = { row: c.id, col: p.id })}
              >
                <button
                  class="group relative grid h-11 w-full place-items-center"
                  aria-pressed={on}
                  aria-label="{on ? 'Remove' : 'Add'} {c.name} {on ? 'from' : 'to'} {p.name}"
                  disabled={pending === `${p.id}:${c.id}`}
                  onclick={() => toggle(p, c.id)}
                >
                  <!-- the patch line runs from the connection to its last routed profile -->
                  {#if j < last}
                    <span class="absolute inset-x-0 top-1/2 h-0.5 -translate-y-1/2 bg-violet/35"></span>
                  {:else if j === last}
                    <span class="absolute left-0 right-1/2 top-1/2 h-0.5 -translate-y-1/2 bg-violet/35"></span>
                  {/if}
                  <span
                    class="relative size-3.5 rounded-full border-2 transition-all
                      {on
                      ? 'border-violet bg-violet shadow-[0_0_0_4px_var(--violet-soft)]'
                      : 'border-grid bg-surface group-hover:border-violet/60'}"
                  ></span>
                </button>
              </td>
            {/each}
          </tr>
        {:else}
          <tr>
            <td colspan={hub.profiles.length + 1} class="px-4 py-8 text-center text-muted">
              No connections to route yet. <a href="#/connections/new" class="text-violet hover:underline">Add one</a>
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
{/if}
