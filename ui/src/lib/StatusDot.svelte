<script lang="ts">
  import type { ConnectionStatus } from './api';

  let { status, label = false }: { status: ConnectionStatus; label?: boolean } = $props();

  const meta = $derived(
    {
      running: { color: 'bg-live', text: 'Running' },
      idle: { color: 'bg-muted/40', text: 'Idle' },
      starting: { color: 'bg-warn animate-pulse', text: 'Starting' },
      needs_auth: { color: 'bg-warn', text: 'Needs sign-in' },
      error: { color: 'bg-fail', text: 'Error' },
      disabled: { color: 'bg-transparent border border-muted/50', text: 'Disabled' },
    }[status.state],
  );
</script>

<span class="inline-flex items-center gap-1.5" title={meta.text}>
  <span class="size-2 shrink-0 rounded-full {meta.color}"></span>
  {#if label}<span class="text-[12px] text-muted">{meta.text}</span>{/if}
</span>
