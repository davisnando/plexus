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
