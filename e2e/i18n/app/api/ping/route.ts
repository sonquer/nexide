import { NextResponse } from "next/server";

export function GET() {
  return NextResponse.json({ message: "pong", runtime: "nexide" });
}
