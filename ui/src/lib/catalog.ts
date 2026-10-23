import type { ConnectionInput, Transport } from './api';

export interface TemplateField {
  key: string;
  label: string;
  /** Where the value goes: a (secret) env var, a positional arg, or the bearer token. */
  target: 'env' | 'secret_env' | 'arg' | 'bearer' | 'header' | 'secret_header';
  placeholder?: string;
  help?: string;
  optional?: boolean;
  default?: string;
}

export interface Template {
  id: string;
  name: string;
  description: string;
  /** Suggested alias prefix; a counter or account name is appended. */
  alias: string;
  /** Two-letter monogram for the catalog tile. */
  mark: string;
  transport: Transport;
  fields: TemplateField[];
  docs?: string;
}

const stdio = (command: string, args: string[]): Transport => ({
  kind: 'stdio',
  command,
  args,
  env: {},
  secret_env: [],
});

const http = (url: string, auth: 'none' | 'bearer' | 'oauth'): Transport => ({
  kind: 'http',
  url,
  auth,
  headers: {},
  secret_headers: [],
});

export const catalog: Template[] = [
  {
    id: 'jira-token',
    name: 'Jira & Confluence',
    description: 'Atlassian Cloud or Server via API token. Add one per site or account.',
    alias: 'jira',
    mark: 'Ji',
    transport: stdio('uvx', ['mcp-atlassian']),
    docs: 'https://github.com/sooperset/mcp-atlassian',
    fields: [
      { key: 'JIRA_URL', label: 'Jira URL', target: 'env', placeholder: 'https://acme.atlassian.net' },
      { key: 'JIRA_USERNAME', label: 'Email', target: 'env', placeholder: 'you@acme.com' },
      {
        key: 'JIRA_API_TOKEN',
        label: 'API token',
        target: 'secret_env',
        help: 'Create one at id.atlassian.com → Security → API tokens.',
      },
      { key: 'CONFLUENCE_URL', label: 'Confluence URL', target: 'env', optional: true, placeholder: 'https://acme.atlassian.net/wiki' },
      { key: 'CONFLUENCE_USERNAME', label: 'Confluence email', target: 'env', optional: true },
      { key: 'CONFLUENCE_API_TOKEN', label: 'Confluence API token', target: 'secret_env', optional: true },
    ],
  },
  {
    id: 'atlassian-remote',
    name: 'Atlassian (sign in)',
    description: "Atlassian's hosted MCP server. Signs in through your browser.",
    alias: 'atlassian',
    mark: 'At',
    transport: http('https://mcp.atlassian.com/v1/mcp', 'oauth'),
    fields: [],
  },
  {
    id: 'sentry-token',
    name: 'Sentry',
    description: 'Sentry SaaS or self-hosted via a user auth token.',
    alias: 'sentry',
    mark: 'Se',
    transport: stdio('npx', ['-y', '@sentry/mcp-server@latest']),
    docs: 'https://github.com/getsentry/sentry-mcp',
    fields: [
      { key: 'SENTRY_ACCESS_TOKEN', label: 'Auth token', target: 'secret_env', help: 'User settings → Auth Tokens.' },
      { key: 'SENTRY_HOST', label: 'Host', target: 'env', optional: true, placeholder: 'sentry.io' },
    ],
  },
  {
    id: 'sentry-remote',
    name: 'Sentry (sign in)',
    description: "Sentry's hosted MCP server. Signs in through your browser.",
    alias: 'sentry',
    mark: 'Se',
    transport: http('https://mcp.sentry.dev/mcp', 'oauth'),
    fields: [],
  },
  {
    id: 'github',
    name: 'GitHub',
    description: "GitHub's hosted MCP server with a personal access token.",
    alias: 'github',
    mark: 'Gh',
    transport: http('https://api.githubcopilot.com/mcp/', 'bearer'),
    fields: [{ key: 'bearer', label: 'Personal access token', target: 'bearer' }],
  },
  {
    id: 'linear',
    name: 'Linear',
    description: "Linear's hosted MCP server. Signs in through your browser.",
    alias: 'linear',
    mark: 'Li',
    transport: http('https://mcp.linear.app/mcp', 'oauth'),
    fields: [],
  },
  {
    id: 'notion',
    name: 'Notion',
    description: "Notion's hosted MCP server. Signs in through your browser.",
    alias: 'notion',
    mark: 'No',
    transport: http('https://mcp.notion.com/mcp', 'oauth'),
    fields: [],
  },
  {
    id: 'filesystem',
    name: 'Filesystem',
    description: 'Read and write files inside one folder on this Mac.',
    alias: 'files',
    mark: 'Fs',
    transport: stdio('npx', ['-y', '@modelcontextprotocol/server-filesystem']),
    fields: [{ key: 'path', label: 'Folder', target: 'arg', placeholder: '/Users/you/projects' }],
  },
  {
    id: 'slack-token',
    name: 'Slack',
    description: 'Read channels, threads and DMs with a user or bot token. Posting is off unless enabled.',
    alias: 'slack',
    mark: 'Sl',
    transport: stdio('npx', ['-y', 'slack-mcp-server@latest', '--transport', 'stdio']),
    docs: 'https://github.com/korotovsky/slack-mcp-server',
    fields: [
      {
        key: 'SLACK_MCP_XOXP_TOKEN',
        label: 'User OAuth token',
        target: 'secret_env',
        placeholder: 'xoxp-…',
        help: 'From a Slack app installed in your workspace (OAuth & Permissions).',
      },
      {
        key: 'SLACK_MCP_ADD_MESSAGE_TOOL',
        label: 'Allow posting',
        target: 'env',
        optional: true,
        placeholder: 'true, or channel IDs: C012,C034',
        help: 'Leave empty to keep the connection read-only.',
      },
    ],
  },
  {
    id: 'gitlab-remote',
    name: 'GitLab (sign in)',
    description: "GitLab.com's hosted MCP server (beta, Premium/Ultimate with Duo). Signs in through your browser.",
    alias: 'gitlab',
    mark: 'Gl',
    transport: http('https://gitlab.com/api/v4/mcp', 'oauth'),
    docs: 'https://docs.gitlab.com/user/gitlab_duo/model_context_protocol/mcp_server/',
    fields: [],
  },
  {
    id: 'postgres',
    name: 'Postgres',
    description: 'Query a Postgres database, read-only with a time limit. Includes index and health advice.',
    alias: 'pg',
    mark: 'Pg',
    transport: stdio('uvx', ['postgres-mcp', '--access-mode=restricted']),
    docs: 'https://github.com/crystaldba/postgres-mcp',
    fields: [
      {
        key: 'DATABASE_URI',
        label: 'Connection URI',
        target: 'secret_env',
        placeholder: 'postgresql://user:password@localhost:5432/db',
        help: 'For read/write, change the argument to --access-mode=unrestricted after adding.',
      },
    ],
  },
  {
    id: 'supabase-remote',
    name: 'Supabase (sign in)',
    description: "Supabase's hosted MCP server for your projects. Signs in through your browser.",
    alias: 'supabase',
    mark: 'Sb',
    transport: http('https://mcp.supabase.com/mcp', 'oauth'),
    docs: 'https://supabase.com/docs/guides/getting-started/mcp',
    fields: [],
  },
  {
    id: 'stripe',
    name: 'Stripe',
    description: "Stripe's hosted MCP server. A restricted key decides which tools work.",
    alias: 'stripe',
    mark: 'St',
    transport: http('https://mcp.stripe.com', 'bearer'),
    docs: 'https://docs.stripe.com/mcp',
    fields: [
      {
        key: 'bearer',
        label: 'Restricted API key',
        target: 'bearer',
        placeholder: 'rk_test_…',
        help: 'Developers → API keys → Create restricted key. Use a sandbox key first.',
      },
    ],
  },
  {
    id: 'honeycomb-remote',
    name: 'Honeycomb (sign in)',
    description: "Honeycomb's hosted MCP server, US region. Needs Honeycomb Intelligence enabled.",
    alias: 'honeycomb',
    mark: 'Hc',
    transport: http('https://mcp.honeycomb.io/mcp', 'oauth'),
    docs: 'https://docs.honeycomb.io/integrations/mcp/configuration-guide',
    fields: [],
  },
  {
    id: 'honeycomb-eu-remote',
    name: 'Honeycomb EU (sign in)',
    description: "Honeycomb's hosted MCP server, EU region. Needs Honeycomb Intelligence enabled.",
    alias: 'honeycomb_eu',
    mark: 'Hc',
    transport: http('https://mcp.eu1.honeycomb.io/mcp', 'oauth'),
    docs: 'https://docs.honeycomb.io/integrations/mcp/configuration-guide',
    fields: [],
  },
  {
    id: 'grafana',
    name: 'Grafana',
    description: 'Dashboards, Prometheus and Loki queries, alerts and incidents. Runs in Docker.',
    alias: 'grafana',
    mark: 'Gr',
    // `-e NAME` without a value passes the variable through from Plexus.
    transport: stdio('docker', [
      'run', '--rm', '-i',
      '-e', 'GRAFANA_URL',
      '-e', 'GRAFANA_SERVICE_ACCOUNT_TOKEN',
      'grafana/mcp-grafana', '-t', 'stdio',
    ]),
    docs: 'https://github.com/grafana/mcp-grafana',
    fields: [
      { key: 'GRAFANA_URL', label: 'Grafana URL', target: 'env', placeholder: 'https://acme.grafana.net' },
      {
        key: 'GRAFANA_SERVICE_ACCOUNT_TOKEN',
        label: 'Service account token',
        target: 'secret_env',
        help: 'Administration → Service accounts → Add token.',
      },
    ],
  },
  {
    id: 'cloudflare-bindings',
    name: 'Cloudflare Workers',
    description: 'Workers, KV, R2, D1 and other bindings on your account. Signs in through your browser.',
    alias: 'cloudflare',
    mark: 'Cf',
    transport: http('https://bindings.mcp.cloudflare.com/mcp', 'oauth'),
    docs: 'https://developers.cloudflare.com/agents/model-context-protocol/mcp-servers-for-cloudflare/',
    fields: [],
  },
  {
    id: 'cloudflare-observability',
    name: 'Cloudflare Observability',
    description: 'Logs and analytics for your Workers. Signs in through your browser.',
    alias: 'cf_logs',
    mark: 'Cf',
    transport: http('https://observability.mcp.cloudflare.com/mcp', 'oauth'),
    docs: 'https://developers.cloudflare.com/agents/model-context-protocol/mcp-servers-for-cloudflare/',
    fields: [],
  },
  {
    id: 'context7',
    name: 'Context7',
    description: 'Up-to-date library and framework docs for the model. Works without a key, with tighter limits.',
    alias: 'context7',
    mark: 'C7',
    transport: http('https://mcp.context7.com/mcp', 'none'),
    docs: 'https://context7.com/docs/installation',
    fields: [
      { key: 'CONTEXT7_API_KEY', label: 'API key', target: 'secret_header', optional: true, placeholder: 'ctx7sk-…' },
    ],
  },
  {
    id: 'playwright',
    name: 'Playwright browser',
    description: 'Drive a local browser: open pages, click, fill forms, take snapshots.',
    alias: 'browser',
    mark: 'Pw',
    transport: stdio('npx', ['-y', '@playwright/mcp@latest']),
    docs: 'https://github.com/microsoft/playwright-mcp',
    fields: [],
  },
  {
    id: 'fetch',
    name: 'Fetch',
    description: 'Fetch a web page and convert it to Markdown.',
    alias: 'fetch',
    mark: 'Fe',
    transport: stdio('uvx', ['mcp-server-fetch']),
    docs: 'https://github.com/modelcontextprotocol/servers/tree/main/src/fetch',
    fields: [],
  },
];

/** Turn a template plus filled-in values into a connection payload. */
export function fromTemplate(
  t: Template,
  values: Record<string, string>,
  name: string,
  alias: string,
): ConnectionInput {
  const transport: Transport = JSON.parse(JSON.stringify(t.transport));
  const secrets: Record<string, string> = {};
  for (const f of t.fields) {
    const v = values[f.key]?.trim();
    if (!v) continue;
    if (transport.kind === 'stdio') {
      if (f.target === 'env') transport.env[f.key] = v;
      if (f.target === 'secret_env') {
        transport.secret_env.push(f.key);
        secrets[`env:${f.key}`] = v;
      }
      if (f.target === 'arg') transport.args.push(v);
    } else if (f.target === 'bearer') {
      secrets.bearer = v;
    } else if (f.target === 'header') {
      transport.headers[f.key] = v;
    } else if (f.target === 'secret_header') {
      transport.secret_headers.push(f.key);
      secrets[`header:${f.key}`] = v;
    }
  }
  return { alias, name, template_id: t.id, transport, enabled: true, secrets };
}

/** `jira`, `jira2`, `jira3`… whichever is free. */
export function suggestAlias(base: string, taken: string[]): string {
  if (!taken.includes(base)) return base;
  for (let i = 2; ; i++) if (!taken.includes(`${base}${i}`)) return `${base}${i}`;
}

export function slugify(s: string, sep = '_'): string {
  return s
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, sep)
    .replace(new RegExp(`^${sep}+|${sep}+$`, 'g'), '')
    .replace(/^[^a-z]+/, '')
    .slice(0, 24);
}

/** Coerce typing into a valid alias shape: `Jira Acme` -> `jira_acme`. */
export function normalizeAlias(s: string): string {
  return s
    .toLowerCase()
    .replace(/[^a-z0-9_]+/g, '_')
    .replace(/_{2,}/g, '_')
    .slice(0, 24);
}

/** Same rules as the backend's `validate_alias`; null when valid. */
export function aliasError(s: string): string | null {
  if (!s) return 'Enter an alias.';
  if (!/^[a-z]/.test(s)) return 'Start the alias with a letter.';
  if (s.endsWith('_')) return 'The alias can’t end with an underscore.';
  if (!/^[a-z][a-z0-9_]{0,23}$/.test(s) || s.includes('__'))
    return 'Use up to 24 lowercase letters, digits and single underscores.';
  return null;
}

/** Profile URL slug: lowercase letters, digits and dashes. */
export function normalizeSlug(s: string): string {
  return s.toLowerCase().replace(/[^a-z0-9-]+/g, '-').slice(0, 32);
}
