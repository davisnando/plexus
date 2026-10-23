<script lang="ts">
  import { disable, enable, isEnabled } from '@tauri-apps/plugin-autostart';
  import { onMount } from 'svelte';
  import { api, type Settings } from '../lib/api';
  import { hub } from '../lib/hub.svelte';

  let s = $state<Settings | null>(null);
  let launchAtLogin = $state(false);
  let saved = $state(false);
  let error = $state('');

  onMount(async () => {
    s = await api.getSettings();
    launchAtLogin = await isEnabled().catch(() => false);
  });

  async function save(e: SubmitEvent) {
    e.preventDefault();
    error = '';
    try {
      await api.saveSettings($state.snapshot(s!));
      saved = true;
      setTimeout(() => (saved = false), 1500);
    } catch (err) {
      error = String(err);
    }
  }

  async function toggleLogin() {
    if (launchAtLogin) await enable();
    else await disable();
  }
</script>

<header class="pb-6 pt-2"><h1 class="page-title">Settings</h1></header>

{#if s}
  <form class="flex max-w-[520px] flex-col gap-6" onsubmit={save}>
    <label class="flex items-center justify-between gap-6">
      <span>
        <span class="block font-medium">Open at login</span>
        <span class="text-[12px] text-muted">Keeps your profiles available whenever you're signed in.</span>
      </span>
      <input type="checkbox" class="size-4 accent-violet" bind:checked={launchAtLogin} onchange={toggleLogin} />
    </label>

    <label class="flex items-center justify-between gap-6">
      <span>
        <span class="block font-medium">Port</span>
        <span class="text-[12px] text-muted">Changing it breaks clients until you update their endpoint URLs.</span>
      </span>
      <input class="field w-24 tabular-nums" type="number" min="1024" max="65535" bind:value={s.port} />
    </label>
    {#if hub.server.error}<p class="-mt-4 text-[12px] text-fail">{hub.server.error}</p>{/if}

    <label class="flex items-center justify-between gap-6">
      <span>
        <span class="block font-medium">Stop idle servers after</span>
        <span class="text-[12px] text-muted">Default for new connections. Local servers restart on the next call.</span>
      </span>
      <span class="flex items-center gap-1.5">
        <input
          class="field w-20 tabular-nums"
          type="number"
          min="1"
          value={Math.round(s.default_idle_timeout_secs / 60)}
          oninput={(e) => (s!.default_idle_timeout_secs = Math.max(1, +e.currentTarget.value) * 60)}
        /> min
      </span>
    </label>

    <label class="flex items-center justify-between gap-6">
      <span>
        <span class="block font-medium">Wait for approval</span>
        <span class="text-[12px] text-muted">After this, an unanswered prompt counts as a denial.</span>
      </span>
      <span class="flex items-center gap-1.5">
        <input class="field w-20 tabular-nums" type="number" min="10" max="600" bind:value={s.approval_timeout_secs} /> sec
      </span>
    </label>

    <label class="flex items-center justify-between gap-6">
      <span>
        <span class="block font-medium">Keep activity for</span>
      </span>
      <span class="flex items-center gap-1.5">
        <input class="field w-20 tabular-nums" type="number" min="1" max="365" bind:value={s.log_retention_days} /> days
      </span>
    </label>

    <div class="flex items-center gap-3 border-t border-grid pt-5">
      <button class="btn btn-primary">Save settings</button>
      {#if saved}<span class="text-[12.5px] text-live">Saved</span>{/if}
      {#if error}<span class="text-[12.5px] text-fail">{error}</span>{/if}
    </div>
  </form>
{/if}
