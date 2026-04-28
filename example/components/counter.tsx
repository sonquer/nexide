"use client";

import { useState } from "react";

/**
 * Tiny client-side counter used as a hydration smoke test for the
 * prerendered homepage. Demonstrates that React state survives the
 * Rust prerender hot path → bundled JS → hydration round-trip.
 */
export function Counter() {
  const [count, setCount] = useState(0);

  return (
    <div className="flex items-center gap-4">
      <button
        type="button"
        data-testid="counter"
        onClick={() => setCount((current) => current + 1)}
        className="inline-flex items-center gap-2 rounded-md border border-white/15 bg-white/4 px-5 py-2.5 text-sm font-medium text-white transition hover:bg-white/8 active:scale-[0.98]"
      >
        Hydrated - clicked {count}
      </button>
      <span className="text-xs text-white/50">
        Increments without a network round-trip.
      </span>
    </div>
  );
}
