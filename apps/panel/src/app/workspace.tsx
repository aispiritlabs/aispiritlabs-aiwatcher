import { Link } from '@tanstack/react-router';
import { SECTIONS } from '@/app/navigation';
import { Card, CardContent, CardHeader, CardTitle } from '@/shared/components/ui/primitives';
import { NavigationSettings, PinnedViews, PreferredStart } from '@/app/navigation-controls';

export function WorkspacePage() {
  return (
    <PreferredStart><div className="flex flex-col gap-5">
      <div>
        <h1 className="text-xl font-semibold">Your work</h1>
        <p className="mt-1 text-sm text-muted-foreground">Start with data, model quality or a running application. Each area connects to the evidence behind it.</p>
        <Link to="/" search={{ start: 'workspace' }} hash="navigation-preferences" className="mt-2 inline-block rounded py-1 text-xs text-primary underline">Navigation preferences</Link>
      </div>
      <PinnedViews />
      <div className="grid gap-4 lg:grid-cols-2">
        {SECTIONS.map((section) => <Card key={section.id}>
          <CardHeader><CardTitle>{section.label}</CardTitle></CardHeader>
          <CardContent className="grid gap-2 sm:grid-cols-2">
            {section.areas.map((area) => <Link key={area.to} to={area.to} className="rounded-md border border-border p-3 hover:bg-accent">
              <div className="flex items-center gap-2 text-sm font-medium"><area.icon className="h-4 w-4 text-primary" />{area.label}</div>
              <p className="mt-2 text-xs leading-relaxed text-muted-foreground">{area.blurb}</p>
            </Link>)}
          </CardContent>
        </Card>)}
      </div>
      <NavigationSettings />
    </div></PreferredStart>
  );
}
