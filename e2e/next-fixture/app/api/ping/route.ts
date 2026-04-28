import { NextResponse } from "next/server";

export function GET(): NextResponse<{
  message: string;
  runtime: string;
}> {
  return NextResponse.json({
    message: "pong",
    runtime: "nexide",
  });
}
