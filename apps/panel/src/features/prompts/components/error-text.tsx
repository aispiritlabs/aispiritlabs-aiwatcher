export function ErrorText({ error }: { error: unknown }) {
  const body = error as { message?: string } | null;
  return (
    <p className="text-xs text-danger">
      {body?.message ?? 'The registry refused that. Check the name and the text.'}
    </p>
  );
}
