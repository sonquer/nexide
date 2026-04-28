import Link from "next/link";
import { USERS } from "./_data";

export const metadata = {
  title: "Users - nexide",
};

export default function UsersIndexPage() {
  return (
    <section
      data-testid="users-index"
      className="flex flex-col gap-10"
    >
      <header className="flex flex-col gap-3">
        <span className="inline-flex w-fit items-center gap-2 rounded-full border border-white/10 bg-white/[0.03] px-3 py-1 text-xs font-medium text-white/60">
          SSG · generateStaticParams
        </span>
        <h1 className="text-4xl font-bold tracking-tight text-white">Users</h1>
        <p className="text-white/70">
          Each profile is pre-rendered at build time. Unknown IDs return 404
          because the route opts out of dynamic params.
        </p>
      </header>

      <ul className="grid gap-4 md:grid-cols-3">
        {USERS.map((user) => (
          <li key={user.id}>
            <Link
              href={`/users/${user.id}`}
              className="group flex h-full flex-col gap-3 rounded-2xl border border-white/5 bg-white/[0.03] p-6 transition hover:-translate-y-0.5 hover:bg-white/[0.06]"
            >
              <span
                aria-hidden
                className="flex h-12 w-12 items-center justify-center rounded-full text-lg font-bold text-white shadow-lg"
                style={{
                  background: `linear-gradient(135deg, hsl(${user.avatarHue}, 80%, 60%), hsl(${user.avatarHue + 40}, 80%, 50%))`,
                }}
              >
                {user.name.charAt(0)}
              </span>
              <div className="flex flex-col gap-0.5">
                <h2 className="text-base font-semibold text-white">
                  {user.name}
                </h2>
                <p className="text-sm text-white/55">{user.role}</p>
              </div>
              <span className="mt-auto text-xs font-mono text-white/40">
                #{user.id}
              </span>
            </Link>
          </li>
        ))}
      </ul>
    </section>
  );
}
