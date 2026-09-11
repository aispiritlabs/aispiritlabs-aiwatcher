import { test } from 'node:test';
import assert from 'node:assert/strict';
import { EvaluationRegistry, EvaluationRegistryError } from '../src/evaluation-registry.ts';

test('a registry conflict rejects without hiding the failure', async () => {
  let calls = 0;
  const client = new EvaluationRegistry('http://localhost', { fetch: async () => {
    calls++;
    return new Response('different result', { status: 409 });
  } });
  await assert.rejects(client.publish({}, []), (e) => e instanceof EvaluationRegistryError && e.status === 409);
  assert.equal(calls, 1);
});
test('an ambiguous retry keeps the body and logical identity', async () => {
  const bodies = [];
  const client = new EvaluationRegistry('http://localhost', { fetch: async (_, init) => {
    bodies.push(init.body);
    if (bodies.length === 1) throw new Error('response lost');
    return Response.json({ evaluation_id: 'stable', version: 'v1' });
  } });
  const manifest = { origin: { evaluation_id: 'stable' } };
  await assert.rejects(client.publish(manifest, []));
  assert.equal((await client.publish(manifest, [])).version, 'v1');
  assert.equal(bodies[0], bodies[1]);
});
test('case pages preserve explicit version and disappearance state', async () => {
  const client = new EvaluationRegistry('http://localhost/', { token: 'test', fetch: async (url, init) => {
    const parsed = new URL(url);
    assert.equal(parsed.pathname, '/api/v1/evaluation-results/a%2Fb/cases');
    assert.equal(parsed.searchParams.get('version'), 'v1');
    assert.equal(parsed.searchParams.get('cursor'), 'v1:200');
    assert.equal(init.headers.authorization, 'Bearer test');
    assert.equal(init.redirect, 'error');
    return Response.json({ version: 'v1', state: 'expired', cases: [], next_cursor: null });
  } });
  assert.equal((await client.cases('a/b', 'v1', 'v1:200')).state, 'expired');
});
