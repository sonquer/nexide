import { NextResponse } from "next/server";

export async function POST(request: Request): Promise<NextResponse> {
  const data = await request.json();
  return NextResponse.json({ echoed: data });
}
