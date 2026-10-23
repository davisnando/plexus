<script lang="ts">
  // Two-step button: the webview has no window.confirm(), so the first click
  // arms it and explains the consequence, the second click runs the action.
  let {
    label,
    confirmLabel,
    warning,
    onconfirm,
    class: className = 'btn btn-danger',
    disabled = false,
  }: {
    label: string;
    confirmLabel: string;
    warning: string;
    onconfirm: () => unknown;
    class?: string;
    disabled?: boolean;
  } = $props();

  let armed = $state(false);
  let timer: ReturnType<typeof setTimeout> | undefined;

  function click() {
    if (!armed) {
      armed = true;
      clearTimeout(timer);
      timer = setTimeout(() => (armed = false), 4000);
      return;
    }
    clearTimeout(timer);
    armed = false;
    onconfirm();
  }
</script>

<span class="inline-flex items-center gap-2">
  {#if armed}<span class="text-[12px] text-muted" role="status">{warning}</span>{/if}
  <button
    type="button"
    class="{className} {armed ? '!border-fail !bg-fail !text-white' : ''}"
    {disabled}
    onclick={click}
    onblur={() => (armed = false)}
  >
    {armed ? confirmLabel : label}
  </button>
</span>
