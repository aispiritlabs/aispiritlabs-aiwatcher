import { Link } from '@tanstack/react-router';
import { Card, CardContent, CardHeader, CardTitle } from '@/shared/components/ui/primitives';

export function LearningPage() {
  return (
    <div className="flex flex-col gap-4">
      <div>
        <h1 className="text-lg font-semibold">Learning</h1>
        <p className="mt-1 text-sm text-muted-foreground">Workshops and practical labs across data preparation, models and applications.</p>
      </div>
      <Card>
        <CardHeader><CardTitle>Workshops are not configured</CardTitle></CardHeader>
        <CardContent className="space-y-2 text-sm">
          <p>Workshop enrollment, participant projects and timed access are not available yet. This page previews the lab structure; it does not grant access or track progress.</p>
          <Link to="/account" className="inline-block text-primary underline">View your current account and identity provider groups</Link>
        </CardContent>
      </Card>
      <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
        {Array.from({ length: 9 }, (_, index) => <Card key={index}>
          <CardHeader><CardTitle>Lab {index + 1}</CardTitle></CardHeader>
          <CardContent className="space-y-2 text-xs text-muted-foreground">
            <p className="text-sm">Awaiting workshop content</p>
            <dl className="grid grid-cols-2 gap-x-3 gap-y-2">
              <dt>Instructions</dt><dd>Not supplied</dd>
              <dt>Project</dt><dd>Not assigned</dd>
              <dt>Tests</dt><dd>Unavailable</dd>
              <dt>Evaluations</dt><dd>Unavailable</dd>
              <dt>Metrics</dt><dd>No results</dd>
            </dl>
          </CardContent>
        </Card>)}
      </div>
    </div>
  );
}
