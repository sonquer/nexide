import { EchoForm } from "@/components/echo-form";

export const metadata = {
  title: "Forms - nexide",
};

export default function FormsPage() {
  return (
    <section
      data-testid="forms-page"
      className="mx-auto flex max-w-2xl flex-col gap-8"
    >
      <header className="flex flex-col gap-3">
        <span className="inline-flex w-fit items-center gap-2 rounded-full border border-white/10 bg-white/[0.03] px-3 py-1 text-xs font-medium text-white/60">
          Client → POST → V8
        </span>
        <h1 className="text-4xl font-bold tracking-tight text-white">
          Echo form
        </h1>
        <p className="text-white/70">
          Submitting this form sends a JSON payload to{" "}
          <code className="rounded bg-white/10 px-1.5 py-0.5 text-sm">
            /api/echo
          </code>
          . The route handler runs inside a V8 isolate and echoes the body
          back. Demonstrates the full POST-with-body round-trip through the
          Rust runtime.
        </p>
      </header>
      <EchoForm />
    </section>
  );
}
