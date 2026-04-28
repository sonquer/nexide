import Link from "next/link";
import { Counter } from "@/components/counter";

interface DemoCard {
  href: string;
  title: string;
  blurb: string;
  badge: string;
  tone: "static" | "isr" | "params" | "api" | "client" | "form";
}

const TONE_STYLES: Record<DemoCard["tone"], string> = {
  static: "border-white/10",
  isr: "border-white/10",
  params: "border-white/10",
  api: "border-white/10",
  client: "border-white/10",
  form: "border-white/10",
};

const DEMOS: ReadonlyArray<DemoCard> = [
  {
    href: "/about",
    title: "About",
    blurb:
      "Pure SSG page served from the Rust prerender hot path with sub-millisecond TTFB.",
    badge: "SSG",
    tone: "static",
  },
  {
    href: "/posts",
    title: "Posts (ISR)",
    blurb:
      "Incremental static regeneration, pre-rendered with revalidate=60 s.",
    badge: "ISR",
    tone: "isr",
  },
  {
    href: "/users/1",
    title: "Users (params)",
    blurb:
      "generateStaticParams + dynamicParams=false. Only known IDs render.",
    badge: "Params",
    tone: "params",
  },
  {
    href: "/api/ping",
    title: "/api/ping",
    blurb: "JSON route handler dispatched into V8. About 15 ms TTFB.",
    badge: "GET",
    tone: "api",
  },
  {
    href: "/api/time",
    title: "/api/time",
    blurb: "Always-fresh server timestamp. Demonstrates dynamic JSON via V8.",
    badge: "GET",
    tone: "api",
  },
  {
    href: "/forms",
    title: "Echo form",
    blurb: "POST → /api/echo round-trip with JSON body and live response.",
    badge: "POST",
    tone: "form",
  },
];

export default function HomePage() {
  return (
    <div data-testid="ssr-marker" className="flex flex-col gap-16">
      <section className="relative overflow-hidden rounded-2xl border border-white/10 bg-white/2 p-10">
        <div className="relative flex flex-col gap-6">
          <h1 className="text-5xl font-semibold tracking-tight text-white md:text-6xl">
            Next.js without Node.js/Deno.
          </h1>
          <p className="max-w-2xl text-lg leading-relaxed text-white/60">
            <code className="rounded bg-white/5 px-1.5 py-0.5 text-sm">
              Nexide
            </code>{" "}
            is a Rust runtime built on V8. It boots a real Next.js 16
            standalone bundle, serves prerendered HTML and RSC payloads from a
            Rust hot path, and dispatches dynamic routes into a pool of
            isolates.
          </p>
          <div className="flex flex-wrap items-center gap-3 pt-2">
            <Link
              href="/about"
              className="inline-flex items-center gap-2 rounded-md bg-white px-5 py-2.5 text-sm font-medium text-ink transition hover:bg-white/90"
            >
              Learn more →
            </Link>
            <Link
              href="/api/ping"
              className="inline-flex items-center gap-2 rounded-md border border-white/15 px-5 py-2.5 text-sm font-medium text-white/80 transition hover:bg-white/4"
            >
              Try /api/ping
            </Link>
          </div>
        </div>
      </section>

      <section className="flex flex-col gap-6">
        <header className="flex items-end justify-between gap-4">
          <div>
            <h2 className="text-xl font-medium text-white">Demo use cases</h2>
            <p className="text-sm text-white/55">
              Each card exercises a different Next.js rendering mode running
              under the nexide runtime.
            </p>
          </div>
        </header>
        <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-3">
          {DEMOS.map((demo) => (
            <Link
              key={demo.href}
              href={demo.href}
              className={`group relative flex flex-col gap-3 rounded-xl border bg-white/2 p-6 transition hover:bg-white/4 ${TONE_STYLES[demo.tone]}`}
            >
              <div className="flex items-center justify-between">
                <span className="rounded-md border border-white/10 px-2 py-0.5 text-[10px] font-medium uppercase tracking-wider text-white/60">
                  {demo.badge}
                </span>
                <span className="text-white/30 transition group-hover:translate-x-0.5 group-hover:text-white/70">
                  →
                </span>
              </div>
              <h3 className="text-base font-medium text-white">{demo.title}</h3>
              <p className="text-sm leading-relaxed text-white/55">
                {demo.blurb}
              </p>
            </Link>
          ))}
        </div>
      </section>

      <section className="flex flex-col gap-4 rounded-xl border border-white/10 bg-white/2 p-8">
        <h2 className="text-xl font-medium text-white">Client interactivity</h2>
        <p className="text-sm text-white/55">
          A standard Next.js{" "}
          <code className="rounded bg-white/5 px-1.5 py-0.5 text-xs">
            &quot;use client&quot;
          </code>{" "}
          component, hydrated from the prerendered HTML served by Rust.
        </p>
        <Counter />
      </section>
    </div>
  );
}
