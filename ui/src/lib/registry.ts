// Search the official MCP Registry and map `server.json` entries to templates.
import type { Template, TemplateField } from './catalog';

const BASE = 'https://registry.modelcontextprotocol.io/v0/servers';

interface Input {
  name?: string;
  description?: string;
  isRequired?: boolean;
  isSecret?: boolean;
  default?: string;
  value?: string;
  type?: 'positional' | 'named';
  valueHint?: string;
}

interface Package {
  registryType: 'npm' | 'pypi' | 'oci' | string;
  identifier: string;
  version?: string;
  transport?: { type: string };
  environmentVariables?: Input[];
  packageArguments?: Input[];
}

interface Remote {
  type: string;
  url: string;
  headers?: Input[];
}

interface ServerJson {
  name: string;
  title?: string;
  description?: string;
  version?: string;
  websiteUrl?: string;
  packages?: Package[];
  remotes?: Remote[];
}

export interface RegistryResult {
  name: string;
  title: string;
  description: string;
  /** One template per way of running it (package or remote). */
  options: { label: string; template: Template }[];
}

function aliasFor(name: string): string {
  const last = name.split('/').pop() ?? name;
  return (
    last
      .toLowerCase()
      .replace(/(^|[-_])mcp([-_]server)?(?=[-_]|$)/g, '_')
      .replace(/[^a-z0-9]+/g, '_')
      .replace(/^_+|_+$/g, '')
      .replace(/^[^a-z]+/, '')
      .slice(0, 20) || 'server'
  );
}

function envFields(vars: Input[] = []): TemplateField[] {
  return vars
    .filter((v) => v.name)
    .map((v) => ({
      key: v.name!,
      label: v.name!,
      target: v.isSecret ? 'secret_env' : 'env',
      help: v.description,
      optional: !v.isRequired,
      default: v.default,
    }));
}

function fromPackage(s: ServerJson, p: Package): Template | null {
  if (p.transport && p.transport.type !== 'stdio') return null;
  const env = p.environmentVariables ?? [];
  const fixedArgs = (p.packageArguments ?? [])
    .filter((a) => a.value)
    .flatMap((a) => (a.type === 'named' && a.name ? [a.name, a.value!] : [a.value!]));
  let command: string;
  let args: string[];
  switch (p.registryType) {
    case 'npm':
      command = 'npx';
      args = ['-y', p.version ? `${p.identifier}@${p.version}` : p.identifier];
      break;
    case 'pypi':
      command = 'uvx';
      args = [p.version ? `${p.identifier}==${p.version}` : p.identifier];
      break;
    case 'oci':
      command = 'docker';
      // `-e NAME` forwards the variable Plexus sets on the docker process.
      args = ['run', '-i', '--rm', ...env.flatMap((v) => ['-e', v.name!]), p.identifier];
      break;
    default:
      return null;
  }
  return {
    id: `registry:${s.name}`,
    name: s.title ?? s.name,
    description: s.description ?? '',
    alias: aliasFor(s.name),
    mark: '',
    transport: { kind: 'stdio', command, args: [...args, ...fixedArgs], env: {}, secret_env: [] },
    fields: envFields(env),
    docs: s.websiteUrl,
  };
}

function fromRemote(s: ServerJson, r: Remote): Template | null {
  if (r.type !== 'streamable-http') return null;
  const headers = r.headers ?? [];
  const bearer = headers.find((h) => h.name?.toLowerCase() === 'authorization');
  const fields: TemplateField[] = headers
    .filter((h) => h.name && h !== bearer)
    .map((h) => ({
      key: h.name!,
      label: h.name!,
      target: h.isSecret ? 'secret_header' : 'header',
      help: h.description,
      optional: !h.isRequired,
      default: h.value ?? h.default,
    }));
  if (bearer) fields.unshift({ key: 'bearer', label: 'Token', target: 'bearer', help: bearer.description });
  return {
    id: `registry:${s.name}`,
    name: s.title ?? s.name,
    description: s.description ?? '',
    alias: aliasFor(s.name),
    mark: '',
    transport: {
      kind: 'http',
      url: r.url,
      auth: bearer ? 'bearer' : 'none',
      headers: {},
      secret_headers: [],
    },
    fields,
    docs: s.websiteUrl,
  };
}

const runtimeLabel: Record<string, string> = { npm: 'npx', pypi: 'uvx', oci: 'Docker' };

export async function searchRegistry(query: string, signal?: AbortSignal): Promise<RegistryResult[]> {
  const url = `${BASE}?search=${encodeURIComponent(query)}&version=latest&limit=30`;
  const res = await fetch(url, { signal });
  if (!res.ok) throw new Error(`Registry returned ${res.status}`);
  const body: { servers: { server: ServerJson }[] } = await res.json();
  return body.servers
    .map(({ server: s }) => {
      const options = [
        ...(s.remotes ?? []).map((r) => ({ label: 'Remote', template: fromRemote(s, r) })),
        ...(s.packages ?? []).map((p) => ({
          label: runtimeLabel[p.registryType] ?? p.registryType,
          template: fromPackage(s, p),
        })),
      ].filter((o): o is { label: string; template: Template } => o.template !== null);
      return { name: s.name, title: s.title ?? s.name, description: s.description ?? '', options };
    })
    .filter((r) => r.options.length);
}
