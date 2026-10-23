import { listen } from '@tauri-apps/api/event';
import { api, type CallLogEntry, type Connection, type HubEvent, type Profile, type ServerStatus } from './api';

/** Live mirror of hub state, kept fresh by `hub-event`s from the backend. */
export const hub = $state({
  connections: [] as Connection[],
  profiles: [] as Profile[],
  calls: [] as CallLogEntry[],
  server: { running: false, port: 7420, error: null } as ServerStatus,
  loaded: false,
});

export async function reloadConnections() {
  hub.connections = await api.listConnections();
}

export async function reloadProfiles() {
  hub.profiles = await api.listProfiles();
}

let started = false;

export async function startHub() {
  if (started) return;
  started = true;
  await Promise.all([
    reloadConnections(),
    reloadProfiles(),
    api.recentCalls(200).then((c) => (hub.calls = c)),
    api.serverStatus().then((s) => (hub.server = s)),
  ]);
  hub.loaded = true;

  await listen<HubEvent>('hub-event', ({ payload: e }) => {
    switch (e.type) {
      case 'connection_status': {
        const c = hub.connections.find((c) => c.id === e.connection_id);
        if (c) c.status = e.status;
        break;
      }
      case 'capabilities_changed':
      case 'connections_changed':
        reloadConnections();
        break;
      case 'profiles_changed':
        reloadProfiles();
        break;
      case 'call_logged':
        hub.calls = [e.entry, ...hub.calls].slice(0, 500);
        break;
    }
  });
  await listen<ServerStatus>('server-status', ({ payload }) => {
    hub.server = payload;
    reloadProfiles();
  });
}

export function connectionById(id: string) {
  return hub.connections.find((c) => c.id === id);
}

export function profileById(id: string) {
  return hub.profiles.find((p) => p.id === id);
}
