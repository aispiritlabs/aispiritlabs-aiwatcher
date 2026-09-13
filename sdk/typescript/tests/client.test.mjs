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
