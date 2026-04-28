import { NextResponse } from "next/server";
import type { Post, User } from "@prisma/client";
import { prisma } from "../../db";

export async function GET() {
  const users = await prisma.user.findMany({
    include: { posts: true },
    orderBy: { id: "asc" }
  });
  return NextResponse.json({
    runtime: "nexide",
    engine: "prisma-library",
    count: users.length,
    users: users.map((u: User & { posts: Post[] }) => ({
      id: u.id,
      email: u.email,
      name: u.name,
      posts: u.posts.length
    }))
  });
}
