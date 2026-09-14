import { test } from 'node:test';
import assert from 'node:assert/strict';
import { AiwatcherClient } from '../src/index.ts';

class Recording {
  events = [];
  send(batch) {
    this.events.push(...batch);
  }
  async close() {}
}

test('each client numbers the events it sends into a run from nought', async () => {
  const sent = new Recording();
  const client = new AiwatcherClient({ service: 'agent', transport: sent });
  const tracerSent = new Recording();
  const tracer = new AiwatcherClient({ service: 'agent', transport: tracerSent });

  await client.run('run-1', undefined, async (run) => {
    tracer.emit('llm.started', { runId: 'run-1', correlationId: 'c' }, {});
    await run.agent('writer', async () => {});
  });
  await client.run('run-2', undefined, async () => {});

  const first = sent.events.filter((event) => event.run_id === 'run-1');
  assert.deepEqual(
    first.map((event) => event.sequence),
    first.map((_, at) => at),
  );
  assert.deepEqual(
    sent.events.filter((event) => event.run_id === 'run-2').map((event) => event.sequence),
    [0, 1],
  );
  assert.equal(tracerSent.events[0].sequence, 0);
  assert.notEqual(tracerSent.events[0].source.client, first[0].source.client);
});

test('each client numbers the runs it opens for a variant, a measurement apart', async () => {
  const sent = new Recording();
  const client = new AiwatcherClient({ service: 'agent', transport: sent, variantId: 'v1' });

  await client.run('run-1', undefined, async () => {});
  await client.run('measured', { evaluationId: 'e1' }, async () => {});
  await client.run('run-2', undefined, async () => {});
  await client.run('elsewhere', { variantId: 'v2' }, async () => {});

  const starts = Object.fromEntries(
    sent.events
      .filter((event) => event.event_type === 'run.started')
      .map((event) => [event.run_id, event.run_sequence]),
  );
  assert.deepEqual(starts, { 'run-1': 0, measured: 0, 'run-2': 1, elsewhere: 0 });
  const began = Object.fromEntries(
    sent.events
      .filter((event) => event.event_type === 'run.started')
      .map((event) => [event.run_id, [event.run_counted_from, event.occurred_at]]),
  );
  assert.equal(began['run-2'][0], began['run-1'][1], 'a count began when its first run started');
  assert.equal(began['elsewhere'][0], began['elsewhere'][1]);
  assert.equal(began.measured[0], began.measured[1], "a measurement's runs are a count of their own");
  assert.ok(
    sent.events.every((event) => event.event_type === 'run.started' || event.run_sequence === undefined),
  );
});

test("a measurement's runs are counted per attempt, and a client says each count that moved as it closes", async () => {
  const sent = new Recording();
  const client = new AiwatcherClient({ service: 'worker', transport: sent, variantId: 'v1' });

  for (const attempt of [1, 1, 2]) {
    await client.run(`case-${attempt}`, { evaluationId: 'e1', generationAttempt: attempt }, async () => {});
  }
  await client.run('served', undefined, async () => {});
  assert.deepEqual(
    sent.events
      .filter((event) => event.event_type === 'run.started')
      .map((event) => [event.data.generation_attempt, event.run_sequence]),
    [
      [1, 0],
      [1, 1],
      [2, 0],
      [undefined, 0],
    ],
    'a retried attempt is no gap',
  );

  await client.close();
  client.publishCounts();
  const counted = sent.events.filter((event) => event.event_type === 'client.counted');
  assert.deepEqual(
    counted.map((event) => event.data),
    [
      { runs: 2, evaluation_id: 'e1', generation_attempt: 1 },
      { runs: 1, evaluation_id: 'e1', generation_attempt: 2 },
      { runs: 1 },
    ],
    'a count that did not move is not said again',
  );
  assert.ok(counted.every((event) => event.run_id === `client-${event.source.client}` && event.sequence === undefined));
});
