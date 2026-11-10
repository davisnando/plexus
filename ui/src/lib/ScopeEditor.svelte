<script lang="ts">
  import { open } from '@tauri-apps/plugin-dialog';
  import { api, toInput, type Connection, type Scope } from './api';
  import { reloadConnections } from './hub.svelte';

  let { connection: c, compact = false }: { connection: Connection; compact?: boolean } = $props();

  let draft = $state('');
  let error = $state('');
  let busy = $state(false);

  const folders = $derived(c.scope.kind === 'folders' ? c.scope.folders : []);

  async function save(scope: Scope) {
    busy = true;
    error = '';
    try {
      await api.updateConnection(c.id, { ...toInput(c), scope });
      await reloadConnections();
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }

  async function add(rule: string) {
    rule = rule.trim();
    if (!rule) return;
    if (!folders.includes(rule)) await save({ kind: 'folders', folders: [...folders, rule] });
    if (!error) draft = '';
  }

  async function pick() {
    const dir = await open({ directory: true, multiple: false, title: `Folder for ${c.name}` });
    if (typeof dir === 'string') await add(dir);
  }

  const remove = (rule: string) => save({ kind: 'folders', folders: folders.filter((f) => f !== rule) });
</script>

<div class="flex flex-col gap-2">
  <div class="inline-flex w-fit rounded-md border border-grid bg-paper p-0.5 text-[11.5px] font-medium">
    {#each [['everywhere', 'Everywhere'], ['folders', 'Only in folders']] as [k, label] (k)}
      <button
        type="button"
        class="rounded px-2 py-0.5 {c.scope.kind === k ? 'bg-surface text-ink shadow-sm' : 'text-muted hover:text-ink'}"
        disabled={busy}
        onclick={() => c.scope.kind !== k && save(k === 'everywhere' ? { kind: 'everywhere' } : { kind: 'folders', folders: [] })}
        >{label}</button
      >
    {/each}
  </div>

  {#if c.scope.kind === 'folders'}
    {#if folders.length}
      <ul class="flex flex-wrap gap-1.5">
        {#each folders as rule (rule)}
          <li class="flex items-center gap-1 rounded-md border border-grid bg-paper py-0.5 pl-2 pr-1 font-mono text-[11.5px]">
            <span class="selectable">{rule}</span>
            <button
              type="button"
              class="rounded px-1 text-muted hover:text-fail"
              aria-label="Remove {rule}"
              disabled={busy}
              onclick={() => remove(rule)}>×</button
            >
          </li>
        {/each}
      </ul>
    {:else}
      <p class="text-[12px] text-warn">No folders yet, so no session gets this connection.</p>
    {/if}
    <form
      class="flex items-center gap-2"
      onsubmit={(e) => {
        e.preventDefault();
        add(draft);
      }}
    >
      <input
        class="field font-mono text-[12px] {compact ? 'max-w-[280px]' : 'max-w-[360px]'}"
        placeholder="/Users/you/projects/client-*"
        bind:value={draft}
        autocapitalize="off"
        autocomplete="off"
        spellcheck="false"
        {...{ autocorrect: 'off' }}
      />
      <button class="btn" disabled={busy || !draft.trim()}>Add</button>
      <button type="button" class="btn btn-ghost" disabled={busy} onclick={pick}>Choose folder…</button>
    </form>
    {#if !compact}
      <p class="text-[12px] text-muted">
        A rule covers its folder and everything inside it. <span class="font-mono">*</span> matches part of a folder name,
        <span class="font-mono">**</span> any depth. Clients that report no folder (Claude Desktop, claude.ai) only get connections
        set to Everywhere.
      </p>
    {/if}
  {/if}
  {#if error}<p class="text-[12px] text-fail">{error}</p>{/if}
</div>
