import { PrismaClient } from "@prisma/client";

const prisma = new PrismaClient();

async function main() {
  await prisma.post.deleteMany();
  await prisma.user.deleteMany();

  const alice = await prisma.user.create({
    data: {
      email: "alice@example.com",
      name: "Alice",
      posts: {
        create: [
          { title: "Hello from Prisma", body: "Running on nexide + SQLite." },
          { title: "Second post", body: "N-API library engine works." }
        ]
      }
    }
  });

  const bob = await prisma.user.create({
    data: {
      email: "bob@example.com",
      name: "Bob",
      posts: {
        create: [{ title: "Bob's note", body: "Hi there." }]
      }
    }
  });

  console.log(`Seeded users: ${alice.id}, ${bob.id}`);
}

main()
  .catch((e) => {
    console.error(e);
    process.exit(1);
  })
  .finally(async () => {
    await prisma.$disconnect();
  });
