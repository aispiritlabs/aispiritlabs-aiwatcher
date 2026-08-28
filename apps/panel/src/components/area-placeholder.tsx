import * as React from 'react';

import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/primitives';

/**
 * An area that exists in the navigation before it exists in the backend.
 *
 * Deliberately not a mock. A screen full of plausible fake rows reads as
 * working software and costs a day the first time someone files a bug against
 * numbers that were never real. This says what the area is for, what it needs
 * that does not exist yet, and stops.
 */
export function AreaPlaceholder({
  title,
  summary,
  sections,
  blockedOn,
}: {
  title: string;
  summary: string;
  /** What this area will hold, one card each. */
  sections: { title: string; description: string }[];
  /** What has to exist first. Named concretely, not as "coming soon". */
  blockedOn: React.ReactNode;
}) {
  return (
    <div className="flex flex-col gap-4">
      <div>
        <h1 className="text-lg font-semibold">{title}</h1>
        <p className="max-w-3xl text-sm text-muted-foreground">{summary}</p>
      </div>

      <div className="grid gap-3 md:grid-cols-3">
        {sections.map((section) => (
          <Card key={section.title} className="border-dashed">
            <CardHeader>
              <CardTitle className="text-sm">{section.title}</CardTitle>
            </CardHeader>
            <CardContent className="text-xs text-muted-foreground">
              {section.description}
            </CardContent>
          </Card>
        ))}
      </div>

      <Card className="border-dashed">
        <CardHeader>
          <CardTitle className="text-sm">Not built yet</CardTitle>
        </CardHeader>
        <CardContent className="max-w-3xl text-xs leading-relaxed text-muted-foreground">
          {blockedOn}
        </CardContent>
      </Card>
    </div>
  );
}
