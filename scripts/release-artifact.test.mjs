import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {repository, repositoryId, workflowId, workflowPath, target, toolchain, digest,
  configuration, validateRun, selectArtifact, artifactName, verifyManifest, main} from './release-artifact.mjs';

const sha = 'a'.repeat(40);
const run = {id: 123, run_attempt: 1, repository: {id: repositoryId, full_name: repository},
  head_repository: {id: repositoryId}, workflow_id: workflowId, path: workflowPath,
  event: 'push', head_branch: 'main', head_sha: sha, status: 'completed', conclusion: 'success'};
const zip = Buffer.from('disposable archive fixture');
const artifact = {id: 456, name: artifactName(sha, 1), expired: false, digest: `sha256:${digest(zip)}`,
  workflow_run: {id: run.id, head_sha: sha, head_branch: 'main', repository_id: repositoryId, head_repository_id: repositoryId}};
const config = {version: '1.2.3', installer: 'FileSolutions-Setup-1.2.3-x64.exe', target, toolchain,
  feature: 'filesolutions/custom-protocol', profile: 'release', crt: 'static', signing: 'unsigned'};
const manifest = {schema: 1, repository, repositoryId, workflowId, workflowPath, runId: run.id,
  runAttempt: 1, commit: sha, workflowRef: `${repository}/${workflowPath}@refs/heads/main`, workflowSha: sha, ...config};
const bytes = Buffer.from('disposable installer fixture');
const checksum = Buffer.from(`${digest(bytes)}  ${config.installer}\r\n`);
manifest.files = {[config.installer]: {size: bytes.length, sha256: digest(bytes)},
  [`${config.installer}.sha256`]: {size: checksum.length, sha256: digest(checksum)}};
const clone = data => structuredClone(data);
function fixture(fn) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'filesolutions-release-'));
  fs.writeFileSync(path.join(dir, config.installer), bytes);
  fs.writeFileSync(path.join(dir, `${config.installer}.sha256`), checksum);
  try { return fn(dir); } finally { fs.rmSync(dir, {recursive: true, force: true}); }
}
test('valid main producer and immutable artifact accepted', () => {
  validateRun(run, sha); assert.equal(selectArtifact([artifact], run).id, artifact.id);
});
for (const [key, value] of Object.entries({repository: {id: 1, full_name: 'other/repo'}, head_repository: {id: 1},
  workflow_id: 1, path: '.github/workflows/other.yml', event: 'pull_request', head_branch: 'feature',
  head_sha: 'b'.repeat(40), status: 'in_progress', conclusion: 'failure', id: null, run_attempt: 0})) {
  test(`reject producer ${key} mismatch`, () => assert.throws(() => validateRun({...run, [key]: value}, sha)));
}
for (const [label, edit] of Object.entries({missing: () => [], duplicate: () => [artifact, artifact],
  expired: () => [{...artifact, expired: true}], staleAttempt: () => [{...artifact, name: artifactName(sha, 2)}],
  missingDigest: () => [{...artifact, digest: undefined}], wrongRun: () => [{...artifact, workflow_run: {...artifact.workflow_run, id: 1}}],
  wrongSha: () => [{...artifact, workflow_run: {...artifact.workflow_run, head_sha: 'b'.repeat(40)}}],
  foreignRepo: () => [{...artifact, workflow_run: {...artifact.workflow_run, repository_id: 1}}]})) {
  test(`reject artifact ${label}`, () => assert.throws(() => selectArtifact(edit(), run)));
}
test('verify final installer and checksum bytes', () => fixture(dir => verifyManifest(manifest, run, config, dir)));
for (const key of ['repository', 'repositoryId', 'workflowId', 'workflowPath', 'runId', 'runAttempt', 'commit',
  'workflowRef', 'workflowSha', 'version', 'installer', 'target', 'toolchain', 'feature', 'profile', 'crt', 'signing']) {
  test(`reject manifest ${key} mismatch`, () => fixture(dir => {
    assert.throws(() => verifyManifest({...manifest, [key]: 'wrong'}, run, config, dir));
  }));
}
test('reject tampered installer', () => fixture(dir => {
  fs.writeFileSync(path.join(dir, config.installer), Buffer.alloc(bytes.length));
  assert.throws(() => verifyManifest(manifest, run, config, dir), /Hash mismatch/);
}));
test('reject missing file and unexpected manifest filename', () => fixture(dir => {
  fs.unlinkSync(path.join(dir, config.installer));
  assert.throws(() => verifyManifest(manifest, run, config, dir));
  const bad = clone(manifest); bad.files['../escape.exe'] = bad.files[config.installer];
  assert.throws(() => verifyManifest(bad, run, config, dir));
}));
test('reject checksum naming error even with matching manifest hash', () => fixture(dir => {
  const bad = clone(manifest), data = Buffer.from(`${digest(bytes)}  wrong.exe`);
  fs.writeFileSync(path.join(dir, `${config.installer}.sha256`), data);
  bad.files[`${config.installer}.sha256`] = {size: data.length, sha256: digest(data)};
  assert.throws(() => verifyManifest(bad, run, config, dir), /checksum file/);
}));
test('CLI resolution, archive, verification and preflight fail closed', async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'filesolutions-promotion-'));
  const cwd = process.cwd(), env = {...process.env}, originalFetch = globalThis.fetch;
  try {
    process.chdir(dir);
    for (const sub of ['src-tauri', 'scripts', 'release', 'published']) fs.mkdirSync(sub);
    fs.writeFileSync('package.json', JSON.stringify({version: config.version}));
    fs.writeFileSync('src-tauri/tauri.conf.json', JSON.stringify({version: config.version, productName: 'FileSolutions'}));
    fs.writeFileSync('scripts/build.ps1', "'--features','filesolutions/custom-protocol'");
    fs.writeFileSync('release/provenance.json', JSON.stringify(manifest));
    for (const sub of ['release', 'published']) {
      fs.writeFileSync(`${sub}/${config.installer}`, bytes);
      fs.writeFileSync(`${sub}/${config.installer}.sha256`, checksum);
    }
    Object.assign(process.env, {GH_TOKEN: 'fixture', GITHUB_REPOSITORY: repository, GITHUB_SHA: sha,
      GITHUB_REF: `refs/tags/v${config.version}`, GITHUB_OUTPUT: path.join(dir, 'outputs')});
    assert.deepEqual(configuration(), config);
    let empty = false, status = 404, corrupt = false, attempt = 1;
    globalThis.fetch = async url => {
      if (url.includes('/releases/tags/')) return new Response('{}', {status});
      if (url.endsWith('/zip')) return new Response(corrupt ? Buffer.from('corrupt') : zip);
      if (url.includes('/workflows/')) return Response.json({workflow_runs: empty ? [] : [run]});
      if (url.includes('/runs/123/artifacts')) return Response.json({artifacts: [artifact]});
      if (url.endsWith('/runs/123')) return Response.json({...run, run_attempt: attempt});
      if (url.endsWith('/artifacts/456')) return Response.json(artifact);
      throw new Error(`Unexpected API URL: ${url}`);
    };
    empty = true; await assert.rejects(main('resolve'), /No successful main CI/);
    empty = false; await main('resolve');
    assert.match(fs.readFileSync('outputs', 'utf8'), /artifact-id=456/);
    await main('download'); assert.equal(digest(fs.readFileSync('trusted-artifact.zip')), digest(zip));
    corrupt = true; await assert.rejects(main('download'), /archive checksum mismatch/);
    await main('verify');
    attempt = 2; await assert.rejects(main('verify'), /rerun since resolution/); attempt = 1;
    await main('preflight');
    for (status of [200, 401, 403, 429, 500]) await assert.rejects(main('preflight'), /Release must be absent/);
    await main('verify-published');
    fs.writeFileSync(`published/${config.installer}`, 'wrong');
    await assert.rejects(main('verify-published'), /Published size/);
    process.env.GITHUB_REF = 'refs/tags/v9.9.9';
    await assert.rejects(main('resolve'), /tag\/version mismatch/);
    globalThis.fetch = async () => new Response('{}', {status: 403});
    process.env.GITHUB_REF = `refs/tags/v${config.version}`;
    await assert.rejects(main('resolve'), /GitHub API 403/);
  } finally {
    globalThis.fetch = originalFetch;
    for (const key of Object.keys(process.env)) if (!(key in env)) delete process.env[key];
    Object.assign(process.env, env); process.chdir(cwd);
    fs.rmSync(dir, {recursive: true, force: true});
  }
});
