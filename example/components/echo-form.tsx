"use client";

import { useState } from "react";

interface EchoResponse {
  echoed: unknown;
}

type RequestState =
  | { status: "idle" }
  | { status: "pending" }
  | { status: "success"; payload: EchoResponse }
  | { status: "error"; message: string };

/**
 * Client component that POSTs a JSON body to `/api/echo` and renders
 * the echoed payload, exercising the dispatcher's POST body delivery
 * path end-to-end.
 */
export function EchoForm() {
  const [name, setName] = useState("Ada");
  const [message, setMessage] = useState("Hello from nexide!");
  const [state, setState] = useState<RequestState>({ status: "idle" });

  async function handleSubmit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setState({ status: "pending" });
    try {
      const response = await fetch("/api/echo", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ name, message, sentAt: Date.now() }),
      });
      if (!response.ok) {
        throw new Error(`Server returned ${response.status}`);
      }
      const payload = (await response.json()) as EchoResponse;
      setState({ status: "success", payload });
    } catch (cause) {
      const reason = cause instanceof Error ? cause.message : "Unknown error";
      setState({ status: "error", message: reason });
    }
  }

  return (
    <form
      onSubmit={handleSubmit}
      data-testid="echo-form"
      className="flex flex-col gap-4 rounded-xl border border-white/10 bg-white/2 p-6"
    >
      <label className="flex flex-col gap-1 text-sm">
        <span className="font-medium text-white/70">Name</span>
        <input
          name="name"
          value={name}
          onChange={(event) => setName(event.target.value)}
          className="rounded-md border border-white/10 bg-black/20 px-3 py-2 text-white placeholder:text-white/30 focus:border-white/30 focus:outline-none"
          required
        />
      </label>
      <label className="flex flex-col gap-1 text-sm">
        <span className="font-medium text-white/70">Message</span>
        <textarea
          name="message"
          value={message}
          onChange={(event) => setMessage(event.target.value)}
          rows={3}
          className="resize-none rounded-md border border-white/10 bg-black/20 px-3 py-2 text-white placeholder:text-white/30 focus:border-white/30 focus:outline-none"
          required
        />
      </label>
      <div className="flex items-center gap-3">
        <button
          type="submit"
          disabled={state.status === "pending"}
          className="inline-flex items-center gap-2 rounded-md border border-white/15 bg-white/4 px-5 py-2.5 text-sm font-medium text-white transition hover:bg-white/8 active:scale-[0.98] disabled:opacity-50"
        >
          {state.status === "pending" ? "Sending…" : "POST /api/echo"}
        </button>
        {state.status === "success" && (
          <span className="text-xs text-white/60">200 OK</span>
        )}
        {state.status === "error" && (
          <span className="text-xs text-white/60">Error: {state.message}</span>
        )}
      </div>
      {state.status === "success" && (
        <pre
          data-testid="echo-response"
          className="overflow-auto rounded-md border border-white/10 bg-black/40 p-4 text-xs leading-relaxed text-white/75"
        >
          {JSON.stringify(state.payload, null, 2)}
        </pre>
      )}
    </form>
  );
}
