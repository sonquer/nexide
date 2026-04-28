import Link from "next/link";

export default function NotFoundPage() {
  return (
    <section
      data-testid="not-found-page"
      className="mx-auto flex max-w-xl flex-col items-center gap-6 py-16 text-center"
    >
      <span className="rounded-full border border-white/10 bg-white/5 px-3 py-1 text-xs font-medium text-white/60">
        404
      </span>
      <h1 className="text-4xl font-bold tracking-tight text-white">
        Page not found
      </h1>
      <p className="text-white/70">
        We couldn&apos;t locate that route. It may have been removed, or the URL
        might be a typo.
      </p>
      <Link
        href="/"
        className="inline-flex items-center gap-2 rounded-full bg-white px-5 py-2.5 text-sm font-semibold text-ink transition hover:bg-white/90"
      >
        ← Back home
      </Link>
    </section>
  );
}
