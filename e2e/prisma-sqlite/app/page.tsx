import { prisma } from "./db";

export const dynamic = "force-dynamic";

export default async function HomePage() {
  const users = await prisma.user.findMany({
    include: { posts: true },
    orderBy: { id: "asc" }
  });

  return (
    <main>
      <h1 data-testid="page-marker">Prisma users</h1>
      <ul data-testid="users-list">
        {users.map((u) => (
          <li key={u.id} data-testid={`user-${u.id}`}>
            <strong>{u.name}</strong> ({u.email}) — posts: {u.posts.length}
          </li>
        ))}
      </ul>
      <p data-testid="user-count">count={users.length}</p>
    </main>
  );
}
