<script lang="ts">
  import type { Transport } from './api';

  let {
    initial,
    secretsPresent = [],
  }: { initial: Transport; secretsPresent?: string[] } = $props();

  type Row = { key: string; value: string; secret: boolean };

  // svelte-ignore state_referenced_locally
  const start = initial;
  let kind = $state<'stdio' | 'http'>(start.kind);
  let command = $state(start.kind === 'stdio' ? start.command : '');
  let args = $state(start.kind === 'stdio' ? start.args.join('\n') : '');
  let url = $state(start.kind === 'http' ? start.url : '');
  let auth = $state<'none' | 'bearer' | 'oauth'>(start.kind === 'http' ? start.auth : 'none');
  let bearer = $state('');
  let rows = $state<Row[]>(
    start.kind === 'stdio'
      ? [
          ...Object.entries(start.env).map(([key, value]) => ({ key, value, secret: false })),
          ...start.secret_env.map((key) => ({ key, value: '', secret: true })),
        ]
      : [
          ...Object.entries(start.headers).map(([key, value]) => ({ key, value, secret: false })),
          ...start.secret_headers.map((key) => ({ key, value: '', secret: true })),
        ],
  );

  const prefix = $derived(kind === 'stdio' ? 'env' : 'header');
  const isSaved = (key: string) => secretsPresent.includes(key);

  /** Current transport plus any secret values typed in (blank = keep saved value). */
  export function build(): { transport: Transport; secrets: Record<string, string> } {
    const clean = rows.filter((r) => r.key.trim());
    const plain = Object.fromEntries(clean.filter((r) => !r.secret).map((r) => [r.key.trim(), r.value]));
    const secretKeys = clean.filter((r) => r.secret).map((r) => r.key.trim());
    const secrets: Record<string, string> = {};
    for (const r of clean) if (r.secret && r.value) secrets[`${prefix}:${r.key.trim()}`] = r.value;
    if (kind === 'stdio') {
      return {
        transport: {
          kind,
          command: command.trim(),
          args: args.split('\n').map((a) => a.trim()).filter(Boolean),
          env: plain,
          secret_env: secretKeys,
        },
        secrets,
      };
    }
    if (auth === 'bearer' && bearer) secrets.bearer = bearer;
    return {
      transport: { kind, url: url.trim(), auth, headers: plain, secret_headers: secretKeys },
      secrets,
    };
  }
</script>

<div class="flex flex-col gap-4">
  <div class="inline-flex w-fit rounded-md border border-grid bg-paper p-0.5 text-[12px] font-medium">
    {#each [['stdio', 'Local command'], ['http', 'Remote URL']] as [k, label] (k)}
      <button
        type="button"
        class="rounded px-3 py-1 {kind === k ? 'bg-surface text-ink shadow-sm' : 'text-muted'}"
        onclick={() => (kind = k as 'stdio' | 'http')}>{label}</button
      >
    {/each}
  </div>

  {#if kind === 'stdio'}
    <label>
      <span class="label">Command</span>
      <input class="field font-mono" bind:value={command} placeholder="npx" spellcheck="false" />
    </label>
    <label>
      <span class="label">Arguments, one per line</span>
      <textarea class="field min-h-[72px] font-mono" bind:value={args} placeholder={'-y\n@scope/some-mcp-server'} spellcheck="false"
      ></textarea>
    </label>
  {:else}
    <label>
      <span class="label">Server URL</span>
      <input class="field font-mono" bind:value={url} placeholder="https://example.com/mcp" spellcheck="false" />
    </label>
    <label>
      <span class="label">Authentication</span>
      <select class="field w-fit" bind:value={auth}>
        <option value="none">None</option>
        <option value="bearer">Bearer token</option>
        <option value="oauth">Sign in with browser (OAuth)</option>
      </select>
    </label>
    {#if auth === 'bearer'}
      <label>
        <span class="label">Token</span>
        <input
          class="field font-mono"
          type="password"
          bind:value={bearer}
          placeholder={isSaved('bearer') ? 'Saved in Keychain. Leave blank to keep.' : ''}
        />
      </label>
    {/if}
  {/if}

  <div>
    <span class="label">{kind === 'stdio' ? 'Environment variables' : 'Headers'}</span>
    <div class="flex flex-col gap-1.5">
      {#each rows as row, i (i)}
        <div class="grid grid-cols-[1fr_1.4fr_auto_auto] items-center gap-2">
          <input class="field font-mono" bind:value={row.key} placeholder="NAME" spellcheck="false" />
          <input
            class="field font-mono"
            type={row.secret ? 'password' : 'text'}
            bind:value={row.value}
            spellcheck="false"
            placeholder={row.secret && isSaved(`${prefix}:${row.key}`) ? 'Saved. Leave blank to keep.' : 'value'}
          />
          <label class="flex items-center gap-1 text-[12px] text-muted" title="Store the value in the macOS Keychain">
            <input type="checkbox" bind:checked={row.secret} class="accent-violet" /> Secret
          </label>
          <button type="button" class="btn btn-ghost px-1.5" aria-label="Remove" onclick={() => rows.splice(i, 1)}>✕</button>
        </div>
      {/each}
      <button
        type="button"
        class="btn btn-ghost w-fit px-0 text-violet"
        onclick={() => rows.push({ key: '', value: '', secret: false })}
      >
        Add {kind === 'stdio' ? 'variable' : 'header'}
      </button>
    </div>
  </div>
</div>
