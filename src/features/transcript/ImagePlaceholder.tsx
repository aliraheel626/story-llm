export function ImagePlaceholder() {
  return (
    <div className="relative h-56 w-full overflow-hidden rounded border border-border bg-surface" aria-label="Generating scene image">
      <div className="absolute inset-0 animate-shimmer bg-gradient-to-r from-transparent via-surface-hover to-transparent motion-reduce:animate-none" />
      <span className="absolute inset-x-0 bottom-2 text-center text-[11px] text-muted">Illustrating this scene...</span>
    </div>
  );
}
