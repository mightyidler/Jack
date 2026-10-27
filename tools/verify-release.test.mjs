import {test} from 'node:test';
import assert from 'node:assert/strict';
import {validateManifest, validateRelease, prepareManifest, installerName, installerUrl, isNewerVersion, latestReleaseMetadata} from './verify-release.mjs';

test('CI release metadata avoids anonymous REST quotas while local verification can remain anonymous', async () => {
  const payload = JSON.stringify({tag_name: 'v0.1.12', draft: false});
  let authenticatedCalls = 0;
  let anonymousCalls = 0;
  const requests = {
    authenticatedRequest: () => { authenticatedCalls++; return payload; },
    anonymousRequest: () => { anonymousCalls++; return Buffer.from(payload); },
  };
  assert.equal((await latestReleaseMetadata({authenticated: true, ...requests})).tag_name, 'v0.1.12');
  assert.equal(authenticatedCalls, 1);
  assert.equal(anonymousCalls, 0);
  assert.equal((await latestReleaseMetadata({authenticated: false, ...requests})).draft, false);
  assert.equal(anonymousCalls, 1);
  await assert.rejects(latestReleaseMetadata({authenticated: true, ...requests, authenticatedRequest: () => { throw new Error('metadata denied'); }}), /metadata denied/);
});

const version = '0.1.6';
const signature = Buffer.from('untrusted comment: signature from tauri secret key\nfixture').toString('base64');
test('a release cannot replace an equal or newer public version', () => {
  assert.ok(isNewerVersion('0.1.6', '0.1.5'));
  assert.ok(isNewerVersion('0.1.10', '0.1.9'));
  assert.ok(isNewerVersion('1.0.0', '0.99.99'));
  assert.equal(isNewerVersion('0.1.6', '0.1.6'), false);
  assert.equal(isNewerVersion('0.1.6', '0.1.7'), false);
});
function manifest() {
  return {version, pub_date: '2026-10-16T00:00:00Z', platforms: Object.fromEntries(
    ['windows-x86_64', 'windows-x86_64-nsis'].map(target => [target, {signature, url: installerUrl(version)}])
  )};
}
function release() {
  return {tag_name: `v${version}`, draft: false, prerelease: false, assets:
    [installerName(version), `${installerName(version)}.sig`, 'latest.json'].map((name, index) => ({id: index + 100, name, size: 100}))};
}

test('action API URLs are converted only when they identify the verified draft installer', () => {
  const draft = release(); draft.draft = true;
  const input = manifest();
  for (const platform of Object.values(input.platforms)) platform.url = 'https://api.github.com/repos/mightyidler/Jack/releases/assets/100';
  const prepared = prepareManifest(input, draft, version, signature);
  validateManifest(prepared, version, signature);
  assert.equal(input.platforms['windows-x86_64'].url, 'https://api.github.com/repos/mightyidler/Jack/releases/assets/100');
  input.platforms['windows-x86_64'].url = 'https://api.github.com/repos/mightyidler/Jack/releases/assets/999';
  assert.throws(() => prepareManifest(input, draft, version, signature));
  assert.throws(() => prepareManifest(manifest(), release(), version, signature));
});

test('public stable release requires all updater assets and rejects the previous draft-only failure', () => {
  validateRelease(release(), version, true);
  for (const mutate of [r => r.draft = true, r => r.prerelease = true, r => r.tag_name = 'v0.1.4', r => r.assets.pop(), r => r.assets[0].size = 0, r => r.assets.push(r.assets[0])]) {
    const value = release(); mutate(value);
    assert.throws(() => validateRelease(value, version, true));
  }
  const draft = release(); draft.draft = true;
  validateRelease(draft, version, false);
});

test('manifest supports both Windows target formats and rejects stale or mismatched updates', () => {
  validateManifest(manifest(), version, `${signature}\n`);
  for (const mutate of [m => m.version = '0.1.4', m => delete m.platforms['windows-x86_64'], m => delete m.platforms['windows-x86_64-nsis'], m => m.platforms['windows-x86_64'].url = installerUrl('0.1.5'), m => m.platforms['windows-x86_64-nsis'].signature = 'wrong', m => m.pub_date = 'invalid']) {
    const value = manifest(); mutate(value);
    assert.throws(() => validateManifest(value, version, signature));
  }
});
