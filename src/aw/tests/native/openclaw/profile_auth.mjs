import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

// Invoke the pinned host's credential store API without printing secret material.
const [mode, packageRoot, keyFile] = process.argv.slice(2);
assert.ok(['seed', 'inspect'].includes(mode));
assert.equal(JSON.parse(readFileSync(resolve(packageRoot, 'package.json'))).version, '2026.9.6');
const dist = resolve(packageRoot, 'dist');
const entry = readdirSync(dist).find(name => /^auth-profiles-.*\.mjs$/.test(name)
  && readFileSync(resolve(dist, name), 'utf8').includes('export { buildPortableAuthProfileStoreForAgentCopy'));
assert.ok(entry, 'Pinned native auth store API must exist');
const native = await import(pathToFileURL(resolve(dist, entry)));
const profileId = 'aw-token-plan:profile-fixture';
const key = readFileSync(keyFile, 'utf8').trim();
assert.ok(key.length > 0);
if (mode === 'seed') {
  native.upsertAuthProfile({ profileId, credential: { type: 'api_key', provider: 'aw-token-plan', key } });
}
const profile = native.loadAuthProfileStoreWithoutExternalProfiles().profiles[profileId];
assert.ok(profile, 'Persistent native profile is missing');
assert.equal(profile.type, 'api_key');
assert.equal(profile.provider, 'aw-token-plan');
assert.ok(profile.key === key, 'Persistent native credential does not match the fixture');
console.log(JSON.stringify({ mode, profileId, nativeStore: true, credentialMatches: true }));
