import { NextResponse } from "next/server";
import { prisma } from "../../db";

export const dynamic = "force-dynamic";

export async function GET() {
  const users = await prisma.user.findMany({
    include: { posts: true },
    orderBy: { id: "asc" }
  });
  return NextResponse.json({
    runtime: "nexide",
    engine: "prisma-library",
    count: users.length,
    users: users.map((u) => ({
      id: u.id,
      email: u.email,
      name: u.name,
      posts: u.posts.length
    }))
  });
}
