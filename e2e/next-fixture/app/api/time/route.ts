import { NextResponse } from "next/server";

/**
 * Always-fresh server timestamp. Forces dynamic execution by reading
 * the wall clock at request time, exercising the V8 dispatch path
 * (the prerender hot path only handles `.html` / `.rsc` files).
 */
export const dynamic = "force-dynamic";

export function GET(): NextResponse<{
  iso: string;
  epochMs: number;
  runtime: string;
}> {
  const now = new Date();
  return NextResponse.json({
    iso: now.toISOString(),
    epochMs: now.getTime(),
    runtime: "nexide",
  });
}
