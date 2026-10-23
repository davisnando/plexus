<script lang="ts">
  import { listen } from '@tauri-apps/api/event';
  import { onMount } from 'svelte';
  import { hub, startHub } from './lib/hub.svelte';
  import { match, navigate, route } from './lib/router.svelte';
  import Activity from './pages/Activity.svelte';
  import Approval from './pages/Approval.svelte';
  import ConnectionDetail from './pages/ConnectionDetail.svelte';
  import Connections from './pages/Connections.svelte';
  import NewConnection from './pages/NewConnection.svelte';
  import ProfileDetail from './pages/ProfileDetail.svelte';
  import Profiles from './pages/Profiles.svelte';
  import Settings from './pages/Settings.svelte';

  const approval = $derived(match('/approval/:id'));

  onMount(() => {
    if (approval) return;
    startHub();
    const un = listen<string>('navigate', ({ payload }) => navigate(payload));
    return () => {
      un.then((f) => f());
    };
  });

  const nav = [
    { path: '/connections', label: 'Connections' },
    { path: '/profiles', label: 'Profiles' },
    { path: '/activity', label: 'Activity' },
    { path: '/settings', label: 'Settings' },
  ];

  const section = $derived('/' + route.path.split('/')[1]);
  const problems = $derived(
    hub.connections.filter((c) => c.status.state === 'error' || c.status.state === 'needs_auth').length,
  );
</script>

{#if approval}
  <Approval id={approval.id} />
{:else}
  <div class="flex h-screen">
    <aside class="flex w-[212px] shrink-0 flex-col border-r border-grid bg-sidebar">
      <div class="h-[52px] shrink-0" data-tauri-drag-region></div>
      <nav class="flex flex-col gap-0.5 px-2.5">
        {#each nav as item (item.path)}
          <a
            href="#{item.path}"
            class="flex items-center justify-between rounded-md px-2.5 py-1.5 text-[13px] font-medium transition-colors
              {section === item.path ? 'bg-violet-soft text-violet' : 'text-ink/80 hover:bg-grid/50'}"
          >
            {item.label}
            {#if item.path === '/connections'}
              <span class="flex items-center gap-1.5 text-[12px] font-normal text-muted tabular-nums">
                {#if problems}<span class="size-1.5 rounded-full bg-fail" title="{problems} need attention"></span>{/if}
                {hub.connections.length}
              </span>
            {/if}
          </a>
        {/each}
      </nav>

      <div class="mt-auto border-t border-grid px-4 py-3 text-[12px] leading-snug">
        {#if hub.server.running}
          <div class="flex items-center gap-1.5 text-ink/80">
            <span class="size-1.5 rounded-full bg-live"></span>
            Listening
          </div>
          <div class="selectable mt-0.5 font-mono text-[11px] text-muted">127.0.0.1:{hub.server.port}</div>
        {:else if hub.server.error}
          <div class="flex items-center gap-1.5 text-fail">
            <span class="size-1.5 rounded-full bg-fail"></span>
            Port {hub.server.port} unavailable
          </div>
          <a href="#/settings" class="mt-0.5 block text-muted underline">Change port</a>
        {:else}
          <div class="text-muted">Starting server…</div>
        {/if}
      </div>
    </aside>

    <main class="relative flex-1 overflow-y-auto">
      <div class="sticky top-0 z-10 h-[28px]" data-tauri-drag-region></div>
      <div class="mx-auto max-w-[920px] px-10 pb-16">
        {#if !hub.loaded}
          <p class="pt-6 text-muted">Loading…</p>
        {:else if match('/connections')}
          <Connections />
        {:else if match('/connections/new')}
          <NewConnection />
        {:else if match('/connections/:id')}
          {@const p = match('/connections/:id')!}
          {#key p.id}<ConnectionDetail id={p.id} />{/key}
        {:else if match('/profiles')}
          <Profiles />
        {:else if match('/profiles/:id')}
          {@const p = match('/profiles/:id')!}
          {#key p.id}<ProfileDetail id={p.id} />{/key}
        {:else if match('/activity')}
          <Activity />
        {:else if match('/settings')}
          <Settings />
        {:else}
          <Connections />
        {/if}
      </div>
    </main>
  </div>
{/if}
