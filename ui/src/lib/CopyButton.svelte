<script lang="ts">
  import { writeText } from '@tauri-apps/plugin-clipboard-manager';

  let { value, label = 'Copy' }: { value: string | (() => Promise<string>); label?: string } = $props();
  let copied = $state(false);

  async function copy() {
    await writeText(typeof value === 'string' ? value : await value());
    copied = true;
    setTimeout(() => (copied = false), 1400);
  }
</script>

<button class="btn" onclick={copy}>{copied ? 'Copied' : label}</button>
