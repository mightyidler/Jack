export interface AudioApplication {
  process_id: number;
  process_name: string;
  display_name: string;
  route: string;
  activity: string;
  routing_error: boolean;
}
interface ApplicationIdentity {
  process_id: number;
  stable_id: string | null;
  requested: string | null;
  origin: string;
  pending?: boolean;
}
export interface ApplicationGroup {
  session: AudioApplication;
  process_ids: number[];
}

// Match verified backend identity and effective routing. A shared display name
// alone never proves that two processes belong to the same application.
export function groupApplications(
  sessions: readonly AudioApplication[],
  identities: readonly ApplicationIdentity[],
): ApplicationGroup[] {
  const metadata = new Map(identities.map(identity => [identity.process_id, identity]));
  const groups = new Map<string, AudioApplication[]>();
  for (const session of sessions) {
    const identity = metadata.get(session.process_id);
    // A verified persisted route belongs to its requested group while the old
    // inactive stream waits to be recreated. Failed requests never set pending.
    const displayed = identity?.stable_id && identity.pending && identity.requested
      ? {...session, route:identity.requested} : session;
    const key = identity?.stable_id
      ? JSON.stringify([identity.stable_id, displayed.route, identity.requested, identity.origin])
      : JSON.stringify(['unverified', session.process_id, session.route]);
    const members = groups.get(key) ?? [];
    members.push(displayed);
    groups.set(key, members);
  }
  return [...groups.values()].map(members => {
    const ordered = members.sort((a,b) => Number(b.activity === 'active') - Number(a.activity === 'active') || a.process_id - b.process_id);
    return {
      session: {...ordered[0], routing_error: members.some(member => member.routing_error)},
      process_ids: ordered.map(member => member.process_id),
    };
  });
}

// Unassigned, ambiguous or removed-channel sessions must remain reachable for
// manual assignment rather than disappearing from the configured group list.
export function partitionApplications(
  sessions:readonly AudioApplication[],
  identities:readonly ApplicationIdentity[],
  visibleChannels:readonly string[],
) {
  const channels=new Map<string,ApplicationGroup[]>();
  const other:ApplicationGroup[]=[];
  for(const application of groupApplications(sessions,identities)) {
    const channel=application.session.route;
    if(!visibleChannels.includes(channel)) {other.push(application);continue;}
    const bucket=channels.get(channel)??[];
    bucket.push(application);channels.set(channel,bucket);
  }
  return {channels,other};
}
