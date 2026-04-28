export const metadata = {
  title: "About - nexide",
};

interface Feature {
  title: string;
  description: string;
}

const FEATURES: ReadonlyArray<Feature> = [
  {
    title: "Rust + V8",
    description:
      "Built on the V8 engine (v147) directly, without Node.js or Deno. Exposes a Node-compatible polyfill layer that Next.js boots against.",
  },
  {
    title: "Prerender hot path",
    description:
      "SSG / ISR HTML and RSC payloads are served from a Tower service in Rust with mtime-validated RAM cache. Sub-millisecond TTFB.",
  },
  {
    title: "Isolate pool",
    description:
      "Dynamic routes (route handlers, force-dynamic SSR) are dispatched to a pool of V8 isolates sized to the available cgroup CPU quota.",
  },
  {
    title: "Drop-in standalone",
    description:
      "Consumes the unchanged output of `next build --output standalone`. No fork of Next.js, no Babel transforms, no patched modules.",
  },
];

export default function AboutPage() {
  return (
    <article
      data-testid="about-page"
      className="prose prose-invert mx-auto flex max-w-3xl flex-col gap-12"
    >
      <header className="flex flex-col gap-4">
        <span className="inline-flex w-fit items-center gap-2 rounded-full border border-white/10 bg-white/5 px-3 py-1 text-xs font-medium text-white/70">
          Statically generated
        </span>
        <h1 className="text-4xl font-bold tracking-tight text-white md:text-5xl">
          About nexide
        </h1>
        <p className="text-lg leading-relaxed text-white/70">
          nexide is a Rust runtime that serves Next.js 16 applications without
          Node.js. This page is statically generated at build time and served
          from the Rust prerender hot path, bypassing V8 entirely.
        </p>
      </header>

      <section className="grid gap-4 md:grid-cols-2">
        {FEATURES.map((feature) => (
          <div
            key={feature.title}
            className="rounded-2xl border border-white/5 bg-white/[0.03] p-6"
          >
            <h2 className="text-base font-semibold text-white">
              {feature.title}
            </h2>
            <p className="mt-2 text-sm leading-relaxed text-white/65">
              {feature.description}
            </p>
          </div>
        ))}
      </section>

      <section className="rounded-2xl border border-white/5 bg-white/[0.02] p-6 text-sm leading-relaxed text-white/70">
        <h2 className="mb-3 text-base font-semibold text-white">
          Inspect this page
        </h2>
        <p>
          Open the network panel and reload. The response headers should
          include{" "}
          <code className="rounded bg-white/10 px-1 text-xs">
            x-nextjs-cache: HIT
          </code>{" "}
          and{" "}
          <code className="rounded bg-white/10 px-1 text-xs">
            x-nextjs-prerender: 1
          </code>
          , with a TTFB well under 2 ms.
        </p>
      </section>
    </article>
  );
}
