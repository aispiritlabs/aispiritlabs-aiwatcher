import { describe, expect, it } from 'vitest';

import { SECTIONS, landingFor, reachOf } from '@/app/navigation';

describe('how far a project reaches into each area', () => {
  it('is declared for every area and every view, so a new one has to decide', () => {
    for (const section of SECTIONS) {
      for (const area of section.areas) {
        expect(area.reach, `${area.to} declares no reach`).toBeDefined();
        expect(reachOf(area.to), area.to).toBeDefined();
        for (const view of area.views) expect(reachOf(view.to), view.to).toBeDefined();
      }
    }
  });

  it('follows a run to the view it belongs to, which no area path covers', () => {
    // A run is reachable from a dimension row, a workflow and a search, and
    // its own URL is under none of them. Left unplaced it would be the one
    // page with no side — and it is a run, which is exactly what a side is
    // about.
    expect(reachOf('/runs/example')).toBe('project');
  });

  it('says nothing about the pages a project cannot scope', () => {
    // `/account` administers the scope; the root is nobody's area. A line
    // there would be a claim about a page that has no side.
    expect(reachOf('/account/access')).toBeUndefined();
    expect(reachOf('/')).toBeUndefined();
  });
});

describe('where a change of project lands', () => {
  it('keeps an area and keeps one of its views', () => {
    expect(landingFor('/observability/metrics')).toBe('/observability/metrics');
    expect(landingFor('/workflows')).toBe('/workflows');
  });

  it('drops an object that belongs to the side it was opened on', () => {
    // `/runs/{id}` under the other project is a run that is not there, and
    // "not found" is a poor way to be told the switch worked. It lands on the
    // list that run came from rather than at the top of the area.
    expect(landingFor('/runs/example')).toBe('/observability/runs');
    expect(landingFor('/prompts/example')).toBe('/prompts');
  });

  it('has somewhere to go from a page inside no area at all', () => {
    expect(landingFor('/')).toBe('/');
    expect(landingFor('/account/access')).toBe('/');
  });
});
