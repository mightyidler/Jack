import {readFileSync, mkdirSync, writeFileSync} from 'node:fs';
import {resolve, join} from 'node:path';
import {pathToFileURL} from 'node:url';
import {execFileSync} from 'node:child_process';
import {createHash} from 'node:crypto';
import assert from 'node:assert/strict';

const repository = 'mightyidler/Jack';
export const endpoint = `https://github.com/${repository}/releases/latest/download/latest.json`;
export const installerName = version => `Jack_${version}_x64-setup.exe`;
export const installerUrl = version => `https://github.com/${repository}/releases/download/v${version}/${installerName(version)}`;
export function isNewerVersion(candidate, current) {
  assert.match(candidate, /^\d+\.\d+\.\d+$/);
  assert.match(current, /^\d+\.\d+\.\d+$/);
  const left = candidate.split('.').map(Number);
  const right = current.split('.').map(Number);
  for (let i = 0; i < 3; i++) if (left[i] !== right[i]) return left[i] > right[i];
  return false;
}
const json = file => JSON.parse(readFileSync(file, 'utf8'));
const digest = bytes => createHash('sha256').update(bytes).digest('hex');

export function validateManifest(manifest, version, signature) {
  assert.equal(manifest.version, version, 'Updater manifest version differs from the app');
  assert.ok(Number.isFinite(Date.parse(manifest.pub_date)), 'Missing or invalid release date');
  for (const target of ['windows-x86_64', 'windows-x86_64-nsis']) {
    const platform = manifest.platforms?.[target];
    assert.equal(platform?.url, installerUrl(version), `Wrong installer URL for ${target}`);
    assert.equal(platform?.signature?.trim(), signature.trim(), `Wrong signature for ${target}`);
    assert.match(Buffer.from(platform.signature, 'base64').toString('utf8'), /^untrusted comment: signature/);
  }
}

export function validateRelease(release, version, published) {
  assert.equal(release.tag_name, `v${version}`);
  assert.equal(release.prerelease, false, 'Stable updates cannot use a prerelease');
  assert.equal(release.draft, !published, published ? 'Release is still a draft; installed apps cannot update' : 'Expected a draft before verification');
  for (const name of [installerName(version), `${installerName(version)}.sig`, 'latest.json']) {
    const matches = release.assets.filter(asset => asset.name === name);
    assert.equal(matches.length, 1, `Expected exactly one ${name}`);
    assert.ok(matches[0].size > 0, `Empty release asset: ${name}`);
  }
}

export function verifySource() {
  const config = json('desktop/src-tauri/tauri.conf.json');
  const pkg = json('desktop/package.json');
  const lock = json('desktop/package-lock.json');
  const cargo = readFileSync('Cargo.toml', 'utf8');
  const version = cargo.match(/\[workspace\.package\][\s\S]*?^version\s*=\s*"([^"]+)"/m)?.[1];
  assert.match(config.version, /^\d+\.\d+\.\d+$/);
  for (const value of [pkg.version, lock.version, lock.packages[''].version, version]) assert.equal(value, config.version);
  const tag = process.env.RELEASE_TAG ?? (process.env.GITHUB_REF_TYPE === 'tag' ? process.env.GITHUB_REF_NAME : undefined);
  if (tag) assert.equal(tag, `v${config.version}`);
  assert.equal(config.bundle.active, true);
  assert.equal(config.bundle.createUpdaterArtifacts, true);
  assert.deepEqual(config.bundle.targets, ['nsis']);
  assert.equal(config.bundle.windows.nsis.installMode, 'currentUser');
  assert.equal(config.plugins.updater.windows.installMode, 'passive');
  assert.match(Buffer.from(config.plugins.updater.pubkey, 'base64').toString('utf8'), /^untrusted comment: minisign public key:/);
  assert.deepEqual(config.plugins.updater.endpoints, [endpoint]);
  assert.equal(config.bundle.windows.nsis.installerHooks, 'windows/hooks.nsh');
  for (const file of ['LICENSE', 'THIRD_PARTY_NOTICES.md', 'THIRD_PARTY_LICENSES.txt', 'desktop/public/assets/LICENSE-sonarctl.txt', 'desktop/public/assets/LICENSE-sds.txt', 'desktop/public/assets/fonts/OFL.txt']) assert.ok(readFileSync(file).length);
  assert.equal(config.bundle.resources['../../THIRD_PARTY_LICENSES.txt'], 'THIRD_PARTY_LICENSES.txt');
  // Keep both the last public release and the manually installed draft updatable.
  for (const tag of ['v0.1.4', 'v0.1.5']) {
    const previous = JSON.parse(execFileSync('git', ['show', `${tag}:desktop/src-tauri/tauri.conf.json`], {encoding: 'utf8'}));
    assert.equal(config.plugins.updater.pubkey, previous.plugins.updater.pubkey, `Updater signing key changed since ${tag}`);
    assert.equal(config.identifier, previous.identifier, `App identity changed since ${tag}`);
    assert.equal(config.bundle.windows.nsis.installMode, previous.bundle.windows.nsis.installMode);
  }
  console.log(`Jack ${config.version}: versions, existing updater keys, endpoint, installer and licenses verified.`);
  return config.version;
}

export function verifyAssets(directory, version) {
  const signature = readFileSync(join(directory, `${installerName(version)}.sig`), 'utf8');
  validateManifest(json(join(directory, 'latest.json')), version, signature);
  const bytes = readFileSync(join(directory, installerName(version)));
  assert.equal(bytes.subarray(0, 2).toString(), 'MZ', 'Installer is not a Windows executable');
  assert.ok(bytes.length > 1_000_000, 'Installer is unexpectedly small');
  execFileSync('cargo', ['run', '--locked', '-p', 'jack-desktop', '--example', 'verify-update', '--', resolve(directory, installerName(version)), resolve(directory, 'latest.json')], {stdio: 'inherit'});
  console.log(`Downloaded release assets verified: ${installerName(version)} SHA256 ${digest(bytes)}`);
}

async function fetchBytes(url) {
  // Deliberately no authorization: the installed app must be able to fetch this too.
  const response = await fetch(url, {signal: AbortSignal.timeout(60_000), headers: {'Cache-Control': 'no-cache', 'User-Agent': 'Jack-release-verification'}});
  assert.equal(response.status, 200, `Public download failed: ${url}`);
  return Buffer.from(await response.arrayBuffer());
}

export async function verifyPublished(directory, version) {
  const release = JSON.parse((await fetchBytes(`https://api.github.com/repos/${repository}/releases/latest`)).toString());
  validateRelease(release, version, true);
  const manifest = JSON.parse((await fetchBytes(endpoint)).toString());
  const signature = (await fetchBytes(`${installerUrl(version)}.sig`)).toString();
  validateManifest(manifest, version, signature);
  const installer = await fetchBytes(manifest.platforms['windows-x86_64'].url);
  mkdirSync(directory, {recursive: true});
  writeFileSync(join(directory, 'latest.json'), JSON.stringify(manifest, null, 2));
  writeFileSync(join(directory, `${installerName(version)}.sig`), signature);
  writeFileSync(join(directory, installerName(version)), installer);
  verifyAssets(directory, version);
  console.log(`Public latest release and anonymous updater downloads verified: v${version}.`);
}

async function main() {
  const version = verifySource();
  const [mode, directory] = process.argv.slice(2);
  if (!mode) return;
  if (mode === '--preflight') {
    const latest = JSON.parse((await fetchBytes(`https://api.github.com/repos/${repository}/releases/latest`)).toString());
    assert.ok(isNewerVersion(version, latest.tag_name.replace(/^v/, '')), 'Refusing to replace an equal or newer public release');
    console.log(`Release preflight: v${version} is newer than public ${latest.tag_name}.`);
    return;
  }
  assert.ok(directory, 'An asset directory is required');
  if (mode === '--assets') verifyAssets(directory, version);
  else if (mode === '--draft') {
    // GitHub's release-by-tag endpoint excludes drafts, including for owners.
    const releases = JSON.parse(execFileSync('gh', ['api', '--paginate', '--slurp', `repos/${repository}/releases?per_page=100`], {encoding: 'utf8'})).flat();
    const release = releases.find(release => release.tag_name === `v${version}`);
    assert.ok(release, `Draft release v${version} was not found`);
    validateRelease(release, version, false);
    verifyAssets(directory, version);
  }
  else if (mode === '--published') await verifyPublished(directory, version);
  else throw new Error(`Unknown mode: ${mode}`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) await main();
