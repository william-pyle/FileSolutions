import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import {createHash} from 'node:crypto';
import {execFileSync} from 'node:child_process';
import {pathToFileURL} from 'node:url';

export const repository = 'william-pyle/FileSolutions';
export const repositoryId = 1447702128;
export const workflowId = 408874372;
export const workflowPath = '.github/workflows/windows.yml';
export const target = 'x86_64-pc-windows-msvc';
export const toolchain = '1.98.1';
export const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const read = file => JSON.parse(fs.readFileSync(file, 'utf8'));
export function configuration() {
  const pkg = read('package.json'), tauri = read('src-tauri/tauri.conf.json');
  assert.match(pkg.version, /^\d+\.\d+\.\d+$/);
  assert.match(tauri.productName, /^[A-Za-z][A-Za-z0-9]+$/);
  assert.equal(tauri.version, pkg.version);
  const feature = fs.readFileSync('scripts/build.ps1', 'utf8').match(/'--features','([^']+)'/)?.[1];
  assert.ok(feature?.endsWith('/custom-protocol'), 'Build feature missing');
  return {version: pkg.version, installer: `${tauri.productName}-Setup-${pkg.version}-x64.exe`,
    target, toolchain, feature, profile: 'release', crt: 'static', signing: 'unsigned'};
}
export function validateRun(run, sha) {
  assert.equal(run.repository?.full_name, repository, 'Producer repository');
  assert.equal(run.repository?.id, repositoryId, 'Producer repository ID');
  assert.equal(run.head_repository?.id, repositoryId, 'Producer head repository');
  assert.equal(run.workflow_id, workflowId, 'Producer workflow ID');
  assert.equal(run.path, workflowPath, 'Producer workflow path');
  assert.equal(run.event, 'push', 'Producer must be a main push');
  assert.equal(run.head_branch, 'main', 'Producer branch');
  assert.equal(run.head_sha, sha, 'Producer commit');
  assert.equal(run.status, 'completed', 'Producer incomplete');
  assert.equal(run.conclusion, 'success', 'Producer CI failed');
  assert.ok(Number.isSafeInteger(run.id) && run.id > 0);
  assert.ok(Number.isSafeInteger(run.run_attempt) && run.run_attempt > 0);
}
export const artifactName = (sha, attempt) => `windows-x64-${sha}-${attempt}`;
export function selectArtifact(artifacts, run) {
  const found = artifacts.filter(a => a.name === artifactName(run.head_sha, run.run_attempt));
  assert.equal(found.length, 1, 'Expected exactly one artifact from the successful run attempt');
  const a = found[0];
  assert.equal(a.expired, false, 'Artifact expired');
  assert.ok(Number.isSafeInteger(a.id) && a.id > 0, 'Artifact ID missing');
  assert.equal(a.workflow_run?.id, run.id, 'Artifact run');
  assert.equal(a.workflow_run?.head_sha, run.head_sha, 'Artifact commit');
  assert.equal(a.workflow_run?.head_branch, 'main', 'Artifact branch');
  assert.equal(a.workflow_run?.repository_id, repositoryId, 'Artifact repository');
  assert.equal(a.workflow_run?.head_repository_id, repositoryId, 'Artifact head repository');
  assert.match(a.digest, /^sha256:[a-f0-9]{64}$/, 'Artifact archive digest missing');
  return a;
}
export function verifyManifest(manifest, run, config, directory) {
  const expected = {schema: 1, repository, repositoryId, workflowId, workflowPath,
    runId: run.id, runAttempt: run.run_attempt, commit: run.head_sha,
    workflowRef: `${repository}/${workflowPath}@refs/heads/main`, workflowSha: run.head_sha, ...config};
  for (const [key, value] of Object.entries(expected)) assert.equal(manifest[key], value, `Manifest ${key}`);
  assert.deepEqual(Object.keys(manifest.files).sort(), [config.installer, `${config.installer}.sha256`].sort());
  for (const [name, info] of Object.entries(manifest.files)) {
    const bytes = fs.readFileSync(path.join(directory, name));
    assert.equal(bytes.length, info.size, `Size mismatch: ${name}`);
    assert.equal(digest(bytes), info.sha256, `Hash mismatch: ${name}`);
  }
  assert.equal(fs.readFileSync(path.join(directory, `${config.installer}.sha256`), 'utf8').trim(),
    `${manifest.files[config.installer].sha256}  ${config.installer}`, 'Installer checksum file');
}
async function request(endpoint) {
  assert.ok(process.env.GH_TOKEN, 'Repository token required');
  const response = await fetch(`https://api.github.com/repos/${repository}/${endpoint}`, {
    headers: {Authorization: `Bearer ${process.env.GH_TOKEN}`, Accept: 'application/vnd.github+json', 'X-GitHub-Api-Version': '2022-11-28'}
  });
  return response;
}
async function api(endpoint) {
  const response = await request(endpoint);
  assert.ok(response.ok, `GitHub API ${response.status}: ${endpoint}`);
  return response.json();
}
async function all(endpoint, key) {
  const items = [];
  for (let page = 1; ; page++) {
    const data = await api(`${endpoint}${endpoint.includes('?') ? '&' : '?'}per_page=100&page=${page}`);
    items.push(...data[key]);
    if (data[key].length < 100) return items;
  }
}
export async function main(command) {
  assert.equal(process.env.GITHUB_REPOSITORY, repository, 'Unexpected repository');
  const sha = process.env.GITHUB_SHA;
  assert.match(sha, /^[a-f0-9]{40}$/);
  const config = configuration();
  if (command === 'stamp') {
    const rust = execFileSync('rustc', ['-vV'], {encoding: 'utf8'});
    assert.match(rust, new RegExp(`^release: ${toolchain.replaceAll('.', '\\.')}\\r?$`, 'm'), 'Rust version');
    assert.ok(rust.includes(`host: ${target}`), 'Rust host target');
    const files = Object.fromEntries([config.installer, `${config.installer}.sha256`].map(name => {
      const bytes = fs.readFileSync(path.join('out', name));
      return [name, {size: bytes.length, sha256: digest(bytes)}];
    }));
    const manifest = {schema: 1, repository, repositoryId, workflowId, workflowPath,
      runId: Number(process.env.GITHUB_RUN_ID), runAttempt: Number(process.env.GITHUB_RUN_ATTEMPT),
      commit: sha, workflowRef: process.env.GITHUB_WORKFLOW_REF, workflowSha: process.env.GITHUB_WORKFLOW_SHA, ...config, files};
    fs.writeFileSync('out/provenance.json', JSON.stringify(manifest, null, 2) + '\n');
  } else if (command === 'resolve') {
    assert.equal(process.env.GITHUB_REF, `refs/tags/v${config.version}`, 'Release tag/version mismatch');
    const runs = await all(`actions/workflows/${workflowId}/runs?event=push&branch=main&head_sha=${sha}&status=success`, 'workflow_runs');
    assert.ok(runs.length, 'No successful main CI for this exact commit. Finish CI before tagging.');
    const run = await api(`actions/runs/${runs[0].id}`);
    validateRun(run, sha);
    const artifact = selectArtifact(await all(`actions/runs/${run.id}/artifacts`, 'artifacts'), run);
    fs.writeFileSync('producer.json', JSON.stringify({run, artifact}, null, 2) + '\n');
    fs.appendFileSync(process.env.GITHUB_OUTPUT, `run-id=${run.id}\nartifact-id=${artifact.id}\n`);
  } else if (command === 'download') {
    const {run, artifact} = read('producer.json');
    validateRun(run, sha);
    selectArtifact([artifact], run);
    const response = await request(`actions/artifacts/${artifact.id}/zip`);
    assert.ok(response.ok, `Artifact download HTTP ${response.status}`);
    const bytes = Buffer.from(await response.arrayBuffer());
    assert.equal(`sha256:${digest(bytes)}`, artifact.digest, 'Artifact archive checksum mismatch');
    fs.writeFileSync('trusted-artifact.zip', bytes);
  } else if (command === 'verify') {
    assert.equal(process.env.GITHUB_REF, `refs/tags/v${config.version}`, 'Release tag/version mismatch');
    const {run: saved, artifact: savedArtifact} = read('producer.json');
    const run = await api(`actions/runs/${saved.id}`);
    validateRun(run, sha);
    assert.equal(run.run_attempt, saved.run_attempt, 'Producer rerun since resolution');
    const artifact = selectArtifact([await api(`actions/artifacts/${savedArtifact.id}`)], run);
    assert.equal(artifact.digest, savedArtifact.digest, 'Artifact archive digest changed');
    verifyManifest(read('release/provenance.json'), run, config, 'release');
    console.log(`Verified ${config.installer}, producer run ${run.id}, commit ${sha}`);
  } else if (command === 'preflight') {
    assert.equal(process.env.GITHUB_REF, `refs/tags/v${config.version}`, 'Release tag/version mismatch');
    const response = await request(`releases/tags/v${config.version}`);
    assert.equal(response.status, 404, 'Release must be absent; existing release or API failure blocks publication');
  } else if (command === 'verify-published') {
    const manifest = read('release/provenance.json');
    for (const [name, info] of Object.entries(manifest.files)) {
      const bytes = fs.readFileSync(path.join('published', name));
      assert.equal(bytes.length, info.size, `Published size: ${name}`);
      assert.equal(digest(bytes), info.sha256, `Published hash: ${name}`);
    }
    console.log('Published installer and checksum match the verified producer bytes.');
  } else throw new Error(`Unknown command: ${command}`);
}
if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) await main(process.argv[2]);
