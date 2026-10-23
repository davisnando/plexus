<script lang="ts">
  import { onMount } from 'svelte';
  import { api, type ApprovalDecision, type ApprovalRequest } from '../lib/api';

  let { id }: { id: string } = $props();
  let req = $state<ApprovalRequest | null>(null);
  let total = $state(60);
  let left = $state(60);

  onMount(() => {
    api.getApproval(id).then((r) => (req = r));
    api.getSettings().then((s) => (total = left = s.approval_timeout_secs)).catch(() => {});
    const started = Date.now();
    const t = setInterval(() => (left = Math.max(0, total - (Date.now() - started) / 1000)), 250);
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') decide('deny');
      if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) decide('allow_once');
    };
    window.addEventListener('keydown', onKey);
    return () => {
      clearInterval(t);
      window.removeEventListener('keydown', onKey);
    };
  });

  const decide = (d: ApprovalDecision) => api.resolveApproval(id, d);
  const args = $derived(req?.arguments == null ? '{}' : JSON.stringify(req.arguments, null, 2));
</script>

<div class="flex h-screen flex-col bg-surface">
  <div class="h-1 bg-grid">
    <div class="h-full bg-warn transition-[width] duration-300 ease-linear" style="width: {(left / total) * 100}%"></div>
  </div>
  {#if req}
    <div class="flex flex-1 flex-col gap-4 overflow-hidden p-5" data-tauri-drag-region>
      <div data-tauri-drag-region>
        <p class="text-[12px] text-muted">{req.profile_name} wants to run</p>
        <p class="selectable mt-1 break-all font-mono text-[15px] font-semibold">
          <span class="text-violet">{req.connection_alias}</span>__{req.tool}
        </p>
        <p class="mt-0.5 text-[12px] text-muted">on {req.connection_name}</p>
      </div>
      {#if req.reason}
        <p class="selectable border-l-2 border-violet pl-3 text-[13px] leading-snug">{req.reason}</p>
      {/if}
      <pre
        class="selectable min-h-0 flex-1 overflow-auto rounded-lg border border-grid bg-paper px-3 py-2.5 font-mono text-[11.5px] leading-relaxed">{args}</pre>
      <div class="flex items-center gap-2">
        <button class="btn btn-danger" onclick={() => decide('deny')}>Deny</button>
        <span class="text-[11.5px] tabular-nums text-muted">{Math.ceil(left)}s</span>
        <button class="btn ml-auto" onclick={() => decide('always_allow')}>Always allow</button>
        <!-- svelte-ignore a11y_autofocus -->
        <button class="btn btn-primary" onclick={() => decide('allow_once')} autofocus>Allow once</button>
      </div>
    </div>
  {:else}
    <p class="p-5 text-muted">This request has already been answered or timed out.</p>
  {/if}
</div>
