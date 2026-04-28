import Link from "next/link";
import { POSTS } from "./_data";

export const metadata = {
  title: "Posts - nexide",
};

export default function PostsIndexPage() {
  return (
    <section
      data-testid="posts-index"
      className="flex flex-col gap-10"
    >
      <header className="flex flex-col gap-3">
        <span className="inline-flex w-fit items-center gap-2 rounded-full border border-white/10 bg-white/[0.03] px-3 py-1 text-xs font-medium text-white/60">
          ISR · revalidate 60 s
        </span>
        <h1 className="text-4xl font-bold tracking-tight text-white">Posts</h1>
        <p className="text-white/70">
          Each post page is statically generated and revalidated every 60
          seconds. The listing itself is also pre-rendered at build time.
        </p>
      </header>

      <ul className="flex flex-col gap-4">
        {POSTS.map((post) => (
          <li key={post.slug}>
            <Link
              href={`/posts/${post.slug}`}
              className="group flex items-center justify-between gap-6 rounded-2xl border border-white/5 bg-white/[0.03] p-6 transition hover:-translate-y-0.5 hover:bg-white/[0.06]"
            >
              <div className="flex flex-col gap-1">
                <h2 className="text-lg font-semibold text-white">
                  {post.title}
                </h2>
                <p className="text-sm text-white/60">
                  By {post.author} · {post.readTimeMinutes} min read
                </p>
              </div>
              <span className="text-white/40 transition group-hover:translate-x-0.5 group-hover:text-white">
                →
              </span>
            </Link>
          </li>
        ))}
      </ul>
    </section>
  );
}
