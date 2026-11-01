<script lang="ts">
  import { disable, enable, isEnabled } from '@tauri-apps/plugin-autostart';
  import { onMount } from 'svelte';
  import { api, type Settings } from '../lib/api';
  import CopyButton from '../lib/CopyButton.svelte';
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

  let tunnelBusy = $state(false);
  const tunnelOn = $derived(hub.tunnel.state === 'running' || hub.tunnel.state === 'starting');

  async function toggleTunnel(enabled: boolean) {
    tunnelBusy = true;
    try {
      await api.setTunnel(enabled);
    } catch {
      // The status line shows why.
    } finally {
      tunnelBusy = false;
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

    <div class="flex flex-col gap-3 border-t border-grid pt-5">
      <label class="flex items-center justify-between gap-6">
        <span>
          <span class="block font-medium">Public tunnel</span>
          <span class="text-[12px] text-muted">
            Makes your profiles reachable from the internet through a Cloudflare quick tunnel, for claude.ai, ChatGPT or
            agents on other machines. Requests still need the profile token or a sign-in you approve on this Mac.
          </span>
        </span>
        <input
          type="checkbox"
          class="size-4 shrink-0 accent-violet"
          checked={tunnelOn}
          disabled={tunnelBusy}
          onchange={(e) => toggleTunnel(e.currentTarget.checked)}
        />
      </label>
      {#if hub.tunnel.state === 'running'}
        <div class="flex items-center gap-3 rounded-lg border border-grid bg-paper px-3 py-2">
          <span class="size-1.5 shrink-0 rounded-full bg-live"></span>
          <span class="selectable min-w-0 flex-1 truncate font-mono text-[12px]">{hub.tunnel.url}</span>
          <CopyButton value={hub.tunnel.url} />
        </div>
        <p class="text-[12px] text-muted">
          The address changes each time the tunnel starts. Profile pages show each profile's public endpoint.
        </p>
      {:else if hub.tunnel.state === 'starting'}
        <p class="text-[12px] text-muted">Starting tunnel…</p>
      {:else if hub.tunnel.state === 'error'}
        <pre class="selectable max-h-40 overflow-auto whitespace-pre-wrap text-[12px] text-fail">{hub.tunnel.message}</pre>
      {/if}
    </div>

    <div class="flex items-center gap-3 border-t border-grid pt-5">
      <button class="btn btn-primary">Save settings</button>
      {#if saved}<span class="text-[12.5px] text-live">Saved</span>{/if}
      {#if error}<span class="text-[12.5px] text-fail">{error}</span>{/if}
    </div>
  </form>
{/if}
