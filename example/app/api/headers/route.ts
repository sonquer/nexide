import { NextResponse } from "next/server";

/**
 * Echoes back the request headers received by the V8 isolate. Useful
 * for verifying that the Rust HTTP shield forwards client headers
 * (user-agent, accept-language, custom probes…) faithfully.
 */
export const dynamic = "force-dynamic";

export function GET(request: Request): NextResponse<{
  headers: Record<string, string>;
}> {
  const headers: Record<string, string> = {};
  request.headers.forEach((value, key) => {
    headers[key] = value;
  });
  return NextResponse.json({ headers });
}
