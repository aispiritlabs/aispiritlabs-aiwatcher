import { expect, it } from 'vitest';

import { factsOf, familyOf, otherAttributes, type Span } from './span-facts';

function span(attributes: Array<[string, unknown]>, name = 'chat gemini-3.7-flash'): Span {
  return {
    trace_id: 'trace',
    span_id: 'span',
    name,
    kind: 'client',
    start: '2026-09-14T12:00:00Z',
    end: '2026-09-14T12:00:03Z',
    status: { status: 'ok' },
    attributes,
  };
}

it('reads the call, its settings and what it spent out of the attributes', () => {
  const facts = factsOf(
    span([
      ['gen_ai.provider.name', 'openrouter.ai'],
      ['gen_ai.request.model', 'google/gemini-3.7-flash'],
      ['gen_ai.response.model', 'google/gemini-3.7-flash-002'],
      ['gen_ai.response.finish_reasons', ['stop']],
      ['gen_ai.request.temperature', 0.2],
      ['gen_ai.usage.input_tokens', 1_240],
      ['gen_ai.usage.output_tokens', 164],
      ['gen_ai.usage.cached_tokens', 1_024],
    ]),
  );

  expect(facts.family).toBe('llm');
  expect(facts.headline).toBe('google/gemini-3.7-flash');
  expect(facts.tokens).toEqual({ input: 1_240, output: 164, cached: 1_024 });
  const call = facts.groups.find((group) => group.title === 'Call');
  expect(call?.facts).toContainEqual({ label: 'Provider', value: 'openrouter.ai', id: undefined });
  expect(call?.facts).toContainEqual({ label: 'Finish reason', value: 'stop', id: undefined });
  // The provider answered with a model the request did not name, which is the
  // only case where repeating it says something.
  expect(call?.facts).toContainEqual({
    label: 'Answered by',
    value: 'google/gemini-3.7-flash-002',
  });
  expect(facts.groups.find((group) => group.title === 'Settings')?.facts).toEqual([
    { label: 'Temperature', value: '0.2', id: undefined },
  ]);
});

it('leaves a setting nobody sent out rather than showing it as nought', () => {
  const facts = factsOf(span([['gen_ai.request.model', 'gpt-5-mini']]));

  expect(facts.groups.some((group) => group.title === 'Settings')).toBe(false);
  expect(facts.tokens).toBeUndefined();
  expect(facts.prompt).toBeUndefined();
});

it('does not repeat the request model as the model that answered', () => {
  const facts = factsOf(
    span([
      ['gen_ai.request.model', 'gpt-5-mini'],
      ['gen_ai.response.model', 'gpt-5-mini'],
    ]),
  );

  const call = facts.groups.find((group) => group.title === 'Call');
  expect(call?.facts.some((fact) => fact.label === 'Answered by')).toBe(false);
});

it('names the registered prompt a call ran on, with what a host verified of it', () => {
  const facts = factsOf(
    span([
      ['aiwatcher.prompt.name', 'planner.assistant'],
      ['aiwatcher.prompt.version_id', 'a'.repeat(64)],
      ['aiwatcher.prompt.verified', true],
    ]),
  );

  expect(facts.prompt).toEqual({
    name: 'planner.assistant',
    versionId: 'a'.repeat(64),
    verified: true,
    exact: undefined,
  });
});

it('takes a step for what its payload called it, whatever this build knows', () => {
  const facts = factsOf(
    span(
      [
        ['aiwatcher.span.step_type', 'policy_check'],
        ['aiwatcher.span.step_name', 'house rules'],
        ['aiwatcher.step.document_count', 8],
      ],
      'house rules',
    ),
  );

  expect(facts.family).toBe('step');
  expect(facts.headline).toBe('house rules');
  expect(facts.groups.find((group) => group.title === 'Call')?.facts).toContainEqual({
    label: 'Step',
    value: 'policy_check',
    id: undefined,
  });
});

it('reads a family off the name only where no attribute says one', () => {
  expect(familyOf(span([], 'run'))).toBe('run');
  expect(familyOf(span([], 'invoke_agent planner-assistant'))).toBe('agent');
  expect(familyOf(span([], 'execute_tool search'))).toBe('tool');
});

it('leaves the correlation ids out of the attribute table until they are asked for', () => {
  const carried = span([
    ['gen_ai.request.model', 'gpt-5-mini'],
    ['messaging.message.id', 'event-1'],
    ['aiwatcher.run.id', 'run-1'],
    ['planner.owner', 'local-developer'],
  ]);

  // What is left is what nothing above drew: the producer's own attribute.
  expect(otherAttributes(carried)).toEqual([['planner.owner', 'local-developer']]);
  expect(otherAttributes(carried, true).map(([key]) => key)).toEqual([
    'aiwatcher.run.id',
    'gen_ai.request.model',
    'messaging.message.id',
    'planner.owner',
  ]);
});
