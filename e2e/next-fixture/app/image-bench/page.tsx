import Image from "next/image";

const SIZES = [128, 256, 384, 640, 1080] as const;
const QUALITIES = [50, 75, 90] as const;

export const dynamic = "force-static";

export const metadata = {
  title: "Image optimizer benchmark | Nexide",
  description:
    "Native /_next/image pipeline served from Rust. Decodes, resizes, and re-encodes without entering V8.",
};

export default function ImageBenchPage() {
  return (
    <div className="flex flex-col gap-10">
      <section className="rounded-2xl border border-white/10 bg-white/2 p-8">
        <h1 className="text-3xl font-semibold tracking-tight text-white">
          Native /_next/image pipeline
        </h1>
        <p className="mt-3 max-w-3xl text-sm leading-relaxed text-white/60">
          Every thumbnail below is produced by the nexide native optimizer.
          Decode, Lanczos3 resize and WebP/JPEG re-encode happen entirely in
          Rust (zero V8 round-trips), backed by an on-disk cache keyed on{" "}
          <code className="rounded bg-white/5 px-1 py-0.5 text-xs">
            (href, w, q, mime)
          </code>
          .
        </p>
      </section>

      <section className="flex flex-col gap-4">
        <h2 className="text-lg font-medium text-white">
          Same source, multiple widths
        </h2>
        <div className="grid gap-6 md:grid-cols-3 lg:grid-cols-5">
          {SIZES.map((w) => (
            <figure
              key={w}
              className="flex flex-col items-center gap-2 rounded-xl border border-white/10 bg-white/2 p-3"
            >
              <Image
                src="/nexide.png"
                alt={`Nexide logo at ${w}px`}
                width={w}
                height={Math.round((w * 189) / 600)}
                quality={75}
                sizes={`${w}px`}
              />
              <figcaption className="text-xs text-white/50">
                w={w}, q=75
              </figcaption>
            </figure>
          ))}
        </div>
      </section>

      <section className="flex flex-col gap-4">
        <h2 className="text-lg font-medium text-white">
          Same width, sweeping quality
        </h2>
        <div className="grid gap-6 md:grid-cols-3">
          {QUALITIES.map((q) => (
            <figure
              key={q}
              className="flex flex-col items-center gap-2 rounded-xl border border-white/10 bg-white/2 p-3"
            >
              <Image
                src="/nexide.png"
                alt={`Nexide logo at quality ${q}`}
                width={384}
                height={Math.round((384 * 189) / 600)}
                quality={q}
              />
              <figcaption className="text-xs text-white/50">
                w=384, q={q}
              </figcaption>
            </figure>
          ))}
        </div>
      </section>

      <section className="rounded-xl border border-white/10 bg-white/2 p-6 text-sm text-white/60">
        <h2 className="mb-2 text-base font-medium text-white">
          What gets benchmarked
        </h2>
        <p>
          The{" "}
          <code className="rounded bg-white/5 px-1 py-0.5 text-xs">
            next-image
          </code>{" "}
          scenario in{" "}
          <code className="rounded bg-white/5 px-1 py-0.5 text-xs">
            nexide-bench
          </code>{" "}
          drives this exact route (
          <code className="rounded bg-white/5 px-1 py-0.5 text-xs">
            /_next/image?url=/nexide.png&amp;w=256&amp;q=75
          </code>
          ) with{" "}
          <code className="rounded bg-white/5 px-1 py-0.5 text-xs">
            Accept: image/webp
          </code>{" "}
          against nexide, Node and Deno hosting the same standalone bundle.
        </p>
      </section>
    </div>
  );
}
