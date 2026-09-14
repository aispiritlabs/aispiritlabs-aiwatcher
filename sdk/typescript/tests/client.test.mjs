import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, readdirSync } from 'node:fs';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { AiwatcherClient, HttpTransport } from '../src/index.ts';
import { fileSpool } from '../src/node.ts';

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

test('a client says its counts that moved by its own clock, with no event after them', async () => {
  const sent = new Recording();
  const client = new AiwatcherClient({ service: 'worker', transport: sent, variantId: 'v1', countsEveryMs: 20 });
  await client.run('served', undefined, async () => {});
  const deadline = Date.now() + 5_000;
  while (!sent.events.some((event) => event.event_type === 'client.counted') && Date.now() < deadline) {
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
  await client.close();
  assert.deepEqual(
    sent.events.filter((event) => event.event_type === 'client.counted').map((event) => event.data),
    [{ runs: 1 }],
    'said once by the clock, and not again as it closes',
  );
});

test('a client killed without closing leaves its count for the next transport on its spool', async (t) => {
  t.mock.method(console, 'warn', () => {});
  const directory = mkdtempSync(join(tmpdir(), 'aiwatcher-spool-'));
  const closed = createServer();
  await new Promise((resolve) => closed.listen(0, '127.0.0.1', resolve));
  const nobody = closed.address().port;
  await new Promise((resolve) => closed.close(resolve));
  const down = new AiwatcherClient({
    service: 'worker',
    variantId: 'v1',
    transport: new HttpTransport({ baseUrl: `http://127.0.0.1:${nobody}`, flushIntervalMs: 60_000, spool: fileSpool(directory) }),
  });
  for (const at of [0, 1, 2]) await down.run(`lost-${at}`, undefined, async () => {});
  // Nothing was sent yet: what a process killed now leaves is what the spool holds.
  const [kept] = readdirSync(directory);
  assert.equal(JSON.parse(readFileSync(join(directory, kept), 'utf8')).data.runs, 3, 'held as each run started');
  await down.close();
  assert.equal(readdirSync(directory).length, 1, 'a count that could not be delivered stays kept');

  const received = [];
  const server = createServer((request, response) => {
    let body = '';
    request.on('data', (chunk) => (body += chunk));
    request.on('end', () => {
      received.push(...JSON.parse(body).events);
      response.writeHead(202).end();
    });
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  try {
    const after = new HttpTransport({ baseUrl: `http://127.0.0.1:${server.address().port}`, spool: fileSpool(directory) });
    await after.close();
  } finally {
    server.close();
  }
  assert.deepEqual(
    received.map((event) => [event.event_type, event.data.runs]),
    [['client.counted', 3]],
  );
  assert.deepEqual(readdirSync(directory), [], 'a delivered count is forgotten');
});
