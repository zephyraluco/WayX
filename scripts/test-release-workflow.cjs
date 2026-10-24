'use strict';
const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

// Execute the actual github-script body with API responses, without remote writes.
const lines = fs.readFileSync(path.join(__dirname, '../.github/workflows/release.yml'), 'utf8').split('\n');
const start = lines.findIndex(line => line.trim() === 'script: |') + 1;
const body = [];
for (const line of lines.slice(start)) {
  if (line.trim() && !line.startsWith('          ')) break;
  body.push(line.slice(10));
}
const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor;
const resolveSource = new AsyncFunction('github', 'context', 'core', 'process', 'setTimeout', body.join('\n'));
const sha = 'a'.repeat(40);
const run = (workflow, status = 'completed', conclusion = 'success', extra = {}) => ({
  id: workflow === 'integration.yml' ? 10 : 20,
  head_sha: sha, event: 'push', head_branch: 'main',
  head_repository: {full_name: 'owner/WayX'}, path: '.github/workflows/' + workflow,
  status, conclusion, ...extra
});

async function execute({integration = [run('integration.yml')], tests = [run('test.yml')], env = {}, artifacts} = {}) {
  const calls = {}, outputs = {}, waits = [];
  const github = {
    rest: {
      repos: {get: async () => ({data: {default_branch: 'main', full_name: 'owner/WayX'}})},
      git: {getRef: async () => ({data: {object: {type: 'commit', sha}}})},
      actions: {
        listWorkflowRuns: 'runs', listWorkflowRunArtifacts: 'artifacts',
        getWorkflowRun: async () => ({data: integration[0]})
      }
    },
    paginate: async (method, args) => {
      if (method === 'artifacts') return artifacts || ['linux', 'windows', 'macos'].map(platform => ({name: `wayx-${platform}-x64-${sha}`, expired: false}));
      assert.equal(args.head_sha, sha);
      assert.equal(args.status, undefined, 'Pending runs must remain visible');
      const sequence = args.workflow_id === 'integration.yml' ? integration : tests;
      const index = calls[args.workflow_id] || 0;
      calls[args.workflow_id] = index + 1;
      const selected = sequence[Math.min(index, sequence.length - 1)];
      return selected ? [selected] : [];
    }
  };
  await resolveSource(github, {repo: {owner: 'owner', repo: 'WayX'}, ref: 'refs/tags/v0.1.0'},
    {info: message => waits.push(message), setOutput: (name, value) => outputs[name] = value},
    {env}, callback => callback());
  return {outputs, waits};
}

test('waits for pending integration and unit tests before selecting artifacts', async () => {
  const result = await execute({
    integration: [run('integration.yml', 'in_progress', null), run('integration.yml')],
    tests: [run('test.yml', 'queued', null), run('test.yml')]
  });
  assert.equal(result.waits.length, 2);
  assert.deepEqual(result.outputs, {tag: 'v0.1.0', sha, 'run-id': '10'});
});

test('failed integration stops publication', async () => {
  await assert.rejects(execute({integration: [run('integration.yml', 'completed', 'failure')]}), /integration.yml failed/);
});

test('failed unit tests stop publication', async () => {
  await assert.rejects(execute({tests: [run('test.yml', 'completed', 'failure')]}), /test.yml failed/);
});

test('explicit run must match the tagged commit and workflow', async () => {
  await assert.rejects(execute({env: {INTEGRATION_RUN_ID: '10'}, integration: [run('integration.yml', 'completed', 'success', {head_sha: 'b'.repeat(40)})]}), /does not match/);
});

test('requires a nonexpired runtime for each platform', async () => {
  await assert.rejects(execute({artifacts: []}), /exactly one tested runtime/);
});


test('allows time for new main-branch runs to appear after a tag push', async () => {
  const result = await execute({integration: [null, run('integration.yml')]});
  assert.equal(result.waits.length, 1);
  assert.equal(result.outputs['run-id'], '10');
});
