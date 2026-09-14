import { test } from 'node:test';
import assert from 'node:assert/strict';
import { AiwatcherClient } from '../src/index.ts';
import { ToolWitness, canonical, normalized, witnessDigest, witnessKey } from '../src/tool-witness.ts';

class Recording {
  events = [];
  send(batch) {
    this.events.push(...batch);
  }
  async close() {}
}

const hex = (bytes) => [...bytes].map((byte) => byte.toString(16).padStart(2, '0')).join('');

test('a digest is the bytes the deployment and the Python gateway compute', async () => {
  const key = await witnessKey('serving-secret');
  assert.equal(hex(key), 'b27f733074077db3bb749314c6dbc46fc967028defacac53f83907b0ecf83c15');
  assert.equal(await witnessDigest(key, 'replied', ' Lima\n'), '424880e43451b760e40c782d90743997');
  assert.equal(
    await witnessDigest(key, 'asked', 'What is the capital of Peru?'),
    '8f3cf1564557884b7b44e9bf0077f300',
  );
  assert.equal(
    await witnessDigest(key, 'taking', canonical({ map: { A: 'Lima' } })),
    '3b1d627f22d3a4b86fcd29126042cd6e',
  );
  const gateway = await witnessKey('gateway-secret');
  assert.equal(
    await witnessDigest(gateway, 'replied', 'Lima\u3000\u0085'),
    'becae0e9938462f335ba0f6c50f98718',
    "trimmed of Unicode's white space, as Rust trims, which is not what String.trim removes",
  );
  assert.equal(
    canonical({ b: 1, '\u00e9': 2, a: [0.1, 1e21] }),
    '{"a":[0.1,1e+21],"b":1,"\u00e9":2}',
  );
  assert.equal(canonical(18446744073709551615n), '18446744073709551615');
});

test("a tool's call is witnessed where it runs, by digests of its arguments and of what it returned", async () => {
  const sent = new Recording();
  const witness = ToolWitness.withKey(
    new AiwatcherClient({ service: 'atlas', transport: sent }),
    hex(await witnessKey('gateway-secret')),
  );
  const args = { query: 'Peru', limit: 3, near: [1.5, true, null, '  '] };

  const answered = await witness.call('atlas', args, { caller: 'app-run' }, async (call) => {
    call.answered('{"capital": "Lima", "population": 10}');
    return 'done';
  });
  await assert.rejects(
    witness.call('atlas', args, { caller: 'app-run' }, async () => {
      throw new Error('the atlas is down');
    }),
    /the atlas is down/,
  );

  assert.equal(answered, 'done');
  const [done, failed] = sent.events
    .filter((event) => event.event_type === 'tool.completed')
    .map((event) => event.data);
  assert.deepEqual(done.arguments_digests, [
    '24a83d3f9aace35b69efd28a9df975a4',
    '16e949dc941cef5287e0308ec8281e84',
    '246b2bf27f397b8ec49643fbf2369b9c',
    'a5fa0def514bd9fbd4317bb79db959f6',
  ]);
  assert.deepEqual(
    done.returned_digests,
    ['0a7ad0bd41f67e2187d714f1f7efa6b1', '8efcf1ceb3a38c44a2e20e13b236885a'],
    'the digests the Python gateway publishes for the same call',
  );
  assert.deepEqual([failed.outcome, failed.returned_digests], ['failed', []]);
  const started = sent.events.filter((event) => event.event_type === 'run.started');
  assert.deepEqual(new Set(started.map((event) => event.data.caller_run_id)), new Set(['app-run']));
  assert.ok(!JSON.stringify(sent.events).includes('Lima'));
  assert.ok(!JSON.stringify(sent.events).includes('Peru'));
});

test('a question normalises to the bytes the deployment and the Python gateway compute', () => {
  for (const [text, normal] of [
    ['  What is the CAPITAL of France?  ', 'what is the capital of france'],
    [
      '\uff30\uff41\uff52\uff49\uff53\uff0c\u3000\uff26\uff32\uff21\uff2e\uff23\uff25\uff01',
      'paris france',
    ],
    ['Don\u2019t\tstop\u2014e-mail\u2026\ufb01ne', 'dont stopemailfine'],
    ['\u039f\u0394\u039f\u03a3 \u03a3', '\u03bf\u03b4\u03bf\u03c2 \u03c3'],
    ['\u0130stanbul', 'i\u0307stanbul'],
    ['\u00a0\u00bfQu\u00e9\u2003pasa?\u200b', 'qu\u00e9 pasa\u200b'],
  ]) {
    assert.equal(normalized(text), normal, text);
  }
});
