export function isFresh(entry,now){return now-entry.createdAt>entry.ttl}
