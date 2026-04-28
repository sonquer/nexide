import Link from "next/link";
import { notFound } from "next/navigation";
import { USERS, findUser } from "../_data";

interface UserPageProps {
  params: Promise<{ id: string }>;
}

export const dynamicParams = false;

export function generateStaticParams() {
  return USERS.map((user) => ({ id: user.id }));
}

export default async function UserPage({ params }: UserPageProps) {
  const { id } = await params;
  const user = findUser(id);
  if (!user) {
    notFound();
  }
  return (
    <article
      data-testid="user-page"
      className="mx-auto flex max-w-2xl flex-col gap-8"
    >
      <Link
        href="/users"
        className="inline-flex w-fit items-center gap-2 text-sm text-white/60 hover:text-white"
      >
        ← All users
      </Link>
      <header className="flex items-center gap-5">
        <span
          aria-hidden
          className="flex h-20 w-20 items-center justify-center rounded-full text-3xl font-bold text-white shadow-xl"
          style={{
            background: `linear-gradient(135deg, hsl(${user.avatarHue}, 80%, 60%), hsl(${user.avatarHue + 40}, 80%, 50%))`,
          }}
        >
          {user.name.charAt(0)}
        </span>
        <div className="flex flex-col">
          <h1 className="text-3xl font-bold text-white">{user.name}</h1>
          <p className="text-white/60">{user.role}</p>
          <p className="mt-1 font-mono text-xs text-white/40">
            ID: <span data-testid="user-id">{user.id}</span>
          </p>
        </div>
      </header>
      <p className="rounded-2xl border border-white/5 bg-white/3 p-5 text-sm leading-relaxed text-white/70">
        This profile was pre-rendered at build time and is being served from the
        Rust prerender hot path. The V8 isolate pool is not involved.
      </p>
    </article>
  );
}
