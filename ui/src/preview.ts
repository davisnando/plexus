// Dev-only: render the dashboard in a plain browser with mocked Tauri IPC.
// Open http://localhost:5173/preview.html#/connections
import { mockIPC, mockWindows } from '@tauri-apps/api/mocks';
import { mount } from 'svelte';
import './app.css';
import App from './App.svelte';

const now = Math.floor(Date.now() / 1000);
const stdio = (command: string, args: string[], env: Record<string, string> = {}, secret_env: string[] = []) => ({
  kind: 'stdio', command, args, env, secret_env,
});
const connections = [
  { id: 'c1', alias: 'jira_acme', name: 'Jira Acme', template_id: 'jira-token', transport: stdio('uvx', ['mcp-atlassian'], { JIRA_URL: 'https://acme.atlassian.net' }, ['JIRA_API_TOKEN']), enabled: true, idle_timeout_secs: 600, created_at: now, status: { state: 'running' }, tool_count: 31, secrets_present: ['env:JIRA_API_TOKEN'] },
  { id: 'c2', alias: 'jira_client', name: 'Jira Client Co', template_id: 'jira-token', transport: stdio('uvx', ['mcp-atlassian'], { JIRA_URL: 'https://clientco.atlassian.net' }, ['JIRA_API_TOKEN']), enabled: true, idle_timeout_secs: 600, created_at: now, status: { state: 'idle' }, tool_count: 31, secrets_present: ['env:JIRA_API_TOKEN'] },
  { id: 'c3', alias: 'jira_oss', name: 'Jira OSS', template_id: 'jira-token', transport: stdio('uvx', ['mcp-atlassian'], { JIRA_URL: 'https://oss.atlassian.net' }, ['JIRA_API_TOKEN']), enabled: true, idle_timeout_secs: 600, created_at: now, status: { state: 'error', message: 'secret env `JIRA_API_TOKEN` is not set' }, tool_count: 0, secrets_present: [] },
  { id: 'c4', alias: 'sentry_prod', name: 'Sentry Production', template_id: 'sentry-token', transport: stdio('npx', ['-y', '@sentry/mcp-server@latest'], {}, ['SENTRY_ACCESS_TOKEN']), enabled: true, idle_timeout_secs: 600, created_at: now, status: { state: 'running' }, tool_count: 18, secrets_present: ['env:SENTRY_ACCESS_TOKEN'] },
  { id: 'c5', alias: 'linear', name: 'Linear', template_id: 'linear', transport: { kind: 'http', url: 'https://mcp.linear.app/mcp', auth: 'oauth', headers: {}, secret_headers: [] }, enabled: true, idle_timeout_secs: 600, created_at: now, status: { state: 'needs_auth' }, tool_count: 0, secrets_present: [] },
];
const profiles = [
  { id: 'p1', slug: 'work', name: 'Work', mode: 'direct', connection_ids: ['c1', 'c2', 'c4'], created_at: now, url: 'http://127.0.0.1:7420/mcp/work' },
  { id: 'p2', slug: 'side', name: 'Side projects', mode: 'meta', connection_ids: ['c3', 'c4', 'c5'], created_at: now, url: 'http://127.0.0.1:7420/mcp/side' },
];
const tools = ['create_issue', 'get_issue', 'search', 'transition_issue', 'add_comment', 'delete_issue'].map((name) => ({
  name, exposed_name: `jira_acme__${name}`, title: null,
  description: `Jira: ${name.replace(/_/g, ' ')} in the configured site.`,
  read_only: name.startsWith('get') || name === 'search', destructive: name.startsWith('delete'),
}));
const calls = [
  ['c1', 'search', 'ok', 412, null], ['c4', 'list_issues', 'ok', 820, null], ['c1', 'delete_issue', 'denied', 4100, 'The user denied this tool call.'],
  ['c2', 'create_issue', 'error', 1290, 'Upstream error: 400 Bad Request: field "priority" is required'], ['c1', 'get_issue', 'ok', 233, null],
].map(([connection_id, tool, status, duration_ms, error], i) => ({
  id: 10 - i, ts: now - i * 97, profile_id: i % 2 ? 'p2' : 'p1', connection_id, tool, status, duration_ms, error,
}));

mockWindows('main');
mockIPC(
  (cmd, args: any) => {
    switch (cmd) {
      case 'list_connections': return connections;
      case 'list_profiles': return profiles;
      case 'recent_calls': return calls;
      case 'server_status': return { running: true, port: 7420, error: null };
      case 'connection_tools': return tools;
      case 'profile_token': return 'pXq8w2mZ0rTn4vKs9LbY1cJd6fHa3uEo';
      case 'profile_tools': return tools.map((t, i) => ({ connection_id: 'c1', connection_alias: 'jira_acme', connection_name: 'Jira Acme', tool: t.name, exposed_name: t.exposed_name, description: t.description, policy: i === 5 ? 'approval' : i === 4 ? 'hidden' : 'allow' }));
      case 'get_settings': return { port: 7420, default_idle_timeout_secs: 600, log_retention_days: 7, approval_timeout_secs: 60 };
      case 'get_approval': return { id: args.id, profile_name: 'Work', connection_name: 'Jira Acme', connection_alias: 'jira_acme', tool: 'delete_issue', arguments: { issue_key: 'ACME-1432', delete_subtasks: true } };
      case 'plugin:autostart|is_enabled': return true;
      default: return null;
    }
  },
  { shouldMockEvents: true },
);

mount(App, { target: document.getElementById('app')! });
